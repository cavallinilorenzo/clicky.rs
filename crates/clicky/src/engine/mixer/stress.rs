//! The offline stress render: the mixer driven the way the render callback
//! drives it (presses started at block boundaries, blocks of varying size),
//! at maximum volume, at 44.1, 48 and 96 kHz device rates, with soundpacks
//! at 44.1 and 48 kHz.
//!
//! Scenarios: fast typing (20 keys/s), mashing (bursts of 6–12 keys), 10 keys
//! held for 60 s (one press each: held keys do not repeat), and fast typing
//! while the volume slider is dragged up and down.
//!
//! Checks on every output sample:
//! - **Full scale**: no |sample| ≥ 1.0 (in fact none above the −1 dBFS ceiling).
//! - **Discontinuities**: the test clips are keystroke-like but band-limited
//!   (damped partials at 300 Hz–2 kHz), so the clean mix has nothing above
//!   8 kHz. A discontinuity (a cut voice, a gain step, a limiter jump, a
//!   resampling glitch) is broadband, so it shows up in the output's
//!   8 kHz high-pass (8th-order Butterworth). The metric is the peak of that
//!   high-pass; it must stay below [`CLICK_THRESHOLD`] (−66 dBFS) outside
//!   onset windows. An onset window is the [`ONSET_WINDOW_SECS`] after a
//!   keystroke starts, the only place where the recording itself may jump.
//!   A test checks that the metric catches a 0.003 step (−50 dBFS) anywhere.

use super::tests::play;
use super::*;

// Every sample at or below the ceiling is below full scale.
const _: () = assert!(CEILING < 1.0);

/// Peak of the 8 kHz high-pass allowed outside onset windows (−66 dBFS).
const CLICK_THRESHOLD: f64 = 5e-4;
/// After a keystroke starts: the limiter's look-ahead (1.5 ms) plus the
/// high-pass's ringing.
const ONSET_WINDOW_SECS: f64 = 0.0025;
const DEVICE_RATES: [f64; 3] = [44_100.0, 48_000.0, 96_000.0];
const PACK_RATES: [u32; 2] = [44_100, 48_000];
/// CoreAudio's block sizes vary; these cycle.
const BLOCKS_AT_48K: [usize; 5] = [512, 471, 128, 1_024, 64];

/// xorshift64*, so every run is the same.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// A soundpack's worth of keystroke-like clips at `rate`: a few damped
/// partials with an abrupt start, peaking at −1 dBFS like the pipeline's
/// clips, 60 to 360 ms long; the first three ring on for 1.5 s (like a
/// spacebar's rattle), so that stolen voices are still loud.
fn clips(rate: u32) -> Vec<&'static [i16]> {
    let mut rng = Rng(u64::from(rate) * 7_919);
    (0..12)
        .map(|k| {
            let long = k < 3;
            let seconds = if long { 1.5 } else { 0.06 + 0.3 * rng.unit() };
            let n = (seconds * f64::from(rate)) as usize;
            let partials: Vec<(f64, f64, f64)> = (0..3)
                .map(|_| {
                    let decay = if long { 0.5 + rng.unit() } else { 0.01 + 0.08 * rng.unit() };
                    (300.0 + 1_700.0 * rng.unit(), 0.3 + rng.unit(), decay)
                })
                .collect();
            let mut x: Vec<f64> = (0..n)
                .map(|i| {
                    let t = i as f64 / f64::from(rate);
                    // The pipeline's clips start with a 0.5 ms linear fade-in.
                    let fade_in = (i as f64 / (0.0005 * f64::from(rate))).min(1.0);
                    let fade_out = ((n - i) as f64 / (0.006 * f64::from(rate))).min(1.0);
                    partials
                        .iter()
                        .map(|(f, a, d)| a * (-t / d).exp() * (std::f64::consts::TAU * f * t).sin())
                        .sum::<f64>()
                        * fade_in
                        * fade_out
                })
                .collect();
            let peak = x.iter().fold(0.0f64, |m, s| m.max(s.abs()));
            let scale = 0.891 * 32_767.0 / peak;
            x.iter_mut().for_each(|s| *s *= scale);
            let clip: Vec<i16> = x.iter().map(|s| s.round() as i16).collect();
            &*Box::leak(clip.into_boxed_slice())
        })
        .chain([&*Box::leak(vec![0i16; rate as usize / 10].into_boxed_slice())])
        .collect()
}

/// The index of a silent clip in [`clips`]: its onsets are not exempt, so
/// whatever happens when it starts (a voice stolen for it) must be clean.
const SILENT: usize = 12;

/// Keystrokes and volume changes over time.
#[derive(Default)]
struct Script {
    /// (time in seconds, clip index, voice gain)
    presses: Vec<(f64, usize, f32)>,
    /// (time in seconds, slider position)
    volume: Vec<(f64, f64)>,
    seconds: f64,
}

/// What a render produced, and where keystrokes started.
struct Render {
    out: Vec<f32>,
    onsets: Vec<usize>,
    rate: f64,
    /// Most voices sounding at once (stolen ones still fading included).
    most_voices: usize,
}

/// Plays `script` through a fresh mixer the way the render callback does.
fn render(script: &Script, clips: &[&'static [i16]], pack_rate: u32, rate: f64) -> Render {
    let mut mixer = Mixer::new(rate);
    let total = (script.seconds * rate) as usize;
    let mut out = Vec::with_capacity(total);
    let mut onsets = Vec::new();
    let (mut next_press, mut next_volume) = (0, 0);
    let mut master = volume_gain(1.0);
    let scale = rate / 48_000.0;
    let mut block = 0;
    let mut most_voices = 0;
    while out.len() < total {
        let frames = ((BLOCKS_AT_48K[block % BLOCKS_AT_48K.len()] as f64 * scale) as usize).max(1);
        block += 1;
        let now = out.len() as f64 / rate;
        while let Some(&(at, clip, gain)) = script.presses.get(next_press)
            && at <= now
        {
            mixer.start(&play(clips[clip], pack_rate, gain));
            if clip != SILENT {
                onsets.push(out.len());
            }
            next_press += 1;
        }
        while let Some(&(at, position)) = script.volume.get(next_volume)
            && at <= now
        {
            master = volume_gain(position);
            next_volume += 1;
        }
        most_voices = most_voices.max(mixer.voices());
        let (mut l, mut r) = (vec![0.0; frames], vec![0.0; frames]);
        mixer.render(&mut l, &mut r, master);
        assert_eq!(l, r);
        out.extend_from_slice(&l);
    }
    Render { out, onsets, rate, most_voices }
}

/// Voice gains up to +2 dB, leaving room for #8's variation and panning.
fn gain(rng: &mut Rng) -> f32 {
    (1.0 + 0.26 * rng.unit()) as f32
}

fn fast_typing(seconds: f64, seed: u64) -> Script {
    let mut rng = Rng(seed);
    let mut script = Script { seconds, ..Script::default() };
    let mut t = 0.01;
    while t < seconds - 0.5 {
        script.presses.push((t, rng.below(12), gain(&mut rng)));
        t += 0.03 + 0.04 * rng.unit(); // 20 keys/s on average, as fast as anyone types
    }
    script
}

fn mashing(seconds: f64) -> Script {
    let mut rng = Rng(42);
    let mut script = Script { seconds, ..Script::default() };
    let mut t = 0.01;
    while t < seconds - 1.5 {
        for _ in 0..6 + rng.below(7) {
            script.presses.push((t + 0.02 * rng.unit(), rng.below(12), gain(&mut rng)));
        }
        t += 0.08 + 0.1 * rng.unit();
    }
    script.presses.sort_by(|a, b| a.0.total_cmp(&b.0));
    script
}

fn ten_keys_held(seconds: f64) -> Script {
    let mut rng = Rng(7);
    let mut script = Script { seconds, ..Script::default() };
    // Ten keys going down within 30 ms, the long clip among them, then held.
    script.presses.push((0.01, 0, 1.26));
    for _ in 0..9 {
        script.presses.push((0.01 + 0.03 * rng.unit(), 1 + rng.below(11), gain(&mut rng)));
    }
    script.presses.sort_by(|a, b| a.0.total_cmp(&b.0));
    script
}

/// Every slot busy with loud ringing voices, then presses of a silent clip
/// that each steal one of them.
fn steal_storm(seconds: f64) -> Script {
    let mut rng = Rng(11);
    let mut script = Script { seconds, ..Script::default() };
    for k in 0..MAX_VOICES {
        script.presses.push((0.01 + 0.008 * k as f64, rng.below(3), gain(&mut rng)));
    }
    let mut t = 0.3;
    while t < seconds - 0.2 {
        script.presses.push((t, SILENT, 1.0));
        t += 0.004 + 0.02 * rng.unit();
    }
    script
}

fn typing_while_dragging_the_slider(seconds: f64) -> Script {
    let mut script = fast_typing(seconds, 99);
    // Slider events at 60 Hz, sweeping the whole range back and forth.
    let mut t = 0.0;
    while t < seconds {
        let phase = (t / 1.5).fract();
        let position = if phase < 0.5 { 1.0 - 2.0 * phase } else { 2.0 * phase - 1.0 };
        script.volume.push((t, position));
        t += 1.0 / 60.0;
    }
    script
}

/// An 8 kHz 8th-order Butterworth high-pass (four RBJ biquads).
fn high_pass(x: &[f32], rate: f64) -> Vec<f64> {
    let mut y: Vec<f64> = x.iter().map(|s| f64::from(*s)).collect();
    for q in [0.509_795_6, 0.601_344_9, 0.899_976_2, 2.562_915_4] {
        let w0 = std::f64::consts::TAU * 8_000.0 / rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        let (b0, b1, b2) = ((1.0 + cos) / 2.0 / a0, -(1.0 + cos) / a0, (1.0 + cos) / 2.0 / a0);
        let (a1, a2) = (-2.0 * cos / a0, (1.0 - alpha) / a0);
        let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
        for s in y.iter_mut() {
            let x0 = *s;
            let y0 = b0 * x0 + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
            (x2, x1, y2, y1) = (x1, x0, y1, y0);
            *s = y0;
        }
    }
    y
}

/// The largest high-passed value outside onset windows, and where.
fn worst_click(render: &Render) -> (f64, usize) {
    let hp = high_pass(&render.out, render.rate);
    let window = (ONSET_WINDOW_SECS * render.rate) as usize;
    let mut onset = render.onsets.iter().peekable();
    let mut exempt_until = 0;
    let mut worst = (0.0, 0);
    for (i, v) in hp.iter().enumerate() {
        while let Some(&&o) = onset.peek()
            && o <= i
        {
            exempt_until = exempt_until.max(o + window);
            onset.next();
        }
        if i >= exempt_until && v.abs() > worst.0 {
            worst = (v.abs(), i);
        }
    }
    worst
}

fn peak(out: &[f32]) -> f32 {
    out.iter().fold(0.0, |m, s| m.max(s.abs()))
}

/// Renders `script` at every device and pack rate and checks every sample.
fn check(name: &str, script: &Script) {
    for pack_rate in PACK_RATES {
        let clips = clips(pack_rate);
        for rate in DEVICE_RATES {
            let r = render(script, &clips, pack_rate, rate);
            let peak = peak(&r.out);
            let (click, at) = worst_click(&r);
            println!(
                "{name}, pack {pack_rate} Hz, device {rate} Hz: peak {:.2} dBFS, \
                 worst high-pass outside onsets {:.1} dBFS at {:.3} s",
                20.0 * peak.log10(),
                20.0 * click.log10(),
                at as f64 / rate,
            );
            assert!(peak <= CEILING, "{name} at {rate} Hz: peak {peak} above the ceiling");
            assert!(
                click < CLICK_THRESHOLD,
                "{name}, pack {pack_rate} Hz, device {rate} Hz: discontinuity of {click:.5} at {:.4} s",
                at as f64 / rate
            );
        }
    }
}

#[test]
fn fast_typing_at_maximum_volume_is_clean() {
    check("fast typing", &fast_typing(4.0, 1));
}

#[test]
fn mashing_at_maximum_volume_is_clean() {
    check("mashing", &mashing(4.0));
}

#[test]
fn ten_keys_held_for_a_minute_at_maximum_volume_are_clean() {
    check("10 keys held 60 s", &ten_keys_held(60.0));
}

#[test]
fn stolen_voices_fade_out_cleanly() {
    check("steal storm", &steal_storm(1.5));
}

#[test]
fn dragging_the_volume_slider_while_typing_is_clean() {
    check("slider while typing", &typing_while_dragging_the_slider(4.0));
}

/// The stress scenarios really stress: voices get stolen and the limiter works.
#[test]
fn mashing_overloads_the_mix() {
    let clips = clips(48_000);
    let script = mashing(4.0);
    let r = render(&script, &clips, 48_000, 48_000.0);
    assert!(peak(&r.out) > CEILING * 0.99, "the limiter is reached");
    assert!(r.most_voices > MAX_VOICES, "voices are stolen ({} at once)", r.most_voices);
}

/// The metric would catch a click: a 0.003 step (−50 dBFS) anywhere in a
/// clean render, outside an onset window.
#[test]
fn the_discontinuity_metric_catches_a_small_step() {
    let clips = clips(44_100);
    for rate in DEVICE_RATES {
        let mut r = render(&fast_typing(1.5, 3), &clips, 44_100, rate);
        assert!(worst_click(&r).0 < CLICK_THRESHOLD);
        let window = (ONSET_WINDOW_SECS * rate) as usize;
        let at = r.onsets[5] + window + (0.003 * rate) as usize;
        r.out[at..].iter_mut().for_each(|s| *s += 0.003);
        let (click, found) = worst_click(&r);
        assert!(click >= CLICK_THRESHOLD, "{rate} Hz: missed a step ({click})");
        assert!(found.abs_diff(at) < 8, "{rate} Hz: found at {found}, step at {at}");
    }
}

/// During playback, a volume sweep never moves the gain faster than the ramp.
#[test]
fn a_volume_sweep_never_steps_faster_than_the_ramp() {
    for rate in DEVICE_RATES {
        for pack_rate in PACK_RATES {
            // A held level (DC) shows the gain directly.
            let clip: &'static [i16] = Box::leak(vec![8_192i16; 3 * pack_rate as usize].into());
            let mut script = Script { seconds: 2.0, ..Script::default() };
            script.presses.push((0.0, 0, 1.0));
            let mut rng = Rng(5);
            let mut t = 0.1;
            while t < 2.0 {
                script.volume.push((t, rng.unit()));
                t += 0.005 + 0.03 * rng.unit();
            }
            let r = render(&script, &[clip], pack_rate, rate);
            // Skip the filter's settling at the clip's start.
            let steady = &r.out[(0.01 * rate) as usize..];
            let level = 0.25;
            // The ramp: a full-scale change takes at least 20 ms.
            let allowed = level / (0.020 * rate) as f32;
            let step = steady.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
            assert!(
                step <= allowed * 1.01,
                "{pack_rate} -> {rate} Hz: step {step} > {allowed} allowed by the ramp"
            );
        }
    }
}
