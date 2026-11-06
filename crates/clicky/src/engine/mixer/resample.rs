//! Band-limited resampling of a voice from its soundpack's rate to the
//! device rate: a Kaiser-windowed sinc, tabulated as a polyphase filter.
//!
//! The filter's cutoff sits at 95 % of the lower rate's Nyquist frequency
//! (20.9 kHz between 44.1 and 48 kHz), with 32 zero crossings on each side
//! of the centre and a Kaiser window of β = 9 (about 90 dB of stopband
//! rejection). The table holds [`PHASES`] fractional positions per source
//! sample; a voice reads between two of them with linear interpolation, so
//! any ratio works. Every row is normalised to unit DC gain.
//!
//! Why not linear or cubic interpolation: the tracer's linear interpolation let
//! a 23.5 kHz tone fold back into a 44.1 kHz output at −4.8 dB, and left a
//! −62 dB residual on a 1 kHz sine. A windowed sinc is the standard remedy.
//! Its cost is a 72-tap dot product per voice and frame (152 taps for 96 → 44.1
//! kHz), vectorised.
//!
//! Measured by the tests in `mixer.rs` (sines at 0.5 of full scale, 44.1 ↔ 48 kHz
//! and 44.1/48 → 96 kHz, bounds in brackets): the passband from 100 Hz to 18 kHz
//! is flat within ±0.0003 dB [±0.05 dB]. Images, aliases and interpolation
//! error together sit at −91 to −99 dB [−80 dB], the 16-bit floor of the test
//! clips. Tones at 22.8 / 23.5 kHz at 48 kHz leak −82.7 / −90.8 dB [−80 dB] into
//! a 44.1 kHz output instead of folding back below 21.3 kHz.
//!
//! Tables are built off the real-time path (when a `Mixer` is created) and
//! cached for the life of the process; the render callback only reads them.

use std::sync::Mutex;

/// Zero crossings of the sinc on each side of the centre.
const ZERO_CROSSINGS: f64 = 32.0;
/// Kaiser window shape.
const BETA: f64 = 9.0;
/// Cutoff as a fraction of the lower rate's Nyquist frequency.
const ROLL_OFF: f64 = 0.95;
/// Fractional positions tabulated per source sample (a power of two).
const PHASE_BITS: u32 = 8;
pub const PHASES: usize = 1 << PHASE_BITS;
/// Rows are padded to a multiple of this, for the vectorised dot product.
const LANES: usize = 8;

/// Soundpack rates with a filter of their own. A pack at another rate uses
/// the filter of the next rate up (a slightly lower cutoff: never aliasing);
/// above the last one it may alias, but the pack pipeline never produces such packs.
const PACK_RATES: [u32; 11] =
    [8_000, 11_025, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000, 64_000, 88_200, 96_000];

/// One tabulated filter.
pub struct Kernel {
    /// Source samples on each side of the read position.
    half: usize,
    /// Coefficients per row (`2 * half`, padded to [`LANES`]).
    taps: usize,
    /// `PHASES + 1` rows; row `p` holds the weights of the samples
    /// `floor(pos) - half + 1 ..` for a fractional position `p / PHASES`.
    rows: Box<[f32]>,
}

impl Kernel {
    /// A filter with its cutoff at `cutoff` times the source's Nyquist frequency.
    fn new(cutoff: f64) -> Kernel {
        let reach = ZERO_CROSSINGS / cutoff;
        let half = reach.ceil() as usize;
        let taps = (2 * half).div_ceil(LANES) * LANES;
        let mut rows = vec![0.0f32; (PHASES + 1) * taps];
        let norm = bessel_i0(BETA);
        let mut row = vec![0.0f64; 2 * half];
        for p in 0..=PHASES {
            let frac = p as f64 / PHASES as f64;
            for (k, w) in row.iter_mut().enumerate() {
                // Distance from the read position to the sample this tap weighs.
                let d = frac + half as f64 - 1.0 - k as f64;
                *w = if d.abs() >= reach {
                    0.0
                } else {
                    let x = d / reach;
                    cutoff * sinc(cutoff * d) * bessel_i0(BETA * (1.0 - x * x).sqrt()) / norm
                };
            }
            let sum: f64 = row.iter().sum();
            for (dst, w) in rows[p * taps..].iter_mut().zip(&row) {
                *dst = (w / sum) as f32;
            }
        }
        Kernel { half, taps, rows: rows.into_boxed_slice() }
    }

    /// The filter with this cutoff, from the process-wide cache (built and
    /// leaked on first use; a handful exist per process). Not real-time safe.
    fn cached(cutoff: f64) -> &'static Kernel {
        static CACHE: Mutex<Vec<(u64, &'static Kernel)>> = Mutex::new(Vec::new());
        let key = cutoff.to_bits();
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, k)) = cache.iter().find(|(c, _)| *c == key) {
            return k;
        }
        let kernel: &'static Kernel = Box::leak(Box::new(Kernel::new(cutoff)));
        cache.push((key, kernel));
        kernel
    }

    /// Source samples the filter reaches past the end of a clip (its tail).
    pub fn half(&self) -> usize {
        self.half
    }

    /// The band-limited value of `clip` (zero outside it) at `pos`, a 32.32
    /// fixed-point source position, in `i16` units. Real-time safe.
    #[inline]
    pub fn read(&self, clip: &[i16], pos: u64) -> f32 {
        let index = (pos >> 32) as i64;
        let frac = (pos & 0xFFFF_FFFF) as u32;
        let phase = (frac >> (32 - PHASE_BITS)) as usize;
        let t = (frac & ((1 << (32 - PHASE_BITS)) - 1)) as f32 / (1u32 << (32 - PHASE_BITS)) as f32;
        let a = &self.rows[phase * self.taps..][..self.taps];
        let b = &self.rows[(phase + 1) * self.taps..][..self.taps];
        let first = index - self.half as i64 + 1;
        let (x, y) = if first >= 0 && first as usize + self.taps <= clip.len() {
            dot2(a, b, &clip[first as usize..][..self.taps])
        } else {
            // Near the clip's edges: gather with zeros outside it.
            let mut window = [0i16; MAX_TAPS];
            for (k, w) in window[..self.taps].iter_mut().enumerate() {
                let i = first + k as i64;
                if i >= 0
                    && let Some(&s) = clip.get(i as usize)
                {
                    *w = s;
                }
            }
            dot2(a, b, &window[..self.taps])
        };
        x + (y - x) * t
    }
}

/// Coefficients per row at most: enough for 96 kHz down to 44.1 kHz (152).
/// A steeper conversion gets a filter of this length with a cutoff that is
/// too high (see [`min_cutoff`]); no real device and pack pair needs one.
const MAX_TAPS: usize = 160;

/// Two dot products of `samples` with the rows `a` and `b`, eight lanes at a
/// time (so the compiler vectorises it). All three have the same length, a
/// multiple of [`LANES`].
#[inline]
fn dot2(a: &[f32], b: &[f32], samples: &[i16]) -> (f32, f32) {
    let mut x = [0.0f32; LANES];
    let mut y = [0.0f32; LANES];
    let (a, b, samples) =
        (a.as_chunks::<LANES>().0, b.as_chunks::<LANES>().0, samples.as_chunks::<LANES>().0);
    for ((a, b), s) in a.iter().zip(b).zip(samples) {
        for l in 0..LANES {
            let s = f32::from(s[l]);
            x[l] += a[l] * s;
            y[l] += b[l] * s;
        }
    }
    (x.iter().sum(), y.iter().sum())
}

/// The filters one device rate needs, prepared off the real-time path.
#[derive(Clone, Copy)]
pub struct Kernels {
    out_rate: f64,
    /// For any source at or below the device rate.
    up: &'static Kernel,
    /// For the pack rates above the device rate, ascending.
    down: [Option<(u32, &'static Kernel)>; PACK_RATES.len()],
}

impl Kernels {
    /// Builds (or takes from the cache) every filter `out_rate` may need. Not real-time safe.
    pub fn new(out_rate: f64) -> Kernels {
        let mut down = [None; PACK_RATES.len()];
        for (slot, &rate) in
            down.iter_mut().zip(PACK_RATES.iter().filter(|r| f64::from(**r) > out_rate))
        {
            let cutoff = (ROLL_OFF * out_rate / f64::from(rate)).max(min_cutoff());
            *slot = Some((rate, Kernel::cached(cutoff)));
        }
        Kernels { out_rate, up: Kernel::cached(ROLL_OFF), down }
    }

    /// The filter for a voice recorded at `src_rate`. Real-time safe.
    pub fn get(&self, src_rate: u32) -> &'static Kernel {
        if f64::from(src_rate) <= self.out_rate {
            return self.up;
        }
        let mut last = self.up;
        for (rate, kernel) in self.down.iter().flatten() {
            last = kernel;
            if *rate >= src_rate {
                return kernel;
            }
        }
        last
    }
}

/// The lowest cutoff whose filter fits in [`MAX_TAPS`].
fn min_cutoff() -> f64 {
    2.0 * ZERO_CROSSINGS / (MAX_TAPS - LANES) as f64
}

fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        1.0
    } else {
        let px = std::f64::consts::PI * x;
        px.sin() / px
    }
}

/// The modified Bessel function of the first kind, order 0 (power series).
fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term) = (1.0, 1.0);
    let q = x * x / 4.0;
    for k in 1..64 {
        term *= q / (k * k) as f64;
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}
