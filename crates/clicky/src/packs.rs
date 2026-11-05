//! Finding and loading the bundled soundpacks (built by `cargo xtask packs`).
//!
//! Search order: `$CLICKY_PACKS`, then `Clicky.app/Contents/Resources/Packs`,
//! then the workspace's `target/packs` (for `cargo run`).
//!
//! Loaded packs are leaked on purpose: voices borrow their samples as
//! `&'static [i16]` from the render callback, so a pack must never be freed.
//! Issue #7 adds the picker; it should load each pack at most once and keep it.

use std::path::{Path, PathBuf};

use clicky_pack::Pack;

/// The soundpack used until issue #7 adds the picker.
pub const DEFAULT_PACK: &str = "cherry-mx-brown-pbt";
/// Overrides the soundpack directory.
pub const PACKS_ENV: &str = "CLICKY_PACKS";
const EXTENSION: &str = "clickypack";

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

/// The file of soundpack `id` in the first directory that has it.
pub fn find(id: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter().map(|dir| dir.join(format!("{id}.{EXTENSION}"))).find(|path| path.is_file())
}

/// Reads and validates a soundpack file.
pub fn load(path: &Path) -> Result<Pack, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Pack::from_bytes(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Loads soundpack `id` for the life of the process.
pub fn load_static(id: &str) -> Result<&'static Pack, String> {
    let dirs = search_dirs();
    let path = find(id, &dirs).ok_or_else(|| {
        let searched: Vec<String> = dirs.iter().map(|d| d.display().to_string()).collect();
        format!("soundpack {id:?} not found in {}", searched.join(", "))
    })?;
    Ok(Box::leak(Box::new(load(&path)?)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clicky_pack::{KEY_SLOTS, Meta};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("clicky-packs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_pack(dir: &Path, id: &str) {
        let mut press = [None; KEY_SLOTS];
        press[0] = Some(0);
        let meta = Meta { id: id.into(), ..Meta::default() };
        let pack = Pack::new(meta, 44_100, &[vec![1, 2, 3]], press, [None; KEY_SLOTS]).unwrap();
        std::fs::write(dir.join(format!("{id}.clickypack")), pack.to_bytes().unwrap()).unwrap();
    }

    #[test]
    fn finds_a_pack_in_the_first_directory_that_has_it() {
        let (a, b) = (temp_dir("a"), temp_dir("b"));
        write_pack(&b, "brown");
        assert_eq!(find("brown", &[a.clone(), b.clone()]), Some(b.join("brown.clickypack")));
        write_pack(&a, "brown");
        assert_eq!(find("brown", &[a.clone(), b.clone()]), Some(a.join("brown.clickypack")));
        assert_eq!(find("blue", &[a, b]), None);
    }

    #[test]
    fn loads_a_valid_pack_and_rejects_a_corrupt_one() {
        let dir = temp_dir("load");
        write_pack(&dir, "brown");
        let pack = load(&dir.join("brown.clickypack")).unwrap();
        assert_eq!(pack.meta.id, "brown");
        std::fs::write(dir.join("bad.clickypack"), b"nope").unwrap();
        let err = load(&dir.join("bad.clickypack")).unwrap_err();
        assert!(err.contains("bad.clickypack"), "{err}");
    }
}
