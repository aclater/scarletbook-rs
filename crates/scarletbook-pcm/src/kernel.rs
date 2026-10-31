// SPDX-License-Identifier: MIT OR Apache-2.0

//! The conversion: the stage plan of each rate and cutoff, and the filter kernels.
//!
//! Stages (multistage decimation, Crochiere & Rabiner 1981):
//!
//! - **S1**, 2822.4 to 352.8 kHz: a symmetric FIR on the 1-bit input, evaluated by table
//!   lookup. Each output is on the byte grid (output `m` is centred on DSD sample `8m`, the
//!   first sample of byte `m`), and the taps are grouped by the byte they fall in: a table
//!   of 256 partial sums per byte position, indexed by the byte (distributed arithmetic:
//!   Peled & Liu 1974; White, IEEE ASSP Magazine 6(3), 1989). Outputs are exact integers.
//! - **Dec2**: a symmetric FIR decimating by 2, on the even and odd samples of its input
//!   (polyphase form), so that the work is per output sample.
//! - **Fir1**: a symmetric FIR at the same rate.
//! - **Interp2**: halfband interpolation by 2: the even outputs are the input samples
//!   unchanged (the halfband's centre tap is 1/2, times the interpolation gain of 2), the
//!   odd outputs a symmetric FIR of the odd taps.
//!
//! Arithmetic. Between stages, samples are integers on a grid where 1.0 (full scale) is
//! 2^26. Coefficients are integers (scaled by 2^26 for S1, 2^24 for the others). A stage
//! sums products exactly and rounds the sum, scaled back to the 2^26 grid, half to even.
//! The sums are carried in `f64`: every operand, product and partial sum is an integer
//! below 2^53 (checked over the generated tables by `design::tests`), and IEEE-754
//! arithmetic on such integers is exact (Goldberg, ACM Computing Surveys 23(1), 1991), so
//! the order of summation, vector width and FMA use cannot change a result.

use std::ops::Range;
use std::sync::OnceLock;

use crate::{ConvertStats, Cutoff, FRAME_BYTES_PER_CHANNEL, FrameRun, PcmRate, tables};

/// Output samples computed together by the kernels (a block of independent sums the
/// compiler can put in vector lanes).
const LANES: usize = 8;

/// Output frames converted per pass. Each pass recomputes its stages' margins, so larger
/// passes waste less; the scratch buffers grow with it (about 0.5 MiB at 4 frames).
const CHUNK_FRAMES: usize = 4;

/// The most stages after S1 (352.8 kHz output: halfband, sharp filter, interpolation).
const MAX_OPS: usize = 3;
/// The coefficient scale of the other stages.
const Q_FIR: i32 = 24;

/// `x` rounded to an integer, half to even: `(x + 1.5 x 2^52) - 1.5 x 2^52` in IEEE-754
/// round-to-nearest-even arithmetic, exact for `|x| < 2^51` (all values here are below
/// 2^30). It vectorises on every x86-64, unlike a call to `round_ties_even`.
#[inline(always)]
fn round_even(x: f64) -> f64 {
    const MAGIC: f64 = 6_755_399_441_055_744.0;
    (x + MAGIC) - MAGIC
}

/// `10^(db / 20)`, for `|db| <= 120`, with IEEE-754 additions, multiplications and
/// divisions only, in a fixed order: `exp(y)` with `y = |db| ln(10) / 20` as
/// `T(y / 32)^32`, where `T` is the Taylor series of `exp` (25 terms; `y / 32 < 0.44`),
/// and the reciprocal for negative `db`. Every step is monotonic in its input, so the
/// result is monotonic in `db`; it is exactly 1 at 0 dB.
pub(crate) fn db_to_linear(db: f64) -> f64 {
    let r = db.abs() * (std::f64::consts::LN_10 / 20.0) / 32.0;
    let (mut term, mut sum) = (1.0f64, 1.0f64);
    for k in 1..=25 {
        term = term * r / f64::from(k);
        sum += term;
    }
    for _ in 0..5 {
        sum *= sum;
    }
    if db < 0.0 { 1.0 / sum } else { sum }
}

/// The S1 lookup tables: entry `[j][b]` is the sum of the taps over the 8 samples of the
/// `j`-th byte of the window when that byte is `b` (+tap for a 1 bit, -tap for a 0 bit).
pub(crate) type Lut = Vec<[i32; 256]>;

/// The tables of the S1 filter `half` (taps from the centre out, half-length `m = 8k + 7`).
/// The window of output `m` is bytes `m - B/2 .. m + B/2`, `B = (m + 1) / 4`; the window's
/// first tap position (offset `-(m + 1)`) is beyond the filter and contributes 0.
pub(crate) fn lut(half: &[i32]) -> Lut {
    let m = half.len() - 1;
    assert_eq!(m % 8, 7, "S1 half-length must fill whole bytes");
    let bytes = (m + 1) / 4;
    let tap = |t: i64| -> i32 { half.get(t.unsigned_abs() as usize).copied().unwrap_or(0) };
    (0..bytes)
        .map(|j| {
            let rel = j as i64 - (bytes / 2) as i64;
            let mut table = [0i32; 256];
            for (b, entry) in table.iter_mut().enumerate() {
                *entry = (0..8)
                    .map(|p| {
                        let h = tap(8 * rel + p);
                        if (b >> (7 - p)) & 1 == 1 { h } else { -h }
                    })
                    .sum();
            }
            table
        })
        .collect()
}

static LUT_S1: OnceLock<Lut> = OnceLock::new();
static LUT_S1_WIDE: OnceLock<Lut> = OnceLock::new();

/// A symmetric filter, from its table (taps from the centre out).
#[derive(Clone, Debug)]
struct Sym {
    /// Taps from the centre out.
    full: Vec<f64>,
    /// Taps at even offsets 0, 2, 4, ... (trailing zeros removed).
    even: Vec<f64>,
    /// Taps at odd offsets 1, 3, 5, ...
    odd: Vec<f64>,
    /// 2^-q: from the product scale back to the sample grid.
    scale: f64,
}

impl Sym {
    fn new(half: &[i32], q: i32) -> Sym {
        let full: Vec<f64> = half.iter().map(|&h| f64::from(h)).collect();
        let mut even: Vec<f64> = full.iter().step_by(2).copied().collect();
        while even.len() > 1 && even.last() == Some(&0.0) {
            even.pop();
        }
        let odd = full.iter().skip(1).step_by(2).copied().collect();
        Sym { full, even, odd, scale: 2f64.powi(-q) }
    }

    /// The largest tap offset.
    fn half_len(&self) -> usize {
        self.full.len() - 1
    }
}

/// One stage after S1.
#[derive(Clone, Debug)]
enum Op {
    Fir1(Sym),
    Dec2(Sym),
    /// Halfband interpolation by 2, by the halfband's odd taps.
    Interp2(Sym),
}

impl Op {
    /// The input range `(start, len)` that outputs `start .. start + len` need. With `pad`,
    /// the kernels' extra outputs (whole blocks of `LANES`) are included.
    fn need(&self, (a, n): (i64, usize), pad: bool) -> (i64, usize) {
        let padn = |n: usize| if pad { n.div_ceil(LANES) * LANES } else { n };
        match self {
            Op::Fir1(s) => {
                let m = s.half_len();
                (a - m as i64, padn(n) + 2 * m)
            }
            Op::Dec2(s) => {
                let m = s.half_len();
                (2 * a - m as i64, 2 * padn(n) + 2 * m - 1)
            }
            Op::Interp2(s) => {
                let o = s.odd.len();
                (a / 2 - o as i64, padn(n / 2) + 2 * o)
            }
        }
    }
}

/// The run being converted.
#[derive(Debug)]
pub(crate) struct Input<'a> {
    pub run: FrameRun<'a>,
    pub channels: usize,
    pub frames: usize,
}

impl Input<'_> {
    /// The run-relative byte positions of channel bytes that exist (the rest are beyond the
    /// edge of the area, or not needed).
    fn valid(&self) -> Range<i64> {
        let fb = FRAME_BYTES_PER_CHANNEL as i64;
        let end = self.frames as i64 * fb;
        let lo = if self.run.before.is_some() { -fb } else { 0 };
        let hi = if self.run.after.is_some() { end + fb } else { end };
        lo..hi
    }

    /// Channel `c`'s bytes from run-relative position `from` on, into `dst` (all within
    /// `valid()`).
    fn copy(&self, c: usize, from: i64, dst: &mut [u8]) {
        let fb = FRAME_BYTES_PER_CHANNEL as i64;
        let end = self.frames as i64 * fb;
        let ch = self.channels;
        for (i, d) in dst.iter_mut().enumerate() {
            let p = from + i as i64;
            let (src, at) = if p < 0 {
                (self.run.before, p + fb)
            } else if p < end {
                (Some(self.run.frames), p)
            } else {
                (self.run.after, p - end)
            };
            // Callers pass only positions within `valid()`, where the source exists.
            if let Some(src) = src {
                *d = src[at as usize * ch + c];
            }
        }
    }
}

/// Buffers reused across calls. They grow on the first conversion to a size set by
/// `CHUNK_FRAMES` and the filter lengths, never by the input.
#[derive(Debug, Default)]
pub(crate) struct Scratch {
    bytes: Vec<u8>,
    a: Vec<f64>,
    b: Vec<f64>,
    even: Vec<f64>,
    odd: Vec<f64>,
}

/// The stages of one rate and cutoff.
#[derive(Debug)]
pub(crate) struct Plan {
    lut: &'static Lut,
    ops: Vec<Op>,
    spf: usize,
    context: usize,
}

impl Plan {
    pub(crate) fn new(rate: PcmRate, cutoff: Cutoff) -> Plan {
        let hb = Sym::new(&tables::HB, Q_FIR);
        let f40 = Sym::new(&tables::F40, Q_FIR);
        let (lut, ops) = match (cutoff, rate) {
            (Cutoff::Wide, PcmRate::Hz352800) => {
                (LUT_S1_WIDE.get_or_init(|| lut(&tables::S1_WIDE)), vec![Op::Fir1(f40)])
            }
            (Cutoff::Wide, _) => (LUT_S1_WIDE.get_or_init(|| lut(&tables::S1_WIDE)), vec![Op::Dec2(f40)]),
            (_, rate) => {
                let f = if cutoff == Cutoff::Low { Sym::new(&tables::F30, Q_FIR) } else { f40 };
                let ops = match rate {
                    PcmRate::Hz88200 => vec![Op::Dec2(hb), Op::Dec2(f)],
                    PcmRate::Hz176400 => vec![Op::Dec2(hb), Op::Fir1(f)],
                    PcmRate::Hz352800 => vec![Op::Dec2(hb.clone()), Op::Fir1(f), Op::Interp2(hb)],
                };
                (LUT_S1.get_or_init(|| lut(&tables::S1)), ops)
            }
        };
        let mut plan = Plan { lut, ops, spf: rate.samples_per_frame(), context: 0 };
        let (bytes_lo, bytes_len) = plan.byte_range((0, plan.spf), false);
        let fb = FRAME_BYTES_PER_CHANNEL as i64;
        plan.context = (-bytes_lo).max(bytes_lo + bytes_len as i64 - fb).max(0) as usize;
        plan
    }

    /// Bytes per channel read from each neighbour (at most; less than a frame).
    pub(crate) fn context_bytes(&self) -> usize {
        self.context
    }

    /// The ranges of every level for outputs `out`: `[0]` is S1's output, the last the
    /// output itself.
    fn levels(&self, out: (i64, usize), pad: bool) -> [(i64, usize); MAX_OPS + 1] {
        let mut levels = [(0, 0); MAX_OPS + 1];
        let k = self.ops.len();
        levels[k] = out;
        for (i, op) in self.ops.iter().enumerate().rev() {
            levels[i] = op.need(levels[i + 1], pad);
        }
        levels
    }

    /// The run-relative bytes that outputs `out` need.
    fn byte_range(&self, out: (i64, usize), pad: bool) -> (i64, usize) {
        let (s, n) = self.levels(out, pad)[0];
        let n = if pad { n.div_ceil(LANES) * LANES } else { n };
        let b = self.lut.len();
        (s - (b / 2) as i64, n + b - 1)
    }

    pub(crate) fn convert(
        &self,
        input: &Input<'_>,
        gain: f64,
        sc: &mut Scratch,
        out: &mut [i32],
        stats: &mut ConvertStats,
    ) {
        debug_assert!(self.context < FRAME_BYTES_PER_CHANNEL);
        let ch = input.channels;
        let valid = input.valid();
        let mut f0 = 0;
        while f0 < input.frames {
            let nf = CHUNK_FRAMES.min(input.frames - f0);
            let o0 = (f0 * self.spf) as i64;
            let n = nf * self.spf;
            let levels = self.levels((o0, n), true);
            let (p0, plen) = self.byte_range((o0, n), true);
            for c in 0..ch {
                // The channel's bytes, and the part of them that exists.
                sc.bytes.clear();
                sc.bytes.resize(plen, 0);
                let lo = valid.start.max(p0);
                let hi = valid.end.min(p0 + plen as i64);
                if lo < hi {
                    input.copy(c, lo, &mut sc.bytes[(lo - p0) as usize..(hi - p0) as usize]);
                }
                let ok = (lo - p0).max(0) as usize..(hi - p0).max(0) as usize;
                // S1 into `a`.
                let n1 = levels[0].1.div_ceil(LANES) * LANES;
                sc.a.clear();
                sc.a.resize(n1, 0.0);
                s1(self.lut, &sc.bytes, ok, &mut sc.a);
                // The other stages, `a` to `b` and swap.
                for (k, op) in self.ops.iter().enumerate() {
                    let (start_out, n_out) = levels[k + 1];
                    let start_in = levels[k].0;
                    match op {
                        Op::Fir1(s) => {
                            sc.b.clear();
                            sc.b.resize(n_out.div_ceil(LANES) * LANES, 0.0);
                            debug_assert_eq!(start_out - start_in, s.half_len() as i64);
                            fir1(s, &sc.a, &mut sc.b);
                        }
                        Op::Dec2(s) => {
                            let m = s.half_len();
                            let (e, o) = (s.even.len() - 1, s.odd.len());
                            let np = n_out.div_ceil(LANES) * LANES;
                            debug_assert_eq!(start_in, 2 * start_out - m as i64);
                            // xe[i] = x[2(a - e + i)], xo[i] = x[2(a - o + i) + 1].
                            let (de, dodd) = (m - 2 * e, m + 1 - 2 * o);
                            sc.even.clear();
                            sc.even.extend(sc.a.iter().skip(de).step_by(2).take(np + 2 * e));
                            sc.odd.clear();
                            sc.odd.extend(sc.a.iter().skip(dodd).step_by(2).take(np + 2 * o));
                            sc.b.clear();
                            sc.b.resize(np, 0.0);
                            dec2(s, &sc.even, &sc.odd, &mut sc.b);
                        }
                        Op::Interp2(s) => {
                            let o = s.odd.len();
                            let half = (n_out / 2).div_ceil(LANES) * LANES;
                            debug_assert_eq!(start_in, start_out / 2 - o as i64);
                            sc.even.clear();
                            sc.even.resize(half, 0.0);
                            interp2_odd(s, &sc.a, &mut sc.even);
                            sc.b.clear();
                            for i in 0..n_out / 2 {
                                sc.b.push(sc.a[o + i]);
                                sc.b.push(sc.even[i]);
                            }
                        }
                    }
                    std::mem::swap(&mut sc.a, &mut sc.b);
                }
                // To 24 bits: `a` holds the outputs on the 2^26 grid.
                let mut clipped = 0u64;
                let dst = out[o0 as usize * ch..].iter_mut().skip(c).step_by(ch);
                for (d, &v) in dst.zip(&sc.a[..n]) {
                    let y = round_even((v * gain) * 0.125);
                    let y = if y > 8_388_607.0 {
                        clipped += 1;
                        8_388_607
                    } else if y < -8_388_608.0 {
                        clipped += 1;
                        -8_388_608
                    } else {
                        y as i32
                    };
                    *d = y;
                }
                stats.clipped[c] += clipped;
            }
            f0 += nf;
        }
    }
}

/// S1: `out[i]` = the sum of `lut[j][bytes[i + j]]`; bytes outside `ok` are beyond the area
/// and contribute nothing.
fn s1(lut: &[[i32; 256]], bytes: &[u8], ok: Range<usize>, out: &mut [f64]) {
    let b = lut.len();
    for (i, o) in out.iter_mut().enumerate() {
        let w = &bytes[i..i + b];
        let sum: i32 = if i >= ok.start && i + b <= ok.end {
            lut.iter().zip(w).map(|(t, &x)| t[usize::from(x)]).sum()
        } else {
            lut.iter()
                .zip(w)
                .enumerate()
                .filter(|&(j, _)| ok.contains(&(i + j)))
                .map(|(_, (t, &x))| t[usize::from(x)])
                .sum()
        };
        *o = f64::from(sum);
    }
}

/// Adds `h[j] (x[hi + j + l] + x[lo - j + l])` over `j`, for the `LANES` outputs `l`.
#[inline(always)]
fn add_pairs(acc: &mut [f64; LANES], h: &[f64], x: &[f64], hi: usize, lo: usize) {
    for (j, &hj) in h.iter().enumerate() {
        let a = &x[hi + j..hi + j + LANES];
        let b = &x[lo - j..lo - j + LANES];
        for l in 0..LANES {
            acc[l] += hj * (a[l] + b[l]);
        }
    }
}

/// Runs `f` on each block of `LANES` outputs and stores the rounded, rescaled sums.
#[inline(always)]
fn blocks(out: &mut [f64], scale: f64, f: impl Fn(usize, &mut [f64; LANES])) {
    for (bi, o) in out.chunks_exact_mut(LANES).enumerate() {
        let mut acc = [0.0f64; LANES];
        f(bi * LANES, &mut acc);
        for (d, a) in o.iter_mut().zip(acc) {
            *d = round_even(a * scale);
        }
    }
}

/// Fir1: `out[i]` centred on `x[i + m]`.
fn fir1(s: &Sym, x: &[f64], out: &mut [f64]) {
    let m = s.half_len();
    let (h0, h) = (s.full[0], &s.full[1..]);
    blocks(out, s.scale, |i, acc| {
        for (l, a) in acc.iter_mut().enumerate() {
            *a = h0 * x[i + m + l];
        }
        add_pairs(acc, h, x, i + m + 1, i + m - 1);
    });
}

/// Dec2: `out[i]` centred on `xe[i + e]` (`e` = the even taps after the centre), with the
/// odd taps on `xo[i + o + j]` and `xo[i + o - 1 - j]`.
fn dec2(s: &Sym, xe: &[f64], xo: &[f64], out: &mut [f64]) {
    let (e, o) = (s.even.len() - 1, s.odd.len());
    let (h0, he) = (s.even[0], &s.even[1..]);
    blocks(out, s.scale, |i, acc| {
        for (l, a) in acc.iter_mut().enumerate() {
            *a = h0 * xe[i + e + l];
        }
        if e > 0 {
            add_pairs(acc, he, xe, i + e + 1, i + e - 1);
        }
        add_pairs(acc, &s.odd, xo, i + o, i + o - 1);
    });
}

/// The odd outputs of Interp2: `out[i]` between `w[i + o]` and `w[i + o + 1]`. The
/// interpolation gain of 2 is in the scale (2^-(q - 1)).
fn interp2_odd(s: &Sym, w: &[f64], out: &mut [f64]) {
    let o = s.odd.len();
    blocks(out, s.scale * 2.0, |i, acc| add_pairs(acc, &s.odd, w, i + o + 1, i + o));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_even_matches_std() {
        for x in [0.0, 0.5, 1.5, 2.5, -0.5, -1.5, -2.5, 3.49, -3.51, 1e9 + 0.5, -(2f64.powi(40)) - 0.5] {
            assert_eq!(round_even(x), x.round_ties_even(), "{x}");
        }
    }

    #[test]
    fn db_to_linear_is_exact_at_0_and_close_elsewhere() {
        assert_eq!(db_to_linear(0.0), 1.0);
        assert_eq!(db_to_linear(-0.0), 1.0);
        for db in [-120.0, -60.0, -6.0, -0.1, 0.1, 6.0, 20.0, 24.0] {
            let want = 10f64.powf(db / 20.0);
            assert!((db_to_linear(db) / want - 1.0).abs() < 1e-13, "{db}");
        }
    }

    #[test]
    fn db_to_linear_is_monotonic() {
        let mut prev = db_to_linear(-120.0);
        // Every 0.0005 dB, plus neighbours in the last bit around 0.
        for i in 1..=288_000 {
            let g = db_to_linear(-120.0 + i as f64 * 0.0005);
            assert!(g >= prev, "{i}");
            prev = g;
        }
        let mut x = -1e-300f64;
        let mut prev = db_to_linear(x);
        for _ in 0..2000 {
            x = f64::from_bits(if x < 0.0 { x.to_bits() - (1 << 40) } else { x.to_bits() + (1 << 40) })
                .max(x);
            let g = db_to_linear(x);
            assert!(g >= prev);
            prev = g;
        }
    }
}
