// SPDX-License-Identifier: MIT OR Apache-2.0

//! The filter kernels' instruction set, chosen at run time.
//!
//! On x86-64 with AVX2 and FMA, the FIR kernels run as a build of the same safe code with
//! those features enabled (`#[target_feature]`), and multiply-adds are fused. That changes
//! no output: every product and partial sum is an exact integer below 2^53 (`kernel`), so
//! fused and separate roundings are both exact. Everywhere else they run as compiled.
//!
//! This is the only module of the crate with `unsafe` (CONTRIBUTING): calling a
//! `#[target_feature]` function is `unsafe` unless the caller has the same features (Rust
//! 1.86; before it, such functions must be `unsafe fn`). Each call is made only with an
//! [`Isa`] that [`Isa::detect`] built after `is_x86_feature_detected!` found both features.
//! The wrappers hold no other unsafe operation: no intrinsics, no raw pointers.

#![allow(unsafe_code)]

use crate::kernel::{self, Sink, Sym};

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
pub(crate) fn fir1(isa: Isa, s: &Sym, x: &[f64], n: usize, sink: &mut Sink<'_, '_>) {
    match isa.0 {
        // SAFETY: `Level::Avx2Fma` is made only by `Isa::detect`, after
        // `is_x86_feature_detected!` found AVX2 and FMA.
        #[cfg(target_arch = "x86_64")]
        Level::Avx2Fma => unsafe { avx2::fir1(s, x, n, sink) },
        Level::Generic => kernel::fir1::<GENERIC_LANES, false>(s, x, n, sink),
    }
}

/// [`kernel::dec2`].
pub(crate) fn dec2(isa: Isa, s: &Sym, xe: &[f64], xo: &[f64], n: usize, sink: &mut Sink<'_, '_>) {
    match isa.0 {
        // SAFETY: `Level::Avx2Fma` is made only by `Isa::detect`, after
        // `is_x86_feature_detected!` found AVX2 and FMA.
        #[cfg(target_arch = "x86_64")]
        Level::Avx2Fma => unsafe { avx2::dec2(s, xe, xo, n, sink) },
        Level::Generic => kernel::dec2::<GENERIC_LANES, false>(s, xe, xo, n, sink),
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

/// The kernels built with AVX2 and FMA. Each is the safe kernel, with fused multiply-adds;
/// callers must have checked that the CPU has both features.
#[cfg(target_arch = "x86_64")]
mod avx2 {
    use super::AVX2_LANES;
    use crate::kernel::{self, Sink, Sym};

    #[target_feature(enable = "avx2,fma")]
    pub(super) unsafe fn fir1(s: &Sym, x: &[f64], n: usize, sink: &mut Sink<'_, '_>) {
        kernel::fir1_reuse(s, x, n, sink)
    }

    #[target_feature(enable = "avx2,fma")]
    pub(super) unsafe fn dec2(s: &Sym, xe: &[f64], xo: &[f64], n: usize, sink: &mut Sink<'_, '_>) {
        kernel::dec2::<AVX2_LANES, true>(s, xe, xo, n, sink)
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
    use crate::{Converter, Cutoff, FRAME_BYTES_PER_CHANNEL, FrameRun, Gain, PcmConfig, PcmRate};

    /// The kernels chosen for this CPU give exactly the output of the generic kernels, in
    /// every configuration, for a whole run and for a frame with neighbours. (On a CPU
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
                    let run = |isa: Isa, run: FrameRun<'_>, frames: usize| {
                        let mut conv = Converter::new(config).unwrap();
                        conv.plan.set_isa(isa);
                        let mut out = vec![0i32; frames * conv.samples_per_frame() * ch];
                        let stats = conv.convert_run(run, &mut out).unwrap();
                        (out, stats)
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
