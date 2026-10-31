// SPDX-License-Identifier: MIT OR Apache-2.0

//! Saved seek indexes (issue #68): saving and loading, merging, and the checks that keep a
//! stale, foreign or hostile index from doing harm.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;
mod harness;

use std::io::Cursor;

use common::{PFrame, Spec, TrackSpec, pack};
use proptest::prelude::*;
use scarletbook::{AreaIndex, Disc, Error, IndexOptions, ResumedFrom, Slot, TrackCursor, TrackFrame};

fn config() -> ProptestConfig {
    if cfg!(miri) {
        ProptestConfig { cases: 2, failure_persistence: None, ..ProptestConfig::default() }
    } else {
        let cases = std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(128);
        ProptestConfig { cases, ..ProptestConfig::default() }
    }
}

/// A disc of DST frames numbered from 0, `fill` mixed into their bytes, with a gap in the
/// numbers so that some positions are missing.
fn disc(frames: u32, fill: u8) -> (Disc<Cursor<Vec<u8>>>, u32) {
    let fs: Vec<PFrame> = (0..frames)
        .filter(|n| n % 11 != 5)
        .map(|n| PFrame { number: n, dst: true, data: vec![n as u8 ^ fill; 1500 + (n as usize * 97) % 3000] })
        .collect();
    let spec = Spec {
        channels: 2,
        playtime: frames,
        tracks: vec![TrackSpec { start: 0, duration: 1 }],
        audio: pack(&fs),
    };
    (Disc::open(Cursor::new(spec.build())).unwrap(), frames)
}

fn complete(disc: &mut Disc<Cursor<Vec<u8>>>, interval: u32) -> AreaIndex {
    let area = disc.toc().areas[0].clone();
    let n = disc.toc().sectors;
    let mut index = AreaIndex::new(&area, n, IndexOptions::new().with_interval(interval)).unwrap();
    while !index.extend(disc.image_mut(), 7).unwrap() {}
    index
}

fn frames(disc: &mut Disc<Cursor<Vec<u8>>>, cursor: TrackCursor) -> (Vec<TrackFrame>, TrackCursor) {
    let mut it = disc.read_frames(cursor);
    let v = it.by_ref().map(Result::unwrap).collect();
    (v, it.into_cursor())
}

fn straight(disc: &mut Disc<Cursor<Vec<u8>>>, first: u32, end: u32) -> Vec<TrackFrame> {
    let c = disc.range_frames_cursor(Slot::TwoChannel, first, end).unwrap();
    frames(disc, c).0
}

#[test]
#[cfg_attr(miri, ignore = "reads a few hundred sectors")]
fn saved_index_round_trips_and_reads_alike() {
    let (mut d, p) = disc(200, 0);
    let area = d.toc().areas[0].clone();
    let n = d.toc().sectors;
    for k in [1, 5, 64] {
        let index = complete(&mut d, k);
        let bytes = index.to_bytes();
        let loaded = AreaIndex::from_bytes(&area, n, &bytes).unwrap();
        assert_eq!(loaded, index);
        assert_eq!(loaded.to_bytes(), bytes);
        for first in [0, 17, 100, 199] {
            let c = TrackCursor::for_range_indexed(&area, first, p, n, &loaded).unwrap();
            assert_eq!(frames(&mut d, c).0, straight(&mut d, first, p), "K {k}, first {first}");
        }
        // Every truncation is refused.
        for len in 0..bytes.len() {
            assert!(AreaIndex::from_bytes(&area, n, &bytes[..len]).is_err(), "{len}");
        }
    }
}

#[test]
#[cfg_attr(miri, ignore = "reads a few hundred sectors")]
fn an_index_of_another_area_or_image_is_refused() {
    let (mut d, _) = disc(100, 0);
    let area = d.toc().areas[0].clone();
    let n = d.toc().sectors;
    let bytes = complete(&mut d, 8).to_bytes();
    assert!(matches!(AreaIndex::from_bytes(&area, n + 1, &bytes), Err(Error::InvalidArgument(_))));
    let mut other = area.clone();
    other.slot = Slot::Multichannel;
    assert!(matches!(AreaIndex::from_bytes(&other, n, &bytes), Err(Error::InvalidArgument(_))));
    other = area.clone();
    other.channel_count = 6;
    assert!(matches!(AreaIndex::from_bytes(&other, n, &bytes), Err(Error::InvalidArgument(_))));
}

/// An index of another image with the same key: the digest at the checkpoint differs, the
/// cursor discards the index and scans from the first sector, with the same result.
#[test]
#[cfg_attr(miri, ignore = "reads a few hundred sectors")]
fn a_stale_index_is_discarded() {
    let (mut a, p) = disc(200, 0);
    let (mut b, _) = disc(200, 0x5A);
    let area = b.toc().areas[0].clone();
    let n = b.toc().sectors;
    assert_eq!((area.clone(), n), (a.toc().areas[0].clone(), a.toc().sectors), "same key");
    let mut index = complete(&mut a, 16);
    let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = std::sync::Arc::clone(&lines);
    b.set_log(scarletbook::log::Log::new(scarletbook::log::Level::Warn, move |_, m| {
        sink.lock().unwrap().push(m.to_string());
    }));
    for first in [30, 150] {
        let c = TrackCursor::for_range_indexed(&area, first, p, n, &index).unwrap();
        let (got, c) = frames(&mut b, c);
        assert!(matches!(c.resumed_from(), ResumedFrom::Discarded { .. }), "{:?}", c.resumed_from());
        assert_eq!(got, straight(&mut b, first, p));
        // A discarded cursor records nothing.
        let before = index.clone();
        index.absorb(&c).unwrap();
        assert_eq!(index, before);
    }
    assert!(lines.lock().unwrap().iter().any(|l| l.contains("seek index does not match the image")));
    // Extending it is refused, not looped on.
    let mut partial = AreaIndex::new(&area, n, IndexOptions::new().with_interval(16)).unwrap();
    partial.extend(a.image_mut(), 40).unwrap();
    assert!(matches!(partial.extend(b.image_mut(), 40), Err(Error::InvalidArgument(_))));
}

#[test]
#[cfg_attr(miri, ignore = "reads a few hundred sectors")]
fn merging_partial_indexes_gives_the_complete_one() {
    let (mut d, _) = disc(200, 0);
    let area = d.toc().areas[0].clone();
    let n = d.toc().sectors;
    let full = complete(&mut d, 8);
    let options = IndexOptions::new().with_interval(8);
    let mut short = AreaIndex::new(&area, n, options).unwrap();
    short.extend(d.image_mut(), 30).unwrap();
    let mut long = short.clone();
    long.extend(d.image_mut(), 100).unwrap();
    // Either way round, and with the complete one.
    let mut x = short.clone();
    x.merge(&long).unwrap();
    assert_eq!(x, long);
    let mut y = long.clone();
    y.merge(&short).unwrap();
    assert_eq!(y, long);
    x.merge(&full).unwrap();
    assert_eq!(x, full);
    // Another interval, or another image's index, is refused and changes nothing.
    let other = AreaIndex::new(&area, n, IndexOptions::new().with_interval(4)).unwrap();
    assert!(x.merge(&other).is_err());
    let (mut e, _) = disc(200, 0x33);
    let foreign = complete(&mut e, 8);
    let mut z = short.clone();
    assert!(z.merge(&foreign).is_err());
    assert_eq!(z, short);
}

proptest! {
    #![proptest_config(config())]

    /// Any bytes, and any change to a valid index's bytes (with or without a fixed
    /// checksum), give an error or an index that saves to the same bytes; never a panic.
    #[test]
    #[cfg_attr(miri, ignore = "reads sectors for every case")]
    fn hostile_index_bytes(
        flips in proptest::collection::vec((any::<proptest::sample::Index>(), any::<u8>()), 1..6),
        seed in 0usize..4,
        cut in proptest::option::of(any::<proptest::sample::Index>()),
        noise in proptest::collection::vec(any::<u8>(), 0..400),
    ) {
        let seeds = harness::seek_index_seeds();
        let mut bytes = seeds[seed % seeds.len()].clone();
        for (at, x) in flips {
            let i = at.index(bytes.len());
            bytes[i] ^= x;
        }
        if let Some(c) = cut {
            bytes.truncate(c.index(bytes.len() + 1));
        }
        harness::seek_index(&bytes);
        harness::seek_index(&noise);
    }
}

/// The seeds of the fuzz target load, and the harness accepts them.
#[test]
#[cfg_attr(miri, ignore = "reads sectors")]
fn harness_seeds() {
    for seed in harness::seek_index_seeds() {
        harness::seek_index(&seed);
    }
}

/// Small enough for Miri: save, load and read through a loaded index.
#[test]
fn small_round_trip() {
    let (mut image, area) = harness::seek_area();
    let n = image.sector_count();
    let mut index = AreaIndex::new(&area, n, IndexOptions::new().with_interval(16)).unwrap();
    while !index.extend(&mut image, 64).unwrap() {}
    let loaded = AreaIndex::from_bytes(&area, n, &index.to_bytes()).unwrap();
    assert_eq!(loaded, index);
    let mut c = TrackCursor::for_range_indexed(&area, 9, 10, n, &loaded).unwrap();
    let f = c.next_frame(&mut image).unwrap().unwrap();
    assert_eq!(f.number, 9);
    assert!(matches!(c.resumed_from(), ResumedFrom::Checkpoint { lsn: 32, .. }));
}

/// A DST sector starting frame `number` (sector count `count`) with a packet of `len`
/// bytes.
fn start_sector(number: u32, count: u8, len: u16) -> [u8; 2048] {
    let mut s = [0u8; 2048];
    s[0] = 1 | (1 << 2) | (1 << 5);
    s[1..3].copy_from_slice(&((1u16 << 15) | (2 << 11) | len).to_be_bytes());
    s[3..6].copy_from_slice(&[(number / 4500) as u8, (number / 75 % 60) as u8, (number % 75) as u8]);
    s[6] = (count & 0x1F) << 2;
    s
}

/// A DST sector of `k` continuation packets of `len` bytes each.
fn continuation(k: u8, len: u16) -> [u8; 2048] {
    let mut s = [0u8; 2048];
    s[0] = 1 | (k << 5);
    for p in 0..usize::from(k) {
        s[1 + 2 * p..3 + 2 * p].copy_from_slice(&((2u16 << 11) | len).to_be_bytes());
    }
    s
}

/// A sector whose packet runs past its end: malformed, and it leaves the frame under
/// assembly as it is.
fn overrun() -> [u8; 2048] {
    let mut s = [0u8; 2048];
    s[0] = 1 << 5;
    s[1..3].copy_from_slice(&((2u16 << 11) | 2047).to_be_bytes());
    s
}

fn spec_of(audio: Vec<[u8; 2048]>) -> (Vec<u8>, u32) {
    let playtime = (7 * audio.len() as u32).min(300);
    let spec = Spec { channels: 2, playtime, tracks: vec![TrackSpec { start: 0, duration: 1 }], audio };
    (spec.build(), playtime)
}

/// A frame under assembly of nearly 64 KiB followed by hundreds of sectors without
/// packets: every checkpoint inside it shares one buffer, so a budget of one frame stores
/// them all and a resume anywhere in it replays nothing.
#[test]
#[cfg_attr(miri, ignore = "reads a few hundred sectors")]
fn checkpoints_share_one_frame_buffer() {
    let mut audio = vec![start_sector(0, 1, 10), start_sector(1, 31, 2000)];
    audio.extend(vec![continuation(1, 2000); 31]);
    audio.extend(vec![continuation(0, 0); 300]);
    audio.push(start_sector(2, 1, 10));
    let (img, playtime) = spec_of(audio);
    let mut d = Disc::open(Cursor::new(img)).unwrap();
    let area = d.toc().areas[0].clone();
    let n = d.toc().sectors;
    let options = IndexOptions::new().with_interval(1).with_inline_budget(70_000);
    let mut index = AreaIndex::new(&area, n, options).unwrap();
    while !index.extend(d.image_mut(), 1000).unwrap() {}
    assert!(index.memory() < 2 * 70_000 + 40 * index.checkpoints(), "{}", index.memory());
    let c = TrackCursor::for_range_indexed(&area, 1, playtime, n, &index).unwrap();
    let (got, c) = frames(&mut d, c);
    let ResumedFrom::Checkpoint { lsn, replay_from } = c.resumed_from() else {
        panic!("{:?}", c.resumed_from())
    };
    assert!(lsn > common::AUDIO_LSN as u64 + 300, "{lsn}");
    assert_eq!(replay_from, lsn, "stored, not replayed");
    assert_eq!(got, straight(&mut d, 1, playtime));
    // Saved and loaded, the buffer stays shared.
    let loaded = AreaIndex::from_bytes(&area, n, &index.to_bytes()).unwrap();
    assert_eq!(loaded, index);
    assert!(index.to_bytes().len() < 70_000 + 60 * index.checkpoints());
}

/// A reader that fails every read reaching past `limit` bytes.
struct FailsPast {
    inner: Cursor<Vec<u8>>,
    limit: u64,
}

impl std::io::Read for FailsPast {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.inner.position() + buf.len() as u64 > self.limit {
            return Err(std::io::Error::other("injected failure"));
        }
        self.inner.read(buf)
    }
}

impl std::io::Seek for FailsPast {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// A read error part way through a replay: the malformed sectors replayed are not counted
/// on top of the checkpoint's count, which already holds them.
#[test]
#[cfg_attr(miri, ignore = "reads a few hundred sectors")]
fn a_read_error_during_replay_counts_nothing_twice() {
    // Malformed sectors 0..10; a frame from sector 10, with malformed sectors 11..21 and
    // 60 sectors without packets inside it, so that the checkpoint at 72 replays from 10.
    let mut audio = vec![overrun(); 10];
    audio.push(start_sector(0, 3, 100));
    audio.extend(vec![overrun(); 10]);
    audio.extend(vec![continuation(0, 0); 60]);
    audio.extend([continuation(1, 100), continuation(1, 100), start_sector(1, 1, 10)]);
    let (img, playtime) = spec_of(audio);
    let mut d = Disc::open(Cursor::new(img.clone())).unwrap();
    let area = d.toc().areas[0].clone();
    let n = d.toc().sectors;
    let options = IndexOptions::new().with_interval(8).with_inline_budget(0);
    let mut index = AreaIndex::new(&area, n, options).unwrap();
    while !index.extend(d.image_mut(), 1000).unwrap() {}
    let s0 = common::AUDIO_LSN as u64;
    // The second read chunk of the replay fails.
    let limit = (s0 + 10 + 32) * 2048 + 1;
    let mut failing = Disc::open(FailsPast { inner: Cursor::new(img), limit }).unwrap();
    let c = TrackCursor::for_range_indexed(&area, 1, playtime, n, &index).unwrap();
    let ResumedFrom::Checkpoint { lsn, replay_from } = c.resumed_from() else { panic!() };
    assert_eq!((lsn - s0, replay_from - s0), (80, 10));
    let mut it = failing.read_frames(c);
    assert!(matches!(it.next(), Some(Err(Error::Io(_)))));
    assert_eq!(it.malformed_sectors(), 20, "the checkpoint's count, nothing replayed added");
    assert_eq!(it.warnings().len(), 20);
}
