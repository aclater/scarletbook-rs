// SPDX-License-Identifier: MIT OR Apache-2.0

//! End-to-end quality on synthetic 1-bit audio (test-only): sines, a stepped sweep, DC,
//! a step, and full-scale input, through a fifth-order sigma-delta modulator.
//!
//! The modulator is the error-feedback form `y = x + NTF(z) e`, with
//! `NTF(z) = (1 - z^-1)^5 / D(z)` (zeros at DC, maximally flat poles placed for a largest
//! NTF gain of 1.5, the stability rule of thumb of Chao, Nadeem, Lee & Sodini, IEEE
//! Trans. CAS 37(3), 1990; the structure as in Schreier & Temes, *Understanding Delta-Sigma
//! Data Converters*, 2005). Its signal transfer function is 1, without delay, so output
//! sample `n` must reproduce the input at DSD sample `d n` exactly in phase.
//!
//! `SCARLETBOOK_PCM_REPORT=1 cargo test -p scarletbook-pcm --release --lib quality --
//! --nocapture` prints the figures of the crate README.

use crate::design::tests::response;
use crate::{Converter, Cutoff, FRAME_BYTES_PER_CHANNEL, FrameRun, Gain, PcmConfig, PcmRate};

const FS: f64 = 2_822_400.0;
const FULL: f64 = 8_388_608.0;

fn report() -> bool {
    std::env::var_os("SCARLETBOOK_PCM_REPORT").is_some()
}

/// The denominator of the NTF, `d[0..=5]` (`d[0]` = 1): the poles of a fifth-order
/// Butterworth high-pass with cutoff `wc` (radians per sample, before warping) mapped by the
/// bilinear transform (Oppenheim & Schafer, *Discrete-Time Signal Processing*, 7.1).
fn ntf_denominator(wc: f64) -> [f64; 6] {
    // Polynomial in z^-1 with complex coefficients: prod (1 - z_k z^-1).
    let mut poly = [(0.0f64, 0.0f64); 6];
    poly[0] = (1.0, 0.0);
    for (k, len) in (0..5).zip(1..) {
        let theta = std::f64::consts::PI * (2 * k + 5 + 1) as f64 / 10.0;
        let (pr, pi) = (wc * theta.cos(), wc * theta.sin());
        // z_k = (1 + p/2) / (1 - p/2).
        let (nr, ni) = (1.0 + pr / 2.0, pi / 2.0);
        let (dr, di) = (1.0 - pr / 2.0, -pi / 2.0);
        let den = dr * dr + di * di;
        let (zr, zi) = ((nr * dr + ni * di) / den, (ni * dr - nr * di) / den);
        for i in (1..=len).rev() {
            let (ar, ai) = poly[i - 1];
            let (br, bi) = if i < len { poly[i] } else { (0.0, 0.0) };
            poly[i] = (br - (zr * ar - zi * ai), bi - (zr * ai + zi * ar));
        }
    }
    let mut d = [0.0; 6];
    for (o, p) in d.iter_mut().zip(poly) {
        *o = p.0;
    }
    d
}

/// One channel of `frames` frames of `x(t)` (t in DSD samples), modulated to 1 bit by a
/// fifth-order loop: NTF zeros at DC, Butterworth poles placed for a largest NTF gain
/// (at half the sample rate) of 1.5.
fn modulate(frames: usize, x: impl Fn(f64) -> f64) -> Vec<u8> {
    let binom = [1.0, 5.0, 10.0, 10.0, 5.0, 1.0];
    let n: Vec<f64> = (0..6).map(|i| binom[i] * if i % 2 == 0 { 1.0 } else { -1.0 }).collect();
    // |NTF(-1)| = 32 / D(-1); bisection on the cutoff.
    let gain = |wc: f64| {
        32.0 / ntf_denominator(wc)
            .iter()
            .enumerate()
            .map(|(i, &c)| if i % 2 == 0 { c } else { -c })
            .sum::<f64>()
    };
    let (mut lo, mut hi) = (1e-4, 3.0);
    for _ in 0..100 {
        let mid = (lo + hi) / 2.0;
        if gain(mid) > 1.5 { hi = mid } else { lo = mid }
    }
    let d = ntf_denominator(lo);
    let (mut e, mut w) = ([0.0f64; 6], [0.0f64; 6]);
    let mut out = vec![0u8; frames * FRAME_BYTES_PER_CHANNEL];
    for s in 0..out.len() * 8 {
        let mut wn = 0.0;
        for i in 1..6 {
            wn += (n[i] - d[i]) * e[i] - d[i] * w[i];
        }
        let xv = x(s as f64);
        let v = xv + wn;
        let y = if v >= 0.0 { 1.0 } else { -1.0 };
        assert!(v.abs() < 20.0, "modulator unstable");
        e.copy_within(1..5, 2);
        w.copy_within(1..5, 2);
        e[1] = y - v;
        w[1] = wn;
        if y > 0.0 {
            out[s / 8] |= 0x80 >> (s % 8);
        }
    }
    out
}

/// The whole area's output (one channel, no neighbours).
fn convert(rate: PcmRate, cutoff: Cutoff, gain: Gain, input: &[u8]) -> (Vec<i32>, u64) {
    let mut conv = Converter::new(PcmConfig::new(rate, cutoff, 1).with_gain(gain)).unwrap();
    let mut out = vec![0i32; input.len() / FRAME_BYTES_PER_CHANNEL * conv.samples_per_frame()];
    let stats = conv.convert_run(FrameRun { before: None, frames: input, after: None }, &mut out).unwrap();
    (out, stats.clipped_total())
}

/// Least-squares fit of `a cos(w n) + b sin(w n) + c` to `y[n]` over `range`: the
/// amplitude, the phase of `sin(w n + phase)`, and the RMS of the residual.
fn fit(y: &[i32], w: f64, range: std::ops::Range<usize>) -> (f64, f64, f64) {
    let mut m = [[0.0f64; 4]; 3];
    for n in range.clone() {
        let row = [(w * n as f64).cos(), (w * n as f64).sin(), 1.0];
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] += row[i] * row[j];
            }
            m[i][3] += row[i] * f64::from(y[n]);
        }
    }
    // Gaussian elimination with partial pivoting.
    #[allow(clippy::needless_range_loop)]
    for col in 0..3 {
        let piv = (col..3).max_by(|&a, &b| m[a][col].abs().total_cmp(&m[b][col].abs())).unwrap();
        m.swap(col, piv);
        for r in 0..3 {
            if r != col {
                let k = m[r][col] / m[col][col];
                for c in col..4 {
                    m[r][c] -= k * m[col][c];
                }
            }
        }
    }
    let (a, b, c) = (m[0][3] / m[0][0], m[1][3] / m[1][1], m[2][3] / m[2][2]);
    let mut sq = 0.0;
    for n in range.clone() {
        let r = f64::from(y[n]) - (a * (w * n as f64).cos() + b * (w * n as f64).sin() + c);
        sq += r * r;
    }
    ((a * a + b * b).sqrt(), a.atan2(b), (sq / range.len() as f64).sqrt())
}

/// A sine of `amp` (of full scale) at `f` Hz: level error against the analytic response
/// (dB), alignment error (output samples), and THD+N (dB below the signal).
fn sine(rate: PcmRate, cutoff: Cutoff, f: f64, amp: f64) -> (f64, f64, f64) {
    // Long enough for at least two periods inside the fitted middle part.
    let frames = (6.0 + 2.0 * 75.0 / f * 2.0).ceil() as usize;
    let w_dsd = 2.0 * std::f64::consts::PI * f / FS;
    let input = modulate(frames, |t| amp * (w_dsd * t).sin());
    let (y, _) = convert(rate, cutoff, Gain::UNITY, &input);
    let spf = rate.samples_per_frame();
    let w = w_dsd * rate.decimation() as f64;
    let (a, phase, rms) = fit(&y, w, spf..y.len() - spf);
    let want = amp * response(rate, cutoff, f) * FULL;
    let level = 20.0 * (a / want).log10();
    let offset = phase / w;
    let thdn = 20.0 * (rms / (a / 2f64.sqrt())).log10();
    (level, offset, thdn)
}

#[test]
fn sines_have_the_level_and_phase_of_the_response() {
    if report() {
        println!("| Rate | Cutoff | Sine | Level error | Alignment | THD+N |");
        println!("|---|---|---|---|---|---|");
    }
    for rate in PcmRate::ALL {
        for cutoff in Cutoff::ALL.into_iter().filter(|c| c.supports(rate)) {
            let fp = f64::from(cutoff.passband_hz());
            for (f, amp) in
                [(1000.0, 0.5), (1000.0, 0.1), (10_000.0, 0.5), (20_000.0, 0.5), (fp - 1000.0, 0.5)]
            {
                let (level, offset, thdn) = sine(rate, cutoff, f, amp);
                if report() {
                    println!(
                        "| {} | {cutoff:?} | {:.1} kHz, {:.0} dBFS | {level:+.4} dB | {offset:+.5} samples | {thdn:.1} dB |",
                        rate.hz(),
                        f / 1000.0,
                        20.0 * amp.log10()
                    );
                }
                // Above 20 kHz the stimulus itself is off, by hundredths of a dB at 40 kHz and
                // tenths at 80 kHz, and slightly in phase: a 1-bit quantiser's effective gain
                // depends on the signal, so the modulator's signal transfer is not exactly 1
                // where its noise transfer is large. The figures are reported, not checked;
                // the converter itself is checked against its filters by `design::tests`.
                if f <= 20_000.0 {
                    assert!(level.abs() < 0.005, "{rate} {cutoff:?} {f} Hz: level {level} dB");
                    assert!(offset.abs() < 0.001, "{rate} {cutoff:?} {f} Hz: offset {offset} samples");
                }
            }
        }
    }
}

/// Stepped log sweep from 20 Hz to the passband edge: the level against the analytic
/// response and the alignment, checked to 20 kHz.
#[test]
fn sweep_follows_the_response() {
    for (rate, cutoff) in [
        (PcmRate::Hz176400, Cutoff::Default),
        (PcmRate::Hz352800, Cutoff::Wide),
        (PcmRate::Hz88200, Cutoff::Low),
    ] {
        let fp = f64::from(cutoff.passband_hz());
        let steps = 16;
        let mut worst = 0.0f64;
        for i in 0..=steps {
            let f = 20.0 * (fp / 20.0).powf(f64::from(i) / f64::from(steps));
            let (level, offset, _) = sine(rate, cutoff, f, 0.5);
            // See `sines_have_the_level_and_phase_of_the_response` on figures above 20 kHz.
            if f <= 20_000.0 {
                worst = worst.max(level.abs());
                assert!(offset.abs() < 0.001, "{rate} {cutoff:?} {f} Hz: offset {offset}");
            } else if report() {
                println!(
                    "sweep {rate} {cutoff:?}: {:.1} kHz {level:+.4} dB, {offset:+.4} samples",
                    f / 1000.0
                );
            }
        }
        if report() {
            println!("sweep {rate} {cutoff:?}: largest level error to 20 kHz {worst:.5} dB");
        }
        assert!(worst < 0.005, "{rate} {cutoff:?}: {worst} dB");
    }
}

#[test]
fn dc_has_its_level_and_gain() {
    for (rate, cutoff) in [
        (PcmRate::Hz176400, Cutoff::Default),
        (PcmRate::Hz88200, Cutoff::Low),
        (PcmRate::Hz352800, Cutoff::Wide),
    ] {
        for level in [0.5, -0.5, 0.25] {
            let input = modulate(4, |_| level);
            let (y, clipped) = convert(rate, cutoff, Gain::UNITY, &input);
            let spf = rate.samples_per_frame();
            let mid = &y[spf..3 * spf];
            let mean = mid.iter().map(|&v| f64::from(v)).sum::<f64>() / mid.len() as f64;
            assert!((mean / (level * FULL) - 1.0).abs() < 1e-4, "{rate} {cutoff:?} {level}: {mean}");
            assert_eq!(clipped, 0);
            // +6.02 dB doubles the level; 0.5 then reaches full scale and clips.
            let (y2, clipped2) = convert(rate, cutoff, Gain::from_db(20.0 * 2f64.log10()).unwrap(), &input);
            let mean2 = y2[spf..3 * spf].iter().map(|&v| f64::from(v)).sum::<f64>() / mid.len() as f64;
            if level.abs() < 0.5 {
                assert!((mean2 / (2.0 * level * FULL) - 1.0).abs() < 1e-4);
            } else {
                assert!(clipped2 > 0);
            }
        }
    }
}

/// A step from mean 0 (bytes 0x55) to mean 0.5 (bytes 0x77) at DSD sample `d x n0`: settles
/// to half scale, crosses half its height at `n0`, and rings symmetrically.
#[test]
fn step_is_centred() {
    for rate in PcmRate::ALL {
        for cutoff in Cutoff::ALL.into_iter().filter(|c| c.supports(rate)) {
            let frames = 4;
            let half = frames * FRAME_BYTES_PER_CHANNEL / 2;
            let input: Vec<u8> = (0..2 * half).map(|k| if k < half { 0x55 } else { 0x77 }).collect();
            let (y, _) = convert(rate, cutoff, Gain::UNITY, &input);
            let n0 = half * 8 / rate.decimation();
            let target = 0.5 * FULL;
            let spf = rate.samples_per_frame();
            // The patterns' tones (at multiples of fs/8 and fs/4) alias to DC at the 352.8 kHz
            // stage; they are suppressed by at least 105 dB, about 47 LSB at full scale.
            assert!((f64::from(y[n0 + spf]) - target).abs() <= 48.0, "{rate} {cutoff:?}: {}", y[n0 + spf]);
            assert!(f64::from(y[n0 - spf]).abs() <= 48.0);
            assert!(f64::from(y[n0 - 1]) < target / 2.0 && f64::from(y[n0 + 1]) > target / 2.0);
            // Linear phase: the ring before the step mirrors the ring after it, up to one tap of
            // the equivalent filter (the step's first sample is on the "after" side).
            for k in 1..spf / 2 {
                let (a, b) = (f64::from(y[n0 + k]) - target, f64::from(y[n0 - k]));
                assert!((a + b).abs() <= 0.03 * target, "{rate} {cutoff:?} {k}");
            }
            if report() {
                let over = y[n0..n0 + spf].iter().copied().max().unwrap();
                println!(
                    "step {rate} {cutoff:?}: overshoot {:.2} %",
                    (f64::from(over) / target - 1.0) * 100.0
                );
            }
        }
    }
}

/// An impulse (one bit flipped in a pattern of mean 0) at DSD sample `d x n0` gives an output
/// exactly symmetric about `n0`: linear phase and alignment, bit for bit.
#[test]
fn impulse_is_exactly_symmetric() {
    for rate in PcmRate::ALL {
        for cutoff in Cutoff::ALL.into_iter().filter(|c| c.supports(rate)) {
            let mut input = vec![0x55u8; 3 * FRAME_BYTES_PER_CHANNEL];
            let byte = 3 * FRAME_BYTES_PER_CHANNEL / 2 / 4 * 4;
            // 0x55 = 01010101: the sample at the byte's MSB is 0; make it 1.
            input[byte] = 0xD5;
            let (y, _) = convert(rate, cutoff, Gain::UNITY, &input);
            let n0 = byte * 8 / rate.decimation();
            assert_eq!(n0 * rate.decimation(), byte * 8);
            for k in 1..500 {
                assert_eq!(y[n0 + k], y[n0 - k], "{rate} {cutoff:?} {k}");
            }
            assert!(y[n0] > y[n0 + 1]);
        }
    }
}

/// 100 % modulation: all ones is +2^23, clamped to 2^23 - 1 and counted; all zeros is -2^23,
/// in range.
#[test]
fn full_scale() {
    for rate in PcmRate::ALL {
        let frame = vec![0xFFu8; FRAME_BYTES_PER_CHANNEL];
        let mut conv = Converter::new(PcmConfig::new(rate, Cutoff::Default, 1)).unwrap();
        let mut out = vec![0i32; conv.samples_per_frame()];
        let run = FrameRun { before: Some(&frame), frames: &frame, after: Some(&frame) };
        let stats = conv.convert_run(run, &mut out).unwrap();
        assert!(out.iter().all(|&v| v == 8_388_607));
        assert_eq!(stats.clipped[0], out.len() as u64);
        let zeros = vec![0u8; FRAME_BYTES_PER_CHANNEL];
        let run = FrameRun { before: Some(&zeros), frames: &zeros, after: Some(&zeros) };
        let stats = conv.convert_run(run, &mut out).unwrap();
        assert!(out.iter().all(|&v| v == -8_388_608));
        assert_eq!(stats.clipped_total(), 0);
    }
}
