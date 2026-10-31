// SPDX-License-Identifier: MIT OR Apache-2.0

//! DSF layouts (specification 8, 8.2; issue #131): the size known from the TOC equals a
//! sequential write's, the header holds 8.2.1's fields, and every byte range read at random
//! equals the same bytes of the sequential write, through [`Layout::read_at`] and through
//! [`Layout::positions_for`] and [`Layout::fill`].

#![cfg(feature = "output")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::borrow::Cow;
use std::io::Cursor;
use std::path::PathBuf;

use common::{PFrame, Spec, TrackSpec, pack};
use proptest::prelude::*;
use scarletbook::audio::FRAME_BYTES_PER_CHANNEL;
use scarletbook::output::{DsfBitsPerSample, Format, Layout, OutputOptions, PositionSource, RegionKind};
use scarletbook::{Disc, Error, OutputRefusal, Slot};

/// The content of every position of a range, decoded.
fn contents<R: std::io::Read + std::io::Seek>(
    disc: &mut Disc<R>,
    slot: Slot,
    first: u32,
    end: u32,
) -> Vec<Vec<u8>> {
    disc.range_audio(slot, first, end).unwrap().map(|f| f.unwrap().data).collect()
}

/// Reverses a byte's bits one at a time (8.2.3).
fn reverse(b: u8) -> u8 {
    (0..8).fold(0, |r, i| r | (((b >> i) & 1) << (7 - i)))
}

/// A DSF file written sequentially from specification 8, 8.2, without a tag.
fn sequential(content: &[Vec<u8>], channels: usize, channel_type: u32, bits: u32) -> Vec<u8> {
    let s = content.len() * FRAME_BYTES_PER_CHANNEL;
    let blocks = s.div_ceil(4096);
    let n = 4096 * blocks * channels;
    let mut f = Vec::new();
    f.extend_from_slice(b"DSD ");
    f.extend_from_slice(&28u64.to_le_bytes());
    f.extend_from_slice(&((92 + n) as u64).to_le_bytes());
    f.extend_from_slice(&0u64.to_le_bytes());
    f.extend_from_slice(b"fmt ");
    f.extend_from_slice(&52u64.to_le_bytes());
    for v in [1, 0, channel_type, channels as u32, 2_822_400, bits] {
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
                f.push(if k < s {
                    let b =
                        content[k / FRAME_BYTES_PER_CHANNEL][(k % FRAME_BYTES_PER_CHANNEL) * channels + c];
                    if bits == 1 { reverse(b) } else { b }
                } else {
                    0
                });
            }
        }
    }
    f
}

/// Positions of a range held in memory.
struct Held<'a> {
    first: u32,
    content: &'a [Vec<u8>],
    asked: Vec<u32>,
}

impl PositionSource for Held<'_> {
    fn content(&mut self, t: u32) -> Result<Cow<'_, [u8]>, Error> {
        self.asked.push(t);
        Ok(Cow::Borrowed(&self.content[(t - self.first) as usize]))
    }
}

/// A small deterministic generator for read ranges.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % n.max(1)
    }
}

/// Checks a layout against the sequential write: size, regions, a whole read, reads at
/// region, row and block boundaries, and random reads through both paths.
fn check<R: std::io::Read + std::io::Seek>(
    disc: &mut Disc<R>,
    layout: &Layout,
    expected: &[u8],
    content: &[Vec<u8>],
    rng: &mut Lcg,
) {
    assert_eq!(layout.size(), expected.len() as u64);
    let regions = layout.regions();
    assert_eq!(regions[0].kind, RegionKind::Header);
    assert_eq!((regions[0].offset, regions[0].len), (0, 92));
    assert_eq!(regions[1].kind, RegionKind::SoundData);
    assert_eq!(regions[1].offset + regions[1].len, layout.size());

    let mut whole = vec![0xAA; expected.len() + 10];
    assert_eq!(layout.read_at(disc, 0, &mut whole).unwrap(), expected.len());
    assert!(whole[..expected.len()] == *expected, "whole read differs");
    assert_eq!(layout.read_at(disc, layout.size(), &mut [0; 4]).unwrap(), 0);

    let (first, _) = layout.range();
    let row = 4096 * u64::from(layout.channels());
    let size = layout.size();
    let mut reads: Vec<(u64, u64)> =
        vec![(0, 92), (90, 4), (91, 1), (92, 1), (92 + row - 1, 2), (size - 1, 1)];
    for _ in 0..24 {
        let at = rng.below(size);
        let len = 1 + rng.below((size - at).min(3 * row));
        reads.push((at, len));
    }
    for (at, len) in reads {
        let len = len.min(size - at);
        let want = &expected[at as usize..(at + len) as usize];
        let mut got = vec![0; len as usize];
        assert_eq!(layout.read_at(disc, at, &mut got).unwrap(), len as usize);
        assert!(got == want, "read_at {at}+{len} differs");

        let positions = layout.positions_for(at..at + len);
        let mut held = Held { first, content, asked: Vec::new() };
        let mut got = vec![0; len as usize];
        assert_eq!(layout.fill(at, &mut got, &mut held).unwrap(), len as usize);
        assert!(got == want, "fill {at}+{len} differs");
        assert!(held.asked.iter().all(|t| positions.contains(t)), "fill read outside positions_for");
    }
}

fn corpus() -> Vec<(String, Vec<u8>)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/images");
    let mut images: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let p = e.unwrap().path();
            (p.file_name().unwrap().to_string_lossy().into_owned(), std::fs::read(&p).unwrap())
        })
        .collect();
    images.sort();
    assert!(images.len() > 50);
    images
}

/// Every corpus area with a defined layout, for each track, each track with pause and the
/// play range, with both bit orders.
#[test]
#[cfg_attr(miri, ignore = "the corpus is too large for Miri")]
fn corpus_layouts_equal_sequential_writes() {
    let mut rng = Lcg(131);
    let mut checked = 0;
    for (_name, bytes) in corpus() {
        let Ok(mut disc) = Disc::open(Cursor::new(bytes)) else { continue };
        for area in disc.toc().areas.clone() {
            let channel_type = match area.channel_speakers().map(<[_]>::len) {
                Some(2) => 2,
                Some(5) => 6,
                Some(6) => 7,
                _ => continue,
            };
            let mut ranges: Vec<(u32, u32)> = vec![(0, area.playtime_frames())];
            for t in &area.tracks {
                ranges.push((t.start_frame, t.end_frame()));
                ranges.push(disc.track_range_with_pause(area.slot, u32::from(t.number)).unwrap());
            }
            ranges.sort_unstable();
            ranges.dedup();
            for (first, end) in ranges {
                if disc.check_range(area.slot, first, end).is_err() {
                    let e = disc.output_layout(area.slot, first, end, Format::Dsf, &OutputOptions::default());
                    assert_eq!(
                        e.unwrap_err().to_string(),
                        disc.check_range(area.slot, first, end).unwrap_err().to_string()
                    );
                    continue;
                }
                let content = contents(&mut disc, area.slot, first, end);
                for bits in [DsfBitsPerSample::One, DsfBitsPerSample::Eight] {
                    let options = OutputOptions::default().with_dsf_bits_per_sample(bits);
                    let layout = disc.output_layout(area.slot, first, end, Format::Dsf, &options).unwrap();
                    let bits = if bits == DsfBitsPerSample::One { 1 } else { 8 };
                    let expected = sequential(&content, usize::from(area.channel_count), channel_type, bits);
                    check(&mut disc, &layout, &expected, &content, &mut rng);
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 100, "only {checked} layouts checked");
}

fn synthetic(
    channels: u8,
    frames: &[PFrame],
    tracks: Vec<TrackSpec>,
    playtime: u32,
) -> Disc<Cursor<Vec<u8>>> {
    let spec = Spec { channels, playtime, tracks, audio: pack(frames) };
    Disc::open(Cursor::new(spec.build())).unwrap()
}

fn plain(channels: u8, n: u32) -> Vec<PFrame> {
    (0..n)
        .map(|number| PFrame {
            number,
            dst: false,
            data: (0..usize::from(channels) * FRAME_BYTES_PER_CHANNEL)
                .map(|i| (i as u32 * 7 + number * 13) as u8)
                .collect(),
        })
        .collect()
}

#[test]
fn unspecified_layouts_are_refused_unless_a_channel_type_is_given() {
    let mut disc = synthetic(4, &plain(4, 3), vec![TrackSpec { start: 0, duration: 3 }], 3);
    let e = disc.output_layout(Slot::TwoChannel, 0, 3, Format::Dsf, &OutputOptions::default()).unwrap_err();
    assert!(
        matches!(e, Error::OutputRefused(OutputRefusal::UnspecifiedLayout { channels: 4, config: 0 })),
        "{e}"
    );
    assert_eq!(e.class(), scarletbook::ErrorClass::Usage);
    for wrong in [0, 1, 2, 3, 6, 7, 8] {
        let options = OutputOptions::default().with_dsf_unspecified_channel_type(Some(wrong));
        let e = disc.output_layout(Slot::TwoChannel, 0, 3, Format::Dsf, &options).unwrap_err();
        assert!(matches!(e, Error::InvalidArgument(_)), "{wrong}: {e}");
    }
    for ok in [4, 5] {
        let options = OutputOptions::default().with_dsf_unspecified_channel_type(Some(ok));
        let layout = disc.output_layout(Slot::TwoChannel, 0, 3, Format::Dsf, &options).unwrap();
        let content = contents(&mut disc, Slot::TwoChannel, 0, 3);
        check(&mut disc, &layout, &sequential(&content, 4, ok, 1), &content, &mut Lcg(ok.into()));
    }
}

#[test]
fn content_of_the_wrong_size_is_an_error() {
    let disc = synthetic(2, &plain(2, 2), vec![TrackSpec { start: 0, duration: 2 }], 2);
    let layout = disc.output_layout(Slot::TwoChannel, 0, 2, Format::Dsf, &OutputOptions::default()).unwrap();
    let short = vec![vec![0u8; 100]; 2];
    let mut held = Held { first: 0, content: &short, asked: Vec::new() };
    let e = layout.fill(0, &mut [0; 200], &mut held).unwrap_err();
    assert!(matches!(e, Error::InvalidArgument(_)), "{e}");
    // The header alone needs no position.
    assert!(layout.positions_for(0..92).is_empty());
    assert_eq!(layout.fill(0, &mut [0; 92], &mut held).unwrap(), 92);
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

    /// Ranges of plain areas of 1 to 6 channels with missing positions, any range and any
    /// read: the layout's bytes are the sequential write's.
    #[test]
    fn synthetic_ranges(channels in 1u8..=6, n in 1u32..6, gaps in proptest::collection::vec(any::<bool>(), 6), a in 0u32..6, len in 1u32..6, seed in any::<u64>()) {
        let frames: Vec<_> = plain(channels, n).into_iter().filter(|f| !gaps[f.number as usize]).collect();
        let playtime = n;
        let first = a % playtime;
        let end = (first + len).min(playtime);
        let mut disc = synthetic(channels, &frames, vec![TrackSpec { start: 0, duration: playtime }], playtime);
        let channel_type = [1, 2, 3, 4, 6, 7][usize::from(channels) - 1];
        let options = OutputOptions::default().with_dsf_unspecified_channel_type(Some(channel_type));
        let layout = disc.output_layout(Slot::TwoChannel, first, end, Format::Dsf, &options).unwrap();
        let content = contents(&mut disc, Slot::TwoChannel, first, end);
        let expected = sequential(&content, usize::from(channels), channel_type, 1);
        check(&mut disc, &layout, &expected, &content, &mut Lcg(seed));
    }
}
