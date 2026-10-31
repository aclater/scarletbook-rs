// SPDX-License-Identifier: MIT OR Apache-2.0

//! Seeking through an area index (issue #68): a cursor started from an index gives exactly
//! what a scan from the area's first audio sector gives, on well-formed and hostile
//! sector streams, and reads a bounded number of sectors once the index covers the range.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::cell::Cell;
use std::io::Cursor;
use std::rc::Rc;

use common::hostile::{HUGE, S, continuation_sector, overrun_sector, start_sector, stream};
use common::{PFrame, Spec, TrackSpec, pack};
use proptest::prelude::*;
use scarletbook::seek::{DEFAULT_INLINE_BUDGET, REPLAY_SECTORS};
use scarletbook::{
    Area, AreaIndex, Disc, IndexOptions, ResumedFrom, Slot, TrackAudioCursor, TrackCursor, TrackFrame,
    Warning,
};

fn config() -> ProptestConfig {
    if cfg!(miri) {
        ProptestConfig { cases: 2, failure_persistence: None, ..ProptestConfig::default() }
    } else {
        let cases = std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(128);
        ProptestConfig { cases, ..ProptestConfig::default() }
    }
}

/// A 2-channel disc of the given audio sectors, with the largest play time (at most 300)
/// that its capacity allows.
fn disc_of(audio: Vec<[u8; S]>) -> (Vec<u8>, u32) {
    let playtime = (7 * audio.len() as u32).min(300);
    let spec = Spec { channels: 2, playtime, tracks: vec![TrackSpec { start: 0, duration: 1 }], audio };
    (spec.build(), playtime)
}

/// What a read of a range gives: positions, then counts and warnings after them.
#[derive(Debug, PartialEq)]
struct Read {
    frames: Vec<TrackFrame>,
    missing: u64,
    malformed: u64,
    warnings: Vec<Warning>,
    /// The malformed count after each position returned.
    malformed_after: Vec<u64>,
}

fn read<R: std::io::Read + std::io::Seek>(
    disc: &mut Disc<R>,
    cursor: TrackCursor,
    take: usize,
) -> (Read, TrackCursor) {
    let mut it = disc.read_frames(cursor);
    let mut frames = Vec::new();
    let mut malformed_after = Vec::new();
    while frames.len() < take {
        let Some(f) = it.next() else { break };
        frames.push(f.unwrap());
        malformed_after.push(it.malformed_sectors());
    }
    let cursor = it.into_cursor();
    let r = Read {
        frames,
        missing: cursor.missing_frames(),
        malformed: cursor.malformed_sectors(),
        warnings: cursor.warnings().to_vec(),
        malformed_after,
    };
    (r, cursor)
}

fn area(disc: &Disc<impl std::io::Read + std::io::Seek>) -> (Area, u64) {
    (disc.toc().areas[0].clone(), disc.toc().sectors)
}

proptest! {
    #![proptest_config(config())]

    /// Reads of random ranges in random order, each from the index the earlier reads
    /// built, give exactly the reads of a scan from the first sector: positions, contents,
    /// missing and malformed counts (after every position) and warnings.
    #[test]
    #[cfg_attr(miri, ignore = "reads many ranges")]
    fn indexed_reads_equal_straight_reads(
        audio in stream(),
        interval in prop_oneof![Just(1u32), Just(2), Just(7), Just(64)],
        budget in prop_oneof![Just(0usize), Just(DEFAULT_INLINE_BUDGET)],
        reads in proptest::collection::vec((0.0f64..1.0, 0.0f64..1.0, 0usize..40), 1..12),
    ) {
        let (img, playtime) = disc_of(audio);
        let mut disc = Disc::open(Cursor::new(img)).unwrap();
        let (area, n) = area(&disc);
        let options = IndexOptions::new().with_interval(interval).with_inline_budget(budget);
        let mut index = AreaIndex::new(&area, n, options).unwrap();
        for (a, b, take) in reads {
            let first = (a * f64::from(playtime)) as u32;
            let end = first + 1 + (b * f64::from(playtime - first - 1)) as u32;
            let straight = disc.range_frames_cursor(Slot::TwoChannel, first, end).unwrap();
            let (want, _) = read(&mut disc, straight, take);
            let sought = TrackCursor::for_range_indexed(&area, first, end, n, &index).unwrap();
            let (got, mut cursor) = read(&mut disc, sought, take);
            prop_assert_eq!(&got, &want, "[{}, {}) take {} from {:?}", first, end, take, cursor.resumed_from());
            index.absorb(&mut cursor).unwrap();
        }
        // Absorbing again changes nothing.
        let before = index.clone();
        let c = TrackCursor::for_range_indexed(&area, 0, 1, n, &index).unwrap();
        let (_, mut c) = read(&mut disc, c, 1);
        index.absorb(&mut c).unwrap();
        prop_assert_eq!(index.checkpoints() >= before.checkpoints(), true);
    }

    /// Once the index is complete, every range read from it equals the straight read, and
    /// decoded audio does too.
    #[test]
    #[cfg_attr(miri, ignore = "reads many ranges")]
    fn complete_index_reads_equal_straight_reads(
        audio in stream(),
        interval in prop_oneof![Just(1u32), Just(3), Just(64)],
        firsts in proptest::collection::vec(0.0f64..1.0, 1..8),
    ) {
        let (img, playtime) = disc_of(audio);
        let mut disc = Disc::open(Cursor::new(img)).unwrap();
        let (area, n) = area(&disc);
        let mut index = AreaIndex::new(&area, n, IndexOptions::new().with_interval(interval)).unwrap();
        // Positions past every accepted frame end only at the end of the area.
        let c = TrackCursor::for_range_indexed(&area, playtime - 1, playtime, n, &index).unwrap();
        let (_, mut c) = read(&mut disc, c, usize::MAX);
        index.absorb(&mut c).unwrap();
        for a in firsts {
            let first = (a * f64::from(playtime)) as u32;
            let straight = disc.range_audio_cursor(Slot::TwoChannel, first, playtime).unwrap();
            let want: Vec<_> = disc.read_track_audio(straight).map(Result::unwrap).collect();
            let sought = TrackAudioCursor::for_range_indexed(&area, first, playtime, n, &index).unwrap();
            let mut it = disc.read_track_audio(sought);
            let got: Vec<_> = it.by_ref().map(Result::unwrap).collect();
            prop_assert_eq!(got, want);
            let straight = disc.range_frames_cursor(Slot::TwoChannel, first, playtime).unwrap();
            let (want, _) = read(&mut disc, straight, usize::MAX);
            let sought = TrackCursor::for_range_indexed(&area, first, playtime, n, &index).unwrap();
            let (got, _) = read(&mut disc, sought, usize::MAX);
            prop_assert_eq!(got, want);
        }
    }
}

/// A frame with the largest timecode early in the area raises `a` past every later frame
/// (specification 4, 4.5 item 2): a seek must not resume at a checkpoint past it for any
/// range before that number, and all later positions are missing (4.5 item 6).
#[test]
#[cfg_attr(miri, ignore = "reads a few hundred sectors")]
fn a_huge_timecode_before_the_range_is_honoured() {
    let mut fs: Vec<PFrame> =
        (0..400u32).map(|n| PFrame { number: n, dst: true, data: vec![n as u8; 3000] }).collect();
    fs.truncate(10);
    let mut audio = pack(&fs);
    let huge_lsn = (common::AUDIO_LSN + audio.len()) as u64;
    audio.push(start_sector(HUGE, 1, 1, false, 0));
    let later: Vec<PFrame> =
        (10..300u32).map(|n| PFrame { number: n, dst: true, data: vec![n as u8; 3000] }).collect();
    audio.extend(pack(&later));
    let (img, playtime) = disc_of(audio);
    let mut disc = Disc::open(Cursor::new(img)).unwrap();
    let (area, n) = area(&disc);
    let mut index = AreaIndex::new(&area, n, IndexOptions::new().with_interval(4)).unwrap();
    // Built eagerly, in steps.
    let mut steps = 0;
    while !index.extend(disc.image_mut(), 50).unwrap() {
        steps += 1;
    }
    assert!(steps > 3, "{steps}");
    for first in [0, 5, 9, 10, 50, 200, playtime - 1] {
        let straight = disc.range_frames_cursor(Slot::TwoChannel, first, playtime).unwrap();
        let (want, _) = read(&mut disc, straight, usize::MAX);
        let sought = TrackCursor::for_range_indexed(&area, first, playtime, n, &index).unwrap();
        let (got, c) = read(&mut disc, sought, usize::MAX);
        assert_eq!(got, want, "first {first}");
        // The huge frame is delivered at the next start packet, in the sector after its
        // own: every checkpoint after that one has a above every position, and the resume
        // is never there.
        let ResumedFrom::Checkpoint { lsn, .. } = c.resumed_from() else { panic!("{:?}", c.resumed_from()) };
        assert!(lsn <= huge_lsn + 1, "first {first}: resumed at {lsn}");
        if first >= 10 {
            assert!(got.frames.iter().all(|f| f.content == scarletbook::FrameContent::Missing));
        }
    }
}

/// A frame under assembly held over many sectors is stored in the index within the
/// budget, and replayed from its first sector without one; both are exact.
#[test]
#[cfg_attr(miri, ignore = "reads a few hundred sectors")]
fn long_frames_under_assembly_are_stored_or_replayed() {
    let mut audio = vec![start_sector(0, 1, 100, false, 1)];
    // Frame 1 (3 sectors' worth of packets) spread over 103 sectors: complete.
    audio.push(start_sector(1, 3, 100, false, 2));
    audio.extend(vec![continuation_sector(0, 0); 100]);
    audio.extend([continuation_sector(1, 50), continuation_sector(1, 50)]);
    audio.push(start_sector(2, 1, 10, false, 3));
    audio.push(start_sector(3, 1, 10, false, 4));
    let (img, playtime) = disc_of(audio);
    for budget in [0, DEFAULT_INLINE_BUDGET] {
        let mut disc = Disc::open(Cursor::new(img.clone())).unwrap();
        let (area, n) = area(&disc);
        let options = IndexOptions::new().with_interval(8).with_inline_budget(budget);
        let mut index = AreaIndex::new(&area, n, options).unwrap();
        let c = TrackCursor::for_range_indexed(&area, 0, playtime, n, &index).unwrap();
        let (_, mut c) = read(&mut disc, c, usize::MAX);
        index.absorb(&mut c).unwrap();
        for first in [0, 1, 2, 3] {
            let straight = disc.range_frames_cursor(Slot::TwoChannel, first, playtime).unwrap();
            let (want, _) = read(&mut disc, straight, usize::MAX);
            let sought = TrackCursor::for_range_indexed(&area, first, playtime, n, &index).unwrap();
            let (got, c) = read(&mut disc, sought, usize::MAX);
            assert_eq!(got, want, "budget {budget}, first {first}");
            if first == 1 {
                let f = &got.frames[0];
                assert!(matches!(&f.content, scarletbook::FrameContent::Dst(d) if d.len() == 200), "{f:?}");
            }
            if first == 2 {
                // Resumed at the last checkpoint before frame 2's sector, inside frame 1.
                let ResumedFrom::Checkpoint { lsn, replay_from } = c.resumed_from() else { panic!() };
                assert!(lsn - replay_from <= REPLAY_SECTORS || budget == 0);
                if budget == 0 {
                    assert_eq!(replay_from, common::AUDIO_LSN as u64 + 1);
                } else {
                    assert_eq!(replay_from, lsn);
                }
            }
        }
    }
}

/// A reader that counts the sectors read through it.
struct Counting<R> {
    inner: R,
    read: Rc<Cell<u64>>,
}

impl<R: std::io::Read> std::io::Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read.set(self.read.get() + n as u64);
        Ok(n)
    }
}

impl<R: std::io::Seek> std::io::Seek for Counting<R> {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// With a complete index, the first position of any range costs at most `W + K` sectors
/// plus a read chunk and the sectors of its own frame, wherever it lies; a scan from the
/// first sector costs its whole prefix.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn first_position_reads_are_bounded() {
    const K: u64 = 64;
    // Well-formed DST frames of 2 to 5 sectors, every number present, over 3000 sectors.
    let fs: Vec<PFrame> = (0..1000u32)
        .map(|n| PFrame { number: n, dst: true, data: vec![n as u8; 4000 + (n as usize * 37) % 6000] })
        .collect();
    let audio = pack(&fs);
    let sectors = audio.len() as u64;
    assert!(sectors > 2000, "{sectors}");
    let spec = Spec { channels: 2, playtime: 1000, tracks: vec![TrackSpec { start: 0, duration: 1 }], audio };
    let read = Rc::new(Cell::new(0));
    let mut disc = Disc::open(Counting { inner: Cursor::new(spec.build()), read: read.clone() }).unwrap();
    let (area, n) = area(&disc);
    let mut index = AreaIndex::new(&area, n, IndexOptions::new()).unwrap();

    // Building it lazily reads each sector once.
    let start = read.get();
    let c = TrackCursor::for_range_indexed(&area, 999, 1000, n, &index).unwrap();
    let mut it = disc.read_frames(c);
    it.by_ref().for_each(drop);
    let mut c = it.into_cursor();
    index.absorb(&mut c).unwrap();
    assert!(index.is_complete());
    assert_eq!((read.get() - start) / S as u64, sectors);
    println!("index of {sectors} sectors: {} checkpoints, {} bytes", index.checkpoints(), index.memory());

    let mut costs = Vec::new();
    for first in (0..1000).step_by(37) {
        let before = read.get();
        let c = TrackCursor::for_range_indexed(&area, first, 1000, n, &index).unwrap();
        let f = disc.read_frames(c).next().unwrap().unwrap();
        assert_eq!(f.number, first);
        let cost = (read.get() - before) / S as u64;
        // One read sized to the range (#124): the replay, an interval and 64 sectors.
        assert!(cost <= REPLAY_SECTORS + K + 64, "first {first}: {cost} sectors");
        costs.push(cost);
        // Without the index: the whole prefix.
        let before = read.get();
        let c = disc.range_frames_cursor(Slot::TwoChannel, first, 1000).unwrap();
        disc.read_frames(c).next().unwrap().unwrap();
        let straight = (read.get() - before) / S as u64;
        assert!(first < 100 || straight > 2 * cost, "first {first}: {straight} vs {cost}");
    }
    println!("sectors read for the first position: {costs:?}");
}

#[test]
fn an_index_of_another_area_is_refused() {
    let (img, playtime) = disc_of(vec![overrun_sector(); 4]);
    let disc = Disc::open(Cursor::new(img)).unwrap();
    let (area, n) = area(&disc);
    let index = AreaIndex::new(&area, n, IndexOptions::new()).unwrap();
    assert!(matches!(
        TrackCursor::for_range_indexed(&area, 0, playtime, n + 1, &index),
        Err(scarletbook::Error::InvalidArgument(_) | scarletbook::Error::TrackExceedsCapacity { .. })
    ));
    let mut other = area.clone();
    other.slot = Slot::Multichannel;
    assert!(matches!(
        TrackCursor::for_range_indexed(&other, 0, playtime, n, &index),
        Err(scarletbook::Error::InvalidArgument(_))
    ));
    // Range errors come first, as for an unindexed cursor.
    assert!(matches!(
        TrackCursor::for_range_indexed(&area, 3, 2, n, &index),
        Err(scarletbook::Error::FrameRangeInvalid { .. })
    ));
    // A cursor of another interval is not absorbed.
    let mut coarse = AreaIndex::new(&area, n, IndexOptions::new().with_interval(2)).unwrap();
    let mut c = TrackCursor::for_range_indexed(&area, 0, playtime, n, &index).unwrap();
    assert!(matches!(coarse.absorb(&mut c), Err(scarletbook::Error::InvalidArgument(_))));
    assert_eq!(coarse.checkpoints(), 0);
    // A cursor made without an index records nothing.
    let mut plain = TrackCursor::for_range(&area, 0, playtime, n).unwrap();
    coarse.absorb(&mut plain).unwrap();
    assert_eq!(coarse.checkpoints(), 0);
}

/// Small enough for Miri: an index built and used over a few sectors.
#[test]
fn small_seek_round_trip() {
    let fs: Vec<PFrame> =
        (0..6u32).map(|n| PFrame { number: n, dst: true, data: vec![n as u8; 2500] }).collect();
    let mut audio = pack(&fs);
    audio.insert(3, overrun_sector());
    let (img, playtime) = disc_of(audio);
    let mut disc = Disc::open(Cursor::new(img)).unwrap();
    let (area, n) = area(&disc);
    let mut index = AreaIndex::new(&area, n, IndexOptions::new().with_interval(2)).unwrap();
    let c = TrackCursor::for_range_indexed(&area, 0, playtime, n, &index).unwrap();
    let (_, mut c) = read(&mut disc, c, usize::MAX);
    index.absorb(&mut c).unwrap();
    assert!(index.is_complete());
    for first in 0..8 {
        let straight = disc.range_frames_cursor(Slot::TwoChannel, first, playtime).unwrap();
        let (want, _) = read(&mut disc, straight, 3);
        let sought = TrackCursor::for_range_indexed(&area, first, playtime, n, &index).unwrap();
        let (got, c) = read(&mut disc, sought, 3);
        assert_eq!(got, want, "first {first}");
        // Past the last accepted frame, nothing is read.
        assert_eq!(c.resumed_from() == ResumedFrom::End, first >= 6, "first {first}");
    }
}

/// More malformed sectors than warnings are kept (specification 4, 4.4.5 item 3): a read
/// from the index keeps the same first warnings and the full count.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn capped_warnings_through_an_index() {
    let cap = scarletbook::audio::MAX_MALFORMED_WARNINGS;
    let mut audio = vec![overrun_sector(); cap + 300];
    for (i, s) in audio.iter_mut().enumerate().skip(cap - 100).step_by(5) {
        *s = start_sector((i - (cap - 100)) as u32 / 5, 1, 10, false, 0);
    }
    let (img, playtime) = disc_of(audio);
    let mut disc = Disc::open(Cursor::new(img)).unwrap();
    let (area, n) = area(&disc);
    let mut index = AreaIndex::new(&area, n, IndexOptions::new()).unwrap();
    while !index.extend(disc.image_mut(), 1000).unwrap() {}
    for first in [0, 100, 200, 299] {
        let straight = disc.range_frames_cursor(Slot::TwoChannel, first, playtime).unwrap();
        let (want, _) = read(&mut disc, straight, 5);
        let sought = TrackCursor::for_range_indexed(&area, first, playtime, n, &index).unwrap();
        let (got, _) = read(&mut disc, sought, 5);
        assert_eq!(got, want, "first {first}");
        assert!(got.malformed > 0);
        if first > 100 {
            assert_eq!((got.warnings.len(), got.malformed), (cap, cap as u64 + 300 - 80));
        }
    }
}

/// Each call of `extend` advances the frontier, however few sectors it is given.
#[test]
#[cfg_attr(miri, ignore = "reads a few hundred sectors")]
fn extend_always_progresses() {
    let (img, _) = disc_of(vec![overrun_sector(); 300]);
    let mut disc = Disc::open(Cursor::new(img)).unwrap();
    let (area, n) = area(&disc);
    for k in [1, 7, 64] {
        let mut index = AreaIndex::new(&area, n, IndexOptions::new().with_interval(k)).unwrap();
        let mut calls = 0;
        let mut frontier = index.frontier();
        while !index.extend(disc.image_mut(), 0).unwrap() {
            assert!(index.frontier() > frontier || calls == 0, "K {k}: stuck at {frontier}");
            frontier = index.frontier();
            calls += 1;
            assert!(calls <= 301, "K {k}");
        }
        assert_eq!(index.checkpoints(), 300usize.div_ceil(k as usize));
    }
}
