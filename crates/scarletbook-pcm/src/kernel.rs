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

use crate::dispatch::{self, Isa};
use crate::{ConvertStats, Cutoff, FRAME_BYTES_PER_CHANNEL, Frames, PcmOut, PcmRate, Run, tables};

/// Stage outputs are computed in multiples of this: a multiple of every kernel's block of
/// independent sums (`dispatch`).
const LANES: usize = 32;

/// Output frames converted per pass. Each pass recomputes its stages' margins, so larger
/// passes waste less; the scratch buffers grow with it (about 0.5 MiB at 4 frames).
const CHUNK_FRAMES: usize = 4;

/// The most stages after S1 (352.8 kHz output: halfband, sharp filter, interpolation).
const MAX_OPS: usize = 3;
/// The coefficient scale of the other stages.
const Q_FIR: i32 = 24;

/// 1.5 x 2^52: an integer below 2^51 in magnitude plus this lies in `[2^52, 2^53)`, where
/// binary64 values are exactly the integers.
const MAGIC: f64 = 6_755_399_441_055_744.0;

/// `x` rounded to an integer, half to even: `(x + 1.5 x 2^52) - 1.5 x 2^52` in IEEE-754
/// round-to-nearest-even arithmetic, exact for `|x| < 2^51` (all values here are below
/// 2^30). It vectorises on every x86-64, unlike a call to `round_ties_even`.
#[inline(always)]
fn round_even(x: f64) -> f64 {
    (x + MAGIC) - MAGIC
}

/// The integer-valued `x`, `-2^31 <= x < 2^31`, as `i32`: `x + 1.5 x 2^52` holds `x` in two's
/// complement in the low bits of its significand. Unlike `x as i32`, which has to handle NaN
/// and saturate, it vectorises (issue #224).
#[inline(always)]
fn to_i32(x: f64) -> i32 {
    (x + MAGIC).to_bits() as i32
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

/// The idle pattern (sacd-spec, specification 0): DSD silence, and the input beyond the
/// edges of an area. S1 has the factor `(1 + z^-1)^2 (1 + z^-4)` (`design::idle_factor`),
/// whose `1 + z^-4` cancels it exactly, so every output whose window holds only this
/// pattern is exactly 0.
pub(crate) const IDLE: u8 = 0x69;

static LUT_S1: OnceLock<Lut> = OnceLock::new();
static LUT_S1_WIDE: OnceLock<Lut> = OnceLock::new();

/// `2^e`, built from its bits: exact, unlike `powi`, whose precision Rust does not specify.
fn pow2(e: i32) -> f64 {
    debug_assert!((-1022..=1023).contains(&e));
    f64::from_bits(((1023 + e) as u64) << 52)
}

/// A symmetric filter, from its table (taps from the centre out).
#[derive(Clone, Debug)]
pub(crate) struct Sym {
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
        Sym { full, even, odd, scale: pow2(-q) }
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
    pub run: Run<'a>,
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

    /// Frame `f` of the run: -1 is the frame before it, `frames` the frame after it.
    fn frame(&self, f: i64) -> Option<&[u8]> {
        if f < 0 {
            self.run.before
        } else if f >= self.frames as i64 {
            self.run.after
        } else {
            let f = f as usize;
            match self.run.frames {
                Frames::Contiguous(b) => {
                    let len = FRAME_BYTES_PER_CHANNEL * self.channels;
                    Some(&b[f * len..(f + 1) * len])
                }
                Frames::Separate(s) => Some(s[f]),
            }
        }
    }

    /// Channel `c`'s bytes from run-relative position `from` on, into `dst` (all within
    /// `valid()`), one frame at a time.
    fn copy(&self, isa: Isa, c: usize, from: i64, dst: &mut [u8]) {
        let fb = FRAME_BYTES_PER_CHANNEL as i64;
        let ch = self.channels;
        let mut p = from;
        let mut rest = dst;
        while !rest.is_empty() {
            let f = p.div_euclid(fb);
            let at = (p - f * fb) as usize;
            let take = rest.len().min(FRAME_BYTES_PER_CHANNEL - at);
            let (part, tail) = rest.split_at_mut(take);
            // Callers pass only positions within `valid()`, where the source exists.
            if let Some(src) = self.frame(f) {
                dispatch::pick(isa, part, &src[at * ch..], ch, c);
            }
            rest = tail;
            p += take as i64;
        }
    }
}

/// Byte `c` of each group of `ch` bytes of `src`, into `dst`: one channel of interleaved
/// input. The channel count is a constant in each arm, so that the loop vectorises.
#[inline(always)]
pub(crate) fn pick(dst: &mut [u8], src: &[u8], ch: usize, c: usize) {
    #[inline(always)]
    fn pick_n<const CH: usize>(dst: &mut [u8], src: &[u8], c: usize) {
        for (d, s) in dst.iter_mut().zip(src.chunks_exact(CH)) {
            *d = s[c];
        }
    }
    match ch {
        1 => pick_n::<1>(dst, src, c),
        2 => pick_n::<2>(dst, src, c),
        3 => pick_n::<3>(dst, src, c),
        4 => pick_n::<4>(dst, src, c),
        5 => pick_n::<5>(dst, src, c),
        6 => pick_n::<6>(dst, src, c),
        _ => {
            for (d, s) in dst.iter_mut().zip(src.chunks_exact(ch)) {
                *d = s[c];
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
    /// The chunk's 24-bit samples, channel by channel.
    q: Vec<i32>,
}

/// The stages of one rate and cutoff.
#[derive(Debug)]
pub(crate) struct Plan {
    lut: &'static Lut,
    ops: Vec<Op>,
    spf: usize,
    context: usize,
    /// The kernels' instruction set, detected once.
    isa: Isa,
}

impl Plan {
    pub(crate) fn new(rate: PcmRate, cutoff: Cutoff) -> Plan {
        let hb = Sym::new(&tables::HB, Q_FIR);
        let f40 = Sym::new(&tables::F40, Q_FIR);
        let (lut, ops) = match (cutoff, rate) {
            (Cutoff::Wide, PcmRate::Hz352800) => {
                (LUT_S1_WIDE.get_or_init(|| lut(&tables::S1_WIDE)), vec![Op::Fir1(f40)])
            }
            // 176.4 kHz: its own filter, with the stopband from 88.2 kHz.
            (Cutoff::Wide, _) => (
                LUT_S1_WIDE.get_or_init(|| lut(&tables::S1_WIDE)),
                vec![Op::Dec2(Sym::new(&tables::F80, Q_FIR))],
            ),
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
        let mut plan = Plan { lut, ops, spf: rate.samples_per_frame(), context: 0, isa: Isa::detect() };
        let (bytes_lo, bytes_len) = plan.byte_range((0, plan.spf), false);
        let fb = FRAME_BYTES_PER_CHANNEL as i64;
        plan.context = (-bytes_lo).max(bytes_lo + bytes_len as i64 - fb).max(0) as usize;
        plan
    }

    /// The instruction set the kernels run on.
    #[cfg(test)]
    pub(crate) fn isa(&self) -> Isa {
        self.isa
    }

    /// Runs the kernels on `isa` instead of the detected one.
    #[cfg(test)]
    pub(crate) fn set_isa(&mut self, isa: Isa) {
        self.isa = isa;
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
        out: &mut PcmOut<'_>,
        stats: &mut ConvertStats,
        mut frame_clipped: Option<&mut [u64]>,
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
                // The channel's bytes: those that exist, and the idle pattern beyond the edges
                // of the area.
                sc.bytes.clear();
                sc.bytes.resize(plen, IDLE);
                let lo = valid.start.max(p0);
                let hi = valid.end.min(p0 + plen as i64);
                if lo < hi {
                    input.copy(self.isa, c, lo, &mut sc.bytes[(lo - p0) as usize..(hi - p0) as usize]);
                }
                // The buffers that `s1` and the kernels write are resized, not cleared: those
                // write every element (whole blocks of `LANES`), so zeroing them first is
                // wasted (issue #216).
                // S1 into `a`, unless the first stage is a Dec2: that takes its even and odd
                // inputs straight from S1.
                if !matches!(self.ops.first(), Some(Op::Dec2(_))) {
                    let n1 = levels[0].1.div_ceil(LANES) * LANES;
                    sc.a.resize(n1, 0.0);
                    s1(self.lut, &sc.bytes, 0, 1, &mut sc.a);
                }
                // The other stages, `a` to `b` and swap.
                for (k, op) in self.ops.iter().enumerate() {
                    let (start_out, n_out) = levels[k + 1];
                    let start_in = levels[k].0;
                    match op {
                        Op::Fir1(s) => {
                            sc.b.resize(n_out.div_ceil(LANES) * LANES, 0.0);
                            debug_assert_eq!(start_out - start_in, s.half_len() as i64);
                            dispatch::fir1(self.isa, s, &sc.a, &mut sc.b);
                        }
                        Op::Dec2(s) => {
                            let m = s.half_len();
                            let (e, o) = (s.even.len() - 1, s.odd.len());
                            let np = n_out.div_ceil(LANES) * LANES;
                            debug_assert_eq!(start_in, 2 * start_out - m as i64);
                            // xe[i] = x[2(a - e + i)], xo[i] = x[2(a - o + i) + 1].
                            let (de, dodd) = (m - 2 * e, m + 1 - 2 * o);
                            if k == 0 {
                                sc.even.resize(np + 2 * e, 0.0);
                                sc.odd.resize(np + 2 * o, 0.0);
                                s1(self.lut, &sc.bytes, de, 2, &mut sc.even);
                                s1(self.lut, &sc.bytes, dodd, 2, &mut sc.odd);
                            } else {
                                sc.even.clear();
                                sc.odd.clear();
                                sc.even.extend(sc.a.iter().skip(de).step_by(2).take(np + 2 * e));
                                sc.odd.extend(sc.a.iter().skip(dodd).step_by(2).take(np + 2 * o));
                            }
                            sc.b.resize(np, 0.0);
                            dispatch::dec2(self.isa, s, &sc.even, &sc.odd, &mut sc.b);
                        }
                        Op::Interp2(s) => {
                            let o = s.odd.len();
                            let half = (n_out / 2).div_ceil(LANES) * LANES;
                            debug_assert_eq!(start_in, start_out / 2 - o as i64);
                            sc.even.resize(half, 0.0);
                            dispatch::interp2_odd(self.isa, s, &sc.a, &mut sc.even);
                            sc.b.clear();
                            for i in 0..n_out / 2 {
                                sc.b.push(sc.a[o + i]);
                                sc.b.push(sc.even[i]);
                            }
                        }
                    }
                    std::mem::swap(&mut sc.a, &mut sc.b);
                }
                // To 24 bits, frame by frame: `a` holds the outputs on the 2^26 grid.
                sc.q.resize(ch * n, 0);
                let q = &mut sc.q[c * n..(c + 1) * n];
                for (j, (v, q)) in
                    sc.a[..n].chunks_exact(self.spf).zip(q.chunks_exact_mut(self.spf)).enumerate()
                {
                    let clipped = dispatch::quantise(self.isa, v, gain, q);
                    stats.clipped[c] += clipped;
                    if let Some(fc) = frame_clipped.as_deref_mut() {
                        fc[f0 + j] += clipped;
                    }
                }
            }
            // The channels, interleaved, into the output.
            dispatch::interleave(self.isa, &sc.q, ch, out, o0 as usize * ch);
            f0 += nf;
        }
    }
}

/// Rounds the outputs `v` (on the 2^26 grid) times `gain` to 24 bits, clamped, into `q`;
/// returns the number clamped. Without branches, so that it vectorises.
#[inline(always)]
pub(crate) fn quantise(v: &[f64], gain: f64, q: &mut [i32]) -> u64 {
    const MAX: f64 = 8_388_607.0;
    const MIN: f64 = -8_388_608.0;
    let mut clipped = 0u64;
    for (d, &v) in q.iter_mut().zip(v) {
        let y = round_even((v * gain) * 0.125);
        let c = y.clamp(MIN, MAX);
        // At most `v.len()` clips: no overflow, and no overflow check to keep the loop from
        // vectorising.
        clipped = clipped.wrapping_add(u64::from(c != y));
        // An integer in the 24-bit range: `v` and `gain` are finite (`Converter::new`), so `y`
        // is never NaN.
        *d = to_i32(c);
    }
    clipped
}

/// Writes the samples `q` of `ch` channels, channel by channel (`q[c n + i]` is sample `i`
/// of channel `c`), interleaved into `out` from sample `at` on.
#[inline(always)]
pub(crate) fn interleave(q: &[i32], ch: usize, out: &mut PcmOut<'_>, at: usize) {
    let n = q.len() / ch;
    match out {
        PcmOut::I32(o) => by_channels(q, ch, &mut o[at..at + ch * n], 1, |d, y| d[0] = y),
        PcmOut::S24le(o) => by_channels(q, ch, &mut o[3 * at..3 * (at + ch * n)], 3, |d, y| {
            let [b0, b1, b2, _] = y.to_le_bytes();
            d.copy_from_slice(&[b0, b1, b2]);
        }),
    }
}

/// [`interleave`] into `w` elements of `out` per sample, through `put`. The channel count
/// is a constant in each arm, so that the loop is unrolled.
#[inline(always)]
fn by_channels<T>(q: &[i32], ch: usize, out: &mut [T], w: usize, put: impl Fn(&mut [T], i32)) {
    #[inline(always)]
    fn by<T, const CH: usize>(q: &[i32], out: &mut [T], w: usize, put: impl Fn(&mut [T], i32)) {
        let n = q.len() / CH;
        let q: [&[i32]; CH] = std::array::from_fn(|c| &q[c * n..(c + 1) * n]);
        for (i, d) in out.chunks_exact_mut(CH * w).enumerate() {
            for (c, d) in d.chunks_exact_mut(w).enumerate() {
                put(d, q[c][i]);
            }
        }
    }
    match ch {
        1 => by::<T, 1>(q, out, w, put),
        2 => by::<T, 2>(q, out, w, put),
        3 => by::<T, 3>(q, out, w, put),
        4 => by::<T, 4>(q, out, w, put),
        5 => by::<T, 5>(q, out, w, put),
        // 6: `Converter::new` allows no more.
        _ => by::<T, 6>(q, out, w, put),
    }
}

/// S1: `out[k]` = the sum of `lut[j][bytes[start + stride k + j]]`.
fn s1(lut: &[[i32; 256]], bytes: &[u8], start: usize, stride: usize, out: &mut [f64]) {
    // S1's 8 tables, and `Wide`'s 12, as arrays: the sum over a window is unrolled, with no
    // bounds checks (issue #216).
    if let Ok(lut) = <&[[i32; 256]; 8]>::try_from(lut) {
        s1_n(lut, bytes, start, stride, out);
    } else if let Ok(lut) = <&[[i32; 256]; 12]>::try_from(lut) {
        s1_n(lut, bytes, start, stride, out);
    } else {
        s1_any(lut, bytes, start, stride, out);
    }
}

/// [`s1`] with `B` tables. Each window is read as 64-bit words and taken apart, not loaded a
/// byte at a time, which leaves the CPU's load ports to the table lookups (issue #224).
#[inline(always)]
fn s1_n<const B: usize>(lut: &[[i32; 256]; B], bytes: &[u8], start: usize, stride: usize, out: &mut [f64]) {
    let Some(last) = out.len().checked_sub(1) else { return };
    let windows = bytes[start..start + stride * last + B].windows(B).step_by(stride);
    for (o, w) in out.iter_mut().zip(windows) {
        // As in `s1_any`: no partial sum wraps.
        let mut sum = 0i32;
        for (tables, part) in lut.chunks(8).zip(w.chunks(8)) {
            let mut word = [0u8; 8];
            word[..part.len()].copy_from_slice(part);
            let mut word = u64::from_le_bytes(word);
            for table in tables {
                sum = sum.wrapping_add(table[(word & 0xff) as usize]);
                word >>= 8;
            }
        }
        *o = f64::from(sum);
    }
}

/// [`s1`] with any number of tables.
fn s1_any(lut: &[[i32; 256]], bytes: &[u8], start: usize, stride: usize, out: &mut [f64]) {
    let b = lut.len();
    for (k, o) in out.iter_mut().enumerate() {
        let i = start + stride * k;
        // No partial sum wraps: the sum of the absolute taps is below `i32::MAX`
        // (`design::tests::f64_sums_are_exact`). `wrapping_add` keeps overflow checks out of
        // the loop, so that test, not an overflow check, is the safety net for new tables.
        let sum =
            lut.iter().zip(&bytes[i..i + b]).fold(0i32, |acc, (t, &x)| acc.wrapping_add(t[usize::from(x)]));
        *o = f64::from(sum);
    }
}

// The kernels are generic over their block of independent outputs `L` (a divisor of
// `LANES`), which the compiler puts in vector lanes, and over fused multiply-adds (`FMA`).
// `dispatch` picks both for the CPU. Fused or not, every sum is the same: each product and
// partial sum is an exact integer below 2^53, so both roundings are exact.

/// Adds `h[j] (x[hi + j + l] + x[lo - j + l])` over `j`, for the `L` outputs `l`.
#[inline(always)]
fn add_pairs<const L: usize, const FMA: bool>(
    acc: &mut [f64; L],
    h: &[f64],
    x: &[f64],
    hi: usize,
    lo: usize,
) {
    let Some(last) = h.len().checked_sub(1) else { return };
    // The windows `x[hi + j..]` upwards and `x[lo - j..]` downwards, sliced once so that the
    // loop has no bounds checks.
    let up = x[hi..hi + last + L].windows(L);
    let down = x[lo - last..lo + L].windows(L).rev();
    for ((a, b), &hj) in up.zip(down).zip(h) {
        for l in 0..L {
            if FMA {
                acc[l] = hj.mul_add(a[l] + b[l], acc[l]);
            } else {
                acc[l] += hj * (a[l] + b[l]);
            }
        }
    }
}

/// Runs `f` on each block of `L` outputs and stores the rounded, rescaled sums.
#[inline(always)]
fn blocks<const L: usize>(out: &mut [f64], scale: f64, f: impl Fn(usize, &mut [f64; L])) {
    for (bi, o) in out.chunks_exact_mut(L).enumerate() {
        let mut acc = [0.0f64; L];
        f(bi * L, &mut acc);
        for (d, a) in o.iter_mut().zip(acc) {
            *d = round_even(a * scale);
        }
    }
}

/// Fir1: `out[i]` centred on `x[i + m]`.
#[inline(always)]
pub(crate) fn fir1<const L: usize, const FMA: bool>(s: &Sym, x: &[f64], out: &mut [f64]) {
    let m = s.half_len();
    let (h0, h) = (s.full[0], &s.full[1..]);
    blocks::<L>(out, s.scale, |i, acc| {
        for (l, a) in acc.iter_mut().enumerate() {
            *a = h0 * x[i + m + l];
        }
        add_pairs::<L, FMA>(acc, h, x, i + m + 1, i + m - 1);
    });
}

/// Dec2: `out[i]` centred on `xe[i + e]` (`e` = the even taps after the centre), with the
/// odd taps on `xo[i + o + j]` and `xo[i + o - 1 - j]`.
#[inline(always)]
pub(crate) fn dec2<const L: usize, const FMA: bool>(s: &Sym, xe: &[f64], xo: &[f64], out: &mut [f64]) {
    let (e, o) = (s.even.len() - 1, s.odd.len());
    let (h0, he) = (s.even[0], &s.even[1..]);
    blocks::<L>(out, s.scale, |i, acc| {
        for (l, a) in acc.iter_mut().enumerate() {
            *a = h0 * xe[i + e + l];
        }
        if e > 0 {
            add_pairs::<L, FMA>(acc, he, xe, i + e + 1, i + e - 1);
        }
        add_pairs::<L, FMA>(acc, &s.odd, xo, i + o, i + o - 1);
    });
}

/// The odd outputs of Interp2: `out[i]` between `w[i + o]` and `w[i + o + 1]`. The
/// interpolation gain of 2 is in the scale (2^-(q - 1)).
#[inline(always)]
pub(crate) fn interp2_odd<const L: usize, const FMA: bool>(s: &Sym, w: &[f64], out: &mut [f64]) {
    let o = s.odd.len();
    blocks::<L>(out, s.scale * 2.0, |i, acc| add_pairs::<L, FMA>(acc, &s.odd, w, i + o + 1, i + o));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `pcm-dispatch-avx2` really runs the wrappers. This test exists only in a Miri build
    /// with the features, and the slice requires it by name: if the slice loses its flags,
    /// the test is gone and the slice fails; if [`Isa::detect`] comes to need a feature the
    /// flags don't enable, this fails. (It is here because `dispatch.rs` may name
    /// `target_feature` only on its wrappers: `tests/common/unsafe_scan.rs`.)
    #[cfg(all(miri, target_feature = "avx2", target_feature = "fma"))]
    #[test]
    fn miri_slice_selects_the_wrappers() {
        assert_ne!(Isa::detect(), Isa::generic());
    }

    #[test]
    fn pow2_is_exact() {
        assert_eq!(pow2(0), 1.0);
        assert_eq!(pow2(-24), 1.0 / 16_777_216.0);
        assert_eq!(pow2(3), 8.0);
    }

    #[test]
    fn round_even_matches_std() {
        for x in [0.0, 0.5, 1.5, 2.5, -0.5, -1.5, -2.5, 3.49, -3.51, 1e9 + 0.5, -(2f64.powi(40)) - 0.5] {
            assert_eq!(round_even(x), x.round_ties_even(), "{x}");
        }
    }

    #[test]
    fn to_i32_matches_the_cast() {
        let edges = [0.0, -0.0, 1.0, -1.0, 8_388_607.0, -8_388_608.0, 2_147_483_647.0, -2_147_483_648.0];
        for x in edges {
            assert_eq!(to_i32(x), x as i32, "{x}");
        }
        for i in (-8_388_608..=8_388_607).step_by(if cfg!(miri) { 1 << 20 } else { 97 }) {
            assert_eq!(to_i32(f64::from(i)), i);
        }
    }

    #[test]
    fn quantise_rounds_clamps_and_counts() {
        let v = [
            0.0,
            4.0,
            12.0,
            -4.0,
            -12.0,
            67_108_856.0,
            67_108_864.0,
            -67_108_864.0,
            -67_108_872.0,
            5e8,
            -5e8,
        ];
        let mut q = [0i32; 11];
        assert_eq!(quantise(&v, 1.0, &mut q), 4);
        assert_eq!(q, [0, 0, 2, 0, -2, 8_388_607, 8_388_607, -8_388_608, -8_388_608, 8_388_607, -8_388_608]);
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
