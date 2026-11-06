//! Voice mixer run by the render callback. Real-time safe: a fixed voice pool,
//! no allocation, no locks, no system calls.
//!
//! This is the tracer's mixer: linear-interpolation resampling, a master gain
//! ramped per block, oldest-voice stealing and a hard clamp. Issue #6 replaces
//! the stealing with a fade, adds headroom and a transparent limiter, better
//! resampling and the volume ramp; its stress test lives next to this file.

/// Simultaneous voices; the oldest is stolen when a new one needs a slot.
pub const MAX_VOICES: usize = 32;

const ONE: u64 = 1 << 32;
const FRACTION: u64 = ONE - 1;
const I16_SCALE: f32 = 1.0 / 32_768.0;

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
    end: u64,
    step: u64,
    gain: f32,
    /// Start order, for stealing the oldest voice.
    serial: u64,
}

pub struct Mixer {
    out_rate: f64,
    voices: [Option<Voice>; MAX_VOICES],
    serial: u64,
    /// Master gain reached at the end of the previous block; `None` before the first.
    gain: Option<f32>,
}

impl Mixer {
    pub fn new(out_rate: f64) -> Mixer {
        Mixer { out_rate, voices: [None; MAX_VOICES], serial: 0, gain: None }
    }

    /// Sets the device sample rate. Voices started afterwards are resampled to it.
    #[cfg_attr(not(test), expect(dead_code, reason = "#4 starts each run with a fresh Mixer"))]
    pub fn set_out_rate(&mut self, out_rate: f64) {
        if out_rate > 0.0 {
            self.out_rate = out_rate;
        }
    }

    /// Number of voices currently playing.
    #[cfg(test)]
    pub fn voices(&self) -> usize {
        self.voices.iter().filter(|v| v.is_some()).count()
    }

    /// Starts a voice, stealing the oldest one if every slot is busy.
    pub fn start(&mut self, play: &Play) {
        if play.samples.is_empty() || play.src_rate == 0 {
            return;
        }
        let ratio = f64::from(play.src_rate) / self.out_rate;
        self.serial += 1;
        let voice = Voice {
            samples: play.samples,
            pos: 0,
            end: (play.samples.len() as u64) << 32,
            step: ((ratio * ONE as f64).round() as u64).max(1),
            gain: play.gain,
            serial: self.serial,
        };
        let slot = self.voices.iter().position(Option::is_none).unwrap_or_else(|| {
            self.voices
                .iter()
                .enumerate()
                .min_by_key(|(_, v)| v.as_ref().map_or(0, |v| v.serial))
                .map_or(0, |(i, _)| i)
        });
        self.voices[slot] = Some(voice);
    }

    /// Overwrites `left` and `right` with the mix, ramping the master gain from
    /// the previous block to `master`. A master of 0 silences every voice within
    /// this block and drops them. Returns whether any voice sounded.
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32], master: f32) -> bool {
        let n = left.len().min(right.len());
        let (left, right) = (&mut left[..n], &mut right[..n]);
        left.fill(0.0);
        right.fill(0.0);
        let mut sounding = false;
        for slot in self.voices.iter_mut() {
            let Some(voice) = slot else { continue };
            sounding = true;
            if mix_voice(voice, left) {
                *slot = None;
            }
        }
        let from = self.gain.unwrap_or(master);
        self.gain = Some(master);
        if sounding {
            let step = (master - from) / n.max(1) as f32;
            let mut g = from;
            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                g += step;
                *l = (*l * g).clamp(-1.0, 1.0);
                *r = *l;
            }
        }
        if master <= 0.0 {
            self.voices = [None; MAX_VOICES];
        }
        sounding
    }
}

/// Adds one voice into `out`. Returns `true` once the voice has finished.
fn mix_voice(v: &mut Voice, out: &mut [f32]) -> bool {
    let s = v.samples;
    let gain = v.gain * I16_SCALE;
    for o in out.iter_mut() {
        if v.pos >= v.end {
            return true;
        }
        let i = (v.pos >> 32) as usize;
        let frac = (v.pos & FRACTION) as f32 / ONE as f32;
        let a = f32::from(s[i]);
        let b = s.get(i + 1).map_or(0.0, |&x| f32::from(x));
        *o += (a + (b - a) * frac) * gain;
        v.pos += v.step;
    }
    v.pos >= v.end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn play(samples: &'static [i16], src_rate: u32, gain: f32) -> Play {
        Play { samples, src_rate, gain }
    }

    fn render(mixer: &mut Mixer, frames: usize, master: f32) -> (Vec<f32>, bool) {
        let (mut l, mut r) = (vec![9.0; frames], vec![9.0; frames]);
        let sounding = mixer.render(&mut l, &mut r, master);
        assert_eq!(l, r, "the tracer mix is centred");
        (l, sounding)
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
    fn resamples_to_the_device_rate() {
        static CLIP: [i16; 3] = [0, 16_384, 0];
        let mut mixer = Mixer::new(48_000.0);
        mixer.start(&play(&CLIP, 24_000, 1.0));
        let (out, _) = render(&mut mixer, 8, 1.0);
        assert_eq!(out, vec![0.0, 0.25, 0.5, 0.25, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn follows_a_new_device_rate() {
        static CLIP: [i16; 2] = [16_384, 16_384];
        let mut mixer = Mixer::new(44_100.0);
        mixer.set_out_rate(88_200.0);
        mixer.start(&play(&CLIP, 44_100, 1.0));
        let (out, _) = render(&mut mixer, 6, 1.0);
        assert_eq!(out.iter().filter(|s| **s != 0.0).count(), 4, "{out:?}: twice as long");
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
    fn never_leaves_full_scale() {
        static CLIP: [i16; 1] = [32_767];
        let mut mixer = Mixer::new(48_000.0);
        for _ in 0..8 {
            mixer.start(&play(&CLIP, 48_000, 1.0));
        }
        let (out, _) = render(&mut mixer, 1, 1.0);
        assert!(out[0] <= 1.0 && out[0] > 0.99, "{out:?}");
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
    fn steals_the_oldest_voice_when_every_slot_is_busy() {
        static CLIP: [i16; 100] = [100; 100];
        let mut mixer = Mixer::new(48_000.0);
        for _ in 0..MAX_VOICES + 5 {
            mixer.start(&play(&CLIP, 48_000, 1.0));
        }
        assert_eq!(mixer.voices(), MAX_VOICES);
    }
}
