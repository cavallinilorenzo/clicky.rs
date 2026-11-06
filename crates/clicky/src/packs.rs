//! The bundled soundpacks (built by `cargo xtask packs`): the [`Library`] the
//! picker lists, grouped by [`SwitchType`], and the [`SELECTED_PACK`] setting.
//!
//! Search order: `$CLICKY_PACKS`, then `Clicky.app/Contents/Resources/Packs`,
//! then the workspace's `target/packs` (for `cargo run`). An id found in more
//! than one place is taken from the first.
//!
//! The library reads only each pack's description at launch; a pack's audio is
//! read the first time it is played and then kept. Loaded packs are leaked on
//! purpose: voices borrow their samples as `&'static [i16]` from the render
//! callback, so a pack must never be freed, even after switching away from it
//! (its last voices finish playing from it).
//!
//! The library's order and fallback follow Rustyvibes 2 (MIT, © 2021–2026 Kunal
//! Bagaria, https://github.com/KunalBagaria/rustyvibes-v2), `crates/rustyvibes/src/packs.rs`.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use clicky_pack::format::INFO_HEADER_LEN;
use clicky_pack::{Pack, PackInfo};

use crate::settings::Setting;

/// The soundpack used on first launch, and whenever the saved one is not bundled.
pub const DEFAULT_PACK: &str = "cherry-mx-brown-pbt";
/// The chosen soundpack, by id.
pub const SELECTED_PACK: Setting<&str> = Setting { key: "Soundpack", default: DEFAULT_PACK };
/// Overrides the soundpack directory.
pub const PACKS_ENV: &str = "CLICKY_PACKS";
const EXTENSION: &str = "clickypack";

/// The picker's groups, in menu order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SwitchType {
    Linear,
    Tactile,
    /// A Clicky switch (glossary): its group is labelled "Clicky".
    Clicky,
}

impl SwitchType {
    /// From a pack's `switch_type` metadata.
    pub fn from_id(id: &str) -> Option<SwitchType> {
        match id {
            "linear" => Some(SwitchType::Linear),
            "tactile" => Some(SwitchType::Tactile),
            "clicky" => Some(SwitchType::Clicky),
            _ => None,
        }
    }

    /// The group's heading in the picker.
    pub fn label(self) -> &'static str {
        match self {
            SwitchType::Linear => "Linear",
            SwitchType::Tactile => "Tactile",
            SwitchType::Clicky => "Clicky",
        }
    }
}

/// One bundled soundpack.
#[derive(Debug)]
pub struct Entry {
    pub info: PackInfo,
    pub switch_type: SwitchType,
    path: PathBuf,
    loaded: OnceLock<&'static Pack>,
}

impl Entry {
    pub fn id(&self) -> &str {
        &self.info.meta.id
    }

    /// The name shown in the picker, e.g. "Cherry MX Brown · PBT".
    pub fn title(&self) -> String {
        let meta = &self.info.meta;
        if meta.variant.is_empty() {
            meta.name.clone()
        } else {
            format!("{} · {}", meta.name, meta.variant)
        }
    }

    /// Whether the pack was recorded with release sounds.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "issue #8 greys out Key Release Sounds with it")
    )]
    pub fn has_release(&self) -> bool {
        self.info.has_release
    }

    /// The pack's audio, read on first use and kept for the life of the process.
    pub fn load(&self) -> Result<&'static Pack, String> {
        if let Some(pack) = self.loaded.get() {
            return Ok(pack);
        }
        let pack = load(&self.path)?;
        if pack.meta.id != self.info.meta.id {
            return Err(format!("{}: the file changed since launch", self.path.display()));
        }
        let pack: &'static Pack = Box::leak(Box::new(pack));
        Ok(*self.loaded.get_or_init(|| pack))
    }

    /// Whether the audio has been read.
    pub fn is_loaded(&self) -> bool {
        self.loaded.get().is_some()
    }
}

/// Every bundled soundpack, sorted by switch type and then catalog order.
#[derive(Debug, Default)]
pub struct Library {
    entries: Vec<Entry>,
    /// Files that could not be read, for the diagnostics log.
    pub problems: Vec<String>,
}

impl Library {
    /// Reads the description of every `*.clickypack` in `dirs`.
    pub fn scan(dirs: &[PathBuf]) -> Library {
        let mut library = Library::default();
        for dir in dirs {
            let Ok(listing) = std::fs::read_dir(dir) else { continue };
            let mut paths: Vec<PathBuf> = listing
                .flatten()
                .map(|item| item.path())
                .filter(|path| path.extension().is_some_and(|e| e == EXTENSION))
                .collect();
            paths.sort();
            for path in paths {
                match read_info(&path) {
                    Ok(info) if library.get(&info.meta.id).is_some() => {}
                    Ok(info) => match SwitchType::from_id(&info.meta.switch_type) {
                        Some(switch_type) => library.entries.push(Entry {
                            info,
                            switch_type,
                            path,
                            loaded: OnceLock::new(),
                        }),
                        None => library.problems.push(format!(
                            "{}: unknown switch type {:?}",
                            path.display(),
                            info.meta.switch_type
                        )),
                    },
                    Err(problem) => library.problems.push(problem),
                }
            }
        }
        library.entries.sort_by_key(|e| (e.switch_type, e.info.meta.order));
        library
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id() == id)
    }

    /// The pack to use for a saved id: that pack, else [`DEFAULT_PACK`], else
    /// the first bundled one. `None` only when nothing is bundled.
    pub fn resolve(&self, saved: &str) -> Option<&Entry> {
        self.get(saved).or_else(|| self.get(DEFAULT_PACK)).or_else(|| self.entries.first())
    }

    /// The pack to play for a saved id, with its audio: [`Library::resolve`]'s
    /// choice, else the next bundled pack that loads. Also returns why the
    /// packs tried before it failed. `None` only when no pack loads.
    pub fn load_saved(&self, saved: &str) -> (Option<(&Entry, &'static Pack)>, Vec<String>) {
        let mut problems = Vec::new();
        let first = self.resolve(saved);
        let rest = self.entries.iter().filter(|e| !first.is_some_and(|f| std::ptr::eq(*e, f)));
        for entry in first.into_iter().chain(rest) {
            match entry.load() {
                Ok(pack) => return (Some((entry, pack)), problems),
                Err(problem) => problems.push(problem),
            }
        }
        (None, problems)
    }

    /// The packs of each switch type, in menu order; empty groups are left out.
    pub fn groups(&self) -> Vec<(SwitchType, &[Entry])> {
        self.entries
            .chunk_by(|a, b| a.switch_type == b.switch_type)
            .map(|group| (group[0].switch_type, group))
            .collect()
    }
}

/// The library of the bundled soundpacks, scanned on first use.
pub fn library() -> &'static Library {
    static LIBRARY: OnceLock<Library> = OnceLock::new();
    LIBRARY.get_or_init(|| Library::scan(&search_dirs()))
}

/// Directories that may hold soundpacks, most specific first.
pub fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = std::env::var_os(PACKS_ENV) {
        dirs.push(PathBuf::from(dir));
    }
    // Contents/MacOS/Clicky → Contents/Resources/Packs
    if let Some(contents) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().and_then(Path::parent).map(Path::to_path_buf))
    {
        dirs.push(contents.join("Resources/Packs"));
    }
    dirs.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/packs"));
    dirs
}

/// Reads and validates a soundpack file.
pub fn load(path: &Path) -> Result<Pack, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Pack::from_bytes(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Loads soundpack `id` from the library, for the life of the process.
pub fn load_static(id: &str) -> Result<&'static Pack, String> {
    let library = library();
    let entry = library.get(id).ok_or_else(|| {
        let searched: Vec<String> = search_dirs().iter().map(|d| d.display().to_string()).collect();
        format!("soundpack {id:?} not found in {}", searched.join(", "))
    })?;
    entry.load()
}

/// Reads a pack's description: the header and metadata, not the audio.
fn read_info(path: &Path) -> Result<PackInfo, String> {
    let fail = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let mut file = File::open(path).map_err(|e| fail(&e))?;
    let mut prefix = vec![0; INFO_HEADER_LEN];
    file.read_exact(&mut prefix).map_err(|e| fail(&e))?;
    let needed = Pack::info_len(&prefix).map_err(|e| fail(&e))?;
    prefix.resize(needed, 0);
    file.read_exact(&mut prefix[INFO_HEADER_LEN..]).map_err(|e| fail(&e))?;
    Pack::info(&prefix).map_err(|e| fail(&e))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::settings::Store;
    use clicky_pack::{KEY_SLOTS, Meta};

    pub(crate) fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("clicky-packs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes a one-clip pack `id` (its A plays `[1, 2, 3]`).
    pub(crate) fn write_pack(dir: &Path, id: &str, switch_type: &str, order: u32, release: bool) {
        let mut press = [None; KEY_SLOTS];
        press[0] = Some(0);
        let mut releases = [None; KEY_SLOTS];
        if release {
            releases[0] = Some(0);
        }
        let meta = Meta {
            id: id.into(),
            name: format!("Switch {id}"),
            switch_type: switch_type.into(),
            order,
            ..Meta::default()
        };
        let pack = Pack::new(meta, 44_100, &[vec![1, 2, 3]], press, releases).unwrap();
        std::fs::write(dir.join(format!("{id}.clickypack")), pack.to_bytes().unwrap()).unwrap();
    }

    #[test]
    fn groups_packs_by_switch_type_in_catalog_order() {
        let dir = temp_dir("groups");
        write_pack(&dir, "blue", "clicky", 4, false);
        write_pack(&dir, "red", "linear", 1, false);
        write_pack(&dir, "black", "linear", 0, false);
        write_pack(&dir, "brown", "tactile", 2, true);
        let library = Library::scan(&[dir]);
        let groups: Vec<(&str, Vec<&str>)> = library
            .groups()
            .iter()
            .map(|(t, entries)| (t.label(), entries.iter().map(Entry::id).collect()))
            .collect();
        assert_eq!(
            groups,
            [
                ("Linear", vec!["black", "red"]),
                ("Tactile", vec!["brown"]),
                ("Clicky", vec!["blue"])
            ]
        );
        assert!(library.problems.is_empty());
    }

    #[test]
    fn records_which_packs_have_release_sounds() {
        let dir = temp_dir("release");
        write_pack(&dir, "with", "linear", 0, true);
        write_pack(&dir, "without", "linear", 1, false);
        let library = Library::scan(&[dir]);
        assert!(library.get("with").unwrap().has_release());
        assert!(!library.get("without").unwrap().has_release());
    }

    #[test]
    fn an_unknown_saved_id_falls_back_to_the_default_then_the_first() {
        let dir = temp_dir("resolve");
        write_pack(&dir, "red", "linear", 0, false);
        let library = Library::scan(std::slice::from_ref(&dir));
        assert_eq!(library.resolve("gone").map(Entry::id), Some("red"), "no default bundled");
        write_pack(&dir, DEFAULT_PACK, "tactile", 1, false);
        let library = Library::scan(&[dir]);
        assert_eq!(library.resolve("red").map(Entry::id), Some("red"));
        assert_eq!(library.resolve("gone").map(Entry::id), Some(DEFAULT_PACK));
        assert_eq!(library.resolve("").map(Entry::id), Some(DEFAULT_PACK));
        assert!(Library::scan(&[]).resolve(DEFAULT_PACK).is_none());
    }

    #[test]
    fn the_saved_pack_is_played_after_a_relaunch_and_an_unknown_one_falls_back() {
        let dir = temp_dir("saved");
        write_pack(&dir, "red", "linear", 0, false);
        write_pack(&dir, DEFAULT_PACK, "tactile", 1, false);
        let store = Store::in_memory();
        store.set(&SELECTED_PACK, &"red".to_owned());
        let library = Library::scan(std::slice::from_ref(&dir));
        let (chosen, problems) = library.load_saved(&store.get(&SELECTED_PACK));
        assert_eq!(chosen.map(|(e, p)| (e.id(), p.meta.id.as_str())), Some(("red", "red")));
        assert!(problems.is_empty());
        store.set(&SELECTED_PACK, &"retired-pack".to_owned());
        let (chosen, _) = library.load_saved(&store.get(&SELECTED_PACK));
        assert_eq!(chosen.map(|(e, _)| e.id()), Some(DEFAULT_PACK));
        assert_eq!(Store::in_memory().get(&SELECTED_PACK), DEFAULT_PACK, "first launch");
    }

    #[test]
    fn a_pack_that_fails_to_load_is_skipped_for_the_next_one() {
        let dir = temp_dir("broken");
        write_pack(&dir, DEFAULT_PACK, "tactile", 0, false);
        write_pack(&dir, "red", "linear", 1, false);
        let library = Library::scan(std::slice::from_ref(&dir));
        // Its description still reads, but its audio was cut short after launch.
        let path = dir.join(format!("{DEFAULT_PACK}.clickypack"));
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() - 2]).unwrap();
        let (chosen, problems) = library.load_saved(DEFAULT_PACK);
        assert_eq!(chosen.map(|(e, _)| e.id()), Some("red"));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains(DEFAULT_PACK), "{problems:?}");
        assert_eq!(Library::scan(&[]).load_saved(DEFAULT_PACK).0.map(|(e, _)| e.id()), None);
    }

    #[test]
    fn reads_audio_only_when_played_and_only_once() {
        let dir = temp_dir("lazy");
        write_pack(&dir, "brown", "tactile", 0, false);
        let library = Library::scan(&[dir]);
        let entry = library.get("brown").unwrap();
        assert!(!entry.is_loaded());
        let first = entry.load().unwrap();
        assert_eq!(first.clip(0), Some(&[1i16, 2, 3][..]));
        assert!(std::ptr::eq(first, entry.load().unwrap()), "the same pack is kept");
    }

    #[test]
    fn the_first_directory_wins_and_bad_files_are_reported_not_fatal() {
        let (a, b) = (temp_dir("first-a"), temp_dir("first-b"));
        write_pack(&a, "brown", "tactile", 0, true);
        write_pack(&b, "brown", "tactile", 0, false);
        write_pack(&b, "blue", "clicky", 1, false);
        std::fs::write(b.join("bad.clickypack"), b"nope").unwrap();
        write_pack(&b, "odd", "mushy", 2, false);
        let library = Library::scan(&[a.join("missing"), a, b]);
        assert_eq!(library.entries().len(), 2);
        assert!(library.get("brown").unwrap().has_release(), "from the first directory");
        assert_eq!(library.problems.len(), 2, "{:?}", library.problems);
        assert!(library.problems.iter().any(|p| p.contains("bad.clickypack")));
        assert!(library.problems.iter().any(|p| p.contains("mushy")));
    }

    #[test]
    fn loads_a_valid_pack_and_rejects_a_corrupt_one() {
        let dir = temp_dir("load");
        write_pack(&dir, "brown", "tactile", 0, false);
        let pack = load(&dir.join("brown.clickypack")).unwrap();
        assert_eq!(pack.meta.id, "brown");
        std::fs::write(dir.join("bad.clickypack"), b"nope").unwrap();
        let err = load(&dir.join("bad.clickypack")).unwrap_err();
        assert!(err.contains("bad.clickypack"), "{err}");
    }

    #[test]
    fn titles_join_name_and_variant() {
        let dir = temp_dir("title");
        let meta = Meta {
            id: "x".into(),
            name: "Cherry MX Brown".into(),
            variant: "PBT".into(),
            switch_type: "tactile".into(),
            ..Meta::default()
        };
        let mut press = [None; KEY_SLOTS];
        press[0] = Some(0);
        let pack = Pack::new(meta, 44_100, &[vec![1]], press, [None; KEY_SLOTS]).unwrap();
        std::fs::write(dir.join("x.clickypack"), pack.to_bytes().unwrap()).unwrap();
        write_pack(&dir, "topre", "tactile", 1, false);
        let library = Library::scan(&[dir]);
        assert_eq!(library.get("x").unwrap().title(), "Cherry MX Brown · PBT");
        assert_eq!(library.get("topre").unwrap().title(), "Switch topre");
    }

    /// The packs `cargo xtask packs` built, when present: all 21, grouped 9/7/5.
    #[test]
    fn the_built_library_has_every_bundled_pack() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/packs");
        let library = Library::scan(&[dir]);
        if library.entries().is_empty() {
            eprintln!("skipped: run `cargo xtask packs` first");
            return;
        }
        let counts: Vec<(SwitchType, usize)> =
            library.groups().iter().map(|(t, e)| (*t, e.len())).collect();
        assert_eq!(
            counts,
            [(SwitchType::Linear, 9), (SwitchType::Tactile, 7), (SwitchType::Clicky, 5)]
        );
        assert_eq!(library.resolve("").map(Entry::id), Some(DEFAULT_PACK));
        assert!(library.problems.is_empty(), "{:?}", library.problems);
    }
}
