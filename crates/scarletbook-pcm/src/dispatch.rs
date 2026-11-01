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

#![allow(unsafe_code)]

use crate::PcmOut;
use crate::kernel::{self, Sym};

/// Outputs per block in the generic kernels: 4 vectors of SSE2, or 2 of AVX.
const GENERIC_LANES: usize = 16;
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
        Level::Generic => kernel::fir1::<GENERIC_LANES, false>(s, x, out),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::tests::signal;
    use crate::{
        Converter, Cutoff, FRAME_BYTES_PER_CHANNEL, FrameRun, Frames, Gain, PcmConfig, PcmRate, Run,
    };

    /// The kernels chosen for this CPU give exactly the output of the generic kernels, in
    /// every configuration, for a whole run and for a frame with neighbours, in both sample
    /// layouts and with the same clip counts. (On a CPU
    /// without AVX2 and FMA, and under Miri, both are the generic kernels.)
    #[test]
    fn detected_kernels_match_generic() {
        let cases: &[(u8, usize, f64)] =
            if cfg!(miri) { &[(2, 1, 0.0)] } else { &[(1, 3, 0.0), (2, 3, 0.0), (3, 3, 7.5), (6, 3, -3.0)] };
        for rate in PcmRate::ALL {
            for cutoff in Cutoff::ALL.into_iter().filter(|c| c.supports(rate)) {
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
