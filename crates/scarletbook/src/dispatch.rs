// SPDX-License-Identifier: MIT OR Apache-2.0

//! The DST decoder's instruction set, chosen at run time.
//!
//! On x86-64 with AVX2, BMI1, BMI2, LZCNT and MOVBE, a group of frames is decoded by a
//! build of the same safe decoder with those features enabled (`#[target_feature]`):
//! LZCNT and the BMI shifts shorten the arithmetic decoder's serial chain, MOVBE loads its
//! big-endian code bits, and AVX2 (with the 3-operand AVX encoding) sums the prediction
//! rows. The decoder is integer-only, so the output is the same to the bit. Everywhere
//! else it runs as compiled.
//!
//! This is the only module of the crate with `unsafe` (CONTRIBUTING): calling a
//! `#[target_feature]` function is `unsafe` unless the caller has the same features (Rust
//! 1.86; before it, such functions must be `unsafe fn`). Each call is made only with an
//! [`Isa`] that [`Isa::detect`] built after `is_x86_feature_detected!` found every feature.
//! The wrapper holds no other unsafe operation: no intrinsics, no raw pointers. The
//! exception is to be revisited under #199, for a safe alternative such as struct target
//! features (RFC 3525) or per-CPU builds.

#![allow(unsafe_code)]

use crate::dst::{DstDecoder, DstError};

/// The instruction set the decoder uses; only [`Isa::detect`] and [`Isa::generic`] make one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Isa(Level);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Level {
    Generic,
    #[cfg(target_arch = "x86_64")]
    Avx2Bmi,
}

impl Isa {
    /// The best instruction set this CPU has.
    pub(crate) fn detect() -> Isa {
        #[cfg(target_arch = "x86_64")]
        if std::arch::is_x86_feature_detected!("avx2")
            && std::arch::is_x86_feature_detected!("bmi1")
            && std::arch::is_x86_feature_detected!("bmi2")
            && std::arch::is_x86_feature_detected!("lzcnt")
            && std::arch::is_x86_feature_detected!("movbe")
        {
            return Isa(Level::Avx2Bmi);
        }
        Isa::generic()
    }

    /// The decoder as compiled, without run-time features.
    pub(crate) fn generic() -> Isa {
        Isa(Level::Generic)
    }
}

/// [`DstDecoder::group`].
pub(crate) fn decode_group(
    isa: Isa,
    dec: &mut DstDecoder,
    frames: &[&[u8]],
    channels: u8,
    outs: &mut [&mut [u8]],
    results: &mut [Result<(), DstError>],
) {
    match isa.0 {
        // SAFETY: `Level::Avx2Bmi` is made only by `Isa::detect`, after
        // `is_x86_feature_detected!` found AVX2, BMI1, BMI2, LZCNT and MOVBE.
        #[cfg(target_arch = "x86_64")]
        Level::Avx2Bmi => unsafe { avx2::decode_group(dec, frames, channels, outs, results) },
        Level::Generic => dec.group::<false>(frames, channels, outs, results),
    }
}

/// The decoder built with AVX2, BMI1, BMI2, LZCNT and MOVBE: the safe decoder, with its
/// inner loops inlined; callers must have checked that the CPU has every feature.
#[cfg(target_arch = "x86_64")]
mod avx2 {
    use crate::dst::{DstDecoder, DstError};

    #[target_feature(enable = "avx2,bmi1,bmi2,lzcnt,movbe")]
    pub(super) unsafe fn decode_group(
        dec: &mut DstDecoder,
        frames: &[&[u8]],
        channels: u8,
        outs: &mut [&mut [u8]],
        results: &mut [Result<(), DstError>],
    ) {
        dec.group::<true>(frames, channels, outs, results)
    }
}

#[cfg(test)]
#[path = "../tests/common/dst_enc.rs"]
mod dst_enc;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dst::{FL, MAX_LANES};
    use crate::{Disc, FrameContent};

    /// Up to `per_track` DST frames of every track of every corpus image, by channel count.
    fn corpus_frames(per_track: usize) -> Vec<Vec<Vec<u8>>> {
        let images = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/images");
        let mut pools = vec![Vec::new(); 7];
        let mut paths: Vec<_> = std::fs::read_dir(images).unwrap().map(|e| e.unwrap().path()).collect();
        paths.sort();
        for path in paths.iter().filter(|p| p.extension().is_some_and(|e| e == "iso")) {
            let Ok(file) = std::fs::File::open(path) else { continue };
            let Ok(mut disc) = Disc::open(std::io::BufReader::new(file)) else { continue };
            let areas: Vec<_> = disc.toc().areas.iter().map(|a| (a.slot, a.tracks.len() as u32)).collect();
            for (slot, tracks) in areas {
                for n in 1..=tracks {
                    let Ok(frames) = disc.track_frames(slot, n) else { continue };
                    let channels = usize::from(frames.channels());
                    for frame in frames.take(per_track).flatten() {
                        if let FrameContent::Dst(bytes) = frame.content {
                            pools[channels].push(bytes);
                        }
                    }
                }
            }
        }
        pools
    }

    /// Every frame of `pool` and a damaged copy of every third one, decoded in groups of 1 to
    /// [`MAX_LANES`] frames with `isa`: the results and the outputs.
    fn decode_all(isa: Isa, pool: &[Vec<u8>], channels: u8) -> Vec<(Result<(), DstError>, Vec<u8>)> {
        let mut frames: Vec<Vec<u8>> = pool.to_vec();
        for (i, f) in pool.iter().enumerate().step_by(3) {
            let mut f = f.clone();
            if i % 2 == 0 {
                f.truncate(f.len() * 2 / 3);
            } else {
                let at = (i * 7919) % f.len();
                f[at] ^= 1 << (i % 8);
            }
            frames.push(f);
        }
        let mut dec = DstDecoder::with_isa(isa);
        let mut out = Vec::new();
        let mut rest = &frames[..];
        let mut group = 1;
        while !rest.is_empty() {
            let (now, later) = rest.split_at(group.min(rest.len()));
            let refs: Vec<&[u8]> = now.iter().map(Vec::as_slice).collect();
            let mut bufs = vec![vec![0u8; FL * usize::from(channels)]; now.len()];
            let mut outs: Vec<&mut [u8]> = bufs.iter_mut().map(Vec::as_mut_slice).collect();
            let results = dec.decode_many(&refs, channels, &mut outs);
            out.extend(
                results.into_iter().zip(bufs).map(|(r, b)| (r, if r.is_ok() { b } else { Vec::new() })),
            );
            rest = later;
            group = group % MAX_LANES + 1;
        }
        out
    }

    fn assert_same(pool: &[Vec<u8>], channels: u8, what: &str) {
        let detected = decode_all(Isa::detect(), pool, channels);
        let generic = decode_all(Isa::generic(), pool, channels);
        assert_eq!(detected.len(), generic.len());
        assert!(generic.iter().any(|(r, _)| r.is_ok()), "{what}, {channels} channels: no valid frame");
        for (i, (d, g)) in detected.iter().zip(&generic).enumerate() {
            assert!(
                d == g,
                "{what}, {channels} channels, frame {i}: {:?} differs ({:?})",
                d.0,
                Isa::detect()
            );
        }
    }

    /// The decoder chosen for this CPU gives exactly the output and results of the generic
    /// decoder on the DST frames of the corpus (1 to 6 channels), on the generated stress
    /// frames of the benchmarks (2 and 6 channels), and on damaged copies of both. (On a
    /// CPU without the features, and under Miri, both are the generic decoder.)
    #[test]
    #[cfg_attr(
        miri,
        ignore = "the corpus is too large for Miri; see detected_decoder_matches_generic_simple"
    )]
    fn detected_decoder_matches_generic() {
        let pools = corpus_frames(6);
        let mut seen = 0;
        for channels in 1..=6u8 {
            let pool = &pools[usize::from(channels)];
            assert!(!pool.is_empty(), "the corpus has no {channels}-channel DST frames");
            assert_same(pool, channels, "corpus");
            seen += pool.len();
        }
        for nch in [2u8, 6] {
            let pool: Vec<Vec<u8>> =
                dst_enc::stress_set(2).into_iter().filter(|f| f.channels == nch).map(|f| f.frame).collect();
            assert!(!pool.is_empty());
            assert_same(&pool, nch, "stress");
            seen += pool.len();
        }
        assert!(seen > 100, "only {seen} frames");
    }

    /// The same on small generated frames, short enough for Miri.
    #[test]
    fn detected_decoder_matches_generic_simple() {
        let mut seed = 0x51_3C_7A_D2u64;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let coefs: Vec<i32> = (0..20).map(|i| [96, -40, 17, -5][i % 4] >> (i / 4)).collect();
        let ptable: Vec<u32> = (0..24).map(|i| 128 - 5 * i).collect();
        let channels: &[u8] = if cfg!(miri) { &[2] } else { &[1, 2, 3, 4, 5, 6] };
        for &nch in channels {
            let pool: Vec<Vec<u8>> = (0..if cfg!(miri) { 2 } else { 6 })
                .map(|k| {
                    let signal: Vec<Vec<u8>> =
                        (0..nch).map(|_| (0..FL).map(|_| rnd() as u8).collect()).collect();
                    let half: Vec<bool> = (0..nch).map(|c| (usize::from(c) + k) % 3 == 0).collect();
                    dst_enc::encode_simple(&signal, &coefs, &ptable, &half)
                })
                .collect();
            assert_same(&pool, nch, "generated");
        }
    }
}
