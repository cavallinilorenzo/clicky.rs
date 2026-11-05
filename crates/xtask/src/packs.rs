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
use crate::dsp;
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
        "{:<24} {:>5} {:>8} {:>7} {:>8} {:>8} {:>9} {:>5} {:>7}",
        "pack", "clips", "size", "audio", "gain", "lead", "peak", "keys", "release"
    );
    for r in &reports {
        println!(
            "{:<24} {:>5} {:>6}KB {:>6.1}s {:>+6.1}dB {:>6.2}ms {:>6.1}dBFS {:>5} {:>7}",
            r.id,
            r.clips,
            r.bytes / 1024,
            r.seconds,
            r.gain_db,
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
    for entry in &catalog.entries {
        let (pack, gain_db) = convert(entry)?;
        let report = check(&pack, gain_db)?;
        let bytes = pack.to_bytes().map_err(|e| format!("{}: {e}", entry.id))?;
        let path = out_dir.join(format!("{}.{EXTENSION}", entry.id));
        write_atomically(&path, &bytes)?;
        reports.push(Report { bytes: bytes.len(), ..report });
    }
    Ok(reports)
}

/// Reads one catalog entry's original recordings and converts them.
pub fn convert(entry: &Entry) -> crate::Result<(Pack, f32)> {
    let raw = match entry.kind {
        SourceKind::Mechvibes => sources::mechvibes::load(&entry.dir)?,
    };
    finish(entry, raw)
}

/// Cleans up the raw clips and assembles the pack. Returns it with the gain applied (dB).
pub fn finish(entry: &Entry, raw: RawPack) -> crate::Result<(Pack, f32)> {
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
    let gain_db = dsp::normalize(&mut clips, &is_press, rate, dsp::TARGET_DB, dsp::CEILING_DB);
    let mut seed = fnv1a(entry.id.as_bytes()) | 1;
    let clips: Vec<Vec<i16>> = clips.iter().map(|clip| dsp::to_i16(clip, &mut seed)).collect();
    let meta = Meta {
        id: entry.id.clone(),
        name: entry.name.clone(),
        variant: entry.variant.clone(),
        switch_type: entry.switch_type.clone(),
        credit: entry.credit.clone(),
    };
    let pack =
        Pack::new(meta, rate, &clips, press, release).map_err(|e| format!("{}: {e}", entry.id))?;
    Ok((pack, gain_db))
}

fn clip_index(clip: usize, id: &str) -> crate::Result<u16> {
    u16::try_from(clip).map_err(|_| format!("{id}: too many clips"))
}

/// Checks the invariants the app relies on and summarises the pack: every known
/// key has a press sound, every clip starts on its transient (≤ 1 ms) and no
/// clip peaks above the ceiling.
pub fn check(pack: &Pack, gain_db: f32) -> crate::Result<Report> {
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
    Ok(Report {
        id: id.clone(),
        clips: pack.clip_count(),
        bytes: 0,
        seconds: samples as f32 / rate,
        gain_db,
        max_lead_ms,
        max_peak_db,
        keys: mapped,
        has_release: pack.has_release(),
    })
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
        let report = check(&pack, 0.0).unwrap();
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

    /// The tracer's soundpack, end to end from the committed source audio.
    #[test]
    fn default_soundpack_converts_from_its_committed_source() {
        let root = crate::workspace_root();
        let out = crate::testutil::temp_dir("packs");
        std::fs::write(out.join("stale.clickypack"), b"old").unwrap();
        let reports = build_all(&root.join("assets/soundpacks/catalog.json"), &out).unwrap();

        let brown = reports.iter().find(|r| r.id == "cherry-mx-brown-pbt").unwrap();
        assert_eq!(brown.keys, keys::KEYS.len());
        assert!(brown.max_lead_ms <= 1.0, "{brown:?}");
        assert!(brown.max_peak_db <= dsp::CEILING_DB + 0.1, "{brown:?}");
        assert!(brown.clips > 50, "Mechvibes records most keys separately: {brown:?}");
        assert!(!out.join("stale.clickypack").exists(), "stale packs are removed");

        let bytes = std::fs::read(out.join("cherry-mx-brown-pbt.clickypack")).unwrap();
        let pack = Pack::from_bytes(&bytes).unwrap();
        assert_eq!(pack.sample_rate, 44_100);
        assert_eq!(pack.meta.name, "Cherry MX Brown");
        assert_eq!(pack.meta.variant, "PBT");
        assert_ne!(
            pack.press_clip(u16::from(keys::code::SPACE)),
            pack.press_clip(u16::from(keys::code::A)),
            "Space has its own recording"
        );
    }
}
