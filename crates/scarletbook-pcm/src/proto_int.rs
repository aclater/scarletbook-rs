// SPDX-License-Identifier: MIT OR Apache-2.0

//! PROTOTYPE for #372 item 3, not for merge (feature `proto-int-372`): exact integer Fir1
//! and Dec2 kernels on SSE2 integer instructions. The arithmetic, its overflow bound and the
//! measurement are in `examples/int_kernels.rs`.
//!
//! The kernels call `core::arch` intrinsics, which are safe inside a function built with
//! `#[target_feature(enable = "sse2")]`. Calling such a function needs `unsafe`, which stays
//! in `dispatch.rs` (`proto_fir1_int`, `proto_dec2_int`): SSE2 is enabled for the whole
//! build on every target this module compiles for. No load or store uses a pointer: loads
//! are `_mm_set_epi16` of a bounds-checked slice, which LLVM merges into one `movdqu`.

#[cfg(target_arch = "x86")]
use core::arch::x86::*;
#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::*;

/// Taps between normalisations of the integer sums.
pub const K: usize = 15;

/// `x` as balanced limbs `(lo, hi)`: `x = hi 2^14 + lo`, `lo` in `[-2^13, 2^13)`.
#[inline(always)]
pub fn limbs(x: i32) -> (i32, i32) {
    let lo = (x.wrapping_add(1 << 13) & 0x3fff) - (1 << 13);
    (lo, x.wrapping_sub(lo) >> 14)
}

/// `x` packed as `[lo, hi]` into `dst[0..2]`: the layout the kernels read.
#[inline(always)]
pub fn pack(x: i32, dst: &mut [i16]) {
    let (lo, hi) = limbs(x);
    dst[0] = lo as i16;
    dst[1] = hi as i16;
}

/// 8 lanes of `i16`, aligned so that `pmaddwd` can take it from memory.
#[derive(Clone, Copy, Debug)]
#[repr(align(16))]
struct A16([i16; 8]);

/// One tap's three `pmaddwd` operands: `[hl, 0]`, `[hh, hl]`, `[0, hh]` in each 32-bit lane,
/// for the LL, MID and HH sums.
#[derive(Clone, Copy, Debug)]
struct C3([A16; 3]);

impl C3 {
    fn new(h: i32) -> C3 {
        let (hl, hh) = limbs(h);
        let pair =
            |a: i32, b: i32| A16(core::array::from_fn(|i| if i % 2 == 0 { a as i16 } else { b as i16 }));
        C3([pair(hl, 0), pair(hh, hl), pair(0, hh)])
    }
}

/// A symmetric filter's taps (from the centre out, scaled by 2^24) as limb pairs.
#[derive(Clone, Debug)]
pub struct IntSym {
    full: Vec<C3>,
    even: Vec<C3>,
    odd: Vec<C3>,
}

impl IntSym {
    /// The filter `half` (taps from the centre out). Each tap must be below 2^23 + 2^13 in
    /// magnitude, so that its high limb fits 16 bits.
    pub fn new(half: &[i32]) -> IntSym {
        assert!(half.iter().all(|h| h.unsigned_abs() <= 1 << 23), "taps too large");
        let mut even: Vec<i32> = half.iter().step_by(2).copied().collect();
        while even.len() > 1 && even.last() == Some(&0) {
            even.pop();
        }
        IntSym {
            full: half.iter().map(|&h| C3::new(h)).collect(),
            even: even.iter().map(|&h| C3::new(h)).collect(),
            odd: half.iter().skip(1).step_by(2).map(|&h| C3::new(h)).collect(),
        }
    }

    /// The largest tap offset.
    pub fn half_len(&self) -> usize {
        self.full.len() - 1
    }

    /// Even taps after the centre (Dec2's `e`).
    pub fn even_after_centre(&self) -> usize {
        self.even.len() - 1
    }

    /// Odd taps (Dec2's `o`).
    pub fn odd_len(&self) -> usize {
        self.odd.len()
    }
}

/// Fir1: `out[i]` (rounded to the 2^26 grid) centred on packed sample `x[i + m]`. `out` is
/// a multiple of `4 G` long, `x` holds `out.len() + 2 m` packed samples.
pub fn fir1_int<const G: usize>(s: &IntSym, x: &[i16], out: &mut [i32]) {
    crate::dispatch::proto_fir1_int::<G>(s, x, out)
}

/// Dec2: `out[i]` centred on packed `xe[i + e]`, the odd taps on `xo[i + o + j]` and
/// `xo[i + o - 1 - j]`, as `kernel::dec2`.
pub fn dec2_int<const G: usize>(s: &IntSym, xe: &[i16], xo: &[i16], out: &mut [i32]) {
    crate::dispatch::proto_dec2_int::<G>(s, xe, xo, out)
}

/// The SSE2 bodies; called only through `dispatch`. The helpers are macros, not functions:
/// a `#[target_feature]` function can't be `#[inline(always)]`, and LLVM left both helper
/// functions and closures out of line, passing the sums through memory.
pub(crate) mod sse2 {
    use super::*;

    /// 8 `i16` from a bounds-checked slice: one unaligned load (`movdqu`).
    macro_rules! ld {
        ($w:expr) => {{
            let w: &[i16] = &$w[..8];
            _mm_set_epi16(w[7], w[6], w[5], w[4], w[3], w[2], w[1], w[0])
        }};
    }

    /// One tap's three operands (aligned loads).
    macro_rules! c3 {
        ($c:expr) => {{
            let c: &C3 = $c;
            [ld!(c.0[0].0), ld!(c.0[1].0), ld!(c.0[2].0)]
        }};
    }

    /// Carries LL into MID and MID into HH: LL and MID end in `[0, 2^14)`.
    macro_rules! normalise {
        ($acc:ident) => {
            for a in $acc.iter_mut() {
                let c = _mm_srai_epi32::<14>(a[0]);
                a[0] = _mm_and_si128(a[0], _mm_set1_epi32(0x3fff));
                a[1] = _mm_add_epi32(a[1], c);
                let c = _mm_srai_epi32::<14>(a[1]);
                a[1] = _mm_and_si128(a[1], _mm_set1_epi32(0x3fff));
                a[2] = _mm_add_epi32(a[2], c);
            }
        };
    }

    /// The centre tap at packed sample `at` (unfolded): sets the sums.
    macro_rules! centre {
        ($acc:ident, $c:expr, $x:expr, $at:expr) => {{
            let cs = c3!($c);
            let (x, at): (&[i16], usize) = ($x, $at);
            for (g, a) in $acc.iter_mut().enumerate() {
                let v = ld!(x[2 * at + 8 * g..]);
                for t in 0..3 {
                    a[t] = _mm_madd_epi16(v, cs[t]);
                }
            }
        }};
    }

    /// Adds `h[t] (x[up + t] + x[down - t])` over the taps `h` (limb-wise fold, then three
    /// `pmaddwd`), normalising every `K` taps and at the end.
    macro_rules! pairs {
        ($G:ident, $acc:ident, $h:expr, $x:expr, $up:expr, $down:expr) => {{
            let (h, x, up, down): (&[C3], &[i16], usize, usize) = ($h, $x, $up, $down);
            let w = 8 * $G;
            for (ci, cc) in h.chunks(K).enumerate() {
                let (t0, last) = (ci * K, cc.len() - 1);
                let ups = x[2 * (up + t0)..2 * (up + t0 + last) + w].windows(w).step_by(2);
                let downs = x[2 * (down - t0 - last)..2 * (down - t0) + w].windows(w).step_by(2).rev();
                for ((u, d), c) in ups.zip(downs).zip(cc) {
                    let cs = c3!(c);
                    for (g, a) in $acc.iter_mut().enumerate() {
                        let f = _mm_add_epi16(ld!(u[8 * g..]), ld!(d[8 * g..]));
                        for t in 0..3 {
                            a[t] = _mm_add_epi32(a[t], _mm_madd_epi16(f, cs[t]));
                        }
                    }
                }
                normalise!($acc);
            }
        }};
    }

    /// The normalised sums times 2^-24, rounded half to even: `16 HH + (low >> 24)` plus the
    /// rounding of `low mod 2^24`, `low = MID 2^14 + LL`.
    macro_rules! finish {
        ($acc:ident, $out:expr) => {{
            let half = _mm_set1_epi32(0x80_0000);
            for (a, o) in $acc.iter().zip($out.chunks_exact_mut(4)) {
                let low = _mm_or_si128(_mm_slli_epi32::<14>(a[1]), a[0]);
                let q = _mm_add_epi32(_mm_slli_epi32::<4>(a[2]), _mm_srai_epi32::<24>(low));
                let rem = _mm_and_si128(low, _mm_set1_epi32(0xff_ffff));
                let gt = _mm_cmpgt_epi32(rem, half);
                let tie_odd = _mm_and_si128(_mm_cmpeq_epi32(rem, half), q);
                let up = _mm_and_si128(_mm_or_si128(gt, tie_odd), _mm_set1_epi32(1));
                let r = _mm_add_epi32(q, up);
                o[0] = _mm_cvtsi128_si32(r);
                o[1] = _mm_cvtsi128_si32(_mm_shuffle_epi32::<0b01>(r));
                o[2] = _mm_cvtsi128_si32(_mm_shuffle_epi32::<0b10>(r));
                o[3] = _mm_cvtsi128_si32(_mm_shuffle_epi32::<0b11>(r));
            }
        }};
    }

    #[target_feature(enable = "sse2")]
    pub(crate) fn fir1<const G: usize>(s: &IntSym, x: &[i16], out: &mut [i32]) {
        let m = s.half_len();
        for (bi, o) in out.chunks_exact_mut(4 * G).enumerate() {
            let i = bi * 4 * G;
            let mut acc = [[_mm_setzero_si128(); 3]; G];
            centre!(acc, &s.full[0], x, i + m);
            pairs!(G, acc, &s.full[1..], x, i + m + 1, i + m - 1);
            finish!(acc, o);
        }
    }

    #[target_feature(enable = "sse2")]
    pub(crate) fn dec2<const G: usize>(s: &IntSym, xe: &[i16], xo: &[i16], out: &mut [i32]) {
        let (e, o) = (s.even.len() - 1, s.odd.len());
        for (bi, d) in out.chunks_exact_mut(4 * G).enumerate() {
            let i = bi * 4 * G;
            let mut acc = [[_mm_setzero_si128(); 3]; G];
            centre!(acc, &s.even[0], xe, i + e);
            if e > 0 {
                pairs!(G, acc, &s.even[1..], xe, i + e + 1, i + e - 1);
            }
            pairs!(G, acc, &s.odd, xo, i + o, i + o - 1);
            finish!(acc, d);
        }
    }
}
