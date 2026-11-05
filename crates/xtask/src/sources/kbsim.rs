//! kbsim packs: `press/GENERIC_R0`…`R4` (one recording per keyboard row) plus
//! `SPACE`, `ENTER` and `BACKSPACE`, and `release/GENERIC` plus the same three
//! specials. Files may be `.mp3`, `.wav` or `.ogg`. Anything else in the
//! directory (e.g. Blue Alps' `release/GENERIC_long.mp3`) is ignored, as kbsim
//! itself ignores it.
//!
//! Key mapping: the row comes from the physical key table (`row` 0 = function
//! row … 4 = bottom letter row; the space-bar row uses R4, like kbsim's
//! `default:` case). Space, Return / Keypad Enter and Delete use their own
//! recordings, falling back to their row. Every key releases with its own
//! release recording, else `release/GENERIC`.
//!
//! Derived from Rustyvibes 2 (MIT, © 2021–2026 Kunal Bagaria,
//! https://github.com/KunalBagaria/rustyvibes-v2), `crates/xtask/src/sources/kbsim.rs`.

use std::path::{Path, PathBuf};

use clicky_pack::keys::{self, code};

use super::RawPack;
use crate::{decode::decode_file, dsp};

/// Rows with a `press/GENERIC_R<n>` recording.
const ROWS: u8 = 5;

pub fn load(dir: &Path) -> crate::Result<RawPack> {
    let mut pack = RawPack::new(0);
    let rows: Vec<Option<usize>> = (0..ROWS)
        .map(|row| load_clip(dir, &format!("press/GENERIC_R{row}"), &mut pack))
        .collect::<crate::Result<_>>()?;
    let space = load_clip(dir, "press/SPACE", &mut pack)?;
    let enter = load_clip(dir, "press/ENTER", &mut pack)?;
    let backspace = load_clip(dir, "press/BACKSPACE", &mut pack)?;
    let up_generic = load_clip(dir, "release/GENERIC", &mut pack)?;
    let up_space = load_clip(dir, "release/SPACE", &mut pack)?;
    let up_enter = load_clip(dir, "release/ENTER", &mut pack)?;
    let up_backspace = load_clip(dir, "release/BACKSPACE", &mut pack)?;

    let any_row = rows
        .iter()
        .flatten()
        .next()
        .copied()
        .ok_or_else(|| format!("{}: no press/GENERIC_R* recordings", dir.display()))?;
    for key in keys::KEYS {
        let row = rows[usize::from(key.row.min(ROWS - 1))].unwrap_or(any_row);
        let (press, release) = match key.code {
            code::SPACE => (space.unwrap_or(row), up_space.or(up_generic)),
            code::RETURN | code::KEYPAD_ENTER => (enter.unwrap_or(row), up_enter.or(up_generic)),
            code::DELETE => (backspace.unwrap_or(row), up_backspace.or(up_generic)),
            _ => (row, up_generic),
        };
        pack.press[usize::from(key.code)] = Some(press);
        pack.release[usize::from(key.code)] = release;
    }
    Ok(pack)
}

fn load_clip(dir: &Path, stem: &str, pack: &mut RawPack) -> crate::Result<Option<usize>> {
    let Some(path) = find_audio(dir, stem) else { return Ok(None) };
    let mut audio = decode_file(&path)?;
    dsp::dc_block(&mut audio.samples, audio.rate);
    pack.push(audio.rate, audio.samples, &path.display().to_string()).map(Some)
}

fn find_audio(dir: &Path, stem: &str) -> Option<PathBuf> {
    ["mp3", "wav", "ogg"]
        .iter()
        .map(|ext| dir.join(format!("{stem}.{ext}")))
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{click_i16, temp_dir, write_wav};

    fn write_stems(dir: &Path, stems: &[&str]) {
        std::fs::create_dir_all(dir.join("press")).unwrap();
        std::fs::create_dir_all(dir.join("release")).unwrap();
        for (i, stem) in stems.iter().enumerate() {
            let click = click_i16(5, 1_000 + i * 10, 0.5);
            write_wav(&dir.join(format!("{stem}.wav")), 44_100, 1, &click);
        }
    }

    #[test]
    fn rows_and_special_keys_map_like_kbsim() {
        let dir = temp_dir("kbsim");
        write_stems(
            &dir,
            &[
                "press/GENERIC_R0",
                "press/GENERIC_R1",
                "press/GENERIC_R2",
                "press/GENERIC_R3",
                "press/GENERIC_R4",
                "press/SPACE",
                "press/ENTER",
                "release/GENERIC",
                "release/SPACE",
            ],
        );
        let pack = load(&dir).unwrap();
        let press = |c: u8| pack.press[usize::from(c)].unwrap();
        let release = |c: u8| pack.release[usize::from(c)];
        assert_eq!(press(code::ESCAPE), 0, "function row → R0");
        assert_eq!(press(0x12), 1, "number row (1) → R1");
        assert_eq!(press(0x0C), 2, "Q → R2");
        assert_eq!(press(code::A), 3, "home row → R3");
        assert_eq!(press(0x06), 4, "Z → R4");
        assert_eq!(press(code::COMMAND), 4, "space-bar row → R4");
        assert_eq!(press(code::SPACE), 5);
        assert_eq!(press(code::RETURN), 6);
        assert_eq!(press(code::KEYPAD_ENTER), 6);
        assert_eq!(press(code::DELETE), 1, "no BACKSPACE recording → its row");
        assert_eq!(release(code::A), Some(7));
        assert_eq!(release(code::SHIFT), Some(7), "modifiers release too");
        assert_eq!(release(code::SPACE), Some(8));
        assert_eq!(release(code::RETURN), Some(7), "no ENTER release → generic");
        for key in keys::KEYS {
            assert!(pack.press[usize::from(key.code)].is_some(), "{} has no press", key.name);
        }
    }

    #[test]
    fn a_missing_row_falls_back_and_extra_files_are_ignored() {
        let dir = temp_dir("kbsim-partial");
        write_stems(&dir, &["press/GENERIC_R2", "release/GENERIC_long"]);
        let pack = load(&dir).unwrap();
        assert_eq!(pack.clips.len(), 1, "GENERIC_long is not loaded");
        assert_eq!(pack.press[usize::from(code::ESCAPE)], Some(0));
        assert!(pack.release.iter().all(Option::is_none), "no release recordings");
    }

    #[test]
    fn missing_row_recordings_is_an_error() {
        let dir = temp_dir("kbsim-empty");
        let err = load(&dir).unwrap_err();
        assert!(err.contains("GENERIC_R"), "{err}");
    }
}
