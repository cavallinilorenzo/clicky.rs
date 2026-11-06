//! The look-ahead peak limiter at the end of the mix.
//!
//! It adds no latency: the mixer renders every voice [`Limiter::lookahead`]
//! frames ahead of what it outputs (a voice's future is known, it is a
//! recording), so the limiter sees each peak coming. Only a keystroke that
//! starts in the current block is not foreseen, and then only its first
//! frames.
//!
//! The gain each frame needs on its own (`TARGET / |x|`, else 1) goes
//! through a minimum over the look-ahead window, an exponential release
//! (attack is instant at this stage), and two moving averages whose lengths
//! add up to the window. The result is a smooth gain curve (no steps, no
//! corners) that is already down to what a peak needs when the peak
//! arrives. A last `min` with the frame's own need keeps the ceiling exact
//! even for a peak that was not foreseen (a keystroke starting this block).
//! Below the ceiling it is the identity, bit for bit.

/// No output sample exceeds this: −1 dBFS.
pub const CEILING: f32 = 0.891_250_94;
/// What the gain aims for, a hair under the ceiling so rounding cannot pass it.
const TARGET: f32 = CEILING * 0.999_9;
/// How far ahead the limiter looks.
const LOOKAHEAD_SECS: f64 = 0.0015;
/// Release time constant.
const RELEASE_SECS: f64 = 0.060;
/// Look-ahead frames at most (1.5 ms up to 213 kHz).
pub const MAX_LOOKAHEAD: usize = 320;
/// Frames the limiter handles per call at most.
pub const CHUNK: usize = 256;
const MAX_BOX: usize = MAX_LOOKAHEAD / 2 + 2;

/// A moving average over the last `len` values.
struct Average {
    values: [f32; MAX_BOX],
    len: usize,
    next: usize,
}

impl Average {
    fn new(len: usize) -> Average {
        Average { values: [1.0; MAX_BOX], len, next: 0 }
    }

    fn push(&mut self, value: f32) -> f32 {
        self.values[self.next] = value;
        self.next = (self.next + 1) % self.len;
        // Summed afresh each time: no drift, and exactly 1 once every value is.
        let sum: f32 = self.values[..self.len].iter().sum();
        if sum == self.len as f32 { 1.0 } else { sum / self.len as f32 }
    }
}

pub struct Limiter {
    /// The released envelope (before smoothing) at the previous frame.
    envelope: f32,
    /// Per-frame release coefficient.
    release: f32,
    lookahead: usize,
    first: Average,
    second: Average,
    /// Frames until the averages hold only ones again.
    settling: usize,
    /// Gain each frame needs on its own (1 below the ceiling).
    need: [f32; CHUNK + MAX_LOOKAHEAD],
}

impl Limiter {
    pub fn new(rate: f64) -> Limiter {
        let lookahead = ((rate * LOOKAHEAD_SECS).round() as usize).clamp(2, MAX_LOOKAHEAD);
        // first + second - 2 == lookahead: each average reaches back at most
        // as far as the window reaches forward.
        let first = lookahead / 2 + 1;
        let second = lookahead + 2 - first;
        let release = 1.0 - (-1.0 / (RELEASE_SECS * rate)).exp();
        Limiter {
            envelope: 1.0,
            release: release as f32,
            lookahead,
            first: Average::new(first),
            second: Average::new(second),
            settling: 0,
            need: [1.0; CHUNK + MAX_LOOKAHEAD],
        }
    }

    pub fn lookahead(&self) -> usize {
        self.lookahead
    }

    /// Back to unity gain (after the mix was silenced).
    pub fn reset(&mut self) {
        self.envelope = 1.0;
        self.first = Average::new(self.first.len);
        self.second = Average::new(self.second.len);
        self.settling = 0;
    }

    /// Limits `x[..out.len()]` into `out`, looking at `x[out.len()..]` ahead.
    /// `x` holds `out.len() + lookahead` frames; `out.len() <= CHUNK`.
    /// Real-time safe.
    pub fn process(&mut self, x: &[f32], out: &mut [f32]) {
        let n = out.len();
        let la = self.lookahead;
        let x = &x[..n + la];
        let need = &mut self.need[..n + la];
        let mut limiting = self.envelope < 1.0 || self.settling > 0;
        for (r, s) in need.iter_mut().zip(x) {
            let a = s.abs();
            *r = if a > TARGET { TARGET / a } else { 1.0 };
            limiting |= *r < 1.0;
        }
        if !limiting {
            out.copy_from_slice(&x[..n]);
            return;
        }
        let span = self.first.len + self.second.len;
        for (t, o) in out.iter_mut().enumerate() {
            let hold = need[t..=t + la].iter().copied().fold(1.0f32, f32::min);
            let mut env = self.envelope + (1.0 - self.envelope) * self.release;
            if env > 0.999_999 {
                env = 1.0;
            }
            env = env.min(hold);
            self.envelope = env;
            self.settling = if env < 1.0 { span } else { self.settling.saturating_sub(1) };
            let smooth = self.second.push(self.first.push(env));
            *o = x[t] * smooth.min(need[t]);
        }
    }
}
