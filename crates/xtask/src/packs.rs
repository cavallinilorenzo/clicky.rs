//! `cargo xtask packs`: converts every catalog entry into a `.clickypack`.
//!
//! Pipeline per soundpack: decode → slice per key (source adapter) → trim →
//! normalise → dither to 16-bit → assemble → check invariants → write.
//!
//! Derived from Rustyvibes 2 (MIT, © 2021–2026 Kunal Bagaria,
//! https://github.com/KunalBagaria/rustyvibes-v2), `crates/xtask/src/packs.rs`.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Instant;

use clicky_pack::{KEY_SLOTS, Meta, Pack, keys};

use crate::catalog::{self, Entry, SourceKind};
use crate::dsp::{self, Normalized};
use crate::loudness;
use crate::sources::{self, RawPack};

/// File extension of converted soundpacks.
pub const EXTENSION: &str = "clickypack";

/// Summary of one converted soundpack.
#[derive(Debug)]
pub struct Report {
    pub id: String,
    pub clips: usize,
    pub bytes: usize,
    pub seconds: f32,
    pub gain_db: f32,
    /// Loudness of the built pack, measured on its 16-bit clips ([`typing_keys`]).
    pub loudness_db: f32,
    /// Clips turned down below the pack gain to respect the peak ceiling.
    pub limited: usize,
    /// The largest extra cut among them (dB).
    pub max_cut_db: f32,
    pub max_lead_ms: f32,
    pub max_peak_db: f32,
    pub keys: usize,
    pub has_release: bool,
}

/// `cargo xtask packs`
pub fn run(root: &Path) -> crate::Result<()> {
    let started = Instant::now();
    let out = root.join("target/packs");
    let reports = build_all(&root.join("assets/soundpacks/catalog.json"), &out)?;
    println!(
        "{:<24} {:>5} {:>8} {:>7} {:>8} {:>8} {:>11} {:>8} {:>9} {:>5} {:>7}",
        "pack",
        "clips",
        "size",
        "audio",
        "gain",
        "loudness",
        "limited",
        "lead",
        "peak",
        "keys",
        "release"
    );
    for r in &reports {
        println!(
            "{:<24} {:>5} {:>6}KB {:>6.1}s {:>+6.1}dB {:>6.1}dB {:>3} {:>5.1}dB {:>6.2}ms {:>6.1}dBFS {:>5} {:>7}",
            r.id,
            r.clips,
            r.bytes / 1024,
            r.seconds,
            r.gain_db,
            r.loudness_db,
            r.limited,
            r.max_cut_db,
            r.max_lead_ms,
            r.max_peak_db,
            r.keys,
            if r.has_release { "yes" } else { "no" }
        );
    }
    println!(
        "{} soundpack(s) built in {:.1}s → {}",
        reports.len(),
        started.elapsed().as_secs_f32(),
        out.display()
    );
    Ok(())
}

/// Converts every catalog entry into `out_dir/<id>.clickypack` and removes stale packs.
pub fn build_all(catalog_path: &Path, out_dir: &Path) -> crate::Result<Vec<Report>> {
    let catalog = catalog::load(catalog_path)?;
    std::fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    let listing = std::fs::read_dir(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    for item in listing.flatten() {
        let path = item.path();
        let stale = path.extension().is_some_and(|e| e == EXTENSION)
            && !catalog.entries.iter().any(|c| path.file_stem().is_some_and(|s| *s == *c.id));
        if stale {
            std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }

    let mut reports = Vec::new();
    for (order, entry) in catalog.entries.iter().enumerate() {
        let (mut pack, normalized) = convert(entry)?;
        pack.meta.order = u32::try_from(order).map_err(|_| "catalog: too many packs")?;
        let report = check(&pack, normalized)?;
        let bytes = pack.to_bytes().map_err(|e| format!("{}: {e}", entry.id))?;
        let path = out_dir.join(format!("{}.{EXTENSION}", entry.id));
        write_atomically(&path, &bytes)?;
        reports.push(Report { bytes: bytes.len(), ..report });
    }
    Ok(reports)
}

/// Reads one catalog entry's original recordings and converts them.
pub fn convert(entry: &Entry) -> crate::Result<(Pack, Normalized)> {
    let raw = match entry.kind {
        SourceKind::Mechvibes => sources::mechvibes::load(&entry.dir)?,
        SourceKind::Kbsim => sources::kbsim::load(&entry.dir)?,
    };
    finish(entry, raw)
}

/// The keys whose press sounds set a pack's loudness: the letters and Space,
/// which make up most of what is typed.
pub fn typing_keys() -> impl Iterator<Item = u8> {
    keys::KEYS
        .iter()
        .filter(|k| k.name.len() == 1 && k.name.as_bytes()[0].is_ascii_uppercase())
        .map(|k| k.code)
        .chain([keys::code::SPACE])
}

/// Cleans up the raw clips and assembles the pack. Returns it with what
/// loudness matching did.
pub fn finish(entry: &Entry, raw: RawPack) -> crate::Result<(Pack, Normalized)> {
    let rate = raw.rate;
    let params = dsp::TrimParams::default();
    // Keep only clips that some key uses; silent clips are dropped.
    let used: BTreeSet<usize> = raw.press.iter().chain(&raw.release).flatten().copied().collect();
    let mut remap: Vec<Option<usize>> = vec![None; raw.clips.len()];
    let mut clips: Vec<Vec<f32>> = Vec::new();
    for &index in &used {
        if let Some(trimmed) = dsp::trim_clip(&raw.clips[index], rate, &params) {
            remap[index] = Some(clips.len());
            clips.push(trimmed);
        }
    }
    if clips.is_empty() {
        return Err(format!("{}: every clip is silent", entry.id));
    }
    let fallback = raw.press[usize::from(keys::code::A)].and_then(|i| remap[i]).unwrap_or(0);

    let mut is_press = vec![false; clips.len()];
    let mut press = [None; KEY_SLOTS];
    let mut release = [None; KEY_SLOTS];
    for slot in 0..KEY_SLOTS {
        if let Some(index) = raw.press[slot] {
            let clip = remap[index].unwrap_or(fallback);
            is_press[clip] = true;
            press[slot] = Some(clip_index(clip, &entry.id)?);
        }
        if let Some(clip) = raw.release[slot].and_then(|i| remap[i]) {
            release[slot] = Some(clip_index(clip, &entry.id)?);
        }
    }
    // Loudness is measured on the typing keys, once per key, so the clip most keys share
    // weighs most; packs without them (tests) fall back to every press clip.
    let mut measured: Vec<usize> =
        typing_keys().filter_map(|c| press[usize::from(c)]).map(usize::from).collect();
    if measured.is_empty() {
        measured = (0..clips.len()).filter(|&i| is_press[i]).collect();
    }
    let normalized = dsp::normalize(&mut clips, &measured, rate, dsp::TARGET_DB, dsp::CEILING_DB);
    let mut seed = fnv1a(entry.id.as_bytes()) | 1;
    let clips: Vec<Vec<i16>> = clips.iter().map(|clip| dsp::to_i16(clip, &mut seed)).collect();
    let meta = Meta {
        id: entry.id.clone(),
        name: entry.name.clone(),
        variant: entry.variant.clone(),
        switch_type: entry.switch_type.clone(),
        credit: entry.credit.clone(),
        order: 0,
    };
    let pack =
        Pack::new(meta, rate, &clips, press, release).map_err(|e| format!("{}: {e}", entry.id))?;
    Ok((pack, normalized))
}

fn clip_index(clip: usize, id: &str) -> crate::Result<u16> {
    u16::try_from(clip).map_err(|_| format!("{id}: too many clips"))
}

/// Checks the invariants the app relies on and summarises the pack: every known
/// key has a press sound, every clip starts on its transient (≤ 1 ms), no
/// clip peaks above the ceiling, and the pack's measured loudness is within
/// [`LOUDNESS_TOLERANCE_DB`] of the target.
pub fn check(pack: &Pack, normalized: Normalized) -> crate::Result<Report> {
    let id = &pack.meta.id;
    let rate = pack.sample_rate as f32;
    let (mut max_lead_ms, mut max_peak, mut samples) = (0.0f32, 0i32, 0usize);
    for index in 0..pack.clip_count() {
        let clip = u16::try_from(index).ok().and_then(|i| pack.clip(i)).unwrap_or_default();
        let peak = clip.iter().map(|&s| i32::from(s).abs()).max().unwrap_or(0);
        let threshold = ((peak as f32 * dsp::db_to_lin(-45.0)) as i32).max(2);
        let lead = clip.iter().position(|&s| i32::from(s).abs() >= threshold).unwrap_or(0);
        max_lead_ms = max_lead_ms.max(lead as f32 * 1000.0 / rate);
        max_peak = max_peak.max(peak);
        samples += clip.len();
    }
    let max_peak_db = dsp::lin_to_db(max_peak as f32 / 32_767.0);
    let mapped = keys::KEYS.iter().filter(|k| pack.press_clip(u16::from(k.code)).is_some()).count();
    if max_lead_ms > 1.0 {
        return Err(format!("{id}: a clip starts {max_lead_ms:.2} ms late (limit 1 ms)"));
    }
    if max_peak_db > dsp::CEILING_DB + 0.1 {
        return Err(format!(
            "{id}: a clip peaks at {max_peak_db:.2} dBFS (ceiling {} dBFS)",
            dsp::CEILING_DB
        ));
    }
    if mapped != keys::KEYS.len() {
        return Err(format!("{id}: only {mapped} of {} keys have a sound", keys::KEYS.len()));
    }
    let loudness_db = measured_loudness_db(pack);
    if (loudness_db - dsp::TARGET_DB).abs() > LOUDNESS_TOLERANCE_DB {
        return Err(format!(
            "{id}: measured loudness {loudness_db:.2} dB is more than {LOUDNESS_TOLERANCE_DB} dB off the {} dB target",
            dsp::TARGET_DB
        ));
    }
    Ok(Report {
        id: id.clone(),
        clips: pack.clip_count(),
        bytes: 0,
        seconds: samples as f32 / rate,
        gain_db: normalized.gain_db,
        loudness_db,
        limited: normalized.limited,
        max_cut_db: normalized.max_cut_db,
        max_lead_ms,
        max_peak_db,
        keys: mapped,
        has_release: pack.has_release(),
    })
}

/// How far a built pack's measured loudness may be from [`dsp::TARGET_DB`].
pub const LOUDNESS_TOLERANCE_DB: f32 = 0.5;

/// The loudness of a built pack, measured on its 16-bit press sounds of the
/// [`typing_keys`] (once per key), independently of the gain the pipeline chose.
pub fn measured_loudness_db(pack: &Pack) -> f32 {
    let rate = pack.sample_rate;
    let levels: Vec<f32> = typing_keys()
        .filter_map(|c| pack.press_clip(u16::from(c)).and_then(|i| pack.clip(i)))
        .map(|clip| loudness::clip_loudness_db_i16(clip, rate))
        .collect();
    dsp::median(levels)
}

/// Writes through a temporary file and a rename, so a running app that reads
/// the pack never sees a half-written file.
fn write_atomically(path: &Path, bytes: &[u8]) -> crate::Result<()> {
    let tmp = path.with_extension(format!("{EXTENSION}.tmp"));
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

fn fnv1a(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0x811C_9DC5_u32, |h, &b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;
    use std::path::PathBuf;

    fn entry() -> Entry {
        Entry {
            id: "test".into(),
            name: "Test".into(),
            variant: String::new(),
            switch_type: "linear".into(),
            credit: "unit test".into(),
            kind: SourceKind::Mechvibes,
            dir: PathBuf::new(),
        }
    }

    fn click(amplitude: f32) -> Vec<f32> {
        let mut c = vec![0.0; 1_000];
        c.extend((0..4_000).map(|i| {
            let t = i as f32 / 44_100.0;
            amplitude * (-t * 120.0).exp() * (2.0 * PI * 1_800.0 * t).sin()
        }));
        c
    }

    #[test]
    fn finish_trims_normalises_and_maps_every_key() {
        let mut raw = RawPack::new(44_100);
        let a = raw.push(44_100, click(0.2), "a").unwrap();
        let silent = raw.push(44_100, vec![0.0; 500], "silent").unwrap();
        raw.push(44_100, click(0.9), "unused").unwrap();
        for key in keys::KEYS {
            raw.press[usize::from(key.code)] = Some(a);
        }
        raw.press[usize::from(keys::code::SPACE)] = Some(silent);

        let (pack, _) = finish(&entry(), raw).unwrap();
        assert_eq!(pack.clip_count(), 1, "silent and unused clips are dropped");
        assert_eq!(
            pack.press_clip(u16::from(keys::code::SPACE)),
            pack.press_clip(u16::from(keys::code::A)),
            "a silent clip falls back to A"
        );
        let normalized = Normalized { gain_db: 0.0, limited: 0, max_cut_db: 0.0 };
        let report = check(&pack, normalized).unwrap();
        assert!((report.loudness_db - dsp::TARGET_DB).abs() < 0.1, "{report:?}");
        assert!(report.max_lead_ms <= 1.0);
        assert!(report.max_peak_db <= dsp::CEILING_DB + 0.1);
        assert!(!report.has_release);
        assert_eq!(report.keys, keys::KEYS.len());
    }

    #[test]
    fn check_rejects_a_pack_that_leaves_keys_silent() {
        let mut raw = RawPack::new(44_100);
        let a = raw.push(44_100, click(0.3), "a").unwrap();
        raw.press[usize::from(keys::code::A)] = Some(a);
        let (pack, gain) = finish(&entry(), raw).unwrap();
        let err = check(&pack, gain).unwrap_err();
        assert!(err.contains("keys have a sound"), "{err}");
    }

    /// Packs recorded with release sounds: every kbsim pack, and the one
    /// Mechvibes pack with key-up defines.
    const WITH_RELEASE: [&str; 11] = [
        "cherry-mx-black-abs",
        "gateron-black-ink",
        "gateron-red-ink",
        "novelkeys-cream",
        "alpaca",
        "turquoise-tealios",
        "holy-panda",
        "topre",
        "kailh-box-navy",
        "skcm-blue-alps",
        "buckling-spring",
    ];

    /// All 21 bundled soundpacks, end to end from the committed source audio:
    /// each decodes, maps every key, starts on its transient, stays under the
    /// ceiling and lands on the loudness target, measured again on the file.
    #[test]
    fn every_bundled_soundpack_converts_and_is_loudness_matched() {
        let root = crate::workspace_root();
        let out = crate::testutil::temp_dir("packs");
        std::fs::write(out.join("stale.clickypack"), b"old").unwrap();
        let reports = build_all(&root.join("assets/soundpacks/catalog.json"), &out).unwrap();
        assert_eq!(reports.len(), 21);
        assert!(!out.join("stale.clickypack").exists(), "stale packs are removed");

        let mut loudness = Vec::new();
        let mut groups = [0; 3];
        for (order, report) in reports.iter().enumerate() {
            let bytes = std::fs::read(out.join(format!("{}.clickypack", report.id))).unwrap();
            let pack = Pack::from_bytes(&bytes).unwrap();
            let id = &pack.meta.id;
            // `check` measures the 16-bit file: onset ≤ 1 ms, peak, every key, loudness.
            let again = check(&pack, Normalized { gain_db: 0.0, limited: 0, max_cut_db: 0.0 })
                .unwrap_or_else(|e| panic!("{e}"));
            assert!(again.max_lead_ms <= 1.0, "{again:?}");
            assert!(again.max_peak_db <= dsp::CEILING_DB + 0.1, "{again:?}");
            assert_eq!(again.keys, keys::KEYS.len());
            assert!(
                (again.loudness_db - dsp::TARGET_DB).abs() <= LOUDNESS_TOLERANCE_DB,
                "{again:?}"
            );
            assert!(
                report.max_cut_db <= 3.5,
                "{id}: a clip is {:.1} dB quieter than its pack to respect the ceiling",
                report.max_cut_db
            );
            loudness.push(again.loudness_db);

            assert_eq!(pack.has_release(), WITH_RELEASE.contains(&id.as_str()), "{id}");
            assert_eq!(Pack::info(&bytes).unwrap().has_release, pack.has_release(), "{id}");
            assert_eq!(pack.meta.order as usize, order, "{id} keeps its catalog position");
            assert!(!pack.meta.name.is_empty() && pack.meta.credit.contains("MIT"), "{id}");
            let group = crate::catalog::SWITCH_TYPES
                .iter()
                .position(|t| *t == pack.meta.switch_type)
                .unwrap_or_else(|| panic!("{id}: {}", pack.meta.switch_type));
            groups[group] += 1;
            assert_ne!(
                pack.press_clip(u16::from(keys::code::SPACE)),
                pack.press_clip(u16::from(keys::code::A)),
                "{id}: Space has its own recording"
            );
        }
        assert_eq!(groups, [9, 7, 5], "Linear, Tactile, Clicky");
        let spread = loudness.iter().copied().fold(f32::MIN, f32::max)
            - loudness.iter().copied().fold(f32::MAX, f32::min);
        assert!(spread <= 1.0, "loudness spread across packs {spread:.2} dB: {loudness:?}");

        let brown =
            Pack::from_bytes(&std::fs::read(out.join("cherry-mx-brown-pbt.clickypack")).unwrap())
                .unwrap();
        assert_eq!(brown.sample_rate, 44_100);
        assert_eq!(
            (brown.meta.name.as_str(), brown.meta.variant.as_str()),
            ("Cherry MX Brown", "PBT")
        );
        assert!(brown.clip_count() > 50, "Mechvibes records most keys separately");
        let oreo = Pack::from_bytes(&std::fs::read(out.join("everglide-oreo.clickypack")).unwrap())
            .unwrap();
        assert_eq!(oreo.sample_rate, 48_000, "packs keep their recorded rate");
    }
}
