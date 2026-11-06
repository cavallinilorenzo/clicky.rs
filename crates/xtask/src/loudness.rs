//! Measured loudness of a keystroke clip, used to match the soundpacks.
//!
//! A clip's loudness is its K-weighted energy (the ITU-R BS.1770 frequency
//! weighting used by LUFS meters) over a fixed [`WINDOW_MS`] window from the
//! clip's start, in LUFS-style dB. A clip shorter than the window is padded with
//! silence, so a short click reads quieter than a long one of the same level:
//! for sounds this short the ear integrates energy, not level. The window is
//! shorter than BS.1770's 400 ms "momentary" block because a keystroke is
//! over in well under 400 ms, and a long block would let the next key's sound in.
//!
//! This is a measurement, not a perceptual model: listening tests decide.

/// Integration window from the clip's start.
pub const WINDOW_MS: f32 = 100.0;

/// BS.1770's offset, so a full-scale 997 Hz sine reads −3.01 dB.
const OFFSET_DB: f64 = -0.691;

/// A direct-form-I biquad.
#[derive(Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
}

impl Biquad {
    fn run(&self, x: &[f64]) -> Vec<f64> {
        let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
        x.iter()
            .map(|&x0| {
                let y0 = self.b[0] * x0 + self.b[1] * x1 + self.b[2] * x2
                    - self.a[0] * y1
                    - self.a[1] * y2;
                (x2, x1, y2, y1) = (x1, x0, y1, y0);
                y0
            })
            .collect()
    }
}

/// The two K-weighting stages for `rate` (the pre-filter high shelf and the
/// RLB high-pass), from their analogue prototypes as in libebur128, so any
/// sample rate gets the same curve.
fn k_weighting(rate: u32) -> [Biquad; 2] {
    let fs = f64::from(rate);
    let shelf = {
        let (f0, gain_db, q) =
            (1_681.974_450_955_533, 3.999_843_853_973_347, 0.707_175_236_955_419_6);
        let k = (std::f64::consts::PI * f0 / fs).tan();
        let vh = 10f64.powf(gain_db / 20.0);
        let vb = vh.powf(0.499_666_774_154_541_6);
        let a0 = 1.0 + k / q + k * k;
        Biquad {
            b: [
                (vh + vb * k / q + k * k) / a0,
                2.0 * (k * k - vh) / a0,
                (vh - vb * k / q + k * k) / a0,
            ],
            a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        }
    };
    let high_pass = {
        let (f0, q) = (38.135_470_876_024_44, 0.500_327_037_323_877_3);
        let k = (std::f64::consts::PI * f0 / fs).tan();
        let a0 = 1.0 + k / q + k * k;
        Biquad { b: [1.0, -2.0, 1.0], a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0] }
    };
    [shelf, high_pass]
}

/// Loudness of `clip` (samples in ±1.0 full scale) at `rate`, in dB.
pub fn clip_loudness_db(clip: &[f32], rate: u32) -> f32 {
    let window = ((f64::from(WINDOW_MS) / 1000.0) * f64::from(rate)).round() as usize;
    let x: Vec<f64> = clip.iter().take(window).map(|&s| f64::from(s)).collect();
    let [shelf, high_pass] = k_weighting(rate);
    let y = high_pass.run(&shelf.run(&x));
    let mean_square = y.iter().map(|s| s * s).sum::<f64>() / window.max(1) as f64;
    (OFFSET_DB + 10.0 * mean_square.max(1e-20).log10()) as f32
}

/// [`clip_loudness_db`] of 16-bit samples.
pub fn clip_loudness_db_i16(clip: &[i16], rate: u32) -> f32 {
    let x: Vec<f32> = clip.iter().map(|&s| f32::from(s) / 32_768.0).collect();
    clip_loudness_db(&x, rate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn sine(freq: f32, amplitude: f32, ms: f32, rate: u32) -> Vec<f32> {
        let n = (ms / 1000.0 * rate as f32) as usize;
        (0..n).map(|i| amplitude * (2.0 * PI * freq * i as f32 / rate as f32).sin()).collect()
    }

    #[test]
    fn a_full_scale_997_hz_sine_reads_minus_3_db_at_both_pack_rates() {
        for rate in [44_100, 48_000] {
            let level = clip_loudness_db(&sine(997.0, 1.0, 200.0, rate), rate);
            assert!((level + 3.01).abs() < 0.15, "{rate} Hz: {level}");
        }
    }

    #[test]
    fn level_follows_amplitude() {
        let loud = clip_loudness_db(&sine(997.0, 0.5, 200.0, 44_100), 44_100);
        let quiet = clip_loudness_db(&sine(997.0, 0.05, 200.0, 44_100), 44_100);
        assert!((loud - quiet - 20.0).abs() < 0.05, "{loud} {quiet}");
    }

    #[test]
    fn treble_weighs_more_and_rumble_much_less() {
        let mid = clip_loudness_db(&sine(997.0, 0.5, 200.0, 48_000), 48_000);
        let treble = clip_loudness_db(&sine(6_000.0, 0.5, 200.0, 48_000), 48_000);
        let rumble = clip_loudness_db(&sine(20.0, 0.5, 200.0, 48_000), 48_000);
        assert!((3.0..4.5).contains(&(treble - mid)), "high shelf ≈ +4 dB: {}", treble - mid);
        assert!(mid - rumble > 10.0, "high-pass: {}", mid - rumble);
    }

    #[test]
    fn a_clip_half_the_window_long_reads_3_db_quieter() {
        let full = clip_loudness_db(&sine(997.0, 0.5, WINDOW_MS, 44_100), 44_100);
        let half = clip_loudness_db(&sine(997.0, 0.5, WINDOW_MS / 2.0, 44_100), 44_100);
        assert!((full - half - 3.01).abs() < 0.2, "{full} {half}");
    }

    #[test]
    fn sound_after_the_window_does_not_count() {
        let mut clip = sine(997.0, 0.5, WINDOW_MS, 44_100);
        let before = clip_loudness_db(&clip, 44_100);
        clip.extend(sine(997.0, 1.0, 300.0, 44_100));
        assert!((clip_loudness_db(&clip, 44_100) - before).abs() < 0.01);
    }

    #[test]
    fn i16_and_f32_agree() {
        let x = sine(997.0, 0.25, 80.0, 44_100);
        let q: Vec<i16> = x.iter().map(|s| (s * 32_768.0) as i16).collect();
        assert!((clip_loudness_db(&x, 44_100) - clip_loudness_db_i16(&q, 44_100)).abs() < 0.01);
    }
}
