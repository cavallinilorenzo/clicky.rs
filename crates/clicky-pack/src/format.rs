//! The `.clickypack` binary soundpack format.
//!
//! Layout (all integers little-endian):
//!
//! | offset | size  | field                                              |
//! |-------:|------:|----------------------------------------------------|
//! | 0      | 4     | magic `CLKP`                                       |
//! | 4      | 2     | format version (1)                                 |
//! | 6      | 2     | flags (bit 0: the pack has release sounds)         |
//! | 8      | 4     | sample rate in Hz (mono audio)                     |
//! | 12     | 4     | clip count `n`                                     |
//! | 16     | 4     | PCM length in `i16` samples                        |
//! | 20     | 4     | metadata length in bytes                           |
//! | 24     | 256   | press map: 128 × `u16` clip index (`0xFFFF` none)  |
//! | 280    | 256   | release map, same shape                            |
//! | 536    | 8 × n | clip table: n × (`start: u32`, `len: u32`) samples |
//! | …      | …     | metadata: UTF-8 `key=value` lines                  |
//! | …      | …     | PCM: mono `i16`                                    |
//!
//! Key maps are indexed by macOS virtual keycode. The layout is a simplified
//! version of the `.rvpack` format of Rustyvibes 2 (MIT, © 2021–2026 Kunal
//! Bagaria, https://github.com/KunalBagaria/rustyvibes-v2).

use std::fmt;

/// File magic.
pub const MAGIC: [u8; 4] = *b"CLKP";
/// Current format version.
pub const VERSION: u16 = 1;
/// Number of keycode slots per key map (macOS virtual keycodes are 7-bit).
pub const KEY_SLOTS: usize = 128;
/// Flag bit: the pack contains release sounds.
const FLAG_HAS_RELEASE: u16 = 1;
/// Key map entry meaning "no sound for this key".
const NO_CLIP: u16 = u16::MAX;

const HEADER_LEN: usize = 24;
const MAP_LEN: usize = KEY_SLOTS * 2;
const CLIPS_OFFSET: usize = HEADER_LEN + 2 * MAP_LEN;
const CLIP_ENTRY_LEN: usize = 8;

/// Location of a clip inside the PCM block, in samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clip {
    pub start: u32,
    pub len: u32,
}

/// Human-facing description of a soundpack.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Meta {
    /// Stable identifier, e.g. `cherry-mx-brown-pbt`.
    pub id: String,
    /// Switch name, e.g. `Cherry MX Brown`.
    pub name: String,
    /// Optional variant such as `PBT`; empty when there is none.
    pub variant: String,
    /// Switch type: `linear`, `tactile` or `clicky`.
    pub switch_type: String,
    /// Who recorded the sounds, and the licence.
    pub credit: String,
    /// Position in the catalog, which orders the packs within a switch type.
    pub order: u32,
}

impl Meta {
    fn encode(&self) -> Result<String, FormatError> {
        if self.id.is_empty() {
            return Err(FormatError::BadMeta);
        }
        let fields = [
            ("id", &self.id),
            ("name", &self.name),
            ("variant", &self.variant),
            ("switch_type", &self.switch_type),
            ("credit", &self.credit),
        ];
        let order = self.order.to_string();
        let mut out = String::new();
        for (key, value) in fields.into_iter().chain([("order", &order)]) {
            if value.contains(['\n', '\r']) {
                return Err(FormatError::BadMeta);
            }
            out.push_str(key);
            out.push('=');
            out.push_str(value);
            out.push('\n');
        }
        Ok(out)
    }

    fn decode(bytes: &[u8]) -> Result<Meta, FormatError> {
        let text = std::str::from_utf8(bytes).map_err(|_| FormatError::BadMeta)?;
        let mut meta = Meta::default();
        for line in text.lines() {
            let (key, value) = line.split_once('=').ok_or(FormatError::BadMeta)?;
            let value = value.to_owned();
            match key {
                "id" => meta.id = value,
                "name" => meta.name = value,
                "variant" => meta.variant = value,
                "switch_type" => meta.switch_type = value,
                "credit" => meta.credit = value,
                "order" => meta.order = value.parse().map_err(|_| FormatError::BadMeta)?,
                _ => {} // Unknown keys are ignored so newer writers stay readable.
            }
        }
        if meta.id.is_empty() {
            return Err(FormatError::BadMeta);
        }
        Ok(meta)
    }
}

/// Bytes of a pack file that [`Pack::info_len`] needs to see.
pub const INFO_HEADER_LEN: usize = HEADER_LEN;

/// What the soundpack picker shows, read without the audio ([`Pack::info`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackInfo {
    pub meta: Meta,
    pub sample_rate: u32,
    /// Whether any key has a release sound (Key Release Sounds is unavailable otherwise).
    pub has_release: bool,
}

/// Why a pack could not be written or read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormatError {
    TooShort,
    BadMagic,
    UnsupportedVersion(u16),
    BadSampleRate,
    OutOfBounds(&'static str),
    BadClipRef { key: usize },
    BadClip { index: usize },
    BadMeta,
    TooManyClips,
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FormatError::TooShort => write!(f, "file is too short to be a soundpack"),
            FormatError::BadMagic => write!(f, "not a soundpack (bad magic)"),
            FormatError::UnsupportedVersion(v) => write!(f, "unsupported soundpack version {v}"),
            FormatError::BadSampleRate => write!(f, "invalid sample rate"),
            FormatError::OutOfBounds(what) => write!(f, "{what} lies outside the file"),
            FormatError::BadClipRef { key } => write!(f, "key slot {key} refers to a missing clip"),
            FormatError::BadClip { index } => write!(f, "clip {index} is empty or out of range"),
            FormatError::BadMeta => write!(f, "metadata is malformed"),
            FormatError::TooManyClips => write!(f, "too many clips (maximum 65534)"),
        }
    }
}

impl std::error::Error for FormatError {}

/// A soundpack in memory: mono `i16` clips and, per macOS keycode, the press
/// sound and optional release sound.
#[derive(Clone, Debug, PartialEq)]
pub struct Pack {
    pub sample_rate: u32,
    pub meta: Meta,
    press: [Option<u16>; KEY_SLOTS],
    release: [Option<u16>; KEY_SLOTS],
    clips: Vec<Clip>,
    pcm: Vec<i16>,
}

impl Pack {
    /// Assembles a pack. `press` and `release` index into `clips`.
    pub fn new(
        meta: Meta,
        sample_rate: u32,
        clips: &[Vec<i16>],
        press: [Option<u16>; KEY_SLOTS],
        release: [Option<u16>; KEY_SLOTS],
    ) -> Result<Pack, FormatError> {
        if clips.len() >= usize::from(NO_CLIP) {
            return Err(FormatError::TooManyClips);
        }
        let mut table = Vec::with_capacity(clips.len());
        let mut pcm = Vec::with_capacity(clips.iter().map(Vec::len).sum());
        for clip in clips {
            let start = u32::try_from(pcm.len()).map_err(|_| FormatError::TooManyClips)?;
            let len = u32::try_from(clip.len()).map_err(|_| FormatError::TooManyClips)?;
            table.push(Clip { start, len });
            pcm.extend_from_slice(clip);
        }
        let pack = Pack { sample_rate, meta, press, release, clips: table, pcm };
        pack.validate()?;
        Ok(pack)
    }

    /// Number of distinct clips.
    pub fn clip_count(&self) -> usize {
        self.clips.len()
    }

    /// The samples of clip `index`.
    pub fn clip(&self, index: u16) -> Option<&[i16]> {
        let clip = self.clips.get(usize::from(index))?;
        let start = clip.start as usize;
        self.pcm.get(start..start + clip.len as usize)
    }

    /// Index of the press sound for macOS keycode `keycode`, if the pack maps it.
    pub fn press_clip(&self, keycode: u16) -> Option<u16> {
        self.press.get(usize::from(keycode)).copied().flatten()
    }

    /// Index of the release sound for macOS keycode `keycode`, if any.
    pub fn release_clip(&self, keycode: u16) -> Option<u16> {
        self.release.get(usize::from(keycode)).copied().flatten()
    }

    /// Whether any key has a release sound.
    pub fn has_release(&self) -> bool {
        self.release.iter().any(Option::is_some)
    }

    /// Serialises the pack.
    pub fn to_bytes(&self) -> Result<Vec<u8>, FormatError> {
        self.validate()?;
        let meta = self.meta.encode()?;
        let pcm_len = u32::try_from(self.pcm.len()).map_err(|_| FormatError::TooManyClips)?;
        let meta_len = u32::try_from(meta.len()).map_err(|_| FormatError::BadMeta)?;
        let mut out = Vec::with_capacity(
            CLIPS_OFFSET + self.clips.len() * CLIP_ENTRY_LEN + meta.len() + self.pcm.len() * 2,
        );
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        let flags = if self.has_release() { FLAG_HAS_RELEASE } else { 0 };
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(&self.sample_rate.to_le_bytes());
        out.extend_from_slice(&(self.clips.len() as u32).to_le_bytes());
        out.extend_from_slice(&pcm_len.to_le_bytes());
        out.extend_from_slice(&meta_len.to_le_bytes());
        for map in [&self.press, &self.release] {
            for entry in map {
                out.extend_from_slice(&entry.unwrap_or(NO_CLIP).to_le_bytes());
            }
        }
        for clip in &self.clips {
            out.extend_from_slice(&clip.start.to_le_bytes());
            out.extend_from_slice(&clip.len.to_le_bytes());
        }
        out.extend_from_slice(meta.as_bytes());
        for sample in &self.pcm {
            out.extend_from_slice(&sample.to_le_bytes());
        }
        Ok(out)
    }

    /// Reads only the description of a pack from the start of its file, without
    /// its audio. `prefix` must hold at least [`Pack::info_len`] bytes.
    pub fn info(prefix: &[u8]) -> Result<PackInfo, FormatError> {
        let needed = Pack::info_len(prefix)?;
        if prefix.len() < needed {
            return Err(FormatError::TooShort);
        }
        let clip_count = read_u32(prefix, 12) as usize;
        let meta_offset = CLIPS_OFFSET + clip_count * CLIP_ENTRY_LEN;
        let meta = Meta::decode(&prefix[meta_offset..needed])?;
        let flags = read_u16(prefix, 6);
        Ok(PackInfo {
            meta,
            sample_rate: read_u32(prefix, 8),
            has_release: flags & FLAG_HAS_RELEASE != 0,
        })
    }

    /// How many bytes from the start of a pack file [`Pack::info`] needs, given
    /// at least the first [`INFO_HEADER_LEN`] bytes.
    pub fn info_len(header: &[u8]) -> Result<usize, FormatError> {
        if header.len() < INFO_HEADER_LEN {
            return Err(FormatError::TooShort);
        }
        if header[0..4] != MAGIC {
            return Err(FormatError::BadMagic);
        }
        let version = read_u16(header, 4);
        if version != VERSION {
            return Err(FormatError::UnsupportedVersion(version));
        }
        let clip_count = read_u32(header, 12) as usize;
        if clip_count >= usize::from(NO_CLIP) {
            return Err(FormatError::TooManyClips);
        }
        Ok(CLIPS_OFFSET + clip_count * CLIP_ENTRY_LEN + read_u32(header, 20) as usize)
    }

    /// Parses and validates a serialised pack.
    pub fn from_bytes(bytes: &[u8]) -> Result<Pack, FormatError> {
        if bytes.len() < CLIPS_OFFSET {
            return Err(FormatError::TooShort);
        }
        if bytes[0..4] != MAGIC {
            return Err(FormatError::BadMagic);
        }
        let version = read_u16(bytes, 4);
        if version != VERSION {
            return Err(FormatError::UnsupportedVersion(version));
        }
        let sample_rate = read_u32(bytes, 8);
        let clip_count = read_u32(bytes, 12) as usize;
        let pcm_len = read_u32(bytes, 16) as usize;
        let meta_len = read_u32(bytes, 20) as usize;
        if clip_count >= usize::from(NO_CLIP) {
            return Err(FormatError::TooManyClips);
        }
        let mut press = [None; KEY_SLOTS];
        let mut release = [None; KEY_SLOTS];
        for (slot, (p, r)) in press.iter_mut().zip(release.iter_mut()).enumerate() {
            *p = Some(read_u16(bytes, HEADER_LEN + slot * 2)).filter(|&c| c != NO_CLIP);
            *r = Some(read_u16(bytes, HEADER_LEN + MAP_LEN + slot * 2)).filter(|&c| c != NO_CLIP);
        }
        let meta_offset = CLIPS_OFFSET + clip_count * CLIP_ENTRY_LEN;
        let pcm_offset = meta_offset + meta_len;
        if bytes.len() < meta_offset {
            return Err(FormatError::OutOfBounds("clip table"));
        }
        if bytes.len() < pcm_offset {
            return Err(FormatError::OutOfBounds("metadata"));
        }
        if bytes.len() != pcm_offset + pcm_len * 2 {
            return Err(FormatError::OutOfBounds("PCM"));
        }
        let clips = (0..clip_count)
            .map(|i| {
                let at = CLIPS_OFFSET + i * CLIP_ENTRY_LEN;
                Clip { start: read_u32(bytes, at), len: read_u32(bytes, at + 4) }
            })
            .collect();
        let meta = Meta::decode(&bytes[meta_offset..pcm_offset])?;
        let pcm = bytes[pcm_offset..]
            .as_chunks::<2>()
            .0
            .iter()
            .copied()
            .map(i16::from_le_bytes)
            .collect();
        let pack = Pack { sample_rate, meta, press, release, clips, pcm };
        pack.validate()?;
        Ok(pack)
    }

    fn validate(&self) -> Result<(), FormatError> {
        if !(8_000..=384_000).contains(&self.sample_rate) {
            return Err(FormatError::BadSampleRate);
        }
        for (index, clip) in self.clips.iter().enumerate() {
            let end = u64::from(clip.start) + u64::from(clip.len);
            if clip.len == 0 || end > self.pcm.len() as u64 {
                return Err(FormatError::BadClip { index });
            }
        }
        for (key, entry) in self.press.iter().chain(&self.release).enumerate() {
            if entry.is_some_and(|clip| usize::from(clip) >= self.clips.len()) {
                return Err(FormatError::BadClipRef { key: key % KEY_SLOTS });
            }
        }
        Ok(())
    }
}

fn read_u16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn read_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> Meta {
        Meta {
            id: "test-pack".into(),
            name: "Test Switch".into(),
            variant: "PBT".into(),
            switch_type: "tactile".into(),
            credit: "unit test".into(),
            order: 3,
        }
    }

    fn sample_pack() -> Pack {
        let clips = vec![vec![1, -2, 3], vec![100; 5], vec![-7, 7]];
        let mut press = [None; KEY_SLOTS];
        let mut release = [None; KEY_SLOTS];
        press[0x00] = Some(0); // A
        press[0x31] = Some(1); // Space
        release[0x31] = Some(2);
        Pack::new(meta(), 44_100, &clips, press, release).unwrap()
    }

    #[test]
    fn round_trip_preserves_clips_maps_and_metadata() {
        let pack = sample_pack();
        let parsed = Pack::from_bytes(&pack.to_bytes().unwrap()).unwrap();
        assert_eq!(parsed, pack);
        assert_eq!(parsed.meta.name, "Test Switch");
        assert_eq!(parsed.sample_rate, 44_100);
        assert_eq!(parsed.clip(parsed.press_clip(0x31).unwrap()), Some(&[100i16; 5][..]));
        assert_eq!(parsed.clip(parsed.press_clip(0x00).unwrap()), Some(&[1i16, -2, 3][..]));
        assert_eq!(parsed.release_clip(0x31), Some(2));
        assert_eq!(parsed.press_clip(0x01), None, "S is not mapped");
        assert_eq!(parsed.press_clip(500), None, "out-of-range keycodes are unmapped");
        assert!(parsed.has_release());
    }

    #[test]
    fn rejects_files_that_are_not_soundpacks() {
        let mut bytes = sample_pack().to_bytes().unwrap();
        assert_eq!(Pack::from_bytes(&bytes[..10]), Err(FormatError::TooShort));
        assert_eq!(
            Pack::from_bytes(&bytes[..bytes.len() - 1]),
            Err(FormatError::OutOfBounds("PCM"))
        );
        bytes[4] = 9;
        assert_eq!(Pack::from_bytes(&bytes), Err(FormatError::UnsupportedVersion(9)));
        bytes[0] = b'X';
        assert_eq!(Pack::from_bytes(&bytes), Err(FormatError::BadMagic));
    }

    #[test]
    fn rejects_a_key_that_points_to_a_missing_clip() {
        let mut press = [None; KEY_SLOTS];
        press[3] = Some(7);
        let err = Pack::new(meta(), 44_100, &[vec![1]], press, [None; KEY_SLOTS]).unwrap_err();
        assert_eq!(err, FormatError::BadClipRef { key: 3 });
    }

    #[test]
    fn rejects_empty_clips_and_missing_ids() {
        let none = [None; KEY_SLOTS];
        assert_eq!(
            Pack::new(meta(), 44_100, &[vec![]], none, none),
            Err(FormatError::BadClip { index: 0 })
        );
        let nameless = Meta { id: String::new(), ..meta() };
        let pack = Pack::new(nameless, 44_100, &[vec![1]], none, none).unwrap();
        assert_eq!(pack.to_bytes(), Err(FormatError::BadMeta));
    }

    #[test]
    fn info_reads_the_description_from_the_start_of_the_file_only() {
        let pack = Pack { meta: Meta { order: 7, ..meta() }, ..sample_pack() };
        let bytes = pack.to_bytes().unwrap();
        let needed = Pack::info_len(&bytes[..INFO_HEADER_LEN]).unwrap();
        assert!(needed < bytes.len(), "the audio is not needed");
        let info = Pack::info(&bytes[..needed]).unwrap();
        assert_eq!(info.meta, pack.meta);
        assert_eq!(info.meta.order, 7);
        assert_eq!(info.sample_rate, 44_100);
        assert!(info.has_release);
        assert_eq!(Pack::info(&bytes[..needed - 1]), Err(FormatError::TooShort));
        assert_eq!(Pack::info_len(b"nope"), Err(FormatError::TooShort));
        assert_eq!(Pack::info_len(&[0; INFO_HEADER_LEN]), Err(FormatError::BadMagic));
    }

    #[test]
    fn metadata_without_an_order_reads_as_first() {
        let meta = Meta::decode(b"id=x\nname=X\n").unwrap();
        assert_eq!(meta.order, 0);
        assert_eq!(Meta::decode(b"id=x\norder=soon\n"), Err(FormatError::BadMeta));
    }

    #[test]
    fn a_pack_without_release_sounds_says_so() {
        let mut press = [None; KEY_SLOTS];
        press[0] = Some(0);
        let pack = Pack::new(meta(), 48_000, &[vec![5, 6]], press, [None; KEY_SLOTS]).unwrap();
        let bytes = pack.to_bytes().unwrap();
        assert!(!Pack::from_bytes(&bytes).unwrap().has_release());
        assert!(!Pack::info(&bytes).unwrap().has_release);
    }
}
