// SPDX-License-Identifier: MIT OR Apache-2.0

//! `Converter::convert` (issue #172): runs of separate frames, packed 24-bit output and
//! per-frame clip counts give exactly the samples and counts of `convert_run` + `pack_s24le`,
//! at every rate and cutoff, for 1 to 6 channels, with and without neighbours.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use scarletbook_pcm::{
    ConvertStats, Converter, Cutoff, FRAME_BYTES_PER_CHANNEL, FrameRun, Frames, Gain, PcmConfig, PcmError,
    PcmOut, PcmRate, Run, pack_s24le,
};

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

/// What a conversion of a run gave: samples, packed bytes, stats and per-frame clip counts.
#[derive(Debug, PartialEq)]
struct Got {
    pcm: Vec<i32>,
    packed: Vec<u8>,
    stats: ConvertStats,
    per_frame: Vec<u64>,
}

/// A frame before a run, its frames, and the frame after it.
type Parts<'a> = (Option<&'a [u8]>, Vec<&'a [u8]>, Option<&'a [u8]>);

/// The frames `a..b` of `input` and their neighbours.
fn parts(input: &[u8], frame: usize, a: usize, b: usize) -> Parts<'_> {
    let frames = input.len() / frame;
    let before = a.checked_sub(1).map(|p| &input[p * frame..a * frame]);
    let after = (b < frames).then(|| &input[b * frame..(b + 1) * frame]);
    (before, input[a * frame..b * frame].chunks_exact(frame).collect(), after)
}

/// Frames `a..b` of `input` by today's `convert_run` + `pack_s24le`, and each frame's clip
/// count from converting it alone with its neighbours.
fn reference(conv: &mut Converter, input: &[u8], a: usize, b: usize) -> Got {
    let ch = usize::from(conv.config().channels);
    let frame = ch * FRAME_BYTES_PER_CHANNEL;
    let per = conv.samples_per_frame() * ch;
    let (before, _, after) = parts(input, frame, a, b);
    let mut pcm = vec![0i32; (b - a) * per];
    let run = FrameRun::new(&input[a * frame..b * frame]).with_before(before).with_after(after);
    let stats = conv.convert_run(run, &mut pcm).unwrap();
    let mut packed = vec![0u8; 3 * pcm.len()];
    pack_s24le(&pcm, &mut packed);
    let per_frame = (a..b)
        .map(|f| {
            let (before, cur, after) = parts(input, frame, f, f + 1);
            let mut one = vec![0i32; per];
            conv.convert_frame(before, cur[0], after, &mut one).unwrap().clipped_total()
        })
        .collect();
    Got { pcm, packed, stats, per_frame }
}

/// Frames `a..b` of `input` by `convert`, from separate or contiguous frames, in both
/// layouts, with per-frame counts; the stats of both layouts must agree.
fn new_path(conv: &mut Converter, input: &[u8], a: usize, b: usize, separate: bool) -> Got {
    let ch = usize::from(conv.config().channels);
    let frame = ch * FRAME_BYTES_PER_CHANNEL;
    let per = conv.samples_per_frame() * ch;
    let (before, list, after) = parts(input, frame, a, b);
    let frames =
        if separate { Frames::Separate(&list) } else { Frames::Contiguous(&input[a * frame..b * frame]) };
    let run = Run::new(frames).with_before(before).with_after(after);
    let mut pcm = vec![0i32; (b - a) * per];
    let mut per_frame = vec![u64::MAX; b - a];
    let stats = conv.convert(run, PcmOut::I32(&mut pcm), Some(&mut per_frame)).unwrap();
    let mut packed = vec![0u8; 3 * (b - a) * per];
    let mut per_frame2 = vec![0; b - a];
    let stats2 = conv.convert(run, PcmOut::S24le(&mut packed), Some(&mut per_frame2)).unwrap();
    assert_eq!(stats, stats2);
    assert_eq!(per_frame, per_frame2);
    // Without counts, the same output.
    let mut packed3 = vec![0u8; packed.len()];
    assert_eq!(conv.convert(run, PcmOut::S24le(&mut packed3), None).unwrap(), stats);
    assert!(packed3 == packed);
    Got { pcm, packed, stats, per_frame }
}

#[test]
fn every_run_matches_convert_run_and_pack() {
    let frames = if cfg!(miri) { 2 } else { 3 };
    // Whole area, no neighbours; each neighbour alone; both.
    let runs: &[(usize, usize)] =
        if cfg!(miri) { &[(0, 2), (1, 2)] } else { &[(0, 3), (0, 2), (1, 3), (1, 2)] };
    let mut clipped = 0;
    for (rate, cutoff) in combos() {
        for channels in 1..=6u8 {
            if cfg!(miri) && channels > 1 {
                continue;
            }
            let config = PcmConfig::new(rate, cutoff, channels).with_gain(Gain::from_db(3.0).unwrap());
            let input = area(u64::from(channels) + 11, usize::from(channels), frames);
            let mut conv = Converter::new(config).unwrap();
            for &(a, b) in runs {
                let want = reference(&mut conv, &input, a, b);
                assert_eq!(want.per_frame.iter().sum::<u64>(), want.stats.clipped_total());
                clipped += want.stats.clipped_total();
                for separate in [false, true] {
                    let got = new_path(&mut conv, &input, a, b, separate);
                    assert!(
                        got == want,
                        "{rate} {cutoff:?} {channels} ch, frames {a}..{b}, separate {separate}"
                    );
                }
            }
        }
        if cfg!(miri) {
            break;
        }
    }
    // The clip counts were exercised.
    assert!(clipped > 0);
}

fn config() -> ProptestConfig {
    if cfg!(miri) {
        ProptestConfig { cases: 2, failure_persistence: None, ..ProptestConfig::default() }
    } else {
        // PROPTEST_CASES, if set, overrides the default number of cases.
        let cases = std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(48);
        ProptestConfig { cases, ..ProptestConfig::default() }
    }
}

proptest! {
    #![proptest_config(config())]

    /// Any split of an area into runs of separate or contiguous frames, packed or not, gives
    /// the whole area's packed bytes, and per-frame counts equal to single-frame ones.
    #[test]
    fn any_split_matches_the_whole_area(
        combo in 0usize..8,
        channels in 1u8..=6,
        frames in 1usize..=6,
        cuts in proptest::collection::vec(0usize..6, 0..4),
        separate in proptest::collection::vec(any::<bool>(), 7),
        seed in any::<u64>(),
        gain in -6.0f64..12.0,
    ) {
        let (rate, cutoff) = combos()[combo];
        let config = PcmConfig::new(rate, cutoff, channels).with_gain(Gain::from_db(gain).unwrap());
        let input = area(seed, usize::from(channels), frames);
        let mut conv = Converter::new(config).unwrap();
        let want = reference(&mut conv, &input, 0, frames);
        let mut points: Vec<usize> = cuts.into_iter().filter(|&c| c > 0 && c < frames).collect();
        points.push(0);
        points.push(frames);
        points.sort_unstable();
        points.dedup();
        let (mut packed, mut per_frame, mut total) = (Vec::new(), Vec::new(), 0);
        for (i, w) in points.windows(2).enumerate() {
            let got = new_path(&mut conv, &input, w[0], w[1], separate[i]);
            prop_assert_eq!(got.per_frame.iter().sum::<u64>(), got.stats.clipped_total());
            total += got.stats.clipped_total();
            packed.extend(got.packed);
            per_frame.extend(got.per_frame);
        }
        prop_assert!(packed == want.packed);
        prop_assert_eq!(per_frame, want.per_frame);
        prop_assert_eq!(total, want.stats.clipped_total());
    }
}

#[test]
fn errors() {
    let mut conv = Converter::new(PcmConfig::new(PcmRate::Hz176400, Cutoff::Standard, 2)).unwrap();
    let frame = vec![0u8; 2 * FRAME_BYTES_PER_CHANNEL];
    let mut out = vec![0u8; 2 * 2352 * 3];
    let mut counts = [7u64; 2];
    let bad: &[&[u8]] = &[&frame, &frame[1..]];
    assert!(matches!(
        conv.convert(Run::new(bad), PcmOut::S24le(&mut out), None),
        Err(PcmError::InputLength { .. })
    ));
    let one: &[&[u8]] = &[&frame];
    assert!(matches!(
        conv.convert(Run::new(one).with_after(Some(&frame[1..])), PcmOut::S24le(&mut out), None),
        Err(PcmError::InputLength { .. })
    ));
    assert!(matches!(
        conv.convert(Run::new(one), PcmOut::S24le(&mut out[1..]), None),
        Err(PcmError::OutputBytes { len: 14_111, expected: 14_112, .. })
    ));
    assert!(matches!(
        conv.convert(Run::new(one), PcmOut::I32(&mut [0; 4703]), None),
        Err(PcmError::OutputLength { len: 4703, expected: 4704, .. })
    ));
    assert!(matches!(
        conv.convert(Run::new(one), PcmOut::S24le(&mut out), Some(&mut counts)),
        Err(PcmError::FrameCounts { len: 2, expected: 1, .. })
    ));
    // Nothing was written.
    assert!(out.iter().all(|&v| v == 0));
    assert_eq!(counts, [7, 7]);
    // (The step from silence to all -1 at the edges overshoots, and clips.)
    let stats = conv.convert(Run::new(&frame[..]), PcmOut::S24le(&mut out), Some(&mut counts[..1])).unwrap();
    assert_eq!(counts, [stats.clipped_total(), 7]);
    // An empty run converts to nothing, from either form.
    let none: &[&[u8]] = &[];
    assert!(conv.convert(Run::new(none), PcmOut::S24le(&mut []), Some(&mut [])).is_ok());
    assert!(conv.convert(Run::new(&[0u8; 0][..]), PcmOut::I32(&mut []), None).is_ok());
}
