// SPDX-License-Identifier: MIT OR Apache-2.0

//! Seek hints (issue #124; specification 4, 4.2 item 8 locator samples, sacd-spec
//! `cb9530b`): saved with a seek index, they let a provisional reading near a range read
//! in an earlier session begin with one read call; they never make a position exact, a
//! stale or foreign one is found and dropped, and an index of another disc is refused.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::cell::Cell;
use std::io::Cursor;
use std::rc::Rc;

use common::hostile::S;
use common::{PFrame, Spec, TrackSpec, pack};
use scarletbook::seek::DEFAULT_INLINE_BUDGET;
use scarletbook::{
    AreaIndex, Disc, IndexOptions, ResumedFrom, SeekPolicy, Slot, Speculation, SpeculativeOptions, TrackFrame,
};

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

/// A well-formed 2-channel area of 2000 DST frames numbered from `offset`, in `tracks`
/// tracks; the frame sizes alone set where each frame lies.
fn image(offset: u32, tracks: u32) -> Vec<u8> {
    let fs: Vec<PFrame> = (0..2000u32)
        .map(|k| PFrame {
            number: k + offset,
            dst: true,
            data: vec![k as u8; 3000 + (k as usize * 53) % 6000],
        })
        .collect();
    let per = 2000 / tracks;
    let tracks = (0..tracks).map(|i| TrackSpec { start: i * per, duration: per }).collect();
    Spec { channels: 2, playtime: 2000, tracks, audio: pack(&fs) }.build()
}

type Counted = Disc<Counting<Cursor<Vec<u8>>>>;

fn open(img: Vec<u8>) -> (Counted, Rc<Cell<u64>>, Rc<Cell<u64>>) {
    let (bytes, calls) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
    let disc =
        Disc::open(Counting { inner: Cursor::new(img), bytes: Rc::clone(&bytes), calls: Rc::clone(&calls) })
            .unwrap();
    (disc, bytes, calls)
}

fn speculative(disc: &mut Disc<impl std::io::Read + std::io::Seek>) {
    let options = SpeculativeOptions::new().with_min_gap_sectors(64);
    disc.set_seek_policy(Slot::TwoChannel, SeekPolicy::Speculative(options)).unwrap();
}

fn read<R: std::io::Read + std::io::Seek>(
    disc: &mut Disc<R>,
    (first, end): (u32, u32),
) -> (Vec<TrackFrame>, scarletbook::TrackCursor) {
    let c = disc.range_frames_cursor(Slot::TwoChannel, first, end).unwrap();
    let mut it = disc.read_frames(c);
    let v = it.by_ref().map(Result::unwrap).collect();
    (v, it.into_cursor())
}

/// The ranges a first session reads: scattered, 14 frames each.
const RANGES: [(u32, u32); 4] = [(300, 314), (900, 914), (1400, 1414), (1900, 1914)];

/// A first session: provisional reads of [`RANGES`], their hints saved with the index.
fn first_session(img: &[u8]) -> Vec<u8> {
    let (mut disc, _, _) = open(img.to_vec());
    disc.enable_seek_index(Slot::TwoChannel, IndexOptions::new()).unwrap();
    speculative(&mut disc);
    for r in RANGES {
        let (_, c) = read(&mut disc, r);
        assert!(matches!(c.resumed_from(), ResumedFrom::Speculative { probes, .. } if probes > 0));
    }
    let index = disc.seek_index(Slot::TwoChannel).unwrap();
    assert!(index.hints() >= RANGES.len(), "{} hints", index.hints());
    index.to_bytes()
}

/// After a remount, a range near one read before takes one read call, of the range's
/// sectors and a few more; its positions equal an exact read's and are provisional.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn one_read_after_a_remount() {
    let img = image(0, 4);
    let saved = first_session(&img);
    let mut plain = Disc::open(Cursor::new(img.clone())).unwrap();
    for (first, end) in RANGES.iter().flat_map(|&(f, e)| [(f, e), (f + 3, e + 3), (f + 6, e + 1)]) {
        let (want, _) = read(&mut plain, (first, end));
        let (mut disc, bytes, calls) = open(img.clone());
        let area = disc.toc().areas[0].clone();
        let index = AreaIndex::from_bytes(&area, disc.toc().sectors, &saved, DEFAULT_INLINE_BUDGET).unwrap();
        disc.set_seek_index(Slot::TwoChannel, index).unwrap();
        speculative(&mut disc);
        let (b0, c0) = (bytes.get(), calls.get());
        let (got, c) = read(&mut disc, (first, end));
        assert_eq!(got, want, "[{first}, {end})");
        assert_eq!(calls.get() - c0, 1, "[{first}, {end}): one read call");
        let sectors = (bytes.get() - b0) / S as u64;
        // The range's frames (at most 5 sectors each here) and the hint spacing before it.
        assert!(sectors <= u64::from(end - first + 8) * 5 + 8, "[{first}, {end}): {sectors} sectors");
        assert!(
            matches!(c.resumed_from(), ResumedFrom::Speculative { probes: 0, .. }),
            "{:?}",
            c.resumed_from()
        );
        // TODO(sacd-spec#31): readings from a hint stay provisional until saved
        // confirmations (4.2 item 9) are kept.
        assert!(matches!(c.speculation(), Speculation::Pending { .. }), "{:?}", c.speculation());
    }
}

/// Hints of another image with the same layout and TOC (frames numbered one higher): the
/// hint's sector does not have its first record, so the cursor searches instead, its
/// positions are those of that image, and the index drops the hints.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn hints_of_another_image_are_dropped() {
    let saved = first_session(&image(0, 4));
    let other = image(1, 4);
    let mut plain = Disc::open(Cursor::new(other.clone())).unwrap();
    let (mut disc, _, _) = open(other);
    let area = disc.toc().areas[0].clone();
    let index = AreaIndex::from_bytes(&area, disc.toc().sectors, &saved, DEFAULT_INLINE_BUDGET).unwrap();
    disc.set_seek_index(Slot::TwoChannel, index).unwrap();
    let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = std::sync::Arc::clone(&lines);
    disc.set_log(scarletbook::log::Log::new(scarletbook::log::Level::Warn, move |_, m| {
        sink.lock().unwrap().push(m.to_string());
    }));
    speculative(&mut disc);
    let (want, _) = read(&mut plain, (903, 917));
    let (got, c) = read(&mut disc, (903, 917));
    assert_eq!(got, want);
    assert!(matches!(c.resumed_from(), ResumedFrom::Speculative { probes, .. } if probes > 0), "searched");
    assert!(lines.lock().unwrap().iter().any(|l| l.contains("seek hint")), "{:?}", lines.lock().unwrap());
    // Dropped (the reading's own hints then come in).
    let index = disc.seek_index(Slot::TwoChannel).unwrap();
    assert!(index.hints() < 20, "{} hints", index.hints());
}

/// An index saved by a disc refuses a disc with another TOC (another track list over the
/// same audio), at no cost in reads; one saved without a fingerprint is taken.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn an_index_of_another_toc_is_refused() {
    let saved = first_session(&image(0, 4));
    let (mut disc, _, calls) = open(image(0, 5));
    let area = disc.toc().areas[0].clone();
    let n = disc.toc().sectors;
    let c0 = calls.get();
    let index = AreaIndex::from_bytes(&area, n, &saved, DEFAULT_INLINE_BUDGET).unwrap();
    let e = disc.set_seek_index(Slot::TwoChannel, index).unwrap_err();
    assert!(matches!(e, scarletbook::Error::InvalidArgument(_)), "{e}");
    assert_eq!(calls.get(), c0);
    disc.set_seek_index(Slot::TwoChannel, AreaIndex::new(&area, n, IndexOptions::new()).unwrap()).unwrap();
}

/// Saving: an index without hints or fingerprint saves as version 1, unchanged; one with
/// them round-trips; hints out of order are refused; a small bound drops every other one.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn hints_are_saved_and_checked() {
    let img = image(0, 4);
    let saved = first_session(&img);
    assert_eq!(u32::from_le_bytes(saved[8..12].try_into().unwrap()), 2, "version 2");
    let disc = Disc::open(Cursor::new(img)).unwrap();
    let area = disc.toc().areas[0].clone();
    let n = disc.toc().sectors;
    let index = AreaIndex::from_bytes(&area, n, &saved, DEFAULT_INLINE_BUDGET).unwrap();
    assert_eq!(index.to_bytes(), saved);
    let plain = AreaIndex::new(&area, n, IndexOptions::new()).unwrap().to_bytes();
    assert_eq!(u32::from_le_bytes(plain[8..12].try_into().unwrap()), 1, "version 1");
    // The last two hints swapped: refused.
    let body = saved.len() - 8;
    let mut bad = saved[..body].to_vec();
    let (a, b) = (body - 16, body - 8);
    let last: Vec<u8> = bad[b..body].to_vec();
    bad.copy_within(a..b, b);
    bad[a..b].copy_from_slice(&last);
    let sum =
        bad.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &x| (h ^ u64::from(x)).wrapping_mul(0x0100_0000_01b3));
    bad.extend_from_slice(&sum.to_le_bytes());
    assert!(AreaIndex::from_bytes(&area, n, &bad, DEFAULT_INLINE_BUDGET).is_err());
    // Within 16 bytes: two hints at most.
    let small = AreaIndex::from_bytes(&area, n, &saved, 16).unwrap();
    assert!(small.hints() <= 2 && small.hints() > 0, "{}", small.hints());
    assert!(index.hints() <= IndexOptions::new().max_hints);
}
