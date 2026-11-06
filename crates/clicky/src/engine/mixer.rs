//! Voice mixer run by the render callback. Real-time safe: a fixed voice pool,
//! fixed buffers, no allocation, no locks, no system calls.
//!
//! ```text
//! voices ─(resample, voice gain, steal fade)─▶ Σ ─▶ × master (ramped per frame)
//!        ─▶ look-ahead limiter (ceiling −1 dBFS) ─▶ left = right
//! ```
//!
//! - **Resampling**: a voice at its soundpack's rate is converted to the
//!   device rate with a windowed-sinc polyphase filter (`resample.rs`); at
//!   the same rate it is read sample for sample.
//! - **Stealing**: with [`MAX_VOICES`] sounding, a new voice makes the oldest
//!   fade out over [`STEAL_FADE_SECS`] in one of the spare slots, instead of
//!   cutting it.
//! - **Master gain**: ramped per frame at most a full-scale change per
//!   [`VOLUME_RAMP_SECS`] and smoothed (2 ms one-pole) so the ramp has no
//!   corners: moving the volume slider cannot step or zip. A target
//!   of 0 (sounds off, volume at zero, or the output's fade-out) is reached
//!   within the block, and then every voice is dropped.
//! - **Headroom and limiter**: the loudest volume is `MASTER_GAIN` (−4.4 dB),
//!   so a lone clip (≤ −1 dBFS) peaks at −5.4 dBFS and only overlapping voices
//!   reach the limiter (`limiter.rs`), which holds every sample at or below
//!   −1 dBFS without latency.
//!
//! The stress render (`stress.rs`) checks all of this end to end.

mod limiter;
mod resample;
#[cfg(test)]
mod stress;

use limiter::{CHUNK, Limiter, MAX_LOOKAHEAD};
use resample::{Kernel, Kernels};

#[cfg(test)]
use limiter::CEILING;

use super::{MASTER_GAIN, Shared};
use crate::settings::{Setting, Store};

/// The volume slider's position, 0 (silent) to 1 (loudest).
pub const VOLUME: Setting<f64> = Setting { key: "Volume", default: 1.0 };
/// The slider's range above silence: each tenth of it is 4 dB.
pub const VOLUME_RANGE_DB: f64 = 40.0;

/// The master gain for a volume slider position: linear in decibels from
/// −40 dB to 0 dB relative to [`MASTER_GAIN`], and silent at 0.
pub fn volume_gain(position: f64) -> f32 {
    let p = if position.is_finite() { position.clamp(0.0, 1.0) } else { VOLUME.default };
    if p <= 0.0 {
        return 0.0;
    }
    (f64::from(MASTER_GAIN) * 10f64.powf((p - 1.0) * VOLUME_RANGE_DB / 20.0)) as f32
}

/// Sets the master gain from the saved volume (at launch, before any key press).
pub fn restore_volume(shared: &Shared, store: &Store) {
    shared.set_master_gain(volume_gain(store.get(&VOLUME)));
}

/// Simultaneous voices; a new one beyond this makes the oldest fade out.
pub const MAX_VOICES: usize = 32;
/// Extra slots for stolen voices while they fade out.
const FADING_SLOTS: usize = 16;
const SLOTS: usize = MAX_VOICES + FADING_SLOTS;
/// How long a stolen voice takes to fade out.
pub const STEAL_FADE_SECS: f64 = 0.005;
/// The master gain moves by at most 1.0 (more than its whole range) in this time.
pub const VOLUME_RAMP_SECS: f64 = 0.020;
/// Time constant of the smoothing after the ramp.
const VOLUME_SMOOTH_SECS: f64 = 0.002;

const ONE: u64 = 1 << 32;
const I16_SCALE: f32 = 1.0 / 32_768.0;
const BUFFER: usize = CHUNK + MAX_LOOKAHEAD;

/// A request to start one voice: a press sound at its recorded sample rate.
#[derive(Clone, Copy, Debug)]
pub struct Play {
    pub samples: &'static [i16],
    pub src_rate: u32,
    pub gain: f32,
}

#[derive(Clone, Copy)]
struct Voice {
    samples: &'static [i16],
    /// Read position, 32.32 fixed point, in source samples.
    pos: u64,
    /// The voice has ended once `pos` reaches this (the clip plus the filter's tail).
    end: u64,
    step: u64,
    /// `None`: same rate as the device, read sample for sample.
    kernel: Option<&'static Kernel>,
    gain: f32,
    /// Start order, for stealing the oldest voice.
    serial: u64,
    /// Frames left of a steal fade-out (`None`: not stolen).
    fading: Option<u32>,
}

impl Voice {
    /// Adds the voice into `out`. Returns `true` once it has ended.
    fn mix(&mut self, out: &mut [f32], fade_frames: u32) -> bool {
        let gain = self.gain * I16_SCALE;
        let fade_step = 1.0 / fade_frames as f32;
        for o in out.iter_mut() {
            if self.pos >= self.end {
                return true;
            }
            let s = self.read(self.pos);
            let env = match &mut self.fading {
                None => 1.0,
                Some(0) => return true,
                Some(left) => {
                    *left -= 1;
                    *left as f32 * fade_step
                }
            };
            *o += s * gain * env;
            self.pos += self.step;
        }
        self.pos >= self.end || self.fading == Some(0)
    }

    /// The voice's value at `pos`, in `i16` units.
    #[inline]
    fn read(&self, pos: u64) -> f32 {
        match self.kernel {
            None => f32::from(self.samples[(pos >> 32) as usize]),
            Some(kernel) => kernel.read(self.samples, pos),
        }
    }
}

pub struct Mixer {
    out_rate: f64,
    kernels: Kernels,
    voices: [Option<Voice>; SLOTS],
    serial: u64,
    /// Master gain reached at the end of the previous block; `None` before the first.
    gain: Option<Master>,
    /// Largest master-gain change per frame.
    slew: f32,
    /// The master gain's one-pole smoothing coefficient.
    smooth: f32,
    fade_frames: u32,
    limiter: Limiter,
    /// The pre-master mix: `mix[..lookahead]` is already rendered, the
    /// frames the next block starts with.
    mix: [f32; BUFFER],
    /// `mix` times the master gain, for the limiter.
    scaled: [f32; BUFFER],
}

impl Mixer {
    /// A mixer for a device at `out_rate`. Builds the resampling filters on
    /// first use of a rate: call it off the real-time path.
    pub fn new(out_rate: f64) -> Mixer {
        let out_rate = if out_rate.is_finite() && out_rate > 0.0 { out_rate } else { 48_000.0 };
        Mixer {
            out_rate,
            kernels: Kernels::new(out_rate),
            voices: [None; SLOTS],
            serial: 0,
            gain: None,
            slew: (1.0 / (VOLUME_RAMP_SECS * out_rate)) as f32,
            smooth: (1.0 - (-1.0 / (VOLUME_SMOOTH_SECS * out_rate)).exp()) as f32,
            fade_frames: ((STEAL_FADE_SECS * out_rate).round() as u32).max(1),
            limiter: Limiter::new(out_rate),
            mix: [0.0; BUFFER],
            scaled: [0.0; BUFFER],
        }
    }

    /// Number of voices sounding, including stolen ones still fading out.
    #[cfg(test)]
    pub fn voices(&self) -> usize {
        self.voices.iter().filter(|v| v.is_some()).count()
    }

    /// Starts a voice from the next frame rendered. With [`MAX_VOICES`]
    /// already sounding, the oldest one starts fading out.
    pub fn start(&mut self, play: &Play) {
        if play.samples.is_empty() || play.src_rate == 0 {
            return;
        }
        let ratio = f64::from(play.src_rate) / self.out_rate;
        let step = ((ratio * ONE as f64).round() as u64).max(1);
        let (kernel, tail) = if step == ONE {
            (None, 0)
        } else {
            let kernel = self.kernels.get(play.src_rate);
            (Some(kernel), kernel.half() as u64)
        };
        self.serial += 1;
        let mut voice = Voice {
            samples: play.samples,
            pos: 0,
            end: (play.samples.len() as u64 + tail) << 32,
            step,
            kernel,
            gain: play.gain,
            serial: self.serial,
            fading: None,
        };
        self.make_room();
        // The voice's first frames go into the look-ahead part of the mix,
        // so that every voice is rendered as far ahead as the others.
        let lookahead = self.limiter.lookahead();
        if voice.mix(&mut self.mix[..lookahead], self.fade_frames) {
            return;
        }
        if let Some(slot) = self.voices.iter_mut().find(|v| v.is_none()) {
            *slot = Some(voice);
        }
    }

    /// Leaves a free slot and fewer than [`MAX_VOICES`] voices not fading.
    fn make_room(&mut self) {
        let sounding = self.voices.iter().flatten().filter(|v| v.fading.is_none()).count();
        if sounding >= MAX_VOICES
            && let Some(oldest) = (0..SLOTS)
                .filter(|&i| self.voices[i].is_some_and(|v| v.fading.is_none()))
                .min_by_key(|&i| self.voices[i].map_or(0, |v| v.serial))
            && let Some(voice) = &mut self.voices[oldest]
        {
            // The fade starts after the look-ahead the voice has already
            // rendered, which the limiter has seen: changing that would
            // change peaks it is already turning down for.
            voice.fading = Some(self.fade_frames);
        }
        if self.voices.iter().all(Option::is_some) {
            // Every spare slot is fading too (a burst of more than
            // FADING_SLOTS presses within one fade): cut the quietest fade.
            let quietest = self
                .voices
                .iter()
                .enumerate()
                .min_by_key(|(_, v)| v.and_then(|v| v.fading).unwrap_or(u32::MAX))
                .map_or(0, |(i, _)| i);
            self.voices[quietest] = None;
        }
    }

    /// Overwrites `left` and `right` with the mix, ramping the master gain
    /// towards `master` frame by frame. A master of 0 silences every voice
    /// within this block and drops them. Returns whether any voice sounded.
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32], master: f32) -> bool {
        let n = left.len().min(right.len());
        let (left, right) = (&mut left[..n], &mut right[..n]);
        let target = if master.is_finite() { master.max(0.0) } else { 0.0 };
        let lookahead = self.limiter.lookahead();
        let sounding = self.voices.iter().any(Option::is_some)
            || self.mix[..lookahead].iter().any(|s| *s != 0.0);
        let mut gain = self.gain.unwrap_or(Master::at(target));
        if !sounding {
            left.fill(0.0);
            right.fill(0.0);
            self.gain = Some(Master::at(target));
            return false;
        }
        let (step, smooth) = if target == 0.0 {
            // Silencing must finish within the block, however short it is:
            // a straight line down from where the gain is.
            gain = Master::at(gain.gain);
            (self.slew.max(gain.gain / n.max(1) as f32), 1.0)
        } else {
            (self.slew, self.smooth)
        };
        let mut done = 0;
        for out in left.chunks_mut(CHUNK) {
            done += out.len();
            self.render_chunk(out, n - done, &mut gain, (target, step, smooth));
        }
        right.copy_from_slice(left);
        self.gain = Some(gain);
        if target == 0.0 {
            self.voices = [None; SLOTS];
            self.mix = [0.0; BUFFER];
            self.limiter.reset();
        }
        true
    }

    /// Renders `out`, followed by `rest` more frames in this block.
    fn render_chunk(
        &mut self,
        out: &mut [f32],
        rest: usize,
        gain: &mut Master,
        (target, step, smooth): (f32, f32, f32),
    ) {
        let n = out.len();
        let la = self.limiter.lookahead();
        self.mix[la..la + n].fill(0.0);
        for slot in self.voices.iter_mut() {
            if let Some(voice) = slot
                && voice.mix(&mut self.mix[la..la + n], self.fade_frames)
            {
                *slot = None;
            }
        }
        // The master gain over this chunk, then over the look-ahead: as it
        // continues within this block, and beyond it (the next block may
        // have a higher target) as high as the ramp could take it, so the
        // limiter never meets a peak higher than it foresaw.
        let mut g = *gain;
        let mut highest = 0.0;
        for (t, (x, m)) in self.scaled[..n + la].iter_mut().zip(&self.mix).enumerate() {
            let at = if t < n + rest {
                highest = g.next(target, step, smooth);
                highest
            } else {
                highest += self.slew;
                highest
            };
            *x = m * at;
            if t + 1 == n {
                *gain = g;
            }
        }
        self.limiter.process(&self.scaled[..n + la], out);
        self.mix.copy_within(n..n + la, 0);
    }
}

/// The master gain's trajectory: a ramp moving at most a step per frame,
/// through a one-pole smoother so its slope never turns a corner. The
/// smoother cannot make a frame's change larger than the ramp's step.
#[derive(Clone, Copy)]
struct Master {
    ramp: f32,
    gain: f32,
}

impl Master {
    fn at(gain: f32) -> Master {
        Master { ramp: gain, gain }
    }

    /// The gain for the next frame. `smooth` 1 means no smoothing.
    fn next(&mut self, target: f32, step: f32, smooth: f32) -> f32 {
        self.ramp = toward(self.ramp, target, step);
        let d = self.ramp - self.gain;
        self.gain = if d.abs() < 1e-7 { self.ramp } else { self.gain + d * smooth };
        self.gain
    }
}

/// `from` moved towards `to` by at most `step`.
fn toward(from: f32, to: f32, step: f32) -> f32 {
    if (to - from).abs() <= step {
        to
    } else if to > from {
        from + step
    } else {
        from - step
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn play(samples: &'static [i16], src_rate: u32, gain: f32) -> Play {
        Play { samples, src_rate, gain }
    }

    fn render(mixer: &mut Mixer, frames: usize, master: f32) -> (Vec<f32>, bool) {
        let (mut l, mut r) = (vec![9.0; frames], vec![9.0; frames]);
        let sounding = mixer.render(&mut l, &mut r, master);
        assert_eq!(l, r, "the mix is centred");
        (l, sounding)
    }

    fn max_step(x: &[f32]) -> f32 {
        x.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max)
    }

    #[test]
    fn renders_silence_without_voices() {
        let mut mixer = Mixer::new(48_000.0);
        let (out, sounding) = render(&mut mixer, 8, 1.0);
        assert!(!sounding);
        assert_eq!(out, vec![0.0; 8]);
    }

    #[test]
    fn plays_a_clip_at_its_own_rate_sample_for_sample() {
        static CLIP: [i16; 3] = [16_384, -16_384, 8_192];
        let mut mixer = Mixer::new(44_100.0);
        mixer.start(&play(&CLIP, 44_100, 1.0));
        let (out, sounding) = render(&mut mixer, 5, 1.0);
        assert!(sounding);
        assert_eq!(out, vec![0.5, -0.5, 0.25, 0.0, 0.0]);
        assert_eq!(mixer.voices(), 0, "the voice ends with its clip");
        assert!(!render(&mut mixer, 5, 1.0).1);
    }

    #[test]
    fn a_long_block_plays_the_same_as_short_ones() {
        let clip = sine(3_000.0, 44_100, 0.3, 0.05);
        let mut long = Mixer::new(48_000.0);
        long.start(&play(clip, 44_100, 1.0));
        let (whole, _) = render(&mut long, 3_000, 0.5);
        let mut short = Mixer::new(48_000.0);
        short.start(&play(clip, 44_100, 1.0));
        let parts: Vec<f32> = (0..30).flat_map(|_| render(&mut short, 100, 0.5).0).collect();
        assert_eq!(whole, parts);
    }

    #[test]
    fn voices_sum_with_their_gains() {
        static CLIP: [i16; 1] = [16_384];
        let mut mixer = Mixer::new(48_000.0);
        mixer.start(&play(&CLIP, 48_000, 0.5));
        mixer.start(&play(&CLIP, 48_000, 0.25));
        let (out, _) = render(&mut mixer, 1, 1.0);
        assert_eq!(out, vec![0.375]);
    }

    #[test]
    fn many_full_scale_voices_stay_at_the_ceiling() {
        static CLIP: [i16; 2_000] = [32_767; 2_000];
        let mut mixer = Mixer::new(48_000.0);
        for _ in 0..16 {
            mixer.start(&play(&CLIP, 48_000, 1.0));
        }
        let (out, _) = render(&mut mixer, 2_048, 1.0);
        let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak <= CEILING, "{peak}");
        assert!(out[1_000] > CEILING * 0.99, "held at the ceiling: {}", out[1_000]);
    }

    #[test]
    fn the_limiter_turns_a_peak_down_before_it_arrives() {
        // A smooth bump (Hann, 400 samples) peaking at 0.8 of full scale; four
        // voices of it sum to 3.2 and must be held at the ceiling without the
        // output changing faster than the bump itself does.
        let bump: Vec<i16> = (0..400)
            .map(|i| {
                let x = std::f64::consts::PI * 2.0 * i as f64 / 400.0;
                (0.8 * 32_767.0 * 0.5 * (1.0 - x.cos())).round() as i16
            })
            .collect();
        let bump: &'static [i16] = Box::leak(bump.into_boxed_slice());
        let mut mixer = Mixer::new(48_000.0);
        let (lead, _) = render(&mut mixer, 1, 1.0);
        assert_eq!(lead, vec![0.0]);
        static SILENCE: [i16; 100] = [0; 100];
        mixer.start(&play(&SILENCE, 48_000, 1.0));
        render(&mut mixer, 1, 1.0);
        for _ in 0..4 {
            mixer.start(&play(bump, 48_000, 1.0));
        }
        let (out, _) = render(&mut mixer, 600, 1.0);
        let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak <= CEILING && peak > 0.85, "{peak}");
        let unlimited: Vec<f32> = bump.iter().map(|s| 4.0 * f32::from(*s) * I16_SCALE).collect();
        assert!(
            max_step(&out) <= max_step(&unlimited),
            "{} > {}",
            max_step(&out),
            max_step(&unlimited)
        );
    }

    #[test]
    fn a_zero_master_silences_within_one_block_and_drops_the_voices() {
        static CLIP: [i16; 1_000] = [16_384; 1_000];
        let mut mixer = Mixer::new(48_000.0);
        mixer.start(&play(&CLIP, 48_000, 1.0));
        let (first, _) = render(&mut mixer, 4, 1.0);
        assert_eq!(first, vec![0.5; 4]);
        let (fade, _) = render(&mut mixer, 4, 0.0);
        assert!(fade.windows(2).all(|w| w[1] < w[0]), "ramps down: {fade:?}");
        assert_eq!(*fade.last().unwrap(), 0.0);
        assert_eq!(mixer.voices(), 0);
        assert!(!render(&mut mixer, 4, 1.0).1, "nothing resumes after the switch is off");
    }

    #[test]
    fn a_volume_change_ramps_frame_by_frame() {
        static CLIP: [i16; 48_000] = [16_384; 48_000];
        let mut mixer = Mixer::new(48_000.0);
        mixer.start(&play(&CLIP, 48_000, 1.0));
        let mut out = render(&mut mixer, 64, 0.6).0;
        // A slider dragged fast: a new target every 128 frames.
        for master in [0.05, 0.6, 0.3, 0.001, 0.45, 0.6] {
            out.extend(render(&mut mixer, 128, master).0);
        }
        out.extend(render(&mut mixer, 2_000, 0.2).0);
        // The ramp: a full-scale change takes at least 20 ms.
        let allowed = 0.5 / (0.020 * 48_000.0);
        assert!(max_step(&out) <= allowed * 1.001, "{} > {allowed}", max_step(&out));
        assert!((out.last().unwrap() - 0.1).abs() < 1e-6, "reaches the target");
    }

    #[test]
    fn a_stolen_voice_fades_out_instead_of_stopping() {
        static HUM: [i16; 48_000] = [300; 48_000];
        static SILENCE: [i16; 48_000] = [0; 48_000];
        for (src, out_rate) in [(48_000, 48_000.0), (44_100, 48_000.0), (48_000, 44_100.0)] {
            let mut mixer = Mixer::new(out_rate);
            for _ in 0..MAX_VOICES {
                mixer.start(&play(&HUM, src, 1.0));
            }
            // Past the resampler's ringing at the clips' start.
            render(&mut mixer, 256, 1.0);
            let mut out = render(&mut mixer, 256, 1.0).0;
            mixer.start(&play(&SILENCE, src, 1.0));
            assert_eq!(mixer.voices(), MAX_VOICES + 1, "the oldest is still fading");
            out.extend(render(&mut mixer, 1_024, 1.0).0);
            let one = 300.0 * I16_SCALE;
            assert!((out[0] - 32.0 * one).abs() < 1e-5);
            assert!((out.last().unwrap() - 31.0 * one).abs() < 1e-5, "one voice gone");
            // A fade of a few milliseconds: no frame drops more than 1/200 of the voice.
            let step = max_step(&out);
            assert!(step <= one / 200.0, "{src}->{out_rate}: {step} vs a cut of {one}");
            assert_eq!(mixer.voices(), MAX_VOICES);
        }
    }

    #[test]
    fn a_burst_beyond_every_slot_keeps_the_newest_voices() {
        static CLIP: [i16; 1_000] = [100; 1_000];
        let mut mixer = Mixer::new(48_000.0);
        for _ in 0..SLOTS * 2 {
            mixer.start(&play(&CLIP, 48_000, 1.0));
        }
        assert_eq!(mixer.voices(), SLOTS);
        render(&mut mixer, 512, 1.0);
        assert_eq!(mixer.voices(), MAX_VOICES, "the stolen ones have faded");
    }

    #[test]
    fn the_volume_curve_is_linear_in_decibels_and_silent_at_zero() {
        assert_eq!(volume_gain(1.0), MASTER_GAIN);
        assert_eq!(volume_gain(0.0), 0.0);
        let db = |p: f64| 20.0 * (volume_gain(p) / MASTER_GAIN).log10();
        assert!((db(0.5) + 20.0).abs() < 1e-4, "{}", db(0.5));
        assert!((db(0.9) + 4.0).abs() < 1e-4, "{}", db(0.9));
        assert!((db(0.01) + 39.6).abs() < 1e-3, "{}", db(0.01));
        assert_eq!(volume_gain(1.5), MASTER_GAIN, "clamped");
        assert_eq!(volume_gain(-1.0), 0.0, "clamped");
    }

    #[test]
    fn the_volume_survives_a_relaunch() {
        use crate::settings::{Store, UserDefaults};
        let suite =
            format!("io.github.cavallinilorenzo.clicky.tests.volume.{}", std::process::id());
        let first = Store::with_backend(UserDefaults::suite(&suite));
        assert_eq!(first.get(&VOLUME), 1.0, "loudest until moved");
        first.set(&VOLUME, &0.35);
        let relaunched = Store::with_backend(UserDefaults::suite(&suite));
        assert_eq!(relaunched.get(&VOLUME), 0.35);
        let shared = Shared::new(true);
        restore_volume(&shared, &relaunched);
        // 0.35 is 26 dB below the loudest (40 dB × 0.65).
        let db = 20.0 * (shared.master_gain() / MASTER_GAIN).log10();
        assert!((db + 26.0).abs() < 1e-3, "the mixer plays at the saved volume: {db} dB");
        UserDefaults::remove_suite(&suite);
    }

    /// A sine of `freq` Hz at `rate`, `amp` of full scale, as a leaked clip.
    pub fn sine(freq: f64, rate: u32, amp: f64, seconds: f64) -> &'static [i16] {
        let n = (f64::from(rate) * seconds) as usize;
        let clip: Vec<i16> = (0..n)
            .map(|i| {
                let t = i as f64 / f64::from(rate);
                (amp * 32_767.0 * (2.0 * std::f64::consts::PI * freq * t).sin()).round() as i16
            })
            .collect();
        Box::leak(clip.into_boxed_slice())
    }

    /// Least-squares fit of a sine at `freq` to `x` (sampled at `rate`):
    /// (amplitude, residual power relative to the fitted sine, in dB).
    fn fit(x: &[f32], freq: f64, rate: f64) -> (f64, f64) {
        let w = 2.0 * std::f64::consts::PI * freq / rate;
        let (mut ss, mut cc, mut sc, mut ys, mut yc) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for (i, &y) in x.iter().enumerate() {
            let (s, c) = (w * i as f64).sin_cos();
            let y = f64::from(y);
            ss += s * s;
            cc += c * c;
            sc += s * c;
            ys += y * s;
            yc += y * c;
        }
        let det = ss * cc - sc * sc;
        let a = (ys * cc - yc * sc) / det;
        let b = (yc * ss - ys * sc) / det;
        let (mut fit_power, mut res_power) = (0.0, 0.0);
        for (i, &y) in x.iter().enumerate() {
            let (s, c) = (w * i as f64).sin_cos();
            let f = a * s + b * c;
            fit_power += f * f;
            res_power += (f64::from(y) - f).powi(2);
        }
        ((a * a + b * b).sqrt(), 10.0 * (res_power / fit_power).log10())
    }

    /// Renders `clip` alone at `out_rate`, master 1, and returns the steady
    /// middle (the clip's edges cut off).
    fn resampled(clip: &'static [i16], src_rate: u32, out_rate: f64) -> Vec<f32> {
        let mut mixer = Mixer::new(out_rate);
        mixer.start(&play(clip, src_rate, 1.0));
        let frames = (clip.len() as f64 * out_rate / f64::from(src_rate)) as usize;
        let (mut l, mut r) = (vec![0.0; frames], vec![0.0; frames]);
        mixer.render(&mut l, &mut r, 1.0);
        l[frames / 4..frames * 3 / 4].to_vec()
    }

    /// Passband flatness and image/alias rejection of the device-rate
    /// resampler, measured with sines (the module docs give the numbers).
    #[test]
    fn resampling_is_flat_and_clean_across_the_audible_band() {
        let pairs =
            [(44_100, 48_000.0), (48_000, 44_100.0), (44_100, 96_000.0), (48_000, 96_000.0)];
        for (src, out) in pairs {
            for freq in [100.0, 1_000.0, 5_000.0, 10_000.0, 15_000.0, 18_000.0] {
                let clip = sine(freq, src, 0.5, 0.2);
                let (amp, residual_db) = fit(&resampled(clip, src, out), freq, out);
                let gain_db = 20.0 * (amp / 0.5).log10();
                println!(
                    "{src}->{out} {freq:>6} Hz: gain {gain_db:+.4} dB, residual {residual_db:.1} dB"
                );
                assert!(gain_db.abs() < 0.05, "{src}->{out} {freq} Hz: gain {gain_db:.4} dB");
                assert!(
                    residual_db < -80.0,
                    "{src}->{out} {freq} Hz: residual {residual_db:.1} dB"
                );
            }
        }
    }

    /// Content above the device's Nyquist frequency must not fold back down.
    #[test]
    fn downsampling_rejects_what_would_alias() {
        for freq in [22_800.0, 23_500.0] {
            let clip = sine(freq, 48_000, 0.5, 0.2);
            let out = resampled(clip, 48_000, 44_100.0);
            let rms =
                (out.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / out.len() as f64).sqrt();
            let db = 20.0 * (rms * 2f64.sqrt() / 0.5).log10();
            println!("48000->44100 {freq} Hz: leaks {db:.1} dB");
            assert!(db < -80.0, "{freq} Hz at 48 kHz leaks {db:.1} dB into 44.1 kHz");
        }
    }
}
