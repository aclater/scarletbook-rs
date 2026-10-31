// SPDX-License-Identifier: MIT OR Apache-2.0

//! Provisional reading (issue #94; specification 4, 4.2 item 8, sacd-spec `82eac6c`): a
//! cursor that may begin far into an area without reading the sectors before it, only when
//! its caller opts in, with its positions marked provisional.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::cell::Cell;
use std::io::Cursor;
use std::rc::Rc;

use common::hostile::{S, overrun_sector};
use common::{PFrame, Spec, TrackSpec, pack};
use proptest::prelude::*;
use scarletbook::{
    AreaIndex, Disc, FrameContent, IndexOptions, ResumedFrom, SeekPolicy, Slot, Speculation,
    SpeculativeOptions, TrackCursor, TrackFrame,
};

fn config() -> ProptestConfig {
    if cfg!(miri) {
        ProptestConfig { cases: 2, failure_persistence: None, ..ProptestConfig::default() }
    } else {
        let cases = std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(64);
        ProptestConfig { cases, ..ProptestConfig::default() }
    }
}

/// A reader that counts the bytes read through it and its read calls.
struct Counting<R> {
    inner: R,
    bytes: Rc<Cell<u64>>,
    calls: Rc<Cell<u64>>,
}

impl<R: std::io::Read> std::io::Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bytes.set(self.bytes.get() + n as u64);
        self.calls.set(self.calls.get() + 1);
        Ok(n)
    }
}

impl<R: std::io::Seek> std::io::Seek for Counting<R> {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// A 2-channel disc of `fs` (in disc order), one track over the play time `playtime`.
fn image(fs: &[PFrame], playtime: u32, before: Vec<[u8; S]>) -> Vec<u8> {
    let mut audio = before;
    audio.extend(pack(fs));
    Spec { channels: 2, playtime, tracks: vec![TrackSpec { start: 0, duration: playtime }], audio }.build()
}

/// DST frames `0..n` of 4 000 to 10 000 bytes, in order: a well-formed area.
fn frames(n: u32) -> Vec<PFrame> {
    (0..n)
        .map(|k| PFrame { number: k, dst: true, data: vec![k as u8; 4000 + (k as usize * 37) % 6000] })
        .collect()
}

fn options() -> SpeculativeOptions {
    SpeculativeOptions::new().with_min_gap_sectors(64)
}

fn read<R: std::io::Read + std::io::Seek>(
    disc: &mut Disc<R>,
    cursor: TrackCursor,
) -> (Vec<TrackFrame>, TrackCursor) {
    let mut it = disc.read_frames(cursor);
    let v = it.by_ref().map(Result::unwrap).collect();
    (v, it.into_cursor())
}

/// Opting in is required: with the default policy no cursor is ever provisional.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn the_default_policy_is_exact() {
    let mut disc = Disc::open(Cursor::new(image(&frames(1000), 1000, Vec::new()))).unwrap();
    assert_eq!(disc.seek_policy(Slot::TwoChannel), SeekPolicy::Exact);
    for first in [0, 400, 999] {
        let c = disc.range_frames_cursor(Slot::TwoChannel, first, 1000).unwrap();
        let (_, c) = read(&mut disc, c);
        assert_eq!(c.speculation(), Speculation::Exact);
        assert!(!matches!(c.resumed_from(), ResumedFrom::Speculative { .. }));
        assert_eq!(c.counts_from(), None);
    }
}

/// On a well-formed area, provisional readings give exactly the positions of a straight
/// read (4.2 item 8, informative: (a) and (b) hold), reading a bounded number of sectors
/// wherever the range lies; they are marked provisional all the same.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn well_formed_areas_read_provisionally_alike() {
    let img = image(&frames(1000), 1000, Vec::new());
    let mut plain = Disc::open(Cursor::new(img.clone())).unwrap();
    let (bytes, calls) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
    let mut disc =
        Disc::open(Counting { inner: Cursor::new(img), bytes: bytes.clone(), calls: calls.clone() }).unwrap();
    disc.set_seek_policy(Slot::TwoChannel, SeekPolicy::Speculative(options())).unwrap();
    let mut costs = Vec::new();
    for first in (100..1000).step_by(97) {
        let end = (first + 14).min(1000);
        let c = plain.range_frames_cursor(Slot::TwoChannel, first, end).unwrap();
        let (want, _) = read(&mut plain, c);
        let (b0, c0) = (bytes.get(), calls.get());
        let c = disc.range_frames_cursor(Slot::TwoChannel, first, end).unwrap();
        assert!(matches!(c.speculation(), Speculation::Pending { lsn: 0, .. }), "chosen on the first read");
        let (got, c) = read(&mut disc, c);
        assert_eq!(got, want, "first {first}");
        let ResumedFrom::Speculative { lsn, probes } = c.resumed_from() else {
            panic!("first {first}: {:?}", c.resumed_from())
        };
        assert!(
            matches!(c.speculation(), Speculation::Pending { lsn: l, first: f } if l == lsn && f == first)
        );
        assert_eq!(c.counts_from(), Some(lsn));
        let sectors = (bytes.get() - b0) / S as u64;
        costs.push((first, probes, calls.get() - c0, sectors));
        // Probes of at most 32 sectors, then W + K + the range's own sectors and a chunk.
        assert!(sectors <= u64::from(probes) * 32 + 32 + 64 + 64 + 32, "first {first}: {sectors} sectors");
    }
    println!("first, probes, read calls, sectors: {costs:?}");
}

/// A frame numbered within the range, accepted before `L`, makes the provisional reading
/// differ at a position the specification fills, not only at missing ones (4.2 item 8,
/// informative): below `a*` positions can differ whether or not they are missing; from
/// `a*` on they are exact.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn below_a_star_any_position_can_differ() {
    // Frames 0..100, then frame 505 (another one), then 101..1000: a straight read accepts
    // the early 505, so positions 101..=504 are missing, 505 holds the early frame, and a
    // is 506 from there (a* = 506 for a provisional reading starting past it).
    let mut fs = frames(1000);
    fs.insert(100, PFrame { number: 505, dst: true, data: vec![0xEE; 3000] });
    let img = image(&fs, 1000, Vec::new());
    let mut plain = Disc::open(Cursor::new(img.clone())).unwrap();
    let mut disc = Disc::open(Cursor::new(img)).unwrap();
    disc.set_seek_policy(Slot::TwoChannel, SeekPolicy::Speculative(options())).unwrap();
    let c = plain.range_frames_cursor(Slot::TwoChannel, 300, 600).unwrap();
    let (want, _) = read(&mut plain, c);
    let c = disc.range_frames_cursor(Slot::TwoChannel, 300, 600).unwrap();
    let (got, c) = read(&mut disc, c);
    assert!(matches!(c.resumed_from(), ResumedFrom::Speculative { .. }));
    let differ: Vec<u32> = got.iter().zip(&want).filter(|(g, w)| g != w).map(|(g, _)| g.number).collect();
    // Missing positions the provisional reading fills...
    assert!(differ.contains(&300) && want[0].content == FrameContent::Missing);
    // ... and position 505, which the specification fills with the early frame.
    assert!(differ.contains(&505));
    assert!(!matches!(want[205].content, FrameContent::Missing));
    // From a* = 506 on, exact.
    assert!(differ.iter().all(|&n| n < 506), "{differ:?}");
}

/// The malformed sectors of a provisional reading are those it processes from `L`
/// (4.4.5 item 5), and it says so.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn counts_begin_at_l() {
    let img = image(&frames(1000), 1000, vec![overrun_sector(); 3]);
    let mut plain = Disc::open(Cursor::new(img.clone())).unwrap();
    let mut disc = Disc::open(Cursor::new(img)).unwrap();
    disc.set_seek_policy(Slot::TwoChannel, SeekPolicy::Speculative(options())).unwrap();
    let c = plain.range_frames_cursor(Slot::TwoChannel, 700, 705).unwrap();
    let (want, p) = read(&mut plain, c);
    let c = disc.range_frames_cursor(Slot::TwoChannel, 700, 705).unwrap();
    let (got, c) = read(&mut disc, c);
    assert_eq!(got, want);
    assert_eq!((p.malformed_sectors(), p.warnings().len()), (3, 3));
    assert_eq!((c.malformed_sectors(), c.warnings().len()), (0, 0));
    assert!(c.counts_from().is_some());
}

/// The policy is per handle: a clone starts with a copy, and changing one leaves the
/// other; near the start, or near a checkpoint of the index, the exact start is kept.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn policy_per_handle_and_exact_when_near() {
    let img: std::sync::Arc<[u8]> = image(&frames(1000), 1000, Vec::new()).into();
    let mut disc = Disc::open(scarletbook::ReadAtReader::new(img)).unwrap();
    disc.set_seek_policy(Slot::TwoChannel, SeekPolicy::Speculative(options())).unwrap();
    let mut clone = disc.clone();
    assert_eq!(clone.seek_policy(Slot::TwoChannel), disc.seek_policy(Slot::TwoChannel));
    clone.set_seek_policy(Slot::TwoChannel, SeekPolicy::Exact).unwrap();
    assert!(matches!(disc.seek_policy(Slot::TwoChannel), SeekPolicy::Speculative(_)));
    let c = clone.range_frames_cursor(Slot::TwoChannel, 900, 910).unwrap();
    assert_eq!(read(&mut clone, c).1.speculation(), Speculation::Exact);
    // Near the start: exact.
    let c = disc.range_frames_cursor(Slot::TwoChannel, 5, 10).unwrap();
    assert_eq!(read(&mut disc, c).1.speculation(), Speculation::Exact);
    // With a complete index: exact, from it.
    disc.enable_seek_index(Slot::TwoChannel, IndexOptions::new()).unwrap();
    while !disc.extend_seek_index(Slot::TwoChannel, 1 << 20).unwrap() {}
    let c = disc.range_frames_cursor(Slot::TwoChannel, 900, 910).unwrap();
    let (_, c) = read(&mut disc, c);
    assert_eq!(c.speculation(), Speculation::Exact);
    assert!(matches!(c.resumed_from(), ResumedFrom::Checkpoint { .. }));
    // No such area.
    assert!(disc.set_seek_policy(Slot::Multichannel, SeekPolicy::Exact).is_err());
}

/// A caller-kept form, with and without an index.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn caller_kept_form() {
    let img = image(&frames(1000), 1000, Vec::new());
    let mut plain = Disc::open(Cursor::new(img.clone())).unwrap();
    let mut disc = Disc::open(Cursor::new(img)).unwrap();
    let area = disc.toc().areas[0].clone();
    let n = disc.toc().sectors;
    let c = plain.range_frames_cursor(Slot::TwoChannel, 800, 820).unwrap();
    let (want, _) = read(&mut plain, c);
    let index = AreaIndex::new(&area, n, IndexOptions::new()).unwrap();
    for index in [None, Some(&index)] {
        let c = TrackCursor::for_range_speculative(&area, 800, 820, n, index, options()).unwrap();
        let (got, c) = read(&mut disc, c);
        assert_eq!(got, want);
        assert!(matches!(c.resumed_from(), ResumedFrom::Speculative { .. }));
    }
    let mut other = area.clone();
    other.slot = Slot::Multichannel;
    let index = AreaIndex::new(&other, n, IndexOptions::new()).unwrap();
    assert!(TrackCursor::for_range_speculative(&area, 800, 820, n, Some(&index), options()).is_err());
}

/// A reader that fails every read past `limit` bytes.
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

/// A read error while choosing `L` is reported, and ends the cursor.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn a_read_error_while_choosing_is_reported() {
    let img = image(&frames(1000), 1000, Vec::new());
    let limit = ((common::AUDIO_LSN + 600) * S) as u64;
    let mut disc = Disc::open(FailsPast { inner: Cursor::new(img), limit }).unwrap();
    disc.set_seek_policy(Slot::TwoChannel, SeekPolicy::Speculative(options())).unwrap();
    let c = disc.range_frames_cursor(Slot::TwoChannel, 900, 910).unwrap();
    let mut it = disc.read_frames(c);
    assert!(matches!(it.next(), Some(Err(scarletbook::Error::Io(_)))));
    assert!(it.next().is_none());
}

proptest! {
    #![proptest_config(config())]

    /// On hostile streams, a provisional cursor never panics, reads a bounded number of
    /// sectors to choose `L`, returns positions in order, and every frame it returns is a
    /// complete frame of the image (one a straight read of the whole area delivers or
    /// discards; here: one of the stored frame list or a frame number below the play time).
    #[test]
    #[cfg_attr(miri, ignore = "reads many sectors")]
    fn hostile_streams_read_provisionally(
        audio in common::hostile::stream(),
        a in 0.0f64..1.0,
        gap in 0u64..64,
    ) {
        let playtime = (7 * audio.len() as u32).min(300);
        let img = Spec { channels: 2, playtime, tracks: vec![TrackSpec { start: 0, duration: 1 }], audio }.build();
        let mut disc = Disc::open(Cursor::new(img)).unwrap();
        let options = SpeculativeOptions::new().with_min_gap_sectors(gap);
        disc.set_seek_policy(Slot::TwoChannel, SeekPolicy::Speculative(options)).unwrap();
        let first = (a * f64::from(playtime - 1)) as u32;
        let c = disc.range_frames_cursor(Slot::TwoChannel, first, playtime).unwrap();
        let (got, c) = read(&mut disc, c);
        prop_assert_eq!(got.len() as u32, playtime - first);
        for (i, f) in got.iter().enumerate() {
            prop_assert_eq!(f.number, first + i as u32);
        }
        if let ResumedFrom::Speculative { probes, .. } = c.resumed_from() {
            prop_assert!(probes <= 48);
        }
    }
}

/// An exact cursor continued from a provisional one does not take its counts, which begin
/// at its `L`: until it reads, it reports a scan that has read nothing (4.4.5 items 4 and
/// 5).
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn an_exact_continuation_of_a_provisional_cursor_takes_no_counts() {
    let mut audio = pack(&frames(1000));
    let mid = audio.len() / 2;
    audio.insert(mid, overrun_sector());
    let img =
        Spec { channels: 2, playtime: 1000, tracks: vec![TrackSpec { start: 0, duration: 1000 }], audio }
            .build();
    let mut disc = Disc::open(Cursor::new(img)).unwrap();
    disc.enable_seek_index(Slot::TwoChannel, IndexOptions::new()).unwrap();
    disc.set_seek_policy(Slot::TwoChannel, SeekPolicy::Speculative(options())).unwrap();
    let c = disc.range_frames_cursor(Slot::TwoChannel, 480, 520).unwrap();
    let (_, prev) = read(&mut disc, c);
    assert!(matches!(prev.speculation(), Speculation::Pending { .. }));
    assert_eq!(prev.malformed_sectors(), 1, "counted from L");
    while !disc.extend_seek_index(Slot::TwoChannel, 1 << 20).unwrap() {}
    let next = disc.continue_range_frames(prev, Slot::TwoChannel, 900, 910).unwrap();
    assert_eq!(next.speculation(), Speculation::Exact);
    assert_eq!(next.counts_from(), None);
    assert_eq!((next.malformed_sectors(), next.warnings().len()), (0, 0));
    // Once it reads, the counts of a scan from the first sector.
    let (_, next) = read(&mut disc, next);
    assert_eq!((next.malformed_sectors(), next.counts_from()), (1, None));
}
