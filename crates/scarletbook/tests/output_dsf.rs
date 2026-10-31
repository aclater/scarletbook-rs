// SPDX-License-Identifier: MIT OR Apache-2.0

//! DSF layouts (specification 8, 8.2; issue #131): the size known from the TOC equals a
//! sequential write's, the header holds 8.2.1's fields, and every byte range read at random
//! equals the same bytes of the sequential write, through [`Layout::read_at`] and through
//! [`Layout::positions_for`] and [`Layout::fill`].

#![cfg(feature = "output")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;
mod output_common;

use std::io::Cursor;

use common::TrackSpec;
use output_common::{Held, Lcg, check, contents, corpus, corpus_ranges, enough, plain, synthetic, variants};
use proptest::prelude::*;
use scarletbook::audio::FRAME_BYTES_PER_CHANNEL;
use scarletbook::output::{DsfBitsPerSample, Format, Layout, OutputOptions, RegionKind};
use scarletbook::{Disc, Error, OutputRefusal, Slot};

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
            for (first, end) in corpus_ranges(&disc, area.slot) {
                if disc.check_range(area.slot, first, end).is_err() {
                    let e = disc.output_layout(area.slot, first, end, Format::Dsf, &OutputOptions::default());
                    assert_eq!(
                        e.unwrap_err().to_string(),
                        disc.check_range(area.slot, first, end).unwrap_err().to_string()
                    );
                    continue;
                }
                let content = contents(&mut disc, area.slot, first, end);
                for bits in variants(&[DsfBitsPerSample::One, DsfBitsPerSample::Eight], checked) {
                    let options = OutputOptions::default().with_dsf_bits_per_sample(bits);
                    let layout = disc.output_layout(area.slot, first, end, Format::Dsf, &options).unwrap();
                    let bits = if bits == DsfBitsPerSample::One { 1 } else { 8 };
                    let expected = sequential(&content, usize::from(area.channel_count), channel_type, bits);
                    assert_eq!(layout.regions()[0].len, 92);
                    assert_eq!(layout.regions()[1].kind, RegionKind::SoundData);
                    check(&mut disc, &layout, &expected, &content, &mut rng);
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > enough(), "only {checked} layouts checked");
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
    for ok in if cfg!(miri) { vec![5] } else { vec![4, 5] } {
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
    fn synthetic_ranges(channels in 1u8..=if cfg!(miri) { 2 } else { 6 }, n in 1u32..if cfg!(miri) { 3 } else { 6 }, gaps in proptest::collection::vec(any::<bool>(), 6), a in 0u32..6, len in 1u32..6, seed in any::<u64>()) {
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

#[test]
fn read_at_refuses_a_disc_of_another_image() {
    let disc = synthetic(2, &plain(2, 2), vec![TrackSpec { start: 0, duration: 2 }], 2);
    let layout = disc.output_layout(Slot::TwoChannel, 0, 2, Format::Dsf, &OutputOptions::default()).unwrap();
    // Another image with the same area: same channel count, other TOC sectors.
    let mut other = synthetic(2, &plain(2, 3), vec![TrackSpec { start: 0, duration: 3 }], 3);
    let e = layout.read_at(&mut other, 0, &mut [0; 200]).unwrap_err();
    assert!(matches!(e, Error::InvalidArgument(_)), "{e}");
    // The same image opened again is the same image.
    let mut again = synthetic(2, &plain(2, 2), vec![TrackSpec { start: 0, duration: 2 }], 2);
    assert_eq!(layout.read_at(&mut again, 0, &mut [0; 200]).unwrap(), 200);
    // A layout made without a disc is checked only for its area.
    let area = again.toc().area(Slot::TwoChannel).unwrap().clone();
    let free = Layout::new(&area, 1 << 20, 0, 2, Format::Dsf, &OutputOptions::default()).unwrap();
    assert_eq!(free.read_at(&mut other, 0, &mut [0; 200]).unwrap(), 200);
}
