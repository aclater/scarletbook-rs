// SPDX-License-Identifier: MIT OR Apache-2.0

//! PROTOTYPE for #372 item 3, not for merge: exact integer Fir1 and Dec2 kernels on SSE2
//! integer instructions (`pmaddwd`), timed against the `f64` kernels, with every output
//! sample checked.
//!
//! ```text
//! RUSTFLAGS="-C target-cpu=pentium4" cargo build --release -p scarletbook-pcm \
//!     --features proto-int-372 --example int_kernels --target i686-unknown-linux-musl
//! target/i686-unknown-linux-musl/release/examples/int_kernels [SECONDS] [ROUNDS]
//! ```
//!
//! (defaults 10 s of audio, 7 rounds). It converts generated DSD (integer arithmetic only:
//! xorshift noise, a second-order sigma-delta of an integer triangle wave, full-scale
//! constants, the bit pattern that drives S1 to its bound and its inverse, and silence; six
//! channels with different phases) at 176.4 kHz and 88.2 kHz, Standard cutoff, four ways:
//!
//! - `lib`: `Converter::convert_run`, the shipped `f64` path (on x86-64 with AVX2 and FMA,
//!   their kernels);
//! - `f64`: the same `f64` kernels as `kernel.rs` (32-bit x86: Fir1 in blocks of 8, Dec2 of
//!   16, no FMA), copied here, under this file's driver;
//! - `int1`, `int2`: the integer kernels (`src/proto_int.rs`, feature `proto-int-372`), in
//!   blocks of 4 and 8 outputs, under the same driver and the same S1.
//!
//! Before timing it checks, and exits non-zero unless all hold: every stage output (S1,
//! the halfband Dec2, the F40 Fir1 or Dec2) of the integer kernels equals the `f64` kernels'
//! for every chunk of every channel; every 24-bit output sample of all four paths is
//! identical; and the stage inputs stay within the bounds used below. Timing: rounds that
//! alternate the order of the paths (ABBA), each on the whole input; the median ms per s
//! of audio, and the median of the per-round ratios against `lib`.
//!
//! # Integer arithmetic
//!
//! Samples are integers on the 2^26 grid and taps integers scaled by 2^24, as in
//! `kernel.rs`. Each sample `x` is stored as two 16-bit limbs, `x = hi 2^14 + lo` with
//! `lo` in `[-2^13, 2^13)` (balanced) and `hi = (x - lo) / 2^14`, packed `[lo, hi]` in one
//! 32-bit lane; each tap `h` likewise as `[hl, hh]`. A tap pair `x[c + j] + x[c - j]` is
//! folded limb-wise with 16-bit adds (`paddw`; no carry crosses the limbs). One `pmaddwd`
//! of the folded `[lo, hi]` against `[hl, 0]`, `[hh, hl]` and `[0, hh]` gives, per lane,
//! the three partial products at scales 1, 2^14 and 2^28:
//!
//! ```text
//! LL = lo hl,   MID = lo hh + hi hl,   HH = hi hh,   sum = HH 2^28 + MID 2^14 + LL
//! ```
//!
//! Each is summed in its own 32-bit lane. After every `K` = 15 taps (and at the end) the
//! sums are normalised: the carry `LL >> 14` moves into MID and `MID >> 14` into HH, which
//! leaves LL and MID in `[0, 2^14)`. At the end, `low = MID 2^14 + LL` is in `[0, 2^28)`
//! and the output, the sum times 2^-24 rounded half to even, is `16 HH + (low >> 24)` plus
//! the rounding of `low mod 2^24`: all in 32 bits, no 64-bit arithmetic.
//!
//! Overflow bound, checked at startup over the real tables (`check_bounds`; it panics if
//! any step fails), with the input bounds of `design.rs`' `f64_sums_are_exact`: S1 outputs
//! `|x| <= sum|S1| = 84_943_204` (< 2^26.34) into the halfband Dec2, its outputs
//! `|x| <= 121_380_530` (< 2^26.86) into F40. For an input bound `X`:
//!
//! - limbs: `|lo| <= 2^13`, `|hi| <= (X + 2^13) >> 14` (5185 and 7408); folded pairs
//!   `|lo_f| <= 2^14`, `|hi_f| <= 2 |hi|` (10370 and 14816), all within `i16`;
//! - taps: `|hl| <= 2^13` (at most 7980 in these tables), `|hh| <= 512` (the halfband's
//!   centre tap is exactly 2^23);
//! - per tap and 32-bit lane (one `pmaddwd` lane): `|LL| <= 2^14 |hl|`,
//!   `|MID| <= 2^14 |hh| + |hi_f| |hl|`, `|HH| <= |hi_f| |hh|`: at most 1.31e8, 1.19e8 and
//!   7.6e6 over these tables (all below 2^27), so a `pmaddwd` lane never overflows;
//! - widening (normalisation) interval `K` = 15 taps: starting from normalised sums (LL and
//!   MID in `[0, 2^14)`), adding up to 15 taps' worst cases and then the carries, the
//!   largest partial sums over the real taps (printed at startup) are, for the halfband
//!   Dec2, LL 5.34e8, MID 3.50e8, HH 7.6e6; for F40 as Fir1, 1.053e9, 9.54e8, 1.56e7; for
//!   F40 as Dec2, 1.071e9, 9.80e8, 1.56e7. All below 2^31 = 2.147e9, with a margin of 2x;
//! - the final `16 HH + 1` is at most 2.5e8, below 2^31.
//!
//! What this measures: the kernels' arithmetic, not a finished design. The driver (chunks
//! of 4 frames, padding to blocks of 32) follows `kernel.rs`, and the gain is unity only.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Instant;

use scarletbook_pcm::proto_int::{IntSym, K, dec2_int, fir1_int, limbs, pack};
use scarletbook_pcm::{Converter, Cutoff, FRAME_BYTES_PER_CHANNEL as FB, FrameRun, PcmConfig, PcmRate};

/// The crate's generated filter tables, as they ship.
#[allow(dead_code)]
mod tables {
    include!("../src/tables.rs");
}

const LANES: usize = 32;
const CHUNK_FRAMES: usize = 4;
const Q_FIR: i32 = 24;
const IDLE: u8 = 0x69;
const CH: usize = 6;

// ---------------------------------------------------------------------------------------
// S1 (shared by every path): the lookup tables and windows of `kernel.rs`.

type Lut = [[i32; 256]; 8];

fn lut(half: &[i32]) -> Lut {
    let m = half.len() - 1;
    assert_eq!(m % 8, 7);
    let bytes = (m + 1) / 4;
    assert_eq!(bytes, 8);
    let tap = |t: i64| -> i32 { half.get(t.unsigned_abs() as usize).copied().unwrap_or(0) };
    core::array::from_fn(|j| {
        let rel = j as i64 - (bytes / 2) as i64;
        core::array::from_fn(|b| {
            (0..8)
                .map(|p| {
                    let h = tap(8 * rel + p);
                    if (b >> (7 - p)) & 1 == 1 { h } else { -h }
                })
                .sum()
        })
    })
}

#[cfg(target_pointer_width = "64")]
type Word = u64;
#[cfg(not(target_pointer_width = "64"))]
type Word = u32;
const WB: usize = core::mem::size_of::<Word>();

/// S1 output `k` = sum of `lut[j][bytes[start + stride k + j]]`, handed to `put(k, v)`.
#[inline(always)]
fn s1(lut: &Lut, bytes: &[u8], start: usize, stride: usize, n: usize, mut put: impl FnMut(usize, i32)) {
    let Some(last) = n.checked_sub(1) else { return };
    let windows = bytes[start..start + stride * last + 8].windows(8).step_by(stride);
    for (k, w) in windows.enumerate() {
        let mut sum = 0i32;
        for (tables, part) in lut.chunks(WB).zip(w.chunks(WB)) {
            let mut word = Word::from_le_bytes(part.try_into().unwrap());
            for table in tables {
                sum = sum.wrapping_add(table[(word & 0xff) as usize]);
                word >>= 8;
            }
        }
        put(k, sum);
    }
}

// ---------------------------------------------------------------------------------------
// The f64 kernels, as in `kernel.rs` (FMA off).

const MAGIC: f64 = 6_755_399_441_055_744.0;

#[inline(always)]
fn round_even(x: f64) -> f64 {
    (x + MAGIC) - MAGIC
}

#[derive(Clone, Debug)]
struct Sym {
    full: Vec<f64>,
    even: Vec<f64>,
    odd: Vec<f64>,
    scale: f64,
}

impl Sym {
    fn new(half: &[i32]) -> Sym {
        let full: Vec<f64> = half.iter().map(|&h| f64::from(h)).collect();
        let mut even: Vec<f64> = full.iter().step_by(2).copied().collect();
        while even.len() > 1 && even.last() == Some(&0.0) {
            even.pop();
        }
        let odd = full.iter().skip(1).step_by(2).copied().collect();
        Sym { full, even, odd, scale: f64::from_bits(((1023 - Q_FIR) as u64) << 52) }
    }
}

#[inline(always)]
fn add_pairs<const L: usize>(acc: &mut [f64; L], h: &[f64], x: &[f64], hi: usize, lo: usize) {
    let Some(last) = h.len().checked_sub(1) else { return };
    let up = x[hi..hi + last + L].windows(L);
    let down = x[lo - last..lo + L].windows(L).rev();
    for ((a, b), &hj) in up.zip(down).zip(h) {
        for l in 0..L {
            acc[l] += hj * (a[l] + b[l]);
        }
    }
}

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

#[inline(never)]
fn fir1_f64<const L: usize>(s: &Sym, x: &[f64], out: &mut [f64]) {
    let m = s.full.len() - 1;
    let (h0, h) = (s.full[0], &s.full[1..]);
    blocks::<L>(out, s.scale, |i, acc| {
        for (l, a) in acc.iter_mut().enumerate() {
            *a = h0 * x[i + m + l];
        }
        add_pairs::<L>(acc, h, x, i + m + 1, i + m - 1);
    });
}

#[inline(never)]
fn dec2_f64<const L: usize>(s: &Sym, xe: &[f64], xo: &[f64], out: &mut [f64]) {
    let (e, o) = (s.even.len() - 1, s.odd.len());
    let (h0, he) = (s.even[0], &s.even[1..]);
    blocks::<L>(out, s.scale, |i, acc| {
        for (l, a) in acc.iter_mut().enumerate() {
            *a = h0 * xe[i + e + l];
        }
        if e > 0 {
            add_pairs::<L>(acc, he, xe, i + e + 1, i + e - 1);
        }
        add_pairs::<L>(acc, &s.odd, xo, i + o, i + o - 1);
    });
}

#[cfg(target_arch = "x86")]
const FIR1_L: usize = 8;
#[cfg(not(target_arch = "x86"))]
const FIR1_L: usize = 16;
const DEC2_L: usize = 16;

// ---------------------------------------------------------------------------------------
// The overflow bound, over the real tables.

/// Simulates the integer kernel's sums for taps `centre` then the folded lists, from an
/// input bound `x`, in i64; panics if any 32-bit lane or 16-bit limb could overflow.
/// Returns the largest |LL|, |MID|, |HH| seen.
fn check_bounds(name: &str, x: i64, centre: i32, lists: &[&[i32]]) -> [i64; 3] {
    const I32: i64 = i32::MAX as i64;
    const I16: i64 = i16::MAX as i64;
    let hi = (x + (1 << 13)) >> 14;
    let (lo_c, hi_c) = (1i64 << 13, hi);
    let (lo_f, hi_f) = (1i64 << 14, 2 * hi);
    assert!(hi_f <= I16 && lo_f - 2 <= I16, "{name}: folded limbs exceed i16");
    let limb = |h: i32| {
        let (l, hh) = limbs(h);
        assert!(i64::from(hh).abs() <= I16);
        (i64::from(l).abs(), i64::from(hh).abs())
    };
    let term = |lo: i64, hi: i64, h: i32| {
        let (hl, hh) = limb(h);
        [lo * hl, lo * hh + hi * hl, hi * hh]
    };
    let mut max = [0i64; 3];
    let mut s = term(lo_c, hi_c, centre);
    let seen = |s: &[i64; 3], max: &mut [i64; 3]| {
        for t in 0..3 {
            assert!(s[t] <= I32, "{name}: sum {t} = {} can overflow i32", s[t]);
            max[t] = max[t].max(s[t]);
        }
    };
    seen(&s, &mut max);
    for list in lists {
        for chunk in list.chunks(K) {
            for &h in chunk {
                let p = term(lo_f, hi_f, h);
                for t in 0..3 {
                    s[t] += p[t];
                }
                seen(&s, &mut max);
            }
            // Normalise: carries are at most ceil(|v| / 2^14).
            s[1] += (s[0] + (1 << 14) - 1) >> 14;
            seen(&s, &mut max);
            s[2] += (s[1] + (1 << 14) - 1) >> 14;
            seen(&s, &mut max);
            s[0] = (1 << 14) - 1;
            s[1] = (1 << 14) - 1;
        }
    }
    // The final 16 HH (+1 for the rounding): within i32.
    assert!(16 * s[2] + 16 <= I32, "{name}: final 16 HH can overflow");
    max
}

fn sum_abs(half: &[i32]) -> i64 {
    i64::from(half[0].abs()) + 2 * half[1..].iter().map(|h| i64::from(h.abs())).sum::<i64>()
}

/// Checks every stage of the two plans; returns the input bounds (S1 output, HB output).
fn check_all() -> (i64, i64) {
    let x1 = sum_abs(&tables::S1);
    assert!(x1 < i64::from(i32::MAX));
    let x2 = ((sum_abs(&tables::HB) * x1) >> Q_FIR) + 1;
    let report = |name: &str, x: i64, m: [i64; 3]| {
        println!(
            "bound {name:<10} input |x| <= {x} (2^{:.2}): max |LL| {} |MID| {} |HH| {} (2^31 = {})",
            (x as f64).log2(),
            m[0],
            m[1],
            m[2],
            1u64 << 31
        );
    };
    // Dec2: centre = even[0], then even taps 1.., then odd taps.
    let dec2_lists = |half: &[i32]| -> (i32, Vec<i32>, Vec<i32>) {
        let mut even: Vec<i32> = half.iter().step_by(2).copied().collect();
        while even.len() > 1 && even.last() == Some(&0) {
            even.pop();
        }
        let odd = half.iter().skip(1).step_by(2).copied().collect();
        (even[0], even[1..].to_vec(), odd)
    };
    let (c, e, o) = dec2_lists(&tables::HB);
    report("Dec2 HB", x1, check_bounds("Dec2 HB", x1, c, &[&e, &o]));
    report("Fir1 F40", x2, check_bounds("Fir1 F40", x2, tables::F40[0], &[&tables::F40[1..]]));
    let (c, e, o) = dec2_lists(&tables::F40);
    report("Dec2 F40", x2, check_bounds("Dec2 F40", x2, c, &[&e, &o]));
    (x1, x2)
}

// ---------------------------------------------------------------------------------------
// The driver: one rate's stages, chunked as in `kernel.rs`.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Second {
    Fir1,
    Dec2,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Path {
    F64,
    Int1,
    Int2,
}

struct Plan {
    lut: Lut,
    hb: Sym,
    f40: Sym,
    hbi: IntSym,
    f40i: IntSym,
    second: Second,
    spf: usize,
}

#[derive(Default)]
struct Scratch {
    bytes: Vec<u8>,
    a: Vec<f64>,
    b: Vec<f64>,
    even: Vec<f64>,
    odd: Vec<f64>,
    pe: Vec<i16>,
    po: Vec<i16>,
    r1: Vec<i32>,
    r2: Vec<i32>,
    q: Vec<i32>,
}

/// Stage outputs, for the check: S1, stage 1, stage 2, per chunk and channel.
#[derive(Default, PartialEq, Debug)]
struct Record(Vec<Vec<i64>>);

fn padn(n: usize) -> usize {
    n.div_ceil(LANES) * LANES
}

impl Plan {
    fn new(rate: PcmRate) -> Plan {
        Plan {
            lut: lut(&tables::S1),
            hb: Sym::new(&tables::HB),
            f40: Sym::new(&tables::F40),
            hbi: IntSym::new(&tables::HB),
            f40i: IntSym::new(&tables::F40),
            second: if rate == PcmRate::Hz176400 { Second::Fir1 } else { Second::Dec2 },
            spf: rate.samples_per_frame(),
        }
    }

    /// Levels: [S1 out, stage-1 out, stage-2 out] as (start, len), padded.
    fn levels(&self, out: (i64, usize)) -> [(i64, usize); 3] {
        let m2 = (self.f40.full.len() - 1) as i64;
        let l1 = match self.second {
            Second::Fir1 => (out.0 - m2, padn(out.1) + 2 * m2 as usize),
            Second::Dec2 => (2 * out.0 - m2, 2 * padn(out.1) + 2 * m2 as usize - 1),
        };
        let m1 = (self.hb.full.len() - 1) as i64;
        let l0 = (2 * l1.0 - m1, 2 * padn(l1.1) + 2 * m1 as usize - 1);
        [l0, l1, out]
    }

    fn convert(
        &self,
        path: Path,
        input: &[u8],
        frames: usize,
        out: &mut [i32],
        mut rec: Option<&mut Record>,
    ) {
        let total = (frames * FB) as i64;
        let mut sc = Scratch::default();
        let mut f0 = 0;
        while f0 < frames {
            let nf = CHUNK_FRAMES.min(frames - f0);
            let o0 = (f0 * self.spf) as i64;
            let n = nf * self.spf;
            let lv = self.levels((o0, n));
            let (p0, plen) = (lv[0].0 - 4, padn(lv[0].1) + 7);
            sc.q.resize(n, 0);
            for c in 0..CH {
                sc.bytes.clear();
                sc.bytes.resize(plen, IDLE);
                let lo = p0.max(0);
                let hi = total.min(p0 + plen as i64);
                for p in lo..hi {
                    sc.bytes[(p - p0) as usize] = input[p as usize * CH + c];
                }
                match path {
                    Path::F64 => self.chunk_f64(&mut sc, lv, n, rec.as_deref_mut()),
                    Path::Int1 => self.chunk_int::<1>(&mut sc, lv, n, rec.as_deref_mut()),
                    Path::Int2 => self.chunk_int::<2>(&mut sc, lv, n, rec.as_deref_mut()),
                }
                // Interleave.
                let q = &sc.q[..n];
                for (i, &v) in q.iter().enumerate() {
                    out[(o0 as usize + i) * CH + c] = v;
                }
            }
            f0 += nf;
        }
    }

    /// Stage 1 (the halfband Dec2, from S1) and stage 2, f64; the 24-bit samples into `sc.q`.
    fn chunk_f64(&self, sc: &mut Scratch, lv: [(i64, usize); 3], n: usize, rec: Option<&mut Record>) {
        let m = self.hb.full.len() - 1;
        let (e, o) = (self.hb.even.len() - 1, self.hb.odd.len());
        let np = padn(lv[1].1);
        let (de, dodd) = (m - 2 * e, m + 1 - 2 * o);
        sc.even.resize(np + 2 * e, 0.0);
        sc.odd.resize(np + 2 * o, 0.0);
        let (ev, od) = (&mut sc.even, &mut sc.odd);
        s1(&self.lut, &sc.bytes, de, 2, ev.len(), |k, v| ev[k] = f64::from(v));
        s1(&self.lut, &sc.bytes, dodd, 2, od.len(), |k, v| od[k] = f64::from(v));
        sc.a.resize(np, 0.0);
        dec2_f64::<DEC2_L>(&self.hb, &sc.even, &sc.odd, &mut sc.a);
        let mut s1rec = Vec::new();
        if rec.is_some() {
            s1rec = sc.even.iter().chain(&sc.odd).map(|&v| v as i64).collect();
        }
        let n2 = padn(lv[2].1);
        sc.b.resize(n2, 0.0);
        match self.second {
            Second::Fir1 => fir1_f64::<FIR1_L>(&self.f40, &sc.a, &mut sc.b),
            Second::Dec2 => {
                let m = self.f40.full.len() - 1;
                let (e, o) = (self.f40.even.len() - 1, self.f40.odd.len());
                let (de, dodd) = (m - 2 * e, m + 1 - 2 * o);
                sc.even.clear();
                sc.odd.clear();
                sc.even.extend(sc.a.iter().skip(de).step_by(2).take(n2 + 2 * e));
                sc.odd.extend(sc.a.iter().skip(dodd).step_by(2).take(n2 + 2 * o));
                dec2_f64::<DEC2_L>(&self.f40, &sc.even, &sc.odd, &mut sc.b);
            }
        }
        if let Some(r) = rec {
            r.0.push(s1rec);
            r.0.push(sc.a.iter().map(|&v| v as i64).collect());
            r.0.push(sc.b.iter().map(|&v| v as i64).collect());
        }
        // To 24 bits (gain 1), as `kernel::quantise`.
        for (d, &v) in sc.q[..n].iter_mut().zip(&sc.b[..n]) {
            *d = round_even(v * 0.125).clamp(-8_388_608.0, 8_388_607.0) as i32;
        }
    }

    fn chunk_int<const G: usize>(
        &self,
        sc: &mut Scratch,
        lv: [(i64, usize); 3],
        n: usize,
        rec: Option<&mut Record>,
    ) {
        let s = &self.hbi;
        let m = s.half_len();
        let (e, o) = (s.even_after_centre(), s.odd_len());
        let np = padn(lv[1].1);
        let (de, dodd) = (m - 2 * e, m + 1 - 2 * o);
        sc.pe.resize(2 * (np + 2 * e), 0);
        sc.po.resize(2 * (np + 2 * o), 0);
        let (pe, po) = (&mut sc.pe, &mut sc.po);
        s1(&self.lut, &sc.bytes, de, 2, pe.len() / 2, |k, v| pack(v, &mut pe[2 * k..2 * k + 2]));
        s1(&self.lut, &sc.bytes, dodd, 2, po.len() / 2, |k, v| pack(v, &mut po[2 * k..2 * k + 2]));
        let mut s1rec = Vec::new();
        if rec.is_some() {
            let un = |p: &[i16]| -> Vec<i64> {
                p.chunks_exact(2).map(|w| i64::from(w[1]) * (1 << 14) + i64::from(w[0])).collect()
            };
            s1rec = un(&sc.pe);
            s1rec.extend(un(&sc.po));
        }
        sc.r1.resize(np, 0);
        dec2_int::<G>(s, &sc.pe, &sc.po, &mut sc.r1);
        let n2 = padn(lv[2].1);
        sc.r2.resize(n2, 0);
        let f = &self.f40i;
        match self.second {
            Second::Fir1 => {
                sc.pe.resize(2 * np, 0);
                for (d, &v) in sc.pe.chunks_exact_mut(2).zip(&sc.r1) {
                    pack(v, d);
                }
                fir1_int::<G>(f, &sc.pe, &mut sc.r2);
            }
            Second::Dec2 => {
                let m = f.half_len();
                let (e, o) = (f.even_after_centre(), f.odd_len());
                let (de, dodd) = (m - 2 * e, m + 1 - 2 * o);
                sc.pe.resize(2 * (n2 + 2 * e), 0);
                sc.po.resize(2 * (n2 + 2 * o), 0);
                for (d, &v) in sc.pe.chunks_exact_mut(2).zip(sc.r1.iter().skip(de).step_by(2)) {
                    pack(v, d);
                }
                for (d, &v) in sc.po.chunks_exact_mut(2).zip(sc.r1.iter().skip(dodd).step_by(2)) {
                    pack(v, d);
                }
                dec2_int::<G>(f, &sc.pe, &sc.po, &mut sc.r2);
            }
        }
        if let Some(r) = rec {
            r.0.push(s1rec);
            r.0.push(sc.r1.iter().map(|&v| i64::from(v)).collect());
            r.0.push(sc.r2.iter().map(|&v| i64::from(v)).collect());
        }
        // To 24 bits (gain 1): round(v / 8) half to even, clamped.
        for (d, &v) in sc.q[..n].iter_mut().zip(&sc.r2[..n]) {
            let (q, rem) = (v >> 3, v & 7);
            let y = q.wrapping_add(i32::from(rem > 4) | (i32::from(rem == 4) & q));
            *d = y.clamp(-8_388_608, 8_388_607);
        }
    }
}

// ---------------------------------------------------------------------------------------
// Input.

fn generate(frames: usize) -> Vec<u8> {
    let bytes = frames * FB;
    // The S1 bit pattern that reaches sum|S1|: each bit the sign of its tap.
    let half = &tables::S1;
    let pat: [u8; 8] = core::array::from_fn(|j| {
        (0..8).fold(0u8, |b, p| {
            let t = 8 * (j as i64 - 4) + p as i64;
            let h = half.get(t.unsigned_abs() as usize).copied().unwrap_or(0);
            b | (u8::from(h >= 0) << (7 - p))
        })
    });
    let mut out = vec![0u8; bytes * CH];
    for c in 0..CH {
        let mut seed = 0x9E37_79B9_7F4A_7C15u64 ^ (c as u64 * 0x1234_5678_9ABC);
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        // Sigma-delta state.
        let (mut v1, mut v2, mut y) = (0i64, 0i64, 0i64);
        let a: i64 = 1 << 20;
        let period = 2000 + 700 * c as i64;
        let mut tri_t = 0i64;
        let shift = c * bytes / CH;
        for k in 0..bytes {
            let pos = (k + shift) % bytes;
            let seg = pos * 20 / bytes;
            let b = match seg {
                0..=6 => rnd() as u8,
                7..=13 => {
                    let mut byte = 0u8;
                    for _ in 0..8 {
                        tri_t = (tri_t + 1) % period;
                        let ph = (4 * a * tri_t) / period; // 0..4a
                        let u = (if ph < 2 * a { ph - a } else { 3 * a - ph }) * 5 / 8;
                        v1 += u - y;
                        v2 += v1 - y;
                        let bit = v2 >= 0;
                        y = if bit { a } else { -a };
                        byte = (byte << 1) | u8::from(bit);
                    }
                    byte
                }
                14 => 0xff,
                15 => 0x00,
                16 => pat[pos % 8],
                17 => !pat[pos % 8],
                _ => IDLE,
            };
            out[k * CH + c] = b;
        }
    }
    out
}

// ---------------------------------------------------------------------------------------

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn main() {
    let mut args = std::env::args().skip(1);
    let seconds: usize = args.next().map(|s| s.parse().unwrap()).unwrap_or(10);
    let rounds: usize = args.next().map(|s| s.parse().unwrap()).unwrap_or(7);
    let frames = 75 * seconds;
    println!(
        "int_kernels (PROTOTYPE, #372): {} pointer width {}, K = {K}, {seconds} s x {CH} ch, {rounds} rounds",
        std::env::consts::ARCH,
        usize::BITS
    );
    let (x1, x2) = check_all();
    let input = generate(frames);
    let mut failed = false;
    for rate in [PcmRate::Hz176400, PcmRate::Hz88200] {
        let plan = Plan::new(rate);
        let len = frames * plan.spf * CH;
        let mut conv = Converter::new(PcmConfig::new(rate, Cutoff::Standard, CH as u8)).unwrap();
        // The check.
        let mut want = vec![0i32; len];
        conv.convert_run(FrameRun::new(&input), &mut want).unwrap();
        let mut rec_f = Record::default();
        let mut got = vec![0i32; len];
        plan.convert(Path::F64, &input, frames, &mut got, Some(&mut rec_f));
        let same_f = got == want;
        let mut ok = same_f;
        let (mut m1, mut m2) = (0i64, 0i64);
        for (i, r) in rec_f.0.iter().enumerate() {
            let m = r.iter().map(|v| v.abs()).max().unwrap_or(0);
            match i % 3 {
                0 => m1 = m1.max(m),
                1 => m2 = m2.max(m),
                _ => {}
            }
        }
        let within = m1 <= x1 && m2 <= x2;
        ok &= within;
        let mut stage_ok = [true; 2];
        for (k, path) in [Path::Int1, Path::Int2].into_iter().enumerate() {
            let mut rec_i = Record::default();
            let mut got_i = vec![0i32; len];
            plan.convert(path, &input, frames, &mut got_i, Some(&mut rec_i));
            stage_ok[k] = rec_i == rec_f && got_i == want;
            if rec_i != rec_f {
                let bad = rec_f.0.iter().zip(&rec_i.0).position(|(a, b)| a != b);
                println!("  {path:?}: stage outputs differ (first at record {bad:?})");
            }
            ok &= stage_ok[k];
        }
        let samples: usize = rec_f.0.iter().map(Vec::len).sum();
        println!(
            "{} Hz check: lib == f64 {same_f}; int1, int2 stage outputs ({samples} values) and all {len} \
             output samples identical: {:?}; max |S1| {m1} <= {x1}, max |stage 1| {m2} <= {x2}: {within}",
            rate.hz(),
            stage_ok
        );
        if !ok {
            failed = true;
            continue;
        }
        // Timing, ABBA.
        let paths = ["lib", "f64", "int1", "int2"];
        let mut times = vec![Vec::new(); paths.len()];
        let mut out = vec![0i32; len];
        let run = |p: usize, out: &mut [i32], conv: &mut Converter| -> f64 {
            let t = Instant::now();
            match p {
                0 => {
                    conv.convert_run(FrameRun::new(&input), out).unwrap();
                }
                1 => plan.convert(Path::F64, &input, frames, out, None),
                2 => plan.convert(Path::Int1, &input, frames, out, None),
                _ => plan.convert(Path::Int2, &input, frames, out, None),
            }
            t.elapsed().as_secs_f64() * 1000.0 / seconds as f64
        };
        // Warm-up.
        for p in 0..paths.len() {
            run(p, &mut out, &mut conv);
        }
        for r in 0..rounds {
            let order: Vec<usize> =
                if r % 2 == 0 { (0..paths.len()).collect() } else { (0..paths.len()).rev().collect() };
            for &p in &order {
                times[p].push(run(p, &mut out, &mut conv));
            }
            assert!(out == want, "output changed during timing");
        }
        println!("{} Hz, Standard, {CH} ch: ms per s of audio (median of {rounds}), ratio to lib", rate.hz());
        for (p, name) in paths.iter().enumerate() {
            let mut ratios: Vec<f64> = times[p].iter().zip(&times[0]).map(|(a, b)| a / b).collect();
            let all: Vec<String> = times[p].iter().map(|t| format!("{t:.2}")).collect();
            println!(
                "  {name:<5} {:7.2} ms/s   ratio {:.3}   [{}]",
                median(&mut times[p].clone()),
                median(&mut ratios),
                all.join(" ")
            );
        }
    }
    if failed {
        eprintln!("CHECK FAILED");
        std::process::exit(1);
    }
}
