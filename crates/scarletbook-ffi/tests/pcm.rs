// SPDX-License-Identifier: MIT OR Apache-2.0

//! DSD64 to PCM through the C ABI (`sb_pcm_*`, #130): for every rate, cutoff, channel
//! count, output format and neighbour case, the bytes `sb_pcm_convert_run` writes are the
//! `scarletbook_pcm::Converter`'s samples; frame by frame with neighbours they are those of
//! the whole run; clip counts match; and every argument error is typed and writes nothing.
//!
//! `CORPUS_PCM_SHA256` is the expected digest of one corpus frame's conversion, checked
//! here against the Rust converter and used by `.github/scripts/clib-smoke.py` (which
//! reads it from this file) to check the packaged library through the README's ctypes
//! bindings.
//!
//! Each unsafe call sits in a small wrapper that states why its arguments meet the
//! function's contract; the tests themselves are safe code.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::ffi::{CStr, CString};
use std::path::PathBuf;
use std::ptr;

use scarletbook_ffi::*;
use scarletbook_pcm::{
    ConvertStats, Converter, Cutoff, FRAME_BYTES_PER_CHANNEL, FrameRun, Gain, PcmConfig, PcmRate, pack_s24le,
};
use sha2::{Digest, Sha256};

/// SHA-256 of frame 1 of track 1 of the 2-channel area of corpus image
/// `dsd-stereo-mch-2048` (plain DSD), converted with frames 0 and 2 as neighbours at
/// 176 400 Hz, `SB_PCM_CUTOFF_DEFAULT`, 0 dB, as `SB_PCM_S24LE` (2352 x 2 samples, 14 112
/// bytes). Computed by `scarletbook_pcm::Converter` (`corpus_frame_digest` checks it);
/// `clib-smoke.py` compares the packaged library's output with it.
const CORPUS_PCM_SHA256: &str = "c0101d3cb79916b45c4b7f8aba1fa50bd2527bea9e0c0888ebe585726eae6e82";

fn last_error() -> String {
    // SAFETY: sb_last_error_message returns a valid NUL-terminated string.
    unsafe { CStr::from_ptr(sb_last_error_message()) }.to_string_lossy().into_owned()
}

/// `sb_pcm_converter_new`; `*out` is checked to be NULL on failure.
fn new(rate: u32, cutoff: u32, channels: u32, gain_db: f64) -> Result<*mut SbPcmConverter, SbStatus> {
    let mut c: *mut SbPcmConverter = ptr::dangling_mut();
    // SAFETY: `&mut c` is a valid out-pointer.
    match unsafe { sb_pcm_converter_new(rate, cutoff, channels, gain_db, &mut c) } {
        SbStatus::Ok => Ok(c),
        s => {
            assert!(c.is_null(), "*out is NULL after a failure");
            Err(s)
        }
    }
}

fn free(c: *mut SbPcmConverter) {
    // SAFETY: `c` is NULL or a live handle that the caller does not use again.
    unsafe { sb_pcm_converter_free(c) };
}

fn samples_per_frame(c: *const SbPcmConverter) -> u32 {
    let mut n = 0;
    // SAFETY: `c` is a live handle; `&mut n` a valid out-pointer.
    assert_eq!(unsafe { sb_pcm_samples_per_frame(c, &mut n) }, SbStatus::Ok);
    n
}

/// The pointer of an optional input (NULL for `None`).
fn ptr_of(b: Option<&[u8]>) -> *const u8 {
    b.map_or(ptr::null(), <[u8]>::as_ptr)
}

/// `sb_pcm_convert_run` into `out` (NULL when `None`), with `out_len` bytes and the clip
/// counts (`None`: NULL `clipped`).
#[allow(clippy::too_many_arguments)]
fn convert_raw(
    c: *mut SbPcmConverter,
    before: Option<&[u8]>,
    frames: Option<&[u8]>,
    frames_len: usize,
    after: Option<&[u8]>,
    fmt: u32,
    out: Option<&mut [u8]>,
    out_len: usize,
    clipped: Option<&mut [u64; 6]>,
) -> SbStatus {
    let o = out.map_or(ptr::null_mut(), |b| {
        assert!(b.len() >= out_len);
        b.as_mut_ptr()
    });
    if let Some(f) = frames {
        assert!(f.len() >= frames_len);
    }
    let k = clipped.map_or(ptr::null_mut(), |k| k.as_mut_ptr());
    // SAFETY: `c` is NULL or a live handle; the inputs are NULL or slices at least as long
    // as the function reads (the neighbours are checked by the callers to be one frame);
    // `o` is NULL or valid for `out_len` bytes and does not overlap the inputs; `k` NULL or
    // valid for 6 writes.
    unsafe {
        sb_pcm_convert_run(
            c,
            ptr_of(before),
            ptr_of(frames),
            frames_len,
            ptr_of(after),
            fmt,
            o.cast(),
            out_len,
            k,
        )
    }
}

/// Converts `frames` with neighbours through the C ABI in format `fmt`: the bytes and the
/// clip counts.
fn convert_c(
    c: *mut SbPcmConverter,
    before: Option<&[u8]>,
    frames: &[u8],
    after: Option<&[u8]>,
    fmt: u32,
    out_len: usize,
) -> (Vec<u8>, [u64; 6]) {
    let mut out = vec![0u8; out_len];
    let mut clipped = [u64::MAX; 6];
    let s = convert_raw(
        c,
        before,
        Some(frames),
        frames.len(),
        after,
        fmt,
        Some(&mut out),
        out_len,
        Some(&mut clipped),
    );
    assert_eq!(s, SbStatus::Ok, "{}", last_error());
    (out, clipped)
}

/// The Rust converter's samples and counts for the same run.
fn convert_rust(config: PcmConfig, run: FrameRun<'_>) -> (Vec<i32>, ConvertStats) {
    let mut conv = Converter::new(config).unwrap();
    let frame = FRAME_BYTES_PER_CHANNEL * usize::from(config.channels);
    let mut pcm =
        vec![0i32; run.frames.len() / frame * conv.samples_per_frame() * usize::from(config.channels)];
    let stats = conv.convert_run(run, &mut pcm).unwrap();
    (pcm, stats)
}

/// Samples as `fmt` writes them.
fn bytes_of(pcm: &[i32], fmt: u32) -> Vec<u8> {
    if fmt == SB_PCM_S32 {
        pcm.iter().flat_map(|s| s.to_ne_bytes()).collect()
    } else {
        let mut b = vec![0u8; pcm.len() * 3];
        pack_s24le(pcm, &mut b);
        b
    }
}

/// `n` bytes of a 1-bit signal: a first-order sigma-delta modulation of two sines, with a
/// different frequency and phase per channel, as a stand-in for real DSD (its noise is
/// shaped and its level stays below full scale).
fn dsd(frames: usize, channels: usize, seed: f64) -> Vec<u8> {
    let bytes = frames * channels * FRAME_BYTES_PER_CHANNEL;
    let mut out = vec![0u8; bytes];
    let mut acc = vec![0.0f64; channels];
    for k in 0..bytes / channels {
        for (c, a) in acc.iter_mut().enumerate() {
            let mut byte = 0u8;
            for bit in 0..8 {
                let t = (k * 8 + bit) as f64 / 2_822_400.0;
                let f = 1000.0 + 3700.0 * c as f64 + seed;
                let x = 0.4 * (2.0 * std::f64::consts::PI * f * t).sin()
                    + 0.1 * (2.0 * std::f64::consts::PI * 31_000.0 * t + seed).sin();
                let y = if *a >= 0.0 { 1.0 } else { -1.0 };
                *a += x - y;
                byte = (byte << 1) | u8::from(y > 0.0);
            }
            out[k * channels + c] = byte;
        }
    }
    out
}

fn cutoff_value(c: Cutoff) -> u32 {
    match c {
        Cutoff::Default => SB_PCM_CUTOFF_DEFAULT,
        Cutoff::Low => SB_PCM_CUTOFF_LOW,
        Cutoff::Wide => SB_PCM_CUTOFF_WIDE,
        _ => unreachable!(),
    }
}

/// For every rate x cutoff, 1, 2 and 6 channels, with and without neighbours, in both
/// formats: the C output is the Rust converter's, in one call over a run longer than the
/// library's internal chunk (8 frames) and frame by frame with each frame's neighbours.
#[test]
#[cfg_attr(miri, ignore = "converts many frames")]
fn c_output_is_the_rust_converters() {
    const FRAMES: usize = 10;
    for rate in PcmRate::ALL {
        for cutoff in Cutoff::ALL.into_iter().filter(|c| c.supports(rate)) {
            for channels in [1u8, 2, 6] {
                let ch = usize::from(channels);
                let frame = ch * FRAME_BYTES_PER_CHANNEL;
                // Frames -1 .. FRAMES + 1 of a stream: the run and its two neighbours.
                let all = dsd(FRAMES + 2, ch, f64::from(rate.hz()) / 1000.0 + cutoff_value(cutoff) as f64);
                let (prev, rest) = all.split_at(frame);
                let (frames, next) = rest.split_at(FRAMES * frame);
                let config = PcmConfig::new(rate, cutoff, channels);
                let c = new(rate.hz(), cutoff_value(cutoff), u32::from(channels), 0.0).unwrap();
                let spf = samples_per_frame(c) as usize;
                assert_eq!(spf, rate.samples_per_frame());
                for (before, after) in
                    [(None, None), (Some(prev), Some(next)), (Some(prev), None), (None, Some(next))]
                {
                    let (pcm, stats) = convert_rust(config, FrameRun { before, frames, after });
                    for fmt in [SB_PCM_S32, SB_PCM_S24LE] {
                        let width = if fmt == SB_PCM_S32 { 4 } else { 3 };
                        let what = format!("{rate} {cutoff:?} {channels} ch fmt {fmt} {}", before.is_some());
                        let want = bytes_of(&pcm, fmt);
                        let (got, clipped) = convert_c(c, before, frames, after, fmt, want.len());
                        assert!(got == want, "{what}: whole run");
                        assert_eq!(clipped, stats.clipped, "{what}");
                        // Frame by frame, each with its neighbours: the same bytes.
                        let per = spf * ch * width;
                        for f in 0..FRAMES {
                            let b = if f == 0 { before } else { Some(&frames[(f - 1) * frame..f * frame]) };
                            let a = if f + 1 == FRAMES {
                                after
                            } else {
                                Some(&frames[(f + 1) * frame..(f + 2) * frame])
                            };
                            let (one, _) = convert_c(c, b, &frames[f * frame..(f + 1) * frame], a, fmt, per);
                            assert!(one[..] == want[f * per..(f + 1) * per], "{what}: frame {f}");
                        }
                    }
                }
                free(c);
            }
        }
    }
}

/// Clipped samples are counted per channel, as the Rust converter counts them, summed over
/// the library's internal chunks; `clipped` may be NULL.
#[test]
#[cfg_attr(miri, ignore = "converts many frames")]
fn clip_counts() {
    const FRAMES: usize = 10;
    // Channel 0 all 1 bits (full scale: clips); channel 1 the signal at +24 dB (clips on
    // its peaks); channel 2 the signal at 0 dB (does not).
    let signal = dsd(FRAMES, 1, 0.0);
    let mut frames = Vec::with_capacity(FRAMES * 3 * FRAME_BYTES_PER_CHANNEL);
    for &s in &signal {
        frames.extend([0xFF, s, s]);
    }
    for gain in [0.0, 24.0] {
        let c = new(176_400, SB_PCM_CUTOFF_DEFAULT, 3, gain).unwrap();
        let config =
            PcmConfig::new(PcmRate::Hz176400, Cutoff::Default, 3).with_gain(Gain::from_db(gain).unwrap());
        let (pcm, stats) = convert_rust(config, FrameRun { before: None, frames: &frames, after: None });
        let want = bytes_of(&pcm, SB_PCM_S24LE);
        let (got, clipped) = convert_c(c, None, &frames, None, SB_PCM_S24LE, want.len());
        assert!(got == want);
        assert_eq!(clipped, stats.clipped);
        assert!(clipped[0] > 0, "{clipped:?}");
        assert_eq!(clipped[3..], [0, 0, 0], "no counts beyond the converter's channels");
        if gain == 0.0 {
            assert_eq!(clipped[1..3], [0, 0]);
        } else {
            assert!(clipped[1] > 0 && clipped[1] == clipped[2], "{clipped:?}");
        }
        // Without clip counts: the same output.
        let mut again = vec![0u8; want.len()];
        let s = convert_raw(
            c,
            None,
            Some(&frames),
            frames.len(),
            None,
            SB_PCM_S24LE,
            Some(&mut again),
            want.len(),
            None,
        );
        assert_eq!(s, SbStatus::Ok);
        assert!(again == want);
        free(c);
    }
}

/// Every argument error: its status, `*out` NULL from `sb_pcm_converter_new`, nothing
/// written by `sb_pcm_convert_run`, and a message. Converts nothing, so it runs under Miri.
#[test]
fn argument_errors() {
    let bad = |r: Result<*mut SbPcmConverter, SbStatus>, what: &str| {
        assert_eq!(r.err(), Some(SbStatus::InvalidArgument), "{what}");
        assert!(!last_error().is_empty(), "{what}");
    };
    bad(new(44_100, SB_PCM_CUTOFF_DEFAULT, 2, 0.0), "rate 44100");
    bad(new(0, SB_PCM_CUTOFF_DEFAULT, 2, 0.0), "rate 0");
    bad(new(88_200, SB_PCM_CUTOFF_WIDE, 2, 0.0), "Wide at 88.2 kHz");
    bad(new(176_400, 3, 2, 0.0), "cutoff 3");
    bad(new(176_400, SB_PCM_CUTOFF_DEFAULT, 0, 0.0), "0 channels");
    bad(new(176_400, SB_PCM_CUTOFF_DEFAULT, 7, 0.0), "7 channels");
    bad(new(176_400, SB_PCM_CUTOFF_DEFAULT, 256 + 2, 0.0), "258 channels");
    bad(new(176_400, SB_PCM_CUTOFF_DEFAULT, 2, 24.5), "+24.5 dB");
    bad(new(176_400, SB_PCM_CUTOFF_DEFAULT, 2, -120.5), "-120.5 dB");
    bad(new(176_400, SB_PCM_CUTOFF_DEFAULT, 2, f64::NAN), "NaN dB");
    bad(new(176_400, SB_PCM_CUTOFF_DEFAULT, 2, f64::INFINITY), "infinite dB");
    // SAFETY: a NULL out-pointer is allowed (and refused).
    let s = unsafe { sb_pcm_converter_new(176_400, SB_PCM_CUTOFF_DEFAULT, 2, 0.0, ptr::null_mut()) };
    assert_eq!(s, SbStatus::InvalidArgument);
    for (rate, cutoff) in [(88_200, SB_PCM_CUTOFF_LOW), (352_800, SB_PCM_CUTOFF_WIDE)] {
        for gain in [-120.0, 24.0] {
            free(new(rate, cutoff, 6, gain).unwrap());
        }
    }
    free(ptr::null_mut());

    // NULL handles.
    let mut n = 0u32;
    // SAFETY: NULL handles are allowed (and refused); `&mut n` is a valid out-pointer.
    assert_eq!(unsafe { sb_pcm_samples_per_frame(ptr::null(), &mut n) }, SbStatus::InvalidArgument);
    let c = new(88_200, SB_PCM_CUTOFF_DEFAULT, 2, 0.0).unwrap();
    // SAFETY: `c` is a live handle; a NULL out-pointer is allowed (and refused).
    assert_eq!(unsafe { sb_pcm_samples_per_frame(c, ptr::null_mut()) }, SbStatus::InvalidArgument);
    assert_eq!(samples_per_frame(c), 1176);

    let frame = 2 * FRAME_BYTES_PER_CHANNEL;
    let frames = vec![0x69u8; 2 * frame];
    let need = 2 * 1176 * 2 * 3;
    let mut out = vec![0xA5u8; need + 7];
    let mut clipped = [7u64; 6];
    let mut call = |c, frames: Option<&[u8]>, len, fmt, out_len, null_out: bool| {
        out.fill(0xA5);
        clipped = [7; 6];
        let o = if null_out { None } else { Some(&mut out[..]) };
        let s = convert_raw(c, None, frames, len, None, fmt, o, out_len, Some(&mut clipped));
        assert!(out.iter().all(|&b| b == 0xA5), "nothing written");
        assert_eq!(clipped, [7; 6], "no counts written");
        assert!(!last_error().is_empty());
        s
    };
    let f = Some(&frames[..]);
    assert_eq!(call(ptr::null_mut(), f, 2 * frame, SB_PCM_S24LE, need, false), SbStatus::InvalidArgument);
    assert_eq!(call(c, f, 2 * frame, 2, need, false), SbStatus::InvalidArgument, "unknown fmt");
    for len in [1, frame - 1, frame + 1, FRAME_BYTES_PER_CHANNEL, 2 * frame - 2] {
        assert_eq!(call(c, f, len, SB_PCM_S24LE, need, false), SbStatus::InvalidArgument, "frames_len {len}");
    }
    assert_eq!(call(c, None, 2 * frame, SB_PCM_S24LE, need, false), SbStatus::InvalidArgument, "NULL frames");
    assert_eq!(call(c, f, 2 * frame, SB_PCM_S24LE, need - 1, false), SbStatus::BufferTooSmall);
    assert_eq!(
        call(c, f, 2 * frame, SB_PCM_S32, need, false),
        SbStatus::BufferTooSmall,
        "S32 needs 4 bytes a sample"
    );
    assert_eq!(call(c, f, 2 * frame, SB_PCM_S24LE, need, true), SbStatus::BufferTooSmall, "NULL out");
    assert!(last_error().contains(&need.to_string()), "{}", last_error());

    // No frames: nothing to do, with NULL frames and out, and zero counts.
    let s = convert_raw(c, None, None, 0, None, SB_PCM_S32, None, 0, Some(&mut clipped));
    assert_eq!((s, clipped), (SbStatus::Ok, [0; 6]));
    free(c);
}

/// One small conversion under Miri: the slices built from the C arguments, an unaligned
/// `SB_PCM_S32` output, bytes past the output untouched, both neighbours.
#[test]
fn small_conversion() {
    let c = new(88_200, SB_PCM_CUTOFF_LOW, 1, -6.0).unwrap();
    let all: Vec<u8> =
        (0..3 * FRAME_BYTES_PER_CHANNEL).map(|i| [0x69, 0x96, 0x5A, 0xA5][i % 4] ^ (i as u8)).collect();
    let (prev, rest) = all.split_at(FRAME_BYTES_PER_CHANNEL);
    let (cur, next) = rest.split_at(FRAME_BYTES_PER_CHANNEL);
    let config = PcmConfig::new(PcmRate::Hz88200, Cutoff::Low, 1).with_gain(Gain::from_db(-6.0).unwrap());
    let (pcm, _) = convert_rust(config, FrameRun { before: Some(prev), frames: cur, after: Some(next) });
    for fmt in [SB_PCM_S32, SB_PCM_S24LE] {
        let want = bytes_of(&pcm, fmt);
        // One byte in, so that an S32 output is misaligned; 5 spare bytes after.
        let mut out = vec![0xEEu8; want.len() + 6];
        let mut clipped = [9u64; 6];
        let s = convert_raw(
            c,
            Some(prev),
            Some(cur),
            cur.len(),
            Some(next),
            fmt,
            Some(&mut out[1..]),
            want.len() + 5,
            Some(&mut clipped),
        );
        assert_eq!(s, SbStatus::Ok, "{}", last_error());
        assert!(out[1..=want.len()] == want[..]);
        assert_eq!(out[0], 0xEE);
        assert!(out[want.len() + 1..].iter().all(|&b| b == 0xEE));
        assert_eq!(clipped, [0; 6]);
    }
    free(c);
}

/// Frame 1 of track 1 of a corpus image, with frames 0 and 2, converts to
/// `CORPUS_PCM_SHA256` through the C ABI and through the Rust converter.
#[test]
#[cfg_attr(miri, ignore = "reads a corpus image")]
fn corpus_frame_digest() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/images/dsd-stereo-mch-2048.iso");
    let path = CString::new(path.to_str().unwrap()).unwrap();
    let mut disc: *mut SbDisc = ptr::null_mut();
    // SAFETY: `path` is a NUL-terminated string; `&mut disc` a valid out-pointer.
    assert_eq!(unsafe { sb_disc_open_file(path.as_ptr(), &mut disc) }, SbStatus::Ok);
    let mut audio: *mut SbTrackAudio = ptr::null_mut();
    // SAFETY: `disc` is a live handle; `&mut audio` a valid out-pointer.
    assert_eq!(unsafe { sb_track_audio_open(disc, 0, 1, &mut audio) }, SbStatus::Ok);
    let frame = 2 * FRAME_BYTES_PER_CHANNEL;
    let mut frames = vec![0u8; 3 * frame];
    for f in frames.chunks_exact_mut(frame) {
        // SAFETY: live handles; `f` is valid for `frame` bytes; NULL info is allowed.
        let s = unsafe { sb_track_audio_read(audio, disc, f.as_mut_ptr(), frame, ptr::null_mut()) };
        assert_eq!(s, SbStatus::Ok);
    }
    // SAFETY: a live handle, not used again.
    unsafe { sb_track_audio_free(audio) };
    // SAFETY: as above.
    unsafe { sb_disc_free(disc) };
    let (prev, rest) = frames.split_at(frame);
    let (cur, next) = rest.split_at(frame);
    let c = new(176_400, SB_PCM_CUTOFF_DEFAULT, 2, 0.0).unwrap();
    let (got, _) = convert_c(c, Some(prev), cur, Some(next), SB_PCM_S24LE, 2352 * 2 * 3);
    free(c);
    let config = PcmConfig::new(PcmRate::Hz176400, Cutoff::Default, 2);
    let (pcm, _) = convert_rust(config, FrameRun { before: Some(prev), frames: cur, after: Some(next) });
    assert!(got == bytes_of(&pcm, SB_PCM_S24LE));
    // The whole track, in one run: frame 1's bytes are the same.
    let (whole, _) = convert_rust(config, FrameRun { before: None, frames: &frames, after: None });
    assert!(got == bytes_of(&whole[2352 * 2..2 * 2352 * 2], SB_PCM_S24LE));
    let digest: String = Sha256::digest(&got).iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(digest, CORPUS_PCM_SHA256);
}
