// SPDX-License-Identifier: MIT OR Apache-2.0

//! DSF byte ranges built from a chain (specification 8, 8.2.7, sacd-spec e069569; issue
//! #169): each file's size and header follow from the TOC, its bytes equal a sequential
//! write of its byte range, the files' streams concatenate to the chain's bytes of the
//! byte timeline (8.2.7 item 4), and a skipped or refused range splits the chain (item 3).

#![cfg(feature = "output")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;
mod output_common;

use std::borrow::Cow;

use common::{PFrame, TrackSpec};
use output_common::{Held, Lcg, check_with, contents, plain, synthetic};
use proptest::prelude::*;
use scarletbook::audio::FRAME_BYTES_PER_CHANNEL;
use scarletbook::output::{
    DsfBitsPerSample, DsfChainFile, Format, Layout, OutputOptions, PositionSource, Tags, plan_dsf_chain,
};
use scarletbook::{Error, Slot};

const PER: u64 = FRAME_BYTES_PER_CHANNEL as u64;

fn untagged() -> OutputOptions {
    OutputOptions::default().with_tags(Tags::None)
}

/// A channel type for `channels` channels (8.2.4 item 2), for unspecified layouts.
fn channel_type(channels: u8) -> u32 {
    [1, 2, 3, 4, 6, 7][usize::from(channels) - 1]
}

fn reverse(b: u8) -> u8 {
    (0..8).fold(0, |r, i| r | (((b >> i) & 1) << (7 - i)))
}

/// Byte `k` of channel `c`'s byte timeline (8.2.7 item 1), from the content of every
/// position of the area from 0.
fn timeline(all: &[Vec<u8>], channels: usize, c: usize, k: u64) -> u8 {
    all[(k / PER) as usize][(k % PER) as usize * channels + c]
}

/// `[p_i, p_{i+1})` of each range of the chain `bounds`, as 8.2.7 item 3 states it.
fn spec_bytes(bounds: &[u32]) -> Vec<(u64, u64)> {
    let a1 = u64::from(bounds[0]);
    let p = |a: u32| PER * a1 + 4096 * ((PER * (u64::from(a) - a1)) / 4096);
    let k = bounds.len() - 1;
    (0..k)
        .map(|i| (p(bounds[i]), if i + 1 == k { PER * u64::from(bounds[k]) } else { p(bounds[i + 1]) }))
        .collect()
}

/// A DSF file holding the byte range `[p, q)`, written sequentially from 8.2.1 to 8.2.3
/// and 8.2.7 item 2, with bits per sample 1 and no tag.
fn sequential(all: &[Vec<u8>], channels: usize, channel_type: u32, (p, q): (u64, u64)) -> Vec<u8> {
    let s = (q - p) as usize;
    let blocks = s.div_ceil(4096);
    let n = 4096 * blocks * channels;
    let mut f = Vec::new();
    f.extend_from_slice(b"DSD ");
    f.extend_from_slice(&28u64.to_le_bytes());
    f.extend_from_slice(&((92 + n) as u64).to_le_bytes());
    f.extend_from_slice(&0u64.to_le_bytes());
    f.extend_from_slice(b"fmt ");
    f.extend_from_slice(&52u64.to_le_bytes());
    for v in [1, 0, channel_type, channels as u32, 2_822_400, 1] {
        f.extend_from_slice(&v.to_le_bytes());
    }
    f.extend_from_slice(&(s as u64 * 8).to_le_bytes());
    f.extend_from_slice(&4096u32.to_le_bytes());
    f.extend_from_slice(&0u32.to_le_bytes());
    f.extend_from_slice(b"data");
    f.extend_from_slice(&((n + 12) as u64).to_le_bytes());
    for j in 0..blocks {
        for c in 0..channels {
            for i in 0..4096 {
                let k = j * 4096 + i;
                f.push(if k < s { reverse(timeline(all, channels, c, p + k as u64)) } else { 0 });
            }
        }
    }
    f
}

/// The streams of a DSF file with bits per sample 1, read back from its bytes alone: the
/// sound data's blocks per channel, cut to the sample count, bits reversed again.
fn streams(file: &[u8]) -> Vec<Vec<u8>> {
    let u32_at = |o: usize| u32::from_le_bytes(file[o..o + 4].try_into().unwrap());
    let u64_at = |o: usize| u64::from_le_bytes(file[o..o + 8].try_into().unwrap());
    let channels = u32_at(52) as usize;
    let s = (u64_at(64) / 8) as usize;
    let blocks = s.div_ceil(4096);
    (0..channels)
        .map(|c| {
            let mut stream = Vec::new();
            for j in 0..blocks {
                let at = 92 + (j * channels + c) * 4096;
                stream.extend_from_slice(&file[at..at + 4096]);
            }
            stream.truncate(s);
            stream.into_iter().map(reverse).collect()
        })
        .collect()
}

/// A source that must not be asked for any position: sizes and headers need no audio.
struct NoAudio;

impl PositionSource for NoAudio {
    fn content(&mut self, t: u32) -> Result<Cow<'_, [u8]>, Error> {
        panic!("position {t} read for a header");
    }
}

/// The layout of a planned file.
fn layout_of(toc: &scarletbook::Toc, file: &DsfChainFile, options: &OutputOptions) -> Layout {
    let options = options.clone().with_dsf_chain(Some(file.chain));
    Layout::new(toc, Slot::TwoChannel, file.range.0, file.range.1, Format::Dsf, &options).unwrap()
}

/// Checks the sizes and header of every file of the chain `bounds`, without audio, against
/// 8.2.5 and 8.2.7 items 3 and 4, and returns each file's `S`.
fn check_sizes(toc: &scarletbook::Toc, channels: u8, bounds: &[u32]) -> Vec<u64> {
    let options = untagged().with_dsf_unspecified_channel_type(Some(channel_type(channels)));
    let plan = plan_dsf_chain(toc, Slot::TwoChannel, bounds).unwrap();
    let want = spec_bytes(bounds);
    let c = u64::from(channels);
    let mut sizes = Vec::new();
    for (i, (file, &(p, q))) in plan.iter().zip(&want).enumerate() {
        let file = file.as_ref().unwrap();
        assert_eq!(file.range, (bounds[i], bounds[i + 1]));
        assert_eq!(file.chain, (bounds[0], *bounds.last().unwrap()));
        assert_eq!(file.bytes, (p, q), "file {i} of {bounds:?}");
        let s = q - p;
        let last = i + 1 == want.len();
        assert_eq!(file.is_last(), last);
        if !last {
            // Whole unpadded blocks (8.2.7 item 4).
            assert!(s % 4096 == 0 && s >= 4096, "file {i} of {bounds:?}: S = {s}");
        }
        let layout = layout_of(toc, file, &options);
        assert_eq!(layout.byte_range(), Some((p, q)));
        let n = 4096 * s.div_ceil(4096) * c;
        assert_eq!(layout.size(), 92 + n);
        let mut h = [0u8; 92];
        assert_eq!(layout.fill(0, &mut h, &mut NoAudio).unwrap(), 92);
        assert_eq!(u64::from_le_bytes(h[12..20].try_into().unwrap()), 92 + n);
        assert_eq!(u64::from_le_bytes(h[64..72].try_into().unwrap()), 8 * s);
        assert_eq!(u64::from_le_bytes(h[84..92].try_into().unwrap()), n + 12);
        sizes.push(s);
    }
    sizes
}

#[test]
fn sizes_follow_from_the_toc_at_the_edge_cases() {
    // 300 mono positions: 128 positions are exactly 147 blocks (4704 x 128 = 4096 x 147).
    let disc = synthetic(1, &plain(1, 300), vec![TrackSpec { start: 0, duration: 300 }], 300);
    let toc = disc.toc().clone();
    // K = 1: the whole range, padded.
    assert_eq!(check_sizes(&toc, 1, &[5, 9]), [4 * PER]);
    // Ranges of one position: each but the last is one block, the rest goes to the next.
    assert_eq!(check_sizes(&toc, 1, &[0, 1, 2, 3]), [4096, 4096, 3 * PER - 8192]);
    assert_eq!(check_sizes(&toc, 1, &[7, 8]), [PER]);
    // Exact multiples of 4096: no carried bytes, and no pad in the last file.
    assert_eq!(check_sizes(&toc, 1, &[0, 128, 256]), [128 * PER, 128 * PER]);
    assert_eq!(check_sizes(&toc, 1, &[0, 128, 129]), [128 * PER, PER]);
    assert_eq!(check_sizes(&toc, 1, &[3, 131, 132, 259]), [128 * PER, 4096, 598_016]);
    // Chains starting inside a block's worth of the area, and other offsets.
    assert_eq!(check_sizes(&toc, 1, &[1, 2, 130]), [4096, 130 * PER - (PER + 4096)]);
    for bounds in [&[10, 11, 13, 20, 21][..], &[0, 127, 128, 255, 256], &[299, 300]] {
        let sizes = check_sizes(&toc, 1, bounds);
        assert_eq!(sizes.iter().sum::<u64>(), PER * u64::from(bounds[bounds.len() - 1] - bounds[0]));
    } // Six channels: n = 4096 x B x 6.
    let disc = synthetic(6, &plain(6, 4), vec![TrackSpec { start: 0, duration: 4 }], 4);
    assert_eq!(check_sizes(disc.toc(), 6, &[0, 1, 2, 4]), [4096, 4096, 4 * PER - 8192]);
}

#[test]
fn one_range_is_the_range_of_positions() {
    let mut disc = synthetic(2, &plain(2, 5), vec![TrackSpec { start: 0, duration: 5 }], 5);
    let plan = disc.plan_dsf_chain(Slot::TwoChannel, &[1, 4]).unwrap();
    let file = *plan[0].as_ref().unwrap();
    assert_eq!((file.range, file.chain, file.bytes), ((1, 4), (1, 4), (PER, 4 * PER)));
    for bits in [DsfBitsPerSample::One, DsfBitsPerSample::Eight] {
        let options = untagged().with_dsf_bits_per_sample(bits);
        let plain_layout = disc.output_layout(Slot::TwoChannel, 1, 4, Format::Dsf, &options).unwrap();
        let chained = options.clone().with_dsf_chain(Some(file.chain));
        let chain_layout = disc.output_layout(Slot::TwoChannel, 1, 4, Format::Dsf, &chained).unwrap();
        let mut a = vec![0; plain_layout.size() as usize];
        let mut b = vec![0; chain_layout.size() as usize];
        plain_layout.read_at(&mut disc, 0, &mut a).unwrap();
        chain_layout.read_at(&mut disc, 0, &mut b).unwrap();
        assert!(a == b);
        assert_eq!(plain_layout.byte_range(), chain_layout.byte_range());
    }
}

#[test]
fn a_skipped_or_refused_range_splits_the_chain() {
    let disc = synthetic(2, &plain(2, 9), vec![TrackSpec { start: 0, duration: 9 }], 9);
    let toc = disc.toc().clone();
    // [2, 2) is empty (track-empty); [7, 12) ends past the play range (frame-range-invalid);
    // [12, 9) runs backwards.
    let bounds = [0, 1, 2, 2, 5, 7, 12, 9];
    let plan = disc.plan_dsf_chain(Slot::TwoChannel, &bounds).unwrap();
    assert_eq!(plan.len(), 7);
    for (i, file) in plan.iter().enumerate() {
        let (a, b) = (bounds[i], bounds[i + 1]);
        let direct = Layout::new(&toc, Slot::TwoChannel, a, b, Format::Dsf, &untagged());
        match file {
            Ok(_) => assert!(direct.is_ok()),
            Err(e) => assert_eq!(e.to_string(), direct.unwrap_err().to_string()),
        }
    }
    assert!(matches!(plan[2], Err(Error::TrackEmpty)));
    assert!(matches!(plan[5], Err(Error::FrameRangeInvalid { .. })));
    assert!(matches!(plan[6], Err(Error::FrameRangeInvalid { .. })));
    let chains: Vec<_> = plan.iter().map(|f| f.as_ref().ok().map(|f| f.chain)).collect();
    assert_eq!(chains, [Some((0, 2)), Some((0, 2)), None, Some((2, 7)), Some((2, 7)), None, None]);
    // The chain after the split starts again at a position boundary.
    let after = plan[3].as_ref().unwrap();
    assert_eq!(after.bytes.0, 2 * PER);
    assert!(plan[1].as_ref().unwrap().is_last());
    assert_eq!(plan[1].as_ref().unwrap().bytes.1, 2 * PER);

    let e = disc.plan_dsf_chain(Slot::TwoChannel, &[3]).unwrap_err();
    assert!(matches!(e, Error::InvalidArgument(_)), "{e}");
    let e = disc.plan_dsf_chain(Slot::Multichannel, &[0, 1]).unwrap_err();
    assert!(matches!(e, Error::NoSuchArea(_)), "{e}");
}

#[test]
fn each_file_has_the_source_track_of_its_range() {
    let tracks = vec![
        TrackSpec { start: 0, duration: 3 },
        TrackSpec { start: 3, duration: 2 },
        TrackSpec { start: 5, duration: 4 },
    ];
    let disc = synthetic(2, &plain(2, 9), tracks, 9);
    let mut bounds = vec![0];
    for n in 1..=3 {
        let (a, b) = disc.track_range_with_pause(Slot::TwoChannel, n).unwrap();
        assert_eq!(a, *bounds.last().unwrap(), "the tracks with pause meet end to end");
        bounds.push(b);
    }
    for (n, file) in disc.plan_dsf_chain(Slot::TwoChannel, &bounds).unwrap().iter().enumerate() {
        let file = file.as_ref().unwrap();
        let options = untagged().with_dsf_chain(Some(file.chain));
        let layout = disc.output_layout(Slot::TwoChannel, file.range.0, file.range.1, Format::Dsf, &options);
        let layout = layout.unwrap();
        assert_eq!(layout.source_track(), Some(n as u8 + 1));
        assert_eq!(layout.range(), file.range);
        // Files after the first read the end of the range before them (8.2.7 item 4).
        let sound = layout.regions()[1];
        let first = layout.positions_for(sound.offset..sound.offset + sound.len).start;
        assert_eq!(first, if n == 0 { 0 } else { file.range.0 - 1 });
    }
}

#[test]
fn chain_options_are_checked() {
    let disc = synthetic(2, &plain(2, 9), vec![TrackSpec { start: 0, duration: 9 }], 9);
    let chain = untagged().with_dsf_chain(Some((2, 7)));
    let bad = [
        (Format::DsdiffDsd, 3, 5, chain.clone()),
        (Format::Dsf, 1, 5, chain.clone()),
        (Format::Dsf, 3, 8, chain.clone()),
        (Format::Dsf, 3, 5, {
            let mut o = chain.clone();
            o.dsf_chain_end = None;
            o
        }),
        (Format::Dsf, 3, 5, {
            let mut o = chain.clone();
            o.dsf_chain_first = None;
            o
        }),
    ];
    for (format, a, b, options) in bad {
        let e = disc.output_layout(Slot::TwoChannel, a, b, format, &options).unwrap_err();
        assert!(matches!(e, Error::InvalidArgument(_)), "{format:?} [{a}, {b}): {e}");
    }
    // The checks of 8.1 item 2 come first.
    let e = disc.output_layout(Slot::TwoChannel, 4, 4, Format::Dsf, &chain).unwrap_err();
    assert!(matches!(e, Error::TrackEmpty), "{e}");
    let ok = disc.output_layout(Slot::TwoChannel, 2, 7, Format::Dsf, &chain).unwrap();
    assert_eq!(ok.byte_range(), Some((2 * PER, 7 * PER)));
    assert_eq!(
        disc.output_layout(Slot::TwoChannel, 2, 7, Format::DsdiffDsd, &untagged()).unwrap().byte_range(),
        None
    );
}

/// Checks every file of the chain `bounds` of an area of `n` positions against its
/// sequential write, through `read_at` and `fill`, and the concatenation of 8.2.7 item 4.
fn check_chain(
    disc: &mut scarletbook::Disc<std::io::Cursor<Vec<u8>>>,
    channels: u8,
    n: u32,
    bounds: &[u32],
    rng: &mut Lcg,
) {
    let all = contents(disc, Slot::TwoChannel, 0, n);
    let toc = disc.toc().clone();
    let ct = channel_type(channels);
    let options = untagged().with_dsf_unspecified_channel_type(Some(ct));
    let c = usize::from(channels);
    let plan = plan_dsf_chain(&toc, Slot::TwoChannel, bounds).unwrap();
    // The concatenated streams of the current chain, per channel.
    let mut joined: Vec<Vec<u8>> = vec![Vec::new(); c];
    for file in plan.iter().flatten() {
        let o = options.clone().with_dsf_chain(Some(file.chain));
        let layout =
            disc.output_layout(Slot::TwoChannel, file.range.0, file.range.1, Format::Dsf, &o).unwrap();
        assert_eq!(layout_of(&toc, file, &options).size(), layout.size());
        let expected = sequential(&all, c, ct, file.bytes);
        check_with(disc, &layout, &expected, &mut Held { first: 0, content: &all, asked: Vec::new() }, rng);
        if file.range.0 == file.chain.0 {
            joined = vec![Vec::new(); c];
        }
        for (j, s) in joined.iter_mut().zip(streams(&expected)) {
            j.extend_from_slice(&s);
        }
        if file.is_last() {
            // 8.2.7 item 4: bytes 4704 a_1 to 4704 b_K - 1 of the timeline, unchanged.
            for (ch, j) in joined.iter().enumerate() {
                let want: Vec<u8> = (PER * u64::from(file.chain.0)..PER * u64::from(file.chain.1))
                    .map(|k| timeline(&all, c, ch, k))
                    .collect();
                assert!(*j == want, "channel {ch} of chain {:?}", file.chain);
            }
        }
    }
}

/// Positions of `channels` channels, each plain, an invalid DST frame of a few bytes, or
/// missing, as `kinds` says (0, 1 and 2).
fn frames(channels: u8, kinds: &[u8]) -> Vec<PFrame> {
    let mut frames = Vec::new();
    for (number, kind) in kinds.iter().enumerate() {
        let number = number as u32;
        match kind {
            0 => frames.push(plain(channels, number + 1).pop().unwrap()),
            1 => frames.push(PFrame { number, dst: true, data: (0..(number as u8 * 17 + 5)).collect() }),
            _ => {}
        }
    }
    frames
}

#[test]
fn carried_bytes_of_missing_and_invalid_positions_are_their_fills() {
    // Positions 2 (missing) and 4 (an invalid DST frame) end the ranges before files 2 and
    // 3, whose first bytes they are.
    let kinds = [0, 0, 2, 0, 1, 0, 0];
    let mut disc = synthetic(2, &frames(2, &kinds), vec![TrackSpec { start: 0, duration: 7 }], 7);
    let bounds = [0, 3, 5, 7];
    check_chain(&mut disc, 2, 7, &bounds, &mut Lcg(169));
    let plan = disc.plan_dsf_chain(Slot::TwoChannel, &bounds).unwrap();
    let all = contents(&mut disc, Slot::TwoChannel, 0, 7);
    for i in [1, 2] {
        let file = plan[i].as_ref().unwrap();
        let carried = PER * u64::from(file.range.0) - file.bytes.0;
        assert!(carried > 0);
        let layout = layout_of(disc.toc(), file, &untagged());
        let mut first = vec![0; carried as usize];
        layout.read_at(&mut disc, 92, &mut first).unwrap();
        let tail = &all[file.range.0 as usize - 1];
        let want: Vec<u8> = (PER - carried..PER).map(|k| reverse(tail[k as usize * 2])).collect();
        assert!(first == want, "file {i}");
    }
}

fn config() -> ProptestConfig {
    if cfg!(miri) {
        ProptestConfig { cases: 2, failure_persistence: None, ..ProptestConfig::default() }
    } else {
        ProptestConfig { cases: 32, ..ProptestConfig::default() }
    }
}

proptest! {
    #![proptest_config(config())]

    /// Chains over areas of 1 to 6 channels with plain, invalid DST and missing positions,
    /// split by empty ranges: every file equals its sequential write, and each chain's
    /// streams concatenate to its bytes of the timeline.
    #[test]
    fn synthetic_chains(channels in 1u8..=if cfg!(miri) { 2 } else { 6 }, kinds in proptest::collection::vec(0u8..3, 1..if cfg!(miri) { 3 } else { 8 }), cuts in proptest::collection::vec(0u32..8, 0..5), a in 0u32..8, seed in any::<u64>()) {
        let n = kinds.len() as u32;
        let mut disc = synthetic(channels, &frames(channels, &kinds), vec![TrackSpec { start: 0, duration: n }], n);
        let first = a % n;
        let mut bounds = vec![first];
        let mut inner: Vec<u32> = cuts.iter().map(|&c| first + c % (n - first + 1)).collect();
        inner.sort_unstable();
        bounds.extend(inner);
        bounds.push(n);
        check_chain(&mut disc, channels, n, &bounds, &mut Lcg(seed));
    }
}
