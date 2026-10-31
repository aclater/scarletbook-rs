// SPDX-License-Identifier: MIT OR Apache-2.0

//! Shared checks of output layouts (issue #131): a layout against a sequential write of
//! the same file.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::borrow::Cow;
use std::io::{Cursor, Read, Seek};
use std::path::PathBuf;

use crate::common::{PFrame, Spec, TrackSpec, pack};
use scarletbook::audio::FRAME_BYTES_PER_CHANNEL;
use scarletbook::output::{Layout, PositionSource, RegionKind};
use scarletbook::{Disc, Error, Slot};

/// The content of every position of a range, decoded.
pub fn contents<R: Read + Seek>(disc: &mut Disc<R>, slot: Slot, first: u32, end: u32) -> Vec<Vec<u8>> {
    disc.range_audio(slot, first, end).unwrap().map(|f| f.unwrap().data).collect()
}

/// Positions of a range held in memory.
pub struct Held<'a> {
    pub first: u32,
    pub content: &'a [Vec<u8>],
    pub asked: Vec<u32>,
}

impl PositionSource for Held<'_> {
    fn content(&mut self, t: u32) -> Result<Cow<'_, [u8]>, Error> {
        self.asked.push(t);
        Ok(Cow::Borrowed(&self.content[(t - self.first) as usize]))
    }
}

/// A small deterministic generator for read ranges.
pub struct Lcg(pub u64);

impl Lcg {
    pub fn below(&mut self, n: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % n.max(1)
    }
}

/// Checks a layout against the sequential write `expected`: size, regions, a whole read,
/// reads at region boundaries and the end, and random reads through [`Layout::read_at`]
/// and through [`Layout::positions_for`] and [`Layout::fill`].
pub fn check<R: Read + Seek>(
    disc: &mut Disc<R>,
    layout: &Layout,
    expected: &[u8],
    content: &[Vec<u8>],
    rng: &mut Lcg,
) {
    let size = layout.size();
    assert_eq!(size, expected.len() as u64);
    let regions = layout.regions();
    assert_eq!(regions[0].kind, RegionKind::Header);
    assert!(regions.iter().any(|r| r.kind == RegionKind::SoundData));
    let mut at = 0;
    for r in regions {
        assert_eq!(r.offset, at);
        at += r.len;
    }
    assert_eq!(at, size);

    let mut whole = vec![0xAA; expected.len() + 10];
    assert_eq!(layout.read_at(disc, 0, &mut whole).unwrap(), expected.len());
    assert!(whole[..expected.len()] == *expected, "whole read differs");
    assert_eq!(layout.read_at(disc, size, &mut [0; 4]).unwrap(), 0);

    let (first, _) = layout.range();
    let span = 4096 * u64::from(layout.channels());
    let mut reads: Vec<(u64, u64)> = vec![(size - 1, 1)];
    for r in &regions[1..] {
        reads.push((r.offset - 1, 2));
        reads.push((r.offset.saturating_sub(span / 2), span));
    }
    for _ in 0..if cfg!(miri) { 2 } else { 24 } {
        let at = rng.below(size);
        let len = 1 + rng.below((size - at).min(3 * span));
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

pub fn corpus() -> Vec<(String, Vec<u8>)> {
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

pub fn synthetic(
    channels: u8,
    frames: &[PFrame],
    tracks: Vec<TrackSpec>,
    playtime: u32,
) -> Disc<Cursor<Vec<u8>>> {
    let spec = Spec { channels, playtime, tracks, audio: pack(frames) };
    Disc::open(Cursor::new(spec.build())).unwrap()
}

/// `n` plain frames of `channels` channels, numbered from 0, each with its own bytes.
pub fn plain(channels: u8, n: u32) -> Vec<PFrame> {
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
