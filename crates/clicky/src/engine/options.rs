//! The sound options (Key Release Sounds, Spatial Stereo) and Natural
//! Variation, which is not an option: it is always on. The settings, and the
//! laws the [`Voicer`](super::Voicer) applies.
//!
//! - **Natural Variation** (always on): each keystroke's loudness varies within
//!   ±[`GAIN_JITTER_DB`] and its pitch (the resampler's rate) within
//!   ±[`PITCH_JITTER_CENTS`], uniformly. A saved `NaturalVariation` setting
//!   from an older version is ignored.
//! - **Spatial Stereo**: a key's pan follows its horizontal position on the
//!   keyboard ([`key_pan`]), at most ±[`PAN_WIDTH`] (27° either side of the
//!   centre, +2 dB on the near side). Off: centred.
//!
//! Derived from Rustyvibes 2 (MIT, © 2021–2026 Kunal Bagaria,
//! https://github.com/KunalBagaria/rustyvibes-v2), `crates/rustyvibes/src/engine/mod.rs`
//! (the jitter and pan constants) and `crates/rustyvibes/src/engine/rng.rs`.

use clicky_pack::keys;

use super::Shared;
use crate::settings::{Setting, Store};

/// Play the soundpack's release sound when a key comes up (if it has them).
pub const RELEASE_SOUNDS: Setting<bool> = Setting { key: "KeyReleaseSounds", default: true };
/// Pan each key's sound by where it sits on the keyboard.
pub const SPATIAL_STEREO: Setting<bool> = Setting { key: "SpatialStereo", default: true };

/// Loudness variation with Natural Variation: ± this many dB.
pub const GAIN_JITTER_DB: f32 = 1.5;
/// Pitch variation with Natural Variation: ± this many cents (±2.0 %).
pub const PITCH_JITTER_CENTS: f32 = 35.0;
/// Largest pan with Spatial Stereo on (1 would be hard left or right).
pub const PAN_WIDTH: f32 = 0.4;

/// The horizontal centre of the main key block, in key units (H), and its
/// half width (Escape to Delete is about 0.5–14.5).
const CENTRE_X: f32 = 7.25;
const HALF_WIDTH: f32 = 7.75;

/// Sets the engine's options from the saved settings (at launch, before any key press).
pub fn restore(shared: &Shared, store: &Store) {
    shared.set_release_sounds(store.get(&RELEASE_SOUNDS));
    shared.set_spatial(store.get(&SPATIAL_STEREO));
}

/// The pan of a key: −[`PAN_WIDTH`] for the left edge of the keyboard,
/// +[`PAN_WIDTH`] from its right edge on (arrows, keypad); 0 for a key with
/// no known position.
pub fn key_pan(keycode: u16) -> f32 {
    let Some(key) = u8::try_from(keycode).ok().and_then(keys::info) else { return 0.0 };
    ((key.x - CENTRE_X) / HALF_WIDTH).clamp(-1.0, 1.0) * PAN_WIDTH
}

/// A keystroke's (gain, pitch) with Natural Variation, from two uniform
/// draws in [0, 1): 0 is the low end of each range, 1 the high end.
pub fn variation(gain_unit: f32, pitch_unit: f32) -> (f32, f32) {
    let spread = |unit: f32| unit.clamp(0.0, 1.0) * 2.0 - 1.0;
    let db = spread(gain_unit) * GAIN_JITTER_DB;
    let cents = spread(pitch_unit) * PITCH_JITTER_CENTS;
    (10f32.powf(db / 20.0), 2f32.powf(cents / 1_200.0))
}

/// xorshift64*: a small, fast generator for the tap thread (no allocation,
/// no lock, no system call once seeded). Not for anything secret.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    /// A generator starting from `seed` (any value; 0 is replaced).
    pub fn new(seed: u64) -> Rng {
        Rng(if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed })
    }

    /// A generator seeded differently on each launch. Not real-time safe
    /// (reads the process's random hash keys): call it when creating a voicer.
    pub fn seeded() -> Rng {
        use std::hash::BuildHasher;
        Rng::new(std::collections::hash_map::RandomState::new().hash_one(0x5EED_u64))
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clicky_pack::keys::code;

    fn db(gain: f32) -> f32 {
        20.0 * gain.log10()
    }

    fn cents(pitch: f32) -> f32 {
        1_200.0 * pitch.log2()
    }

    #[test]
    fn variation_spans_a_few_percent_of_pitch_and_a_db_and_a_half_of_loudness() {
        let (low_gain, low_pitch) = variation(0.0, 0.0);
        let (high_gain, high_pitch) = variation(1.0, 1.0);
        assert!((db(low_gain) + 1.5).abs() < 1e-4 && (db(high_gain) - 1.5).abs() < 1e-4);
        assert!((cents(low_pitch) + 35.0).abs() < 1e-3 && (cents(high_pitch) - 35.0).abs() < 1e-3);
        // 35 cents is 2.0 % of playback rate.
        assert!((high_pitch - 1.020_42).abs() < 1e-4, "{high_pitch}");
        assert_eq!(variation(0.5, 0.5), (1.0, 1.0), "the middle of each range is unchanged");
        assert_eq!(variation(-3.0, 9.0), variation(0.0, 1.0), "clamped");
    }

    #[test]
    fn the_generator_is_uniform_enough_and_never_sticks() {
        let mut rng = Rng::new(0);
        let draws: Vec<f32> = (0..10_000).map(|_| rng.unit()).collect();
        assert!(draws.iter().all(|u| (0.0..1.0).contains(u)));
        let mean = draws.iter().sum::<f32>() / draws.len() as f32;
        assert!((mean - 0.5).abs() < 0.02, "{mean}");
        for decile in 0..10 {
            let lo = decile as f32 / 10.0;
            let n = draws.iter().filter(|u| (lo..lo + 0.1).contains(*u)).count();
            assert!((800..1_200).contains(&n), "decile {decile}: {n}");
        }
        assert_ne!(Rng::seeded().next_u64(), Rng::seeded().next_u64(), "seeds differ");
    }

    #[test]
    fn left_keys_pan_left_and_right_keys_pan_right_within_the_width() {
        let pan = |key: u8| key_pan(u16::from(key));
        assert!(pan(code::ESCAPE) < -0.3, "Escape {}", pan(code::ESCAPE));
        assert!(pan(code::A) < -0.2 && pan(code::SHIFT) < -0.3);
        assert!(pan(code::RETURN) > 0.3 && pan(code::RIGHT_SHIFT) > 0.3);
        assert!(pan(code::J) > 0.0 && pan(code::J) < 0.1, "J is just right of centre");
        assert!(pan(code::SPACE).abs() < 0.02, "Space is centred: {}", pan(code::SPACE));
        assert_eq!(pan(0x52), PAN_WIDTH, "the keypad is as far right as it goes");
        for keycode in 0..=u16::from(u8::MAX) {
            assert!(key_pan(keycode).abs() <= PAN_WIDTH);
        }
        assert_eq!(key_pan(0x34), 0.0, "a key with no position is centred");
        assert_eq!(key_pan(u16::MAX), 0.0);
    }

    #[test]
    fn the_options_survive_a_relaunch() {
        let suite = crate::settings::TestSuite::new("options");
        let first = suite.store();
        let shared = Shared::new(true);
        restore(&shared, &first);
        assert!(shared.release_sounds() && shared.spatial(), "all on");
        first.set(&RELEASE_SOUNDS, &false);
        first.set(&SPATIAL_STEREO, &false);
        let relaunched = suite.store();
        let shared = Shared::new(true);
        restore(&shared, &relaunched);
        assert_eq!((shared.release_sounds(), shared.spatial()), (false, false));
    }
}
