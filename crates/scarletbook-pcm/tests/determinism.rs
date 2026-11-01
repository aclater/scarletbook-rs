// SPDX-License-Identifier: MIT OR Apache-2.0

//! Per-frame and per-run conversions give exactly the bytes of a whole-area conversion, at
//! every rate and cutoff, for 1 to 6 channels.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use scarletbook_pcm::{Converter, Cutoff, FRAME_BYTES_PER_CHANNEL, FrameRun, Gain, PcmConfig, PcmRate};

/// Every supported rate and cutoff.
fn combos() -> Vec<(PcmRate, Cutoff)> {
    PcmRate::ALL
        .into_iter()
        .flat_map(|r| Cutoff::ALL.into_iter().filter(move |c| c.supports(r)).map(move |c| (r, c)))
        .collect()
}

/// `frames` frames of `channels` channels of pseudo-random bytes, with stretches of constant
/// bytes (large, clipping values) mixed in.
fn area(seed: u64, channels: usize, frames: usize) -> Vec<u8> {
    let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut constant = None;
    (0..frames * channels * FRAME_BYTES_PER_CHANNEL)
        .map(|i| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            if i % 4096 == 0 {
                constant = (s % 3 == 0).then_some(if s % 2 == 0 { 0xFF } else { 0x00 });
            }
            constant.unwrap_or(s as u8)
        })
        .collect()
}

fn whole(config: PcmConfig, input: &[u8]) -> Vec<i32> {
    let mut conv = Converter::new(config).unwrap();
    let frames = input.len() / (usize::from(config.channels) * FRAME_BYTES_PER_CHANNEL);
    let mut out = vec![0i32; frames * conv.samples_per_frame() * usize::from(config.channels)];
    conv.convert_run(FrameRun { before: None, frames: input, after: None }, &mut out).unwrap();
    out
}

/// The run of frames `a..b` of `input`, with its neighbours, by `conv`.
fn run(conv: &mut Converter, input: &[u8], a: usize, b: usize) -> Vec<i32> {
    let ch = usize::from(conv.config().channels);
    let frame = ch * FRAME_BYTES_PER_CHANNEL;
    let frames = input.len() / frame;
    let before = a.checked_sub(1).map(|p| &input[p * frame..a * frame]);
    let after = (b < frames).then(|| &input[b * frame..(b + 1) * frame]);
    let mut out = vec![0i32; (b - a) * conv.samples_per_frame() * ch];
    conv.convert_run(FrameRun { before, frames: &input[a * frame..b * frame], after }, &mut out).unwrap();
    out
}

#[test]
fn every_frame_alone_matches_the_whole_area() {
    let frames = if cfg!(miri) { 2 } else { 3 };
    for (rate, cutoff) in combos() {
        for channels in 1..=6u8 {
            if cfg!(miri) && channels > 1 {
                continue;
            }
            let config = PcmConfig::new(rate, cutoff, channels);
            let input = area(u64::from(channels), usize::from(channels), frames);
            let want = whole(config, &input);
            let mut conv = Converter::new(config).unwrap();
            let per = conv.samples_per_frame() * usize::from(channels);
            for f in 0..frames {
                let got = run(&mut conv, &input, f, f + 1);
                assert!(got == want[f * per..(f + 1) * per], "{rate} {cutoff:?} {channels} ch, frame {f}");
            }
            assert!(conv.context_bytes() < FRAME_BYTES_PER_CHANNEL);
        }
        if cfg!(miri) {
            break;
        }
    }
}

/// Only the `context_bytes()` bytes of each neighbour nearest the frame matter.
#[test]
fn only_the_context_of_neighbours_is_read() {
    for (rate, cutoff) in combos() {
        let config = PcmConfig::new(rate, cutoff, 2);
        let input = area(5, 2, 3);
        let mut conv = Converter::new(config).unwrap();
        let ctx = conv.context_bytes();
        let want = run(&mut conv, &input, 1, 2);
        // Change every neighbour byte outside the context.
        let mut changed = input.clone();
        let frame = 2 * FRAME_BYTES_PER_CHANNEL;
        for k in 0..FRAME_BYTES_PER_CHANNEL - ctx {
            for c in 0..2 {
                changed[(FRAME_BYTES_PER_CHANNEL - 1 - (ctx + k)) * 2 + c] ^= 0xA5;
                changed[2 * frame + (ctx + k) * 2 + c] ^= 0x5A;
            }
        }
        assert!(run(&mut conv, &changed, 1, 2) == want, "{rate} {cutoff:?}");
        if cfg!(miri) {
            break;
        }
    }
}

fn config() -> ProptestConfig {
    if cfg!(miri) {
        ProptestConfig { cases: 2, failure_persistence: None, ..ProptestConfig::default() }
    } else {
        // PROPTEST_CASES, if set, overrides the default number of cases.
        let cases = std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(64);
        ProptestConfig { cases, ..ProptestConfig::default() }
    }
}

proptest! {
    #![proptest_config(config())]

    /// Any split of an area into runs, converted by one reused converter, gives the whole
    /// area's output.
    #[test]
    fn any_split_matches_the_whole_area(
        combo in 0usize..8,
        channels in 1u8..=6,
        frames in 1usize..=6,
        cuts in proptest::collection::vec(0usize..6, 0..4),
        seed in any::<u64>(),
        gain in -20.0f64..12.0,
    ) {
        let (rate, cutoff) = combos()[combo];
        let config = PcmConfig::new(rate, cutoff, channels).with_gain(Gain::from_db(gain).unwrap());
        let input = area(seed, usize::from(channels), frames);
        let want = whole(config, &input);
        let mut points: Vec<usize> = cuts.into_iter().filter(|&c| c > 0 && c < frames).collect();
        points.push(0);
        points.push(frames);
        points.sort_unstable();
        points.dedup();
        let mut conv = Converter::new(config).unwrap();
        let mut got = Vec::new();
        for w in points.windows(2) {
            got.extend(run(&mut conv, &input, w[0], w[1]));
        }
        prop_assert!(got == want);
    }
}

#[test]
fn errors() {
    use scarletbook_pcm::PcmError;
    let c = |rate, cutoff, ch| Converter::new(PcmConfig::new(rate, cutoff, ch)).map(|_| ());
    assert!(matches!(c(PcmRate::Hz88200, Cutoff::Wide, 2), Err(PcmError::Unsupported { .. })));
    assert!(matches!(c(PcmRate::Hz176400, Cutoff::Default, 0), Err(PcmError::Channels(0))));
    assert!(matches!(c(PcmRate::Hz176400, Cutoff::Default, 7), Err(PcmError::Channels(7))));
    for db in [f64::NAN, f64::INFINITY, -120.5, 24.01] {
        assert!(matches!(Gain::from_db(db), Err(PcmError::Gain(_))));
    }
    assert_eq!(Gain::from_db(0.0).unwrap().linear(), 1.0);
    let mut conv = Converter::new(PcmConfig::new(PcmRate::Hz176400, Cutoff::Default, 2)).unwrap();
    let frame = vec![0u8; 2 * FRAME_BYTES_PER_CHANNEL];
    let mut out = vec![0i32; 2 * 2352];
    assert!(matches!(
        conv.convert_frame(None, &frame[1..], None, &mut out),
        Err(PcmError::InputLength { .. })
    ));
    assert!(matches!(
        conv.convert_frame(Some(&frame[1..]), &frame, None, &mut out),
        Err(PcmError::InputLength { .. })
    ));
    assert!(matches!(
        conv.convert_frame(None, &frame, None, &mut out[1..]),
        Err(PcmError::OutputLength { .. })
    ));
    // Nothing was written.
    assert!(out.iter().all(|&v| v == 0));
    conv.convert_frame(None, &frame, None, &mut out).unwrap();
    // An empty run converts to nothing.
    assert!(conv.convert_run(FrameRun { before: None, frames: &[], after: None }, &mut []).is_ok());
}

#[test]
fn pack_s24le_layout() {
    let mut dst = [0u8; 9];
    scarletbook_pcm::pack_s24le(&[1, -1, 8_388_607], &mut dst);
    assert_eq!(dst, [1, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F]);
}
