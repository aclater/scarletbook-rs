// SPDX-License-Identifier: MIT OR Apache-2.0

//! A direct implementation of the conversion in `i64` integer arithmetic (test-only): each
//! stage over a whole area, sample by sample, from the tables, with no lookup tables,
//! blocks, chunks or `f64`. The converter must match it exactly; it is also the fallback
//! should a future stage ever fail the 2^53 bound of `kernel` (`design::tests`).

use crate::{Cutoff, FRAME_BYTES_PER_CHANNEL, PcmConfig, PcmRate, tables};

/// The whole area's output for `frames` (interleaved layout, `config.channels` channels).
pub(crate) fn convert_area(config: PcmConfig, frames: &[u8]) -> Vec<i32> {
    let ch = usize::from(config.channels);
    let samples = frames.len() / ch * 8;
    let spf = config.rate.samples_per_frame();
    let n_out = samples / FRAME_BYTES_PER_CHANNEL / 8 * spf;
    let mut out = vec![0i32; n_out * ch];
    for c in 0..ch {
        // +1 / -1 inside the area; 0 outside.
        let x = |s: i64| -> i64 {
            if s < 0 || s >= samples as i64 {
                return 0;
            }
            let byte = frames[(s as usize / 8) * ch + c];
            if (byte >> (7 - s % 8)) & 1 == 1 { 1 } else { -1 }
        };
        // Margin of intermediate samples computed beyond the area on each side.
        let margin = 4000i64;
        let s1_taps: &[i32] = if config.cutoff == Cutoff::Wide { &tables::S1_WIDE } else { &tables::S1 };
        // S1 on the 352.8 kHz grid, m in [-margin, samples / 8 + margin).
        let m0 = -margin;
        let s1: Vec<i64> =
            (m0..samples as i64 / 8 + margin).map(|m| sym(s1_taps, |t| x(8 * m + t))).collect();
        let at = |v: &Vec<i64>, start: i64, i: i64| -> i64 {
            usize::try_from(i - start).ok().and_then(|k| v.get(k).copied()).expect("margin")
        };
        let y: Vec<i64> = if config.cutoff == Cutoff::Wide {
            // 80 kHz filter at 352.8 kHz; 176.4 kHz takes every other sample.
            let step = if config.rate == PcmRate::Hz176400 { 2 } else { 1 };
            (0..n_out as i64)
                .map(|n| div_round(sym(&tables::F40, |t| at(&s1, m0, step * n + t)), 24))
                .collect()
        } else {
            // Halfband to 176.4 kHz, u in [-margin/4, ...).
            let u0 = -margin / 4;
            let v: Vec<i64> = (u0..samples as i64 / 16 + margin / 4)
                .map(|u| div_round(sym(&tables::HB, |t| at(&s1, m0, 2 * u + t)), 24))
                .collect();
            let f: &[i32] = if config.cutoff == Cutoff::Low { &tables::F30 } else { &tables::F40 };
            let w = |u: i64| div_round(sym(f, |t| at(&v, u0, u + t)), 24);
            match config.rate {
                PcmRate::Hz88200 => (0..n_out as i64).map(|n| w(2 * n)).collect(),
                PcmRate::Hz176400 => (0..n_out as i64).map(w).collect(),
                PcmRate::Hz352800 => (0..n_out as i64)
                    .map(|q| {
                        if q % 2 == 0 {
                            w(q / 2)
                        } else {
                            // Zero-stuffed w filtered by 2 x HB: only odd taps meet samples.
                            let u = q / 2;
                            let s: i64 = (0..tables::HB.len() / 2)
                                .map(|j| {
                                    i64::from(tables::HB[2 * j + 1]) * (w(u - j as i64) + w(u + 1 + j as i64))
                                })
                                .sum();
                            div_round(s, 23)
                        }
                    })
                    .collect(),
            }
        };
        for (n, &v) in y.iter().enumerate() {
            let r = ((v as f64 * config.gain.linear()) * 0.125).round_ties_even();
            out[n * ch + c] = r.clamp(-8_388_608.0, 8_388_607.0) as i32;
        }
    }
    out
}

/// `sum_t h[|t|] x(t)` over the symmetric filter `half` (taps from the centre out).
fn sym(half: &[i32], x: impl Fn(i64) -> i64) -> i64 {
    let m = half.len() as i64 - 1;
    (-m..=m).map(|t| i64::from(half[t.unsigned_abs() as usize]) * x(t)).sum()
}

/// `v / 2^q` rounded half to even.
fn div_round(v: i64, q: u32) -> i64 {
    let floor = v >> q;
    let rem = v - (floor << q);
    let half = 1i64 << (q - 1);
    if rem > half || (rem == half && floor & 1 == 1) { floor + 1 } else { floor }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{Converter, FrameRun, Gain};

    /// Bytes of a test signal: a slow sine through a first-order sigma-delta loop, with
    /// some random bytes, so that every table entry and large intermediate values occur.
    pub(crate) fn signal(seed: u64, channels: usize, frames: usize) -> Vec<u8> {
        let mut state = seed | 1;
        let mut rnd = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let n = frames * FRAME_BYTES_PER_CHANNEL;
        let mut out = vec![0u8; n * channels];
        for c in 0..channels {
            let mut acc = 0.0f64;
            let w = 0.0007 * (c + 1) as f64;
            for k in 0..n {
                let mut byte = 0u8;
                if rnd() % 16 == 0 {
                    byte = rnd() as u8;
                } else {
                    for b in 0..8 {
                        let s = 8 * k + b;
                        let v = 0.9 * (w * s as f64).sin();
                        let bit = acc < v;
                        acc += if bit { 1.0 - v } else { -1.0 - v };
                        byte |= u8::from(bit) << (7 - b);
                    }
                }
                out[k * channels + c] = byte;
            }
        }
        out
    }

    #[test]
    fn converter_matches_integer_reference() {
        for rate in PcmRate::ALL {
            for cutoff in Cutoff::ALL {
                if !cutoff.supports(rate) {
                    continue;
                }
                for (channels, frames, gain) in [(1u8, 1usize, 0.0), (2, 3, 0.0), (3, 6, 7.5), (6, 2, -3.0)] {
                    let config =
                        PcmConfig::new(rate, cutoff, channels).with_gain(Gain::from_db(gain).unwrap());
                    let input =
                        signal(u64::from(channels) * 77 + frames as u64, usize::from(channels), frames);
                    let want = convert_area(config, &input);
                    let mut conv = Converter::new(config).unwrap();
                    let mut got = vec![0i32; want.len()];
                    conv.convert_run(FrameRun { before: None, frames: &input, after: None }, &mut got)
                        .unwrap();
                    let bad = got.iter().zip(&want).position(|(a, b)| a != b);
                    assert_eq!(bad, None, "{rate} {cutoff:?} {channels} ch {frames} frames");
                }
            }
        }
    }
}
