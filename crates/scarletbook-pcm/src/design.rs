// SPDX-License-Identifier: MIT OR Apache-2.0

//! The filter design that generates `tables.rs` (test-only; the converter uses only the
//! committed tables, so no design code or libm result reaches a converted sample).
//!
//! Every stage is a linear-phase (type I, odd-length, symmetric) FIR designed by the
//! Parks-McClellan method: the Remez exchange for the best weighted Chebyshev
//! approximation, with the barycentric form of Lagrange interpolation (Parks & McClellan,
//! IEEE Trans. Circuit Theory 19(2), 1972; Rabiner, McClellan & Parks, Proc. IEEE 63(4),
//! 1975). The halfband stage uses the method of Vaidyanathan & Nguyen (IEEE Trans. CAS
//! 34(3), 1987): a type II filter `G` approximating 1 on `[0, 2 wp]` gives the halfband
//! `H(z) = (z^-(2K-1) + G(z^2)) / 2`, whose even taps are exactly zero and whose centre tap
//! is exactly 1/2. Stage edges follow the multistage decimation rules of Crochiere & Rabiner
//! (Proc. IEEE 69(3), 1981): the stopband of a stage that decimates to rate `F` starts at
//! `F - fst`, so nothing aliases into `[0, fst]`.
//!
//! `SCARLETBOOK_PCM_REGENERATE=1 cargo test -p scarletbook-pcm --lib tables` rewrites
//! `src/tables.rs`; without the variable the test checks the committed file.

use std::f64::consts::PI;
use std::fmt::Write as _;

/// The DSD64 sample rate.
pub(crate) const FS: f64 = 2_822_400.0;

/// Stopband attenuation every stage is designed for, in dB (the requirement is 105 dB; the
/// margin is for coefficient quantisation and is checked on the quantised tables).
pub(crate) const STOP_DB: f64 = 110.0;

/// One point of the dense grid: frequency (radians per sample), desired value, weight.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GridPoint {
    w: f64,
    d: f64,
    wt: f64,
}

/// A band: `(lo, hi, desired, weight)`, desired value and weight as functions of the
/// frequency.
type Band<'a> = (f64, f64, &'a dyn Fn(f64) -> f64, &'a dyn Fn(f64) -> f64);

/// A dense grid over `bands`, about 20 points per extremal of an `n`-coefficient
/// approximation.
fn grid(bands: &[Band<'_>], n: usize) -> Vec<Vec<GridPoint>> {
    let density = 20.0;
    let total: f64 = bands.iter().map(|b| b.1 - b.0).sum();
    bands
        .iter()
        .map(|&(lo, hi, d, wt)| {
            let points = (((hi - lo) / total * density * n as f64).ceil() as usize).max(8);
            (0..=points)
                .map(|i| {
                    let w = lo + (hi - lo) * i as f64 / points as f64;
                    GridPoint { w, d: d(w), wt: wt(w) }
                })
                .collect()
        })
        .collect()
}

/// The weighted Chebyshev approximation of the grid's desired values by
/// `A(w) = sum_{k<n} a_k cos(k w)`: the coefficients `a` and the signed weighted error
/// `delta` (Remez exchange). `None` if the exchange fails to converge.
pub(crate) fn remez(bands: &[Vec<GridPoint>], n: usize) -> Option<(Vec<f64>, f64)> {
    let pts: Vec<GridPoint> = bands.iter().flatten().copied().collect();
    let band_of: Vec<usize> =
        bands.iter().enumerate().flat_map(|(b, v)| std::iter::repeat_n(b, v.len())).collect();
    let r = n + 1;
    if pts.len() < 2 * r {
        return None;
    }
    // Initial extremals: equally spaced over the grid.
    let mut ext: Vec<usize> = (0..r).map(|i| i * (pts.len() - 1) / (r - 1)).collect();
    let mut result = None;
    for _ in 0..250 {
        let x: Vec<f64> = ext.iter().map(|&i| pts[i].w).collect();
        // Barycentric weights b_i = 1 / prod_{j != i} (x_i - x_j), x = cos w, computed as
        // logarithms and scaled by a common factor (which cancels) to avoid overflow.
        let logs: Vec<(f64, f64)> = (0..r)
            .map(|i| {
                let (mut l, mut sign) = (0.0, 1.0);
                for j in (0..r).filter(|&j| j != i) {
                    let d = cos_diff(x[i], x[j]);
                    l -= d.abs().ln();
                    sign *= d.signum();
                }
                (l, sign)
            })
            .collect();
        let lmax = logs.iter().map(|v| v.0).fold(f64::NEG_INFINITY, f64::max);
        let b: Vec<f64> = logs.iter().map(|&(l, sign)| sign * (l - lmax).exp()).collect();
        let (mut num, mut den) = (0.0, 0.0);
        for i in 0..r {
            let p = &pts[ext[i]];
            let s = if i % 2 == 0 { 1.0 } else { -1.0 };
            num += b[i] * p.d;
            den += b[i] * s / p.wt;
        }
        let delta = num / den;
        if !delta.is_finite() {
            return None;
        }
        // Values at the first r - 1 extremals; the interpolant through them.
        let c: Vec<f64> = (0..r - 1)
            .map(|i| {
                let p = &pts[ext[i]];
                let s = if i % 2 == 0 { 1.0 } else { -1.0 };
                p.d - s * delta / p.wt
            })
            .collect();
        let dw: Vec<f64> = (0..r - 1).map(|i| b[i] * cos_diff(x[i], x[r - 1])).collect();
        let xs = x[..r - 1].to_vec();
        let interp = move |wv: f64| -> f64 {
            let (mut nu, mut de) = (0.0, 0.0);
            for i in 0..xs.len() {
                let diff = cos_diff(wv, xs[i]);
                if diff == 0.0 {
                    return c[i];
                }
                let t = dw[i] / diff;
                nu += t * c[i];
                de += t;
            }
            nu / de
        };
        let mut err: Vec<f64> = pts.iter().map(|p| p.wt * (p.d - interp(p.w))).collect();
        // At the extremals the error is (-1)^i delta by construction; computed through the
        // interpolant it can lose its sign when delta is tiny, which would break alternation.
        for (i, &j) in ext.iter().enumerate() {
            err[j] = if i % 2 == 0 { delta } else { -delta };
        }
        result = Some((interp, delta));

        // Local extrema of the error within each band, |E| >= |delta|.
        let local = |thresh: f64| -> Vec<usize> {
            let mut v = Vec::new();
            for j in 0..pts.len() {
                let e = err[j];
                let left = (j > 0 && band_of[j - 1] == band_of[j]).then(|| err[j - 1]);
                let right = (j + 1 < pts.len() && band_of[j + 1] == band_of[j]).then(|| err[j + 1]);
                let is_ext = if e > 0.0 {
                    left.is_none_or(|l| e >= l) && right.is_none_or(|r| e >= r)
                } else {
                    left.is_none_or(|l| e <= l) && right.is_none_or(|r| e <= r)
                };
                if is_ext && e.abs() >= thresh {
                    v.push(j);
                }
            }
            v
        };
        let mut cand = local(delta.abs() * (1.0 - 1e-9));
        if cand.len() < r {
            // Too few: every local extremum and the previous extremals.
            cand = local(0.0);
            cand.extend_from_slice(&ext);
            cand.sort_unstable();
            cand.dedup();
        }
        // Alternation: of consecutive candidates of the same sign keep the larger.
        let mut alt: Vec<usize> = Vec::with_capacity(cand.len());
        for j in cand {
            match alt.last() {
                Some(&k) if (err[k] > 0.0) == (err[j] > 0.0) => {
                    if err[j].abs() > err[k].abs() {
                        *alt.last_mut()? = j;
                    }
                }
                _ => alt.push(j),
            }
        }
        while alt.len() > r {
            if err[alt[0]].abs() < err[alt[alt.len() - 1]].abs() {
                alt.remove(0);
            } else {
                alt.pop();
            }
        }
        if alt.len() < r {
            return None;
        }
        let max_err = alt.iter().map(|&j| err[j].abs()).fold(0.0, f64::max);
        let done = alt == ext || max_err - delta.abs() <= 1e-9 * delta.abs();
        ext = alt;
        if done {
            break;
        }
    }
    let (interp, delta) = result?;
    // Coefficients from n samples of A by the DCT-I (A is a cosine polynomial of degree n - 1).
    let big_n = (n - 1) as f64;
    let samples: Vec<f64> = (0..n).map(|j| interp(PI * j as f64 / big_n)).collect();
    let a = (0..n)
        .map(|k| {
            let mut s = 0.0;
            for (j, &aj) in samples.iter().enumerate() {
                let half = if j == 0 || j == n - 1 { 0.5 } else { 1.0 };
                s += half * aj * (PI * (j * k) as f64 / big_n).cos();
            }
            let scale = if k == 0 || k == n - 1 { 1.0 } else { 2.0 };
            s * scale / big_n
        })
        .collect();
    Some((a, delta))
}

/// `cos a - cos b`, accurate when `a` and `b` are close (the barycentric form works on
/// `x = cos w`, which loses resolution near `w = 0` and `w = pi`).
fn cos_diff(a: f64, b: f64) -> f64 {
    -2.0 * ((a + b) / 2.0).sin() * ((a - b) / 2.0).sin()
}

/// A type I low-pass of length `2m + 1` (taps at offsets `-m..=m`), passband `[0, wp]` with
/// weight 1, stopband `[ws, pi]` with weight `k`; returns the taps and the passband ripple.
pub(crate) fn lowpass(m: usize, wp: f64, ws: f64, k: f64) -> Option<(Vec<f64>, f64)> {
    let one = |_: f64| 1.0;
    let zero = |_: f64| 0.0;
    let wk = move |_: f64| k;
    let g = grid(&[(0.0, wp, &one, &one), (ws, PI, &zero, &wk)], m + 1);
    let (a, delta) = remez(&g, m + 1)?;
    let mut h = vec![0.0; 2 * m + 1];
    h[m] = a[0];
    for (i, &ai) in a.iter().enumerate().skip(1) {
        h[m - i] = ai / 2.0;
        h[m + i] = ai / 2.0;
    }
    Some((h, delta.abs()))
}

/// A halfband filter of length `4K - 1` (Vaidyanathan & Nguyen) with passband `[0, wp]`
/// (and so stopband `[pi - wp, pi]`); returns the taps and the ripple of both bands.
pub(crate) fn halfband(k: usize, wp: f64) -> Option<(Vec<f64>, f64)> {
    // G: type II, length 2K, amplitude cos(w/2) P(w), P of degree K - 1, approximating 1
    // on [0, 2 wp]: approximate 1 / cos(w/2) with weight cos(w/2).
    let d = |w: f64| 1.0 / (w / 2.0).cos();
    let wt = |w: f64| (w / 2.0).cos();
    let g = grid(&[(0.0, 2.0 * wp, &d, &wt)], k);
    let (p, delta) = remez(&g, k)?;
    // cos(w/2) cos(i w) = (cos((i + 1/2) w) + cos((i - 1/2) w)) / 2: the coefficient c_j of
    // cos((j - 1/2) w), j = 1..=K.
    let mut c = vec![0.0; k + 1];
    for (i, &pi) in p.iter().enumerate() {
        c[i + 1] += pi / 2.0;
        if i == 0 {
            c[1] += pi / 2.0;
        } else {
            c[i] += pi / 2.0;
        }
    }
    let half = 2 * k - 1;
    let mut h = vec![0.0; 2 * half + 1];
    h[half] = 0.5;
    for (j, &cj) in c.iter().enumerate().skip(1) {
        let off = 2 * j - 1;
        h[half - off] = cj / 4.0;
        h[half + off] = cj / 4.0;
    }
    Some((h, delta.abs() / 2.0))
}

/// Taps scaled by `2^q` and rounded, with the centre adjusted so that they sum to exactly
/// `2^q` (unit gain at DC, exactly). For a halfband (`halfband`), the centre stays exactly
/// `2^(q-1)` and the taps next to it absorb the adjustment instead.
pub(crate) fn quantise(h: &[f64], q: u32, halfband: bool) -> Vec<i64> {
    // Normalise to unit DC gain first, then round; the rounding leaves a small difference
    // from 2^q, removed one unit at a time from the taps whose rounding error was largest
    // in the right direction (so that no tap moves by more than one unit of rounding).
    let dc: f64 = h.iter().sum();
    let scale = (1u64 << q) as f64 / dc;
    let ideal: Vec<f64> = h.iter().map(|&x| x * scale).collect();
    let mut v: Vec<i64> = ideal.iter().map(|&x| x.round() as i64).collect();
    let mid = v.len() / 2;
    if halfband {
        v[mid] = 1i64 << (q - 1);
    }
    let mut diff = (1i64 << q) - v.iter().sum::<i64>();
    if diff % 2 != 0 {
        assert!(!halfband, "halfband taps come in pairs");
        v[mid] += diff.signum();
        diff -= diff.signum();
    }
    // Pairs (mid - k, mid + k), k >= 1, by how far rounding moved them against the needed
    // direction.
    let mut pairs: Vec<usize> = (1..=mid).filter(|&k| v[mid + k] != 0 || ideal[mid + k] != 0.0).collect();
    let dir = diff.signum();
    pairs.sort_by(|&a, &b| {
        let ra = (ideal[mid + a] - v[mid + a] as f64) * dir as f64;
        let rb = (ideal[mid + b] - v[mid + b] as f64) * dir as f64;
        rb.total_cmp(&ra)
    });
    for &k in pairs.iter().cycle().take((diff.abs() / 2) as usize) {
        v[mid - k] += dir;
        v[mid + k] += dir;
    }
    debug_assert_eq!(v.iter().sum::<i64>(), 1i64 << q);
    v
}

/// The amplitude response of a symmetric odd-length filter at `w` (radians per sample),
/// with taps scaled by `2^q`.
pub(crate) fn amplitude(h: &[i64], q: u32, w: f64) -> f64 {
    let m = h.len() / 2;
    let mut s = h[m] as f64;
    for k in 1..=m {
        s += 2.0 * h[m + k] as f64 * (k as f64 * w).cos();
    }
    s / (1u64 << q) as f64
}

/// The shortest type I low-pass with passband ripple at most `dp` and stopband attenuation
/// at least `STOP_DB` after quantisation to `q` bits, searching `m` in `ms` (with the given
/// step, so that S1 lengths fill whole bytes).
fn shortest_lowpass(ms: impl Iterator<Item = usize>, wp: f64, ws: f64, dp: f64, q: u32) -> (Vec<i64>, usize) {
    let ds = 10f64.powf(-STOP_DB / 20.0);
    for m in ms {
        let Some((h, ripple)) = lowpass(m, wp, ws, dp / ds) else { continue };
        if ripple > dp {
            continue;
        }
        let hq = quantise(&h, q, false);
        if worst_stop_db(&hq, q, ws) >= STOP_DB - 2.0 {
            return (hq, m);
        }
    }
    panic!("no filter found");
}

/// The smallest attenuation of `h` over `[ws, pi]`, in dB, on a dense grid.
pub(crate) fn worst_stop_db(h: &[i64], q: u32, ws: f64) -> f64 {
    let points = 40 * h.len();
    let mut worst: f64 = 0.0;
    for i in 0..=points {
        let w = ws + (PI - ws) * i as f64 / points as f64;
        worst = worst.max(amplitude(h, q, w).abs());
    }
    -20.0 * worst.log10()
}

/// Radians per sample of `f` Hz at rate `rate`.
pub(crate) fn rad(f: f64, rate: f64) -> f64 {
    2.0 * PI * f / rate
}

/// The generated stage filters (integer taps, full symmetric length, centre in the middle).
#[derive(Debug)]
pub(crate) struct Designed {
    pub s1: Vec<i64>,
    pub s1w: Vec<i64>,
    pub hb: Vec<i64>,
    pub f40: Vec<i64>,
    pub f30: Vec<i64>,
}

/// Coefficient scale of the first stage (its outputs are on the 2^26 grid directly).
pub(crate) const Q_S1: u32 = 26;
/// Coefficient scale of the later stages.
pub(crate) const Q_FIR: u32 = 24;

/// Passband ripple allowed per stage (linear): S1 and the sharp filter. The halfband's
/// ripple equals its stopband error, far below these.
const DP_S1: f64 = 5.0e-5;
const DP_F: f64 = 2.5e-4;

/// Design every stage.
pub(crate) fn design() -> Designed {
    let r352 = FS / 8.0;
    let r176 = FS / 16.0;
    // S1: decimates by 8 to 352.8 kHz. Half-length m = 8k + 7: the taps then fill 2k + 2
    // whole bytes on each output's grid (one tap position, beyond the end, is zero); see
    // `kernel::lut`.
    let (s1, _) =
        shortest_lowpass((1..20).map(|k| 8 * k + 7), rad(40e3, FS), rad(r352 - 48e3, FS), DP_S1, Q_S1);
    let (s1w, _) =
        shortest_lowpass((1..20).map(|k| 8 * k + 7), rad(80e3, FS), rad(r352 - 96e3, FS), DP_S1, Q_S1);
    // Halfband 352.8 -> 176.4 kHz, passband to 48 kHz (stopband from 128.4 kHz).
    let ds = 10f64.powf(-STOP_DB / 20.0);
    let hb = (2..60)
        .find_map(|k| {
            let (h, ripple) = halfband(k, rad(48e3, r352))?;
            let hq = quantise(&h, Q_FIR, true);
            (ripple <= ds && worst_stop_db(&hq, Q_FIR, rad(r176 - 48e3, r352)) >= STOP_DB - 2.0).then_some(hq)
        })
        .expect("halfband");
    // The sharp filters at 176.4 kHz (F40 also serves as the wide filter at 352.8 kHz).
    let (f40, _) = shortest_lowpass(20..200, rad(40e3, r176), rad(48e3, r176), DP_F, Q_FIR);
    let (f30, _) = shortest_lowpass(20..200, rad(30e3, r176), rad(38e3, r176), DP_F, Q_FIR);
    Designed { s1, s1w, hb, f40, f30 }
}

/// `src/tables.rs` for the designed filters: the taps from the centre outwards (`[0]` is the
/// centre, `[k]` the taps at offsets `-k` and `+k`).
pub(crate) fn tables_rs(d: &Designed) -> String {
    let mut s = String::new();
    s.push_str("// SPDX-License-Identifier: MIT OR Apache-2.0\n\n");
    s.push_str("// @generated by src/design.rs: `SCARLETBOOK_PCM_REGENERATE=1 cargo test -p scarletbook-pcm --lib tables`.\n");
    s.push_str(
        "// Do not edit. Each table holds the taps of one symmetric stage from the centre outwards:\n",
    );
    s.push_str("// `[0]` is the centre tap, `[k]` the taps at offsets -k and +k.\n\n");
    let mut table = |name: &str, doc: &str, h: &[i64], q: u32| {
        let m = h.len() / 2;
        let _ = writeln!(s, "/// {doc} Length {}, taps scaled by 2^{q}.", h.len());
        let _ = writeln!(s, "#[rustfmt::skip]");
        let _ = write!(s, "pub(crate) const {name}: [i32; {}] = [", m + 1);
        for (i, v) in h[m..].iter().enumerate() {
            if i % 8 == 0 {
                s.push_str("\n   ");
            }
            let _ = write!(s, " {v},");
        }
        s.push_str("\n];\n\n");
    };
    table(
        "S1",
        "First stage of the 40 and 30 kHz filters: 2822.4 kHz, passband 40 kHz, stopband from 304.8 kHz.",
        &d.s1,
        Q_S1,
    );
    table(
        "S1_WIDE",
        "First stage of the 80 kHz filter: 2822.4 kHz, passband 80 kHz, stopband from 256.8 kHz.",
        &d.s1w,
        Q_S1,
    );
    table("HB", "Halfband at 352.8 kHz: passband 48 kHz, stopband from 128.4 kHz.", &d.hb, Q_FIR);
    table(
        "F40",
        "Sharp filter at 176.4 kHz: passband 40 kHz, stopband from 48 kHz (also 80/96 kHz at 352.8 kHz).",
        &d.f40,
        Q_FIR,
    );
    table("F30", "Sharp filter at 176.4 kHz: passband 30 kHz, stopband from 38 kHz.", &d.f30, Q_FIR);
    s.truncate(s.trim_end().len());
    s.push('\n');
    s
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The committed tables are the generator's output (exactly on x86-64 Linux, where CI
    /// checks them; within one unit elsewhere, where libm's `cos` and `exp` may differ in
    /// the last bit).
    #[test]
    fn tables() {
        let generated = tables_rs(&design());
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tables.rs");
        if std::env::var_os("SCARLETBOOK_PCM_REGENERATE").is_some() {
            std::fs::write(&path, &generated).unwrap();
            return;
        }
        let committed = std::fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
        if cfg!(all(target_arch = "x86_64", target_os = "linux")) {
            assert!(committed == generated, "src/tables.rs differs from the generator's output");
        } else {
            let numbers = |s: &str| -> Vec<i64> {
                s.split(|c: char| !(c.is_ascii_digit() || c == '-')).filter_map(|t| t.parse().ok()).collect()
            };
            let (a, b) = (numbers(&committed), numbers(&generated));
            assert_eq!(a.len(), b.len());
            assert!(a.iter().zip(&b).all(|(x, y)| (x - y).abs() <= 1));
        }
    }

    use crate::{Cutoff, PcmRate, tables};

    /// Full taps (offsets -m..=m) from a table (centre out).
    fn full(half: &[i32]) -> Vec<i64> {
        let mut v: Vec<i64> = half.iter().rev().map(|&x| i64::from(x)).collect();
        v.extend(half.iter().skip(1).map(|&x| i64::from(x)));
        v
    }

    /// The response of the converter's cascade at `f` Hz (input frequency), relative to
    /// full scale: the product of the stage responses (noble identities). For 352.8 kHz
    /// output of `Default` and `Low`, it includes the interpolator's response at `f`.
    pub(crate) fn response(rate: PcmRate, cutoff: Cutoff, f: f64) -> f64 {
        let (r352, r176) = (FS / 8.0, FS / 16.0);
        if cutoff == Cutoff::Wide {
            return amplitude(&full(&tables::S1_WIDE), Q_S1, rad(f, FS))
                * amplitude(&full(&tables::F40), Q_FIR, rad(f, r352));
        }
        let fq = if cutoff == Cutoff::Low { &tables::F30[..] } else { &tables::F40[..] };
        let base = amplitude(&full(&tables::S1), Q_S1, rad(f, FS))
            * amplitude(&full(&tables::HB), Q_FIR, rad(f, r352))
            * amplitude(&full(fq), Q_FIR, rad(f, r176));
        if rate == PcmRate::Hz352800 {
            base * amplitude(&full(&tables::HB), Q_FIR, rad(f, r352))
        } else {
            base
        }
    }

    /// The published figures of one rate and cutoff.
    #[derive(Debug)]
    struct Figures {
        /// Largest deviation from 0 dB up to 20 kHz, and up to the passband edge.
        ripple_20k: f64,
        ripple_fp: f64,
        /// -0.1 dB and -3 dB points, Hz.
        f_01: f64,
        f_3: f64,
        /// Smallest attenuation at and above the stopband edge, aliases included, dB.
        stop: f64,
        /// For 352.8 kHz from 176.4 kHz: the smallest attenuation of the images.
        images: Option<f64>,
    }

    fn figures(rate: PcmRate, cutoff: Cutoff) -> Figures {
        let db = |f: f64| 20.0 * response(rate, cutoff, f).abs().log10();
        let fp = f64::from(cutoff.passband_hz());
        let fst = f64::from(cutoff.stopband_hz());
        let (mut ripple_20k, mut ripple_fp) = (0.0f64, 0.0f64);
        let mut f = 0.0;
        while f <= fp {
            let d = db(f).abs();
            ripple_fp = ripple_fp.max(d);
            if f <= 20_000.0 {
                ripple_20k = ripple_20k.max(d);
            }
            f += 5.0;
        }
        let first_below = |level: f64| {
            let mut f = 0.0;
            while db(f) > level {
                f += 1.0;
            }
            f
        };
        // The stopband to fs/2, in 10 Hz steps (the narrowest lobes, of the sharp filter at
        // 176.4 kHz, are about 1.5 kHz wide).
        let mut stop = f64::INFINITY;
        let mut f = fst;
        while f <= FS / 2.0 {
            stop = stop.min(-db(f));
            f += 10.0;
        }
        // Images of the 176.4 kHz signal's band [0, fst] at 352.8 kHz: the interpolating
        // halfband's attenuation over [176.4k - fst, 176.4k].
        let images = (rate == PcmRate::Hz352800 && cutoff != Cutoff::Wide).then(|| {
            let mut worst = f64::INFINITY;
            let mut f = FS / 16.0 - 48_000.0;
            while f <= FS / 16.0 {
                worst =
                    worst.min(-20.0 * amplitude(&full(&tables::HB), Q_FIR, rad(f, FS / 8.0)).abs().log10());
                f += 10.0;
            }
            worst
        });
        Figures { ripple_20k, ripple_fp, f_01: first_below(-0.1), f_3: first_below(-3.0), stop, images }
    }

    /// The requirements of #130: stopband >= 105 dB, ripple <= 0.01 dB to 20 kHz and
    /// <= 0.1 dB to the passband edge.
    #[test]
    fn response_meets_the_requirements() {
        for rate in PcmRate::ALL {
            for cutoff in Cutoff::ALL.into_iter().filter(|c| c.supports(rate)) {
                let f = figures(rate, cutoff);
                assert!(f.stop >= 105.0, "{rate} {cutoff:?}: {f:?}");
                assert!(f.images.is_none_or(|i| i >= 105.0), "{rate} {cutoff:?}: {f:?}");
                assert!(f.ripple_20k <= 0.01, "{rate} {cutoff:?}: {f:?}");
                assert!(f.ripple_fp <= 0.1, "{rate} {cutoff:?}: {f:?}");
                assert!(f.f_01 >= f64::from(cutoff.passband_hz()), "{rate} {cutoff:?}: {f:?}");
            }
        }
    }

    /// DC gain exactly 1 at every stage (100 % modulation is full scale), and every halfband
    /// tap at an even offset exactly 0 with the centre exactly 1/2.
    #[test]
    fn exact_dc_gain_and_halfband() {
        for (half, q) in [
            (&tables::S1[..], Q_S1),
            (&tables::S1_WIDE[..], Q_S1),
            (&tables::HB[..], Q_FIR),
            (&tables::F40[..], Q_FIR),
            (&tables::F30[..], Q_FIR),
        ] {
            assert_eq!(full(half).iter().sum::<i64>(), 1 << q);
        }
        assert_eq!(tables::HB[0], 1 << (Q_FIR - 1));
        assert!(tables::HB.iter().skip(2).step_by(2).all(|&h| h == 0));
    }

    /// Every sum the kernels carry in `f64` is an integer below 2^53, so exact: per stage,
    /// from the largest possible input (+-1 bits through the stages before it).
    #[test]
    fn f64_sums_are_exact() {
        let abs = |half: &[i32]| -> i64 { full(half).iter().map(|x| x.abs()).sum() };
        let limit = 1i64 << 53;
        // S1 outputs: integers on the 2^26 grid, summed in i32.
        for s1 in [&tables::S1[..], &tables::S1_WIDE[..]] {
            assert!(abs(s1) < i64::from(i32::MAX));
        }
        // A stage with input bound `x` (on the 2^26 grid) and taps scaled by 2^q: every
        // partial sum is at most sum|h| x, each pair sum at most 2x; the output is rounded
        // back to the grid.
        let stage = |x: i64, half: &[i32], q: u32| -> i64 {
            let worst = abs(half).checked_mul(x).expect("overflow");
            assert!(worst < limit && 2 * x < limit, "sum bound {worst}");
            (worst >> q) + 1
        };
        // Interpolation: odd taps doubled, scale 2^(q - 1).
        let interp = |x: i64, half: &[i32]| -> i64 {
            let odd: i64 = half.iter().skip(1).step_by(2).map(|h| 2 * i64::from(h.abs())).sum();
            let worst = odd * x;
            assert!(worst < limit);
            (worst >> (Q_FIR - 1)) + 1
        };
        let s1 = abs(&tables::S1);
        let v = stage(s1, &tables::HB, Q_FIR);
        for f in [&tables::F40[..], &tables::F30[..]] {
            let w = stage(v, f, Q_FIR);
            let y = interp(w, &tables::HB);
            // The gain multiply: at most +24 dB (x 15.85), still exact-integer input.
            assert!(y * 16 < limit);
        }
        let w = stage(abs(&tables::S1_WIDE), &tables::F40, Q_FIR);
        assert!(w * 16 < limit);
    }

    /// The figures for the crate README: `cargo test -p scarletbook-pcm --release --lib
    /// -- --ignored --nocapture report`.
    #[test]
    #[ignore = "prints the README table"]
    fn report() {
        println!(
            "| Rate | Cutoff | Ripple to 20 kHz | Ripple to passband edge | -0.1 dB | -3 dB | Stopband (min) |"
        );
        println!("|---|---|---|---|---|---|---|");
        for rate in PcmRate::ALL {
            for cutoff in Cutoff::ALL.into_iter().filter(|c| c.supports(rate)) {
                let f = figures(rate, cutoff);
                let stop = match f.images {
                    Some(i) => format!("{:.1} dB (images {:.1} dB)", f.stop, i),
                    None => format!("{:.1} dB", f.stop),
                };
                println!(
                    "| {} | {cutoff:?} | ±{:.4} dB | ±{:.4} dB | {:.1} kHz | {:.1} kHz | {stop} |",
                    rate.hz(),
                    f.ripple_20k,
                    f.ripple_fp,
                    f.f_01 / 1000.0,
                    f.f_3 / 1000.0
                );
            }
        }
        let d = |h: &[i32]| 2 * h.len() - 1;
        println!(
            "\nLengths: S1 {}, S1 wide {}, halfband {}, F40 {}, F30 {}",
            d(&tables::S1),
            d(&tables::S1_WIDE),
            d(&tables::HB),
            d(&tables::F40),
            d(&tables::F30)
        );
    }

    /// `a` followed by `b` at `1/l` of the rate, as one filter (noble identity).
    fn cascade(a: &[f64], b: &[f64], l: usize) -> Vec<f64> {
        let mut out = vec![0.0; a.len() + l * (b.len() - 1)];
        for (j, &bj) in b.iter().enumerate() {
            for (i, &ai) in a.iter().enumerate() {
                out[i + l * j] += ai * bj;
            }
        }
        out
    }

    /// The converter against the ideal (unrounded, `f64`) filters: the largest difference
    /// in 24-bit LSB, and the share of samples that differ at all.
    fn model_residual(rate: PcmRate, cutoff: Cutoff) -> (f64, f64) {
        use crate::{Converter, FRAME_BYTES_PER_CHANNEL, FrameRun, PcmConfig};
        let (channels, frames) = (2usize, 3usize);
        let input = crate::reference::tests::signal(11, channels, frames);
        let config = PcmConfig::new(rate, cutoff, channels as u8);
        let mut conv = Converter::new(config).unwrap();
        let mut got = vec![0i32; frames * conv.samples_per_frame() * channels];
        conv.convert_run(FrameRun { before: None, frames: &input, after: None }, &mut got).unwrap();
        let f = |h: &[i32], q: u32| -> Vec<f64> {
            full(h).iter().map(|&x| x as f64 / (1u64 << q) as f64).collect()
        };
        let samples = (frames * FRAME_BYTES_PER_CHANNEL * 8) as i64;
        let (mut worst, mut differ) = (0.0f64, 0usize);
        for c in 0..channels {
            let x = |s: i64| -> f64 {
                if s < 0 || s >= samples {
                    return 0.0;
                }
                let byte = input[(s as usize / 8) * channels + c];
                if (byte >> (7 - s % 8)) & 1 == 1 { 1.0 } else { -1.0 }
            };
            // An equivalent filter applied at DSD sample `centre`.
            let apply = |h: &[f64], centre: i64| -> f64 {
                let m = (h.len() / 2) as i64;
                h.iter().enumerate().map(|(i, &hi)| hi * x(centre + i as i64 - m)).sum()
            };
            let model: Box<dyn Fn(i64) -> f64> = if cutoff == Cutoff::Wide {
                let heq = cascade(&f(&tables::S1_WIDE, Q_S1), &f(&tables::F40, Q_FIR), 8);
                let d = rate.decimation() as i64;
                Box::new(move |n| apply(&heq, d * n))
            } else {
                let fq = if cutoff == Cutoff::Low { &tables::F30[..] } else { &tables::F40[..] };
                let heq =
                    cascade(&cascade(&f(&tables::S1, Q_S1), &f(&tables::HB, Q_FIR), 8), &f(fq, Q_FIR), 16);
                if rate == PcmRate::Hz352800 {
                    let hb = f(&tables::HB, Q_FIR);
                    let m = (hb.len() / 2) as i64;
                    Box::new(move |q| {
                        if q % 2 == 0 {
                            apply(&heq, 8 * q)
                        } else {
                            // Zero-stuffed 176.4 kHz samples through 2 x halfband.
                            (0..hb.len() as i64)
                                .filter(|t| (q - (t - m)) % 2 == 0)
                                .map(|t| 2.0 * hb[t as usize] * apply(&heq, 8 * (q - (t - m))))
                                .sum()
                        }
                    })
                } else {
                    let d = rate.decimation() as i64;
                    Box::new(move |n| apply(&heq, d * n))
                }
            };
            for n in 0..got.len() / channels {
                let ideal = model(n as i64) * 8_388_608.0;
                let r = (f64::from(got[n * channels + c]) - ideal.clamp(-8_388_608.0, 8_388_607.0)).abs();
                worst = worst.max(r);
                if r >= 0.5 {
                    differ += 1;
                }
            }
        }
        (worst, differ as f64 / got.len() as f64)
    }

    /// The integer cascade is the ideal filter to within one LSB: the roundings between
    /// stages (on a grid 8 times finer than 24 bits) and the final one add less than 1 LSB.
    #[test]
    fn converter_is_the_model_to_one_lsb() {
        for rate in PcmRate::ALL {
            for cutoff in Cutoff::ALL.into_iter().filter(|c| c.supports(rate)) {
                let (worst, share) = model_residual(rate, cutoff);
                assert!(worst <= 1.0, "{rate} {cutoff:?}: {worst} LSB");
                if std::env::var_os("SCARLETBOOK_PCM_REPORT").is_some() {
                    println!("| {} | {cutoff:?} | {worst:.3} | {:.2} % |", rate.hz(), share * 100.0);
                }
            }
        }
    }
}
