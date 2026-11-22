// SPDX-License-Identifier: MIT OR Apache-2.0

//! The filter kernels' instruction set, chosen at run time.
//!
//! On x86-64 with AVX2 and FMA, the FIR kernels run as a build of the same safe code with
//! those features enabled (`#[target_feature]`), and multiply-adds are fused. That changes
//! no output: every product and partial sum is an exact integer below 2^53 (`kernel`), so
//! fused and separate roundings are both exact. So do the loops around them that the
//! compiler vectorises only with AVX2 (issue #216): taking one channel out of the
//! interleaved input, rounding to 24 bits and counting the clipped samples, and interleaving
//! the output. Those are the same IEEE-754 and integer operations, one sample at a time, in
//! either build (the compiler never fuses a multiplication and an addition on its own), so
//! their results are too. Everywhere else they all run as compiled.
//!
//! This is the only module of the crate with `unsafe` (CONTRIBUTING): calling a
//! `#[target_feature]` function is `unsafe` unless the caller has the same features (Rust
//! 1.86; before it, such functions must be `unsafe fn`). Each call is made only with an
//! [`Isa`] that [`Isa::detect`] built after `is_x86_feature_detected!` found both features.
//! The wrappers hold no other unsafe operation: no intrinsics, no raw pointers. The
//! exception is to be revisited under #199, for a safe alternative such as struct target
//! features (RFC 3525) or per-CPU builds.
//!
//! Under Miri, `is_x86_feature_detected!` reports only the features the build enabled, so
//! a plain `cargo miri test` runs the generic kernels on both sides of the test below and
//! never reaches the wrappers. The `pcm-dispatch-avx2` slice (`miri.yml`, main and nightly)
//! builds with the features, which makes Miri run all six wrappers.
//!
//! That slice checks the code behind the wrappers, not their safety contract. With the
//! features in the build, Miri accepts a call made without detection; it reports such a call
//! as undefined behaviour only in a build without them, and this crate's kernels have no such
//! Miri slice. What stands behind the contract is the type: only [`Isa::detect`] can make the
//! fast [`Isa`]. Miri also interprets the Rust code, not the AVX2 machine code: the native
//! tests check what the compiler builds.

#![allow(unsafe_code)]

use crate::PcmOut;
use crate::kernel::{self, Sym};

/// Outputs per block in the generic kernels: 4 vectors of SSE2, or 2 of AVX.
const GENERIC_LANES: usize = 16;
/// Outputs per block of the generic Fir1 kernel. On 32-bit x86, whose SSE2 has 8 vector
/// registers rather than 16, 8 outputs (4 vectors), so that its sums and the operands of one
/// step stay in registers instead of spilling (#372). Dec2 and Interp2 keep 16 there: Dec2,
/// with two input sequences per step, measured slower with 8. The output is the same either
/// way: each output has its own sum, over the taps in the same order, whatever the block
/// (`kernel::add_pairs`).
#[cfg(target_arch = "x86")]
const FIR1_LANES: usize = 8;
#[cfg(not(target_arch = "x86"))]
const FIR1_LANES: usize = GENERIC_LANES;
/// Outputs per block with AVX2 and FMA: 8 independent multiply-add chains, enough to cover
/// the latency of FMA.
#[cfg(target_arch = "x86_64")]
const AVX2_LANES: usize = 32;

/// The instruction set the kernels use; only [`Isa::detect`] and [`Isa::generic`] make one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Isa(Level);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Level {
    Generic,
    #[cfg(target_arch = "x86_64")]
    Avx2Fma,
}

impl Isa {
    /// The best instruction set this CPU has.
    pub(crate) fn detect() -> Isa {
        #[cfg(target_arch = "x86_64")]
        if std::arch::is_x86_feature_detected!("avx2") && std::arch::is_x86_feature_detected!("fma") {
            return Isa(Level::Avx2Fma);
        }
        Isa::generic()
    }

    /// The kernels as compiled, without run-time features.
    pub(crate) fn generic() -> Isa {
        Isa(Level::Generic)
    }
}

/// [`kernel::fir1`].
pub(crate) fn fir1(isa: Isa, s: &Sym, x: &[f64], out: &mut [f64]) {
    match isa.0 {
        // SAFETY: `Level::Avx2Fma` is made only by `Isa::detect`, after
        // `is_x86_feature_detected!` found AVX2 and FMA.
        #[cfg(target_arch = "x86_64")]
        Level::Avx2Fma => unsafe { avx2::fir1(s, x, out) },
        Level::Generic => kernel::fir1::<FIR1_LANES, false>(s, x, out),
    }
}

/// [`kernel::dec2`].
pub(crate) fn dec2(isa: Isa, s: &Sym, xe: &[f64], xo: &[f64], out: &mut [f64]) {
    match isa.0 {
        // SAFETY: `Level::Avx2Fma` is made only by `Isa::detect`, after
        // `is_x86_feature_detected!` found AVX2 and FMA.
        #[cfg(target_arch = "x86_64")]
        Level::Avx2Fma => unsafe { avx2::dec2(s, xe, xo, out) },
        Level::Generic => kernel::dec2::<GENERIC_LANES, false>(s, xe, xo, out),
    }
}

/// [`kernel::interp2_odd`].
pub(crate) fn interp2_odd(isa: Isa, s: &Sym, w: &[f64], out: &mut [f64]) {
    match isa.0 {
        // SAFETY: `Level::Avx2Fma` is made only by `Isa::detect`, after
        // `is_x86_feature_detected!` found AVX2 and FMA.
        #[cfg(target_arch = "x86_64")]
        Level::Avx2Fma => unsafe { avx2::interp2_odd(s, w, out) },
        Level::Generic => kernel::interp2_odd::<GENERIC_LANES, false>(s, w, out),
    }
}

/// [`kernel::pick`].
pub(crate) fn pick(isa: Isa, dst: &mut [u8], src: &[u8], ch: usize, c: usize) {
    match isa.0 {
        // SAFETY: `Level::Avx2Fma` is made only by `Isa::detect`, after
        // `is_x86_feature_detected!` found AVX2 and FMA.
        #[cfg(target_arch = "x86_64")]
        Level::Avx2Fma => unsafe { avx2::pick(dst, src, ch, c) },
        Level::Generic => kernel::pick(dst, src, ch, c),
    }
}

/// [`kernel::quantise`].
pub(crate) fn quantise(isa: Isa, v: &[f64], gain: f64, q: &mut [i32]) -> u64 {
    match isa.0 {
        // SAFETY: `Level::Avx2Fma` is made only by `Isa::detect`, after
        // `is_x86_feature_detected!` found AVX2 and FMA.
        #[cfg(target_arch = "x86_64")]
        Level::Avx2Fma => unsafe { avx2::quantise(v, gain, q) },
        Level::Generic => kernel::quantise(v, gain, q),
    }
}

/// [`kernel::interleave`].
pub(crate) fn interleave(isa: Isa, q: &[i32], ch: usize, out: &mut PcmOut<'_>, at: usize) {
    match isa.0 {
        // SAFETY: `Level::Avx2Fma` is made only by `Isa::detect`, after
        // `is_x86_feature_detected!` found AVX2 and FMA.
        #[cfg(target_arch = "x86_64")]
        Level::Avx2Fma => unsafe { avx2::interleave(q, ch, out, at) },
        Level::Generic => kernel::interleave(q, ch, out, at),
    }
}

/// The kernels built with AVX2 and FMA. Each is the safe kernel, with fused multiply-adds;
/// callers must have checked that the CPU has both features.
#[cfg(target_arch = "x86_64")]
mod avx2 {
    use super::AVX2_LANES;
    use crate::PcmOut;
    use crate::kernel::{self, Sym};

    #[target_feature(enable = "avx2,fma")]
    pub(super) unsafe fn fir1(s: &Sym, x: &[f64], out: &mut [f64]) {
        kernel::fir1::<AVX2_LANES, true>(s, x, out)
    }

    #[target_feature(enable = "avx2,fma")]
    pub(super) unsafe fn pick(dst: &mut [u8], src: &[u8], ch: usize, c: usize) {
        kernel::pick(dst, src, ch, c)
    }

    #[target_feature(enable = "avx2,fma")]
    pub(super) unsafe fn quantise(v: &[f64], gain: f64, q: &mut [i32]) -> u64 {
        kernel::quantise(v, gain, q)
    }

    #[target_feature(enable = "avx2,fma")]
    pub(super) unsafe fn interleave(q: &[i32], ch: usize, out: &mut PcmOut<'_>, at: usize) {
        kernel::interleave(q, ch, out, at)
    }

    #[target_feature(enable = "avx2,fma")]
    pub(super) unsafe fn dec2(s: &Sym, xe: &[f64], xo: &[f64], out: &mut [f64]) {
        kernel::dec2::<AVX2_LANES, true>(s, xe, xo, out)
    }

    #[target_feature(enable = "avx2,fma")]
    pub(super) unsafe fn interp2_odd(s: &Sym, w: &[f64], out: &mut [f64]) {
        kernel::interp2_odd::<AVX2_LANES, true>(s, w, out)
    }
}

/// PROTOTYPE (#372 item 3, not for merge): [`crate::proto_int`]'s SSE2 Fir1.
#[cfg(all(
    feature = "proto-int-372",
    any(target_arch = "x86", target_arch = "x86_64"),
    target_feature = "sse2"
))]
pub(crate) fn proto_fir1_int<const G: usize>(s: &crate::proto_int::IntSym, x: &[i16], out: &mut [i32]) {
    // SAFETY: SSE2 is enabled for the whole build (the `cfg` above), so every CPU this code
    // runs on has it.
    unsafe { proto::fir1::<G>(s, x, out) }
}

/// PROTOTYPE (#372 item 3, not for merge): [`crate::proto_int`]'s SSE2 Dec2.
#[cfg(all(
    feature = "proto-int-372",
    any(target_arch = "x86", target_arch = "x86_64"),
    target_feature = "sse2"
))]
pub(crate) fn proto_dec2_int<const G: usize>(
    s: &crate::proto_int::IntSym,
    xe: &[i16],
    xo: &[i16],
    out: &mut [i32],
) {
    // SAFETY: SSE2 is enabled for the whole build (the `cfg` above), so every CPU this code
    // runs on has it.
    unsafe { proto::dec2::<G>(s, xe, xo, out) }
}

/// PROTOTYPE wrappers: the SSE2 bodies in `proto_int`.
#[cfg(all(
    feature = "proto-int-372",
    any(target_arch = "x86", target_arch = "x86_64"),
    target_feature = "sse2"
))]
mod proto {
    use crate::proto_int::{IntSym, sse2};

    #[target_feature(enable = "sse2")]
    pub(super) unsafe fn fir1<const G: usize>(s: &IntSym, x: &[i16], out: &mut [i32]) {
        sse2::fir1::<G>(s, x, out)
    }

    #[target_feature(enable = "sse2")]
    pub(super) unsafe fn dec2<const G: usize>(s: &IntSym, xe: &[i16], xo: &[i16], out: &mut [i32]) {
        sse2::dec2::<G>(s, xe, xo, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::tests::signal;
    use crate::{
        Converter, Cutoff, FRAME_BYTES_PER_CHANNEL, FrameRun, Frames, Gain, PcmConfig, PcmRate, Run,
    };

    /// [`Isa::detect`] picks the wrappers exactly on a CPU with AVX2 and FMA. The bit-identity
    /// test below can't tell: it would still pass if detection always gave the generic level.
    /// (Under Miri, the features are those the build enabled.)
    #[test]
    fn detect_picks_the_fast_level_on_a_capable_cpu() {
        #[cfg(target_arch = "x86_64")]
        let want =
            if std::arch::is_x86_feature_detected!("avx2") && std::arch::is_x86_feature_detected!("fma") {
                Level::Avx2Fma
            } else {
                Level::Generic
            };
        #[cfg(not(target_arch = "x86_64"))]
        let want = Level::Generic;
        assert_eq!(Isa::detect(), Isa(want));
    }

    /// A new converter runs with the detected instruction set, not the generic one.
    #[test]
    #[cfg_attr(miri, ignore = "builds a filter table; see detect_picks_the_fast_level_on_a_capable_cpu")]
    fn new_converter_uses_the_detected_isa() {
        let config = PcmConfig::new(PcmRate::Hz176400, Cutoff::Standard, 2);
        assert_eq!(Converter::new(config).unwrap().plan.isa(), Isa::detect());
    }

    /// A digest (FNV-1a, 64 bits) of the 32-bit samples and statistics of conversions with
    /// the detected kernels, for every rate and cutoff, 1, 2 and 6 channels and three gains
    /// (0 dB, +7.5 dB, which clips, and -3 dB), of inputs chosen to stress the arithmetic:
    /// full-scale ones (all 0xFF), long full-scale swings (blocks of 0x00 and 0xFF), the idle
    /// pattern, and pseudo-random bytes. It is fixed, so every target that runs it converts
    /// them exactly as x86-64 does: CI runs it on i686 as well (#372). The inputs come from
    /// integer arithmetic only, never from the platform's maths library.
    #[test]
    #[cfg_attr(miri, ignore = "converts too much for Miri; see lane_counts_and_fma_give_the_same_sums")]
    fn converted_digest_is_fixed() {
        let mut h = 0xCBF2_9CE4_8422_2325u64;
        let mut eat = |bytes: &[u8]| {
            for &b in bytes {
                h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01B3);
            }
        };
        let mut conversions = 0;
        let frames = 3;
        for rate in PcmRate::ALL {
            for &cutoff in Cutoff::ALL.iter().filter(|c| c.supports(rate)) {
                for channels in [1u8, 2, 6] {
                    let n = usize::from(channels) * frames * FRAME_BYTES_PER_CHANNEL;
                    let mut seed = 0x9E37_79B9_7F4A_7C15u64 ^ u64::from(channels);
                    let inputs: [Vec<u8>; 4] = [
                        vec![0xFF; n],
                        (0..n).map(|i| if (i / 3001) % 2 == 0 { 0x00 } else { 0xFF }).collect(),
                        vec![0x69; n],
                        (0..n)
                            .map(|_| {
                                seed ^= seed << 13;
                                seed ^= seed >> 7;
                                seed ^= seed << 17;
                                seed as u8
                            })
                            .collect(),
                    ];
                    for input in &inputs {
                        for db in [0.0, 7.5, -3.0] {
                            let config =
                                PcmConfig::new(rate, cutoff, channels).with_gain(Gain::from_db(db).unwrap());
                            let mut conv = Converter::new(config).unwrap();
                            let mut out =
                                vec![0i32; frames * conv.samples_per_frame() * usize::from(channels)];
                            let stats = conv.convert_run(FrameRun::new(input), &mut out).unwrap();
                            eat(format!("{stats:?}").as_bytes());
                            for v in &out {
                                eat(&v.to_le_bytes());
                            }
                            conversions += 1;
                        }
                    }
                }
            }
        }
        assert_eq!((conversions, format!("{h:016x}")), (288, "54c3ced461085e03".to_string()));
    }

    /// The kernels chosen for this CPU give exactly the output of the generic kernels, in
    /// every configuration, for a whole run and for a frame with neighbours, in both sample
    /// layouts and with the same clip counts. (On a CPU without AVX2 and FMA, and under Miri
    /// unless it is built with `-C target-feature=+avx2,+fma` as `pcm-dispatch-avx2` is, both
    /// are the generic kernels. Under Miri, only `Default` is converted, which still runs
    /// every kernel.)
    #[test]
    fn detected_kernels_match_generic() {
        let cases: &[(u8, usize, f64)] =
            if cfg!(miri) { &[(2, 1, 0.0)] } else { &[(1, 3, 0.0), (2, 3, 0.0), (3, 3, 7.5), (6, 3, -3.0)] };
        for rate in PcmRate::ALL {
            let cutoffs = if cfg!(miri) { &[Cutoff::Standard][..] } else { &Cutoff::ALL[..] };
            for &cutoff in cutoffs.iter().filter(|c| c.supports(rate)) {
                for &(channels, frames, gain) in cases {
                    let config =
                        PcmConfig::new(rate, cutoff, channels).with_gain(Gain::from_db(gain).unwrap());
                    let ch = usize::from(channels);
                    let input = signal(u64::from(channels) * 31 + 5, ch, frames);
                    let frame = ch * FRAME_BYTES_PER_CHANNEL;
                    // `i32` samples from one buffer; 3-byte samples and per-frame clip counts
                    // from separate frames.
                    let run = |isa: Isa, run: FrameRun<'_>, frames: usize| {
                        let mut conv = Converter::new(config).unwrap();
                        conv.plan.set_isa(isa);
                        let mut out = vec![0i32; frames * conv.samples_per_frame() * ch];
                        let stats = conv.convert_run(run, &mut out).unwrap();
                        let list: Vec<&[u8]> = run.frames.chunks_exact(frame).collect();
                        let separate =
                            Run::new(Frames::Separate(&list)).with_before(run.before).with_after(run.after);
                        let mut wav = vec![0u8; 3 * out.len()];
                        let mut counts = vec![0u64; frames];
                        let stats24 =
                            conv.convert(separate, PcmOut::S24le(&mut wav), Some(&mut counts)).unwrap();
                        (out, stats, wav, stats24, counts)
                    };
                    let whole = FrameRun::new(&input);
                    let what = format!("{rate} {cutoff:?} {channels} ch, {:?}", Isa::detect());
                    assert_eq!(
                        run(Isa::detect(), whole, frames),
                        run(Isa::generic(), whole, frames),
                        "{what}"
                    );
                    if frames >= 3 {
                        let one = FrameRun::new(&input[frame..2 * frame])
                            .with_before(Some(&input[..frame]))
                            .with_after(Some(&input[2 * frame..3 * frame]));
                        assert_eq!(
                            run(Isa::detect(), one, 1),
                            run(Isa::generic(), one, 1),
                            "{what}, one frame"
                        );
                    }
                }
            }
        }
    }
}
