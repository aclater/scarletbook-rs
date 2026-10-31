// SPDX-License-Identifier: MIT OR Apache-2.0

//! DSD64 to 24-bit PCM conversion for Super Audio CD audio, deterministic per frame.
//!
//! The input is 1-bit audio at 2 822 400 Hz in Super Audio CD frames: 4704 bytes per channel
//! per frame (1/75 s), channels interleaved byte by byte (byte `k x C + c` is byte `k` of
//! channel `c`), the earliest sample of each byte in its most significant bit (sacd-spec,
//! specification 0, 0.6). A 1 bit is +1 and a 0 bit is -1. The output is 24-bit PCM at
//! 88 200, 176 400 or 352 800 Hz, interleaved, as sign-extended `i32`
//! ([`pack_s24le`] packs it into the 3-byte little-endian layout of WAV and FLAC).
//!
//! ```
//! use scarletbook_pcm::{Converter, Cutoff, FrameRun, PcmConfig, PcmRate, FRAME_BYTES_PER_CHANNEL};
//!
//! # fn main() -> Result<(), scarletbook_pcm::PcmError> {
//! let mut conv = Converter::new(PcmConfig::new(PcmRate::Hz176400, Cutoff::Default, 2))?;
//! // Three frames of a 2-channel area, the whole area: no neighbours.
//! let frames = vec![0x69u8; 3 * 2 * FRAME_BYTES_PER_CHANNEL];
//! let mut pcm = vec![0i32; 3 * conv.samples_per_frame() * 2];
//! conv.convert_run(FrameRun { before: None, frames: &frames, after: None }, &mut pcm)?;
//! # Ok(())
//! # }
//! ```
//!
//! # Filters
//!
//! | [`Cutoff`] | Passband | Stopband from | Rates |
//! |---|---|---|---|
//! | `Default` | 40 kHz | 48 kHz | all three |
//! | `Low` | 30 kHz | 38 kHz | all three |
//! | `Wide` | 80 kHz | 96 kHz | 176.4 and 352.8 kHz |
//!
//! Every filter attenuates everything at and above its stopband edge by at least 105 dB,
//! aliases included, has a passband ripple within +-0.01 dB, and has linear phase. Output
//! sample `n` is centred exactly on DSD sample `d x n` of the area (`d` = 32, 16 or 8), so
//! there is no delay to compensate. The crate README publishes the measured response.
//!
//! The 88.2 kHz output is the 176.4 kHz output with every other sample dropped, and the
//! 352.8 kHz output of `Default` and `Low` is the 176.4 kHz output interpolated by 2 (its
//! even samples are the 176.4 kHz samples); `Wide` at 176.4 kHz is its 352.8 kHz output
//! with every other sample dropped. So each cutoff has one frequency response at all rates.
//!
//! # Determinism
//!
//! Every output sample is an exact function of the input bits around it: the filters use
//! integer coefficients and integer arithmetic with one rounding rule, so the result does
//! not depend on the CPU, on how the input is split into calls, or on threads. Converting
//! frame `f` with frames `f - 1` and `f + 1` as neighbours ([`Converter::convert_frame`])
//! gives exactly the bytes of frame `f` in a conversion of the whole area; so does any run
//! of frames with its two neighbours ([`Converter::convert_run`]). A neighbour of `None`
//! marks the edge of the area: the input beyond it is silence (the value 0, not a bit
//! pattern). Only the [`Converter::context_bytes`] bytes per channel of each neighbour
//! nearest the frame are used.
//!
//! # Level
//!
//! 100 % modulation is full scale: a constant input of +1 (all bits 1) gives +1.0, that is
//! 2^23, which is clamped to 2^23 - 1 and reported as a clipped sample. Super Audio CD
//! content stays far below 100 % modulation, so clipped samples mean that the [`Gain`] is
//! too high. Output samples are rounded half to even and are not dithered.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

#[cfg(test)]
mod design;
mod kernel;
#[cfg(test)]
mod quality;
#[cfg(test)]
mod reference;
mod tables;

use std::fmt;

use kernel::Plan;

/// Bytes of one channel in one frame: 37 632 one-bit samples.
pub const FRAME_BYTES_PER_CHANNEL: usize = 4704;

/// The highest channel count of an area.
pub const MAX_CHANNELS: u8 = 6;

/// An output sample rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PcmRate {
    /// 88 200 Hz: 1176 samples per channel per frame.
    Hz88200,
    /// 176 400 Hz: 2352 samples per channel per frame.
    Hz176400,
    /// 352 800 Hz: 4704 samples per channel per frame.
    Hz352800,
}

impl PcmRate {
    /// Every rate.
    pub const ALL: [PcmRate; 3] = [PcmRate::Hz88200, PcmRate::Hz176400, PcmRate::Hz352800];

    /// The rate in Hz.
    pub fn hz(self) -> u32 {
        match self {
            PcmRate::Hz88200 => 88_200,
            PcmRate::Hz176400 => 176_400,
            PcmRate::Hz352800 => 352_800,
        }
    }

    /// The rate of the given value in Hz, if it is one of the supported rates.
    pub fn from_hz(hz: u32) -> Option<PcmRate> {
        PcmRate::ALL.into_iter().find(|r| r.hz() == hz)
    }

    /// DSD samples per output sample (32, 16 or 8).
    pub fn decimation(self) -> usize {
        (2_822_400 / self.hz()) as usize
    }

    /// Output samples per channel per frame.
    pub fn samples_per_frame(self) -> usize {
        FRAME_BYTES_PER_CHANNEL * 8 / self.decimation()
    }
}

impl fmt::Display for PcmRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} Hz", self.hz())
    }
}

/// The low-pass filter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Cutoff {
    /// Passband to 40 kHz, stopband from 48 kHz.
    #[default]
    Default,
    /// Passband to 30 kHz, stopband from 38 kHz.
    Low,
    /// Passband to 80 kHz, stopband from 96 kHz; 176.4 and 352.8 kHz only.
    Wide,
}

impl Cutoff {
    /// Every cutoff.
    pub const ALL: [Cutoff; 3] = [Cutoff::Default, Cutoff::Low, Cutoff::Wide];

    /// The passband edge in Hz.
    pub fn passband_hz(self) -> u32 {
        match self {
            Cutoff::Default => 40_000,
            Cutoff::Low => 30_000,
            Cutoff::Wide => 80_000,
        }
    }

    /// The stopband edge in Hz.
    pub fn stopband_hz(self) -> u32 {
        match self {
            Cutoff::Default => 48_000,
            Cutoff::Low => 38_000,
            Cutoff::Wide => 96_000,
        }
    }

    /// Whether the cutoff is available at `rate` (`Wide` is not at 88.2 kHz).
    pub fn supports(self, rate: PcmRate) -> bool {
        !(self == Cutoff::Wide && rate == PcmRate::Hz88200)
    }
}

/// A gain applied before rounding, from -120 to +24 dB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gain {
    db: f64,
    linear: f64,
}

impl Gain {
    /// 0 dB.
    pub const UNITY: Gain = Gain { db: 0.0, linear: 1.0 };
    /// The lowest gain, in dB.
    pub const MIN_DB: f64 = -120.0;
    /// The highest gain, in dB.
    pub const MAX_DB: f64 = 24.0;

    /// The gain of `db` decibels (amplitude, `10^(db / 20)`).
    ///
    /// The conversion uses only IEEE-754 additions, multiplications and divisions in a fixed
    /// order (no platform `exp` or `pow`), so a given `db` gives the same factor, and the same
    /// output samples, on every platform. It is monotonic, and exactly 1 at 0 dB.
    pub fn from_db(db: f64) -> Result<Gain, PcmError> {
        if !db.is_finite() || !(Self::MIN_DB..=Self::MAX_DB).contains(&db) {
            return Err(PcmError::Gain(db));
        }
        Ok(Gain { db, linear: kernel::db_to_linear(db) })
    }

    /// The gain in dB, as given.
    pub fn db(self) -> f64 {
        self.db
    }

    /// The linear factor.
    pub fn linear(self) -> f64 {
        self.linear
    }
}

impl Default for Gain {
    fn default() -> Gain {
        Gain::UNITY
    }
}

/// What a [`Converter`] produces.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct PcmConfig {
    /// The output rate.
    pub rate: PcmRate,
    /// The low-pass filter.
    pub cutoff: Cutoff,
    /// Channels of the input frames, 1 to 6.
    pub channels: u8,
    /// The gain; [`Gain::UNITY`] by default.
    pub gain: Gain,
}

impl PcmConfig {
    /// A configuration with unity gain.
    pub fn new(rate: PcmRate, cutoff: Cutoff, channels: u8) -> PcmConfig {
        PcmConfig { rate, cutoff, channels, gain: Gain::UNITY }
    }

    /// The same configuration with gain `gain`.
    pub fn with_gain(self, gain: Gain) -> PcmConfig {
        PcmConfig { gain, ..self }
    }
}

/// A conversion error. Nothing is written to the output when one is returned.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum PcmError {
    /// The cutoff is not available at the rate (`Wide` at 88.2 kHz).
    #[error("the {cutoff:?} filter is not available at {rate}")]
    Unsupported {
        /// The rate asked for.
        rate: PcmRate,
        /// The cutoff asked for.
        cutoff: Cutoff,
    },
    /// The channel count is not 1 to 6.
    #[error("{0} channels: 1 to 6 are supported")]
    Channels(u8),
    /// The gain is not finite or is outside -120 to +24 dB.
    #[error("gain {0} dB is outside -120 to +24 dB")]
    Gain(f64),
    /// An input is not a whole number of frames of the converter's channel count (a
    /// neighbour must be exactly one frame).
    #[error("input of {len} bytes is not {what}")]
    InputLength {
        /// Bytes given.
        len: usize,
        /// What was expected.
        what: &'static str,
    },
    /// The output buffer does not have exactly the samples of the input frames.
    #[error("output of {len} samples, {expected} expected")]
    OutputLength {
        /// Samples given.
        len: usize,
        /// Samples the input converts to.
        expected: usize,
    },
}

/// Counts from one conversion call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ConvertStats {
    /// Samples clamped to the 24-bit range, per channel.
    pub clipped: [u64; MAX_CHANNELS as usize],
}

impl ConvertStats {
    /// Clamped samples over all channels.
    pub fn clipped_total(&self) -> u64 {
        self.clipped.iter().sum()
    }
}

/// A run of consecutive frames of one area and the frames around it.
#[derive(Clone, Copy, Debug)]
pub struct FrameRun<'a> {
    /// The frame before the run, or `None` if the run starts the area.
    pub before: Option<&'a [u8]>,
    /// The frames to convert, `C x 4704` bytes each, in interleaved layout.
    pub frames: &'a [u8],
    /// The frame after the run, or `None` if the run ends the area.
    pub after: Option<&'a [u8]>,
}

/// A converter for one stream: one per worker thread.
///
/// It holds scratch buffers of a bounded size (a few frames of one channel, independent of
/// the input), which grow on the first conversion and are reused after that.
#[derive(Debug)]
pub struct Converter {
    config: PcmConfig,
    plan: Plan,
    scratch: kernel::Scratch,
}

impl Converter {
    /// A converter for `config`.
    pub fn new(config: PcmConfig) -> Result<Converter, PcmError> {
        if !(1..=MAX_CHANNELS).contains(&config.channels) {
            return Err(PcmError::Channels(config.channels));
        }
        if !config.cutoff.supports(config.rate) {
            return Err(PcmError::Unsupported { rate: config.rate, cutoff: config.cutoff });
        }
        if !(Gain::MIN_DB..=Gain::MAX_DB).contains(&config.gain.db) || !config.gain.linear.is_finite() {
            return Err(PcmError::Gain(config.gain.db));
        }
        let plan = Plan::new(config.rate, config.cutoff);
        Ok(Converter { config, plan, scratch: kernel::Scratch::default() })
    }

    /// The configuration.
    pub fn config(&self) -> PcmConfig {
        self.config
    }

    /// Output samples per channel per frame.
    pub fn samples_per_frame(&self) -> usize {
        self.config.rate.samples_per_frame()
    }

    /// Bytes per channel of each neighbour frame that a conversion reads (those nearest the
    /// run); always less than one frame.
    pub fn context_bytes(&self) -> usize {
        self.plan.context_bytes()
    }

    /// Converts frame `cur`, with its neighbours `prev` and `next` (`None` at the edges of
    /// the area), into `out`: `samples_per_frame() x channels` interleaved samples.
    pub fn convert_frame(
        &mut self,
        prev: Option<&[u8]>,
        cur: &[u8],
        next: Option<&[u8]>,
        out: &mut [i32],
    ) -> Result<ConvertStats, PcmError> {
        let frame = FRAME_BYTES_PER_CHANNEL * usize::from(self.config.channels);
        if cur.len() != frame {
            return Err(PcmError::InputLength { len: cur.len(), what: "one frame" });
        }
        self.convert_run(FrameRun { before: prev, frames: cur, after: next }, out)
    }

    /// Converts a run of frames into `out`: `frames x samples_per_frame() x channels`
    /// interleaved samples. A whole area is one run with no neighbours.
    pub fn convert_run(&mut self, run: FrameRun<'_>, out: &mut [i32]) -> Result<ConvertStats, PcmError> {
        let channels = usize::from(self.config.channels);
        let frame = FRAME_BYTES_PER_CHANNEL * channels;
        if run.frames.len() % frame != 0 {
            return Err(PcmError::InputLength { len: run.frames.len(), what: "a whole number of frames" });
        }
        for n in [run.before, run.after].into_iter().flatten() {
            if n.len() != frame {
                return Err(PcmError::InputLength { len: n.len(), what: "one frame" });
            }
        }
        let frames = run.frames.len() / frame;
        let expected = frames * self.samples_per_frame() * channels;
        if out.len() != expected {
            return Err(PcmError::OutputLength { len: out.len(), expected });
        }
        let mut stats = ConvertStats::default();
        let input = kernel::Input { run, channels, frames };
        self.plan.convert(&input, self.config.gain.linear, &mut self.scratch, out, &mut stats);
        Ok(stats)
    }
}

/// Packs 24-bit samples into 3-byte little-endian (the layout of 24-bit WAV and FLAC
/// samples). `dst` must hold `3 x src.len()` bytes; any excess is left untouched, and a
/// short `dst` receives only the samples that fit.
pub fn pack_s24le(src: &[i32], dst: &mut [u8]) {
    for (s, d) in src.iter().zip(dst.chunks_exact_mut(3)) {
        d.copy_from_slice(&s.to_le_bytes()[..3]);
    }
}
