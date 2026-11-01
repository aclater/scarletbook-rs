// SPDX-License-Identifier: MIT OR Apache-2.0

//! #171: DSD silence converts to exact digital silence, at the edges of an area too. The
//! idle pattern (0x69) gives samples of exactly 0 at every rate, cutoff and channel count,
//! with and without neighbours, and a neighbour of `None` gives exactly the output of a
//! neighbour frame of 0x69 bytes, whatever the frame holds.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use scarletbook_pcm::{
    Converter, Cutoff, FRAME_BYTES_PER_CHANNEL, FrameRun, Frames, Gain, PcmConfig, PcmOut, PcmRate, Run,
};

fn combos() -> Vec<(PcmRate, Cutoff)> {
    PcmRate::ALL
        .into_iter()
        .flat_map(|r| Cutoff::ALL.into_iter().filter(move |c| c.supports(r)).map(move |c| (r, c)))
        .collect()
}

fn noise(seed: u64, len: usize) -> Vec<u8> {
    let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s as u8
        })
        .collect()
}

fn convert(conv: &mut Converter, run: FrameRun<'_>) -> Vec<i32> {
    let ch = usize::from(conv.config().channels);
    let frames = run.frames.len() / (ch * FRAME_BYTES_PER_CHANNEL);
    let mut out = vec![0i32; frames * conv.samples_per_frame() * ch];
    conv.convert_run(run, &mut out).unwrap();
    out
}

/// Channel counts to test (fewer under Miri).
fn channel_counts() -> std::ops::RangeInclusive<u8> {
    if cfg!(miri) { 1..=1 } else { 1..=6 }
}

#[test]
fn idle_pattern_is_exact_silence() {
    for (rate, cutoff) in combos() {
        for channels in channel_counts() {
            let ch = usize::from(channels);
            let frame = ch * FRAME_BYTES_PER_CHANNEL;
            let idle = vec![0x69u8; 2 * frame];
            let other = vec![0x55u8; frame];
            for gain in [0.0, 24.0] {
                let config = PcmConfig::new(rate, cutoff, channels).with_gain(Gain::from_db(gain).unwrap());
                let mut conv = Converter::new(config).unwrap();
                for (before, after) in [
                    (None, None),
                    (Some(&idle[..frame]), None),
                    (None, Some(&idle[..frame])),
                    (Some(&idle[..frame]), Some(&idle[..frame])),
                ] {
                    let run = FrameRun::new(&idle).with_before(before).with_after(after);
                    let out = convert(&mut conv, run);
                    assert!(out.iter().all(|&v| v == 0), "{rate} {cutoff:?} {channels} ch {gain} dB");
                }
                // The other idle pattern of specification 0 (invalid-frame fill).
                let run = FrameRun::new(&other).with_before(Some(&other)).with_after(Some(&other));
                assert!(convert(&mut conv, run).iter().all(|&v| v == 0), "0x55 {rate} {cutoff:?}");
            }
        }
        if cfg!(miri) {
            break;
        }
    }
}

#[test]
fn none_neighbour_is_an_idle_frame() {
    for (rate, cutoff) in combos() {
        for channels in channel_counts() {
            let ch = usize::from(channels);
            let frame = ch * FRAME_BYTES_PER_CHANNEL;
            let idle = vec![0x69u8; frame];
            let frames = noise(u64::from(channels), 2 * frame);
            let mut conv = Converter::new(PcmConfig::new(rate, cutoff, channels)).unwrap();
            let with_none = convert(&mut conv, FrameRun::new(&frames));
            let run = FrameRun::new(&frames).with_before(Some(&idle)).with_after(Some(&idle));
            assert!(convert(&mut conv, run) == with_none, "{rate} {cutoff:?} {channels} ch");
        }
        if cfg!(miri) {
            break;
        }
    }
}

/// An area that starts in silence has no transient at its start: every output whose filter
/// window holds only the idle pattern and the area's edge is exactly 0.
#[test]
fn silence_then_signal_has_no_edge_transient() {
    let (rate, cutoff) = (PcmRate::Hz176400, Cutoff::Wide);
    let frame = FRAME_BYTES_PER_CHANNEL;
    let mut area = vec![0x69u8; 3 * frame];
    area[2 * frame..].copy_from_slice(&noise(9, frame));
    let mut conv = Converter::new(PcmConfig::new(rate, cutoff, 1)).unwrap();
    let out = convert(&mut conv, FrameRun::new(&area));
    let spf = conv.samples_per_frame();
    // The first frame and most of the second hear nothing but silence.
    assert!(out[..spf + spf / 2].iter().all(|&v| v == 0));
}

/// 0x55, the other idle pattern, gives exact zeros within a run of 0x55, but a change
/// between 0x55 and 0x69 (here the area's edges, which are 0x69) is not silent: the two are
/// different signals, and no filter makes their junction silent.
#[test]
fn idle_0x55_is_silent_only_within_a_run() {
    for (rate, cutoff) in combos() {
        let frame = FRAME_BYTES_PER_CHANNEL;
        let area = vec![0x55u8; 3 * frame];
        let mut conv = Converter::new(PcmConfig::new(rate, cutoff, 1)).unwrap();
        let spf = conv.samples_per_frame();
        let out = convert(&mut conv, FrameRun::new(&area));
        // The middle frame is far from both edges.
        assert!(out[spf..2 * spf].iter().all(|&v| v == 0), "{rate} {cutoff:?}");
        // At the edges, where 0x55 meets the idle pattern 0x69, it is not.
        assert!(out[..spf / 4].iter().any(|&v| v != 0), "{rate} {cutoff:?}");
        assert!(out[3 * spf - spf / 4..].iter().any(|&v| v != 0), "{rate} {cutoff:?}");
        if cfg!(miri) {
            break;
        }
    }
}

/// The same through `Converter::convert`: frames in separate buffers, 3-byte output, and
/// per-frame clip counts. 0x69 frames are all-zero bytes with no clipped sample, with or
/// without neighbours, and a `None` neighbour equals a 0x69 neighbour frame on any content.
#[test]
fn convert_with_separate_frames_and_s24le() {
    for (rate, cutoff) in combos() {
        for channels in channel_counts() {
            let ch = usize::from(channels);
            let frame = ch * FRAME_BYTES_PER_CHANNEL;
            let idle = vec![0x69u8; frame];
            let mut conv = Converter::new(
                PcmConfig::new(rate, cutoff, channels).with_gain(Gain::from_db(24.0).unwrap()),
            )
            .unwrap();
            let bytes = 2 * conv.samples_per_frame() * ch * 3;
            let separate = [&idle[..], &idle[..]];
            for (before, after) in [(None, None), (Some(&idle[..]), Some(&idle[..])), (None, Some(&idle[..]))]
            {
                let run = Run::new(Frames::Separate(&separate)).with_before(before).with_after(after);
                let mut out = vec![0xEEu8; bytes];
                let mut clipped = [7u64; 2];
                let stats = conv.convert(run, PcmOut::S24le(&mut out), Some(&mut clipped)).unwrap();
                assert!(out.iter().all(|&b| b == 0), "{rate} {cutoff:?} {channels} ch");
                assert_eq!(clipped, [0, 0]);
                assert_eq!(stats.clipped_total(), 0);
            }
            // Any content: `None` neighbours are 0x69 neighbour frames, in either layout.
            let content = noise(u64::from(channels) + 40, 2 * frame);
            let parts = [&content[..frame], &content[frame..]];
            let mut with_none = vec![0u8; bytes];
            let mut with_idle = vec![0u8; bytes];
            conv.convert(Run::new(Frames::Separate(&parts)), PcmOut::S24le(&mut with_none), None).unwrap();
            let run = Run::new(Frames::Separate(&parts)).with_before(Some(&idle)).with_after(Some(&idle));
            conv.convert(run, PcmOut::S24le(&mut with_idle), None).unwrap();
            assert!(with_none == with_idle, "{rate} {cutoff:?} {channels} ch");
        }
        if cfg!(miri) {
            break;
        }
    }
}
