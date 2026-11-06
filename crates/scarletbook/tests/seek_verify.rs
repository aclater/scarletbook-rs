// SPDX-License-Identifier: MIT OR Apache-2.0

//! Checking provisional readings (issue #94; specification 4, 4.2 item 8, sacd-spec
//! `82eac6c`): a pass from the first sector confirms them when (a) and (b) hold and refutes
//! them ("possibly different") otherwise; verdicts reach the cursors and an event queue;
//! provisional checkpoints serve later provisional starts.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::io::Cursor;

use common::hostile::S;
use common::{PFrame, Spec, TrackSpec, pack};
use proptest::prelude::*;
use scarletbook::{
    AreaIndex, CursorOptions, Disc, FrameSpan, IndexOptions, ResumedFrom, SeekPolicy, Slot, Speculation,
    SpeculativeOptions, TrackCursor, TrackFrame,
};

fn config() -> ProptestConfig {
    if cfg!(miri) {
        ProptestConfig { cases: 2, failure_persistence: None, ..ProptestConfig::default() }
    } else {
        let cases = std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(128);
        ProptestConfig { cases, ..ProptestConfig::default() }
    }
}

fn frames(n: u32) -> Vec<PFrame> {
    (0..n)
        .map(|k| PFrame { number: k, dst: true, data: vec![k as u8; 4000 + (k as usize * 37) % 6000] })
        .collect()
}

fn image(fs: &[PFrame], playtime: u32) -> Vec<u8> {
    Spec { channels: 2, playtime, tracks: vec![TrackSpec { start: 0, duration: playtime }], audio: pack(fs) }
        .build()
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

/// A disc with an (empty) seek index and the speculative policy.
fn speculative_disc(img: Vec<u8>, interval: u32) -> Disc<Cursor<Vec<u8>>> {
    let mut disc = Disc::open(Cursor::new(img)).unwrap();
    disc.enable_seek_index(Slot::TwoChannel, IndexOptions::new().with_interval(interval)).unwrap();
    disc.set_seek_policy(Slot::TwoChannel, SeekPolicy::Speculative(options())).unwrap();
    disc
}

fn verify_all(disc: &mut Disc<Cursor<Vec<u8>>>) -> Vec<scarletbook::SeekEvent> {
    while !disc.verify_speculation(Slot::TwoChannel, 1 << 20).unwrap() {}
    disc.take_seek_events(Slot::TwoChannel)
}

/// On a well-formed area every provisional reading is confirmed once the index reaches
/// it; the cursors and the events say so.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn well_formed_readings_are_confirmed() {
    let mut disc = speculative_disc(image(&frames(1000), 1000), 64);
    let mut cursors = Vec::new();
    for first in [250, 600, 950] {
        let c = disc
            .frames_cursor(Slot::TwoChannel, FrameSpan::Range(first, first + 10), CursorOptions::new())
            .unwrap();
        let (_, c) = read(&mut disc, c);
        assert!(
            matches!(c.speculation(), Speculation::Pending { lsn, first: f, .. } if lsn > 0 && f == first)
        );
        cursors.push(c);
    }
    // Each reading begins at the sector its search found (#123), not on a checkpoint
    // boundary: verification rebuilds the state there from the checkpoint before it.
    let s0 = u64::from(disc.toc().area(Slot::TwoChannel).unwrap().audio_start_lsn);
    let off_boundary = |c: &TrackCursor| match c.resumed_from() {
        ResumedFrom::Speculative { lsn, .. } => (lsn - s0) % 64 != 0,
        _ => false,
    };
    assert!(
        cursors.iter().any(off_boundary),
        "{:?}",
        cursors.iter().map(TrackCursor::resumed_from).collect::<Vec<_>>()
    );
    let events = verify_all(&mut disc);
    assert_eq!(events.len(), 3, "{events:?}");
    for c in &cursors {
        assert!(matches!(c.speculation(), Speculation::Confirmed { .. }), "{:?}", c.speculation());
        let id = c.claim().unwrap();
        let e = events.iter().find(|e| e.claim == id).unwrap();
        assert_eq!(e.speculation, c.speculation());
    }
    assert!(disc.take_seek_events(Slot::TwoChannel).is_empty(), "taken once");
}

/// A frame numbered within the range accepted before `L` breaks (a): the reading is
/// refuted, the verdict says below which frame positions are possibly different, a warning
/// is logged, and positions from there on are exact.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn a_broken_condition_is_refuted() {
    let mut fs = frames(1000);
    fs.insert(100, PFrame { number: 505, dst: true, data: vec![0xEE; 3000] });
    let img = image(&fs, 1000);
    let mut plain = Disc::open(Cursor::new(img.clone())).unwrap();
    let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = std::sync::Arc::clone(&lines);
    let mut disc = speculative_disc(img, 64);
    disc.set_log(scarletbook::log::Log::new(scarletbook::log::Level::Warn, move |_, m| {
        sink.lock().unwrap().push(m.to_string());
    }));
    let c = plain.frames_cursor(Slot::TwoChannel, FrameSpan::Range(300, 600), CursorOptions::new()).unwrap();
    let (want, _) = read(&mut plain, c);
    let c = disc.frames_cursor(Slot::TwoChannel, FrameSpan::Range(300, 600), CursorOptions::new()).unwrap();
    let (got, c) = read(&mut disc, c);
    let s0 = u64::from(disc.toc().area(Slot::TwoChannel).unwrap().audio_start_lsn);
    assert!(
        matches!(c.resumed_from(), ResumedFrom::Speculative { lsn, .. } if (lsn - s0) % 64 != 0),
        "off a checkpoint boundary: {:?}",
        c.resumed_from()
    );
    let events = verify_all(&mut disc);
    let Speculation::Refuted { first, last_returned, exact_from, .. } = c.speculation() else {
        panic!("{:?}", c.speculation())
    };
    assert_eq!((first, last_returned), (300, Some(599)));
    assert!(exact_from >= 506, "at least a* = 506: {exact_from}");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].speculation, c.speculation());
    for (g, w) in got.iter().zip(&want).filter(|(g, _)| g.number >= exact_from) {
        assert_eq!(g, w);
    }
    assert!(got != want);
    assert!(lines.lock().unwrap().iter().any(|l| l.contains("possibly different")));
    // An exact re-read is now cheap and gives the specification's positions.
    disc.set_seek_policy(Slot::TwoChannel, SeekPolicy::Exact).unwrap();
    let c = disc.frames_cursor(Slot::TwoChannel, FrameSpan::Range(300, 600), CursorOptions::new()).unwrap();
    assert_eq!(read(&mut disc, c).0, want);
}

/// A later provisional start near an earlier provisional reading resumes at one of its
/// provisional checkpoints: no search.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn provisional_checkpoints_serve_later_starts() {
    let img = image(&frames(1000), 1000);
    let mut plain = Disc::open(Cursor::new(img.clone())).unwrap();
    let mut disc = speculative_disc(img, 16);
    let c = disc.frames_cursor(Slot::TwoChannel, FrameSpan::Range(500, 700), CursorOptions::new()).unwrap();
    let (_, c) = read(&mut disc, c);
    assert!(matches!(c.resumed_from(), ResumedFrom::Speculative { probes, .. } if probes > 0));
    for first in [560, 620, 690] {
        let c = disc
            .frames_cursor(Slot::TwoChannel, FrameSpan::Range(first, first + 5), CursorOptions::new())
            .unwrap();
        let (got, c) = read(&mut disc, c);
        assert!(
            matches!(c.resumed_from(), ResumedFrom::Speculative { probes: 0, .. }),
            "{:?}",
            c.resumed_from()
        );
        let p = plain
            .frames_cursor(Slot::TwoChannel, FrameSpan::Range(first, first + 5), CursorOptions::new())
            .unwrap();
        assert_eq!(got, read(&mut plain, p).0, "first {first}");
    }
    // They are checked like any other.
    let events = verify_all(&mut disc);
    assert_eq!(events.len(), 4);
    assert!(events.iter().all(|e| matches!(e.speculation, Speculation::Confirmed { .. })));
}

/// A range between two earlier provisional readings is not started at the end of the
/// first one, far before it: its search finds it (a provisional checkpoint is used only
/// from the reading that covers the range).
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn provisional_checkpoints_of_another_reading_are_not_used() {
    let img = image(&frames(1000), 1000);
    let mut plain = Disc::open(Cursor::new(img.clone())).unwrap();
    let mut disc = speculative_disc(img, 16);
    for first in [100, 900] {
        let c = disc
            .frames_cursor(Slot::TwoChannel, FrameSpan::Range(first, first + 20), CursorOptions::new())
            .unwrap();
        read(&mut disc, c);
    }
    let c = disc.frames_cursor(Slot::TwoChannel, FrameSpan::Range(500, 505), CursorOptions::new()).unwrap();
    let mut it = disc.read_frames(c);
    let got: Vec<TrackFrame> = it.by_ref().map(Result::unwrap).collect();
    let mut c = it.into_cursor();
    let p = plain.frames_cursor(Slot::TwoChannel, FrameSpan::Range(500, 505), CursorOptions::new()).unwrap();
    assert_eq!(got, read(&mut plain, p).0);
    let ResumedFrom::Speculative { lsn, probes, .. } = c.resumed_from() else {
        panic!("{:?}", c.resumed_from())
    };
    assert!(probes > 0, "searched, not started from the first reading's checkpoints (LSN {lsn})");
    // A bounded read: the search and the range, not the 2000 sectors from frame 120 on.
    assert!(c.progress().get().0 < 400, "{} sectors", c.progress().get().0);
}

/// The discs made from one share the verdicts.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn clones_share_verdicts() {
    let img: std::sync::Arc<[u8]> = image(&frames(1000), 1000).into();
    let mut disc = Disc::open(scarletbook::ReadAtReader::new(img)).unwrap();
    disc.enable_seek_index(Slot::TwoChannel, IndexOptions::new()).unwrap();
    disc.set_seek_policy(Slot::TwoChannel, SeekPolicy::Speculative(options())).unwrap();
    let mut clone = disc.clone();
    let c = disc.frames_cursor(Slot::TwoChannel, FrameSpan::Range(800, 810), CursorOptions::new()).unwrap();
    let (_, c) = read(&mut disc, c);
    std::thread::scope(|s| {
        s.spawn(|| while !clone.verify_speculation(Slot::TwoChannel, 1 << 20).unwrap() {});
    });
    assert!(matches!(c.speculation(), Speculation::Confirmed { .. }));
    assert_eq!(disc.take_seek_events(Slot::TwoChannel).len(), 1);
}

/// With a caller-kept index: absorbing takes the reading; extending checks it.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn caller_kept_index_checks_readings() {
    let mut disc = Disc::open(Cursor::new(image(&frames(1000), 1000))).unwrap();
    let area = disc.toc().areas[0].clone();
    let n = disc.toc().sectors;
    let mut index = AreaIndex::new(&area, n, IndexOptions::new()).unwrap();
    let c = TrackCursor::open_range(
        &area,
        700,
        710,
        n,
        CursorOptions::new().with_index(&index).with_policy(SeekPolicy::Speculative(options())),
    )
    .unwrap();
    let (_, mut c) = read(&mut disc, c);
    index.absorb(&mut c).unwrap();
    assert_eq!(index.pending_claim_count(), 1);
    while !index.extend(disc.image_mut(), 1 << 20).unwrap() {}
    assert_eq!(index.pending_claim_count(), 0);
    let events = index.take_events();
    assert_eq!(events.len(), 1);
    assert!(matches!(c.speculation(), Speculation::Confirmed { .. }));
}

/// Past the limit of pending readings, the oldest are dropped as unverified.
#[test]
#[cfg_attr(miri, ignore = "reads many sectors")]
fn the_pending_readings_are_capped() {
    let mut disc = Disc::open(Cursor::new(image(&frames(1000), 1000))).unwrap();
    let area = disc.toc().areas[0].clone();
    let n = disc.toc().sectors;
    let mut index = AreaIndex::new(&area, n, IndexOptions::new()).unwrap();
    for _ in 0..scarletbook::seek::MAX_CLAIMS + 3 {
        let c = TrackCursor::open_range(
            &area,
            700,
            701,
            n,
            CursorOptions::new().with_index(&index).with_policy(SeekPolicy::Speculative(options())),
        )
        .unwrap();
        let (_, mut c) = read(&mut disc, c);
        index.absorb(&mut c).unwrap();
    }
    assert_eq!(index.pending_claim_count(), scarletbook::seek::MAX_CLAIMS);
    let events = index.take_events();
    assert_eq!(events.len(), 3);
    assert!(events.iter().all(|e| matches!(e.speculation, Speculation::Unverified { .. })));
}

proptest! {
    #![proptest_config(config())]

    /// The verdicts are sound on hostile streams (specification 4, 4.2 item 8): a confirmed
    /// reading gives exactly a straight read's positions, and a refuted one gives them from
    /// its `exact_from` on (which is at least `a*`); below it, positions may differ whether
    /// or not they are missing.
    #[test]
    #[cfg_attr(miri, ignore = "reads many sectors")]
    fn verdicts_are_sound(
        audio in common::hostile::stream(),
        a in 0.0f64..1.0,
        interval in prop_oneof![Just(1u32), Just(4), Just(16)],
    ) {
        let playtime = (7 * audio.len() as u32).min(300);
        let img = Spec { channels: 2, playtime, tracks: vec![TrackSpec { start: 0, duration: 1 }], audio }.build();
        let mut plain = Disc::open(Cursor::new(img.clone())).unwrap();
        let mut disc = Disc::open(Cursor::new(img)).unwrap();
        disc.enable_seek_index(Slot::TwoChannel, IndexOptions::new().with_interval(interval)).unwrap();
        let options = SpeculativeOptions::new().with_min_gap_sectors(0);
        disc.set_seek_policy(Slot::TwoChannel, SeekPolicy::Speculative(options)).unwrap();
        let first = (a * f64::from(playtime - 1)) as u32;
        let c = plain.frames_cursor(Slot::TwoChannel, FrameSpan::Range(first, playtime), CursorOptions::new()).unwrap();
        let (want, _) = read(&mut plain, c);
        let c = disc.frames_cursor(Slot::TwoChannel, FrameSpan::Range(first, playtime), CursorOptions::new()).unwrap();
        let (got, c) = read(&mut disc, c);
        while !disc.verify_speculation(Slot::TwoChannel, 1 << 20).unwrap() {}
        match c.speculation() {
            Speculation::Exact | Speculation::Confirmed { .. } => prop_assert_eq!(&got, &want),
            Speculation::Refuted { exact_from, .. } => {
                for (g, w) in got.iter().zip(&want).filter(|(g, _)| g.number >= exact_from) {
                    prop_assert_eq!(g, w);
                }
            }
            other => prop_assert!(false, "{:?}", other),
        }
        let _ = S;
    }
}

/// An index of another image of the same size and layout (here another pressing: same
/// frames, other bytes), merged into a disc's index that waits to check a provisional
/// reading, must not confirm it: the checkpoint's sector digest differs, so the reading is
/// unverified (4.2 item 8: MUST NOT report it as the specification's).
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn a_foreign_index_does_not_confirm() {
    let other: Vec<PFrame> = (0..1000u32)
        .map(|k| PFrame { number: k, dst: true, data: vec![k as u8 ^ 0x5A; 4000 + (k as usize * 37) % 6000] })
        .collect();
    let mut a = Disc::open(Cursor::new(image(&other, 1000))).unwrap();
    let mut b = speculative_disc(image(&frames(1000), 1000), 64);
    let area = b.toc().areas[0].clone();
    let n = b.toc().sectors;
    assert_eq!((area.clone(), n), (a.toc().areas[0].clone(), a.toc().sectors), "same key");
    // A provisional reading of B, waiting in B's (empty) index.
    let c = b.frames_cursor(Slot::TwoChannel, FrameSpan::Range(800, 810), CursorOptions::new()).unwrap();
    let (_, c) = read(&mut b, c);
    assert!(matches!(c.speculation(), Speculation::Pending { .. }));
    // A's complete index, saved and loaded, merged in, and set back on B.
    a.enable_seek_index(Slot::TwoChannel, IndexOptions::new()).unwrap();
    while !a.extend_seek_index(Slot::TwoChannel, 1 << 20).unwrap() {}
    let bytes = a.seek_index(Slot::TwoChannel).unwrap().to_bytes();
    let loaded = AreaIndex::from_bytes(&area, n, &bytes, usize::MAX).unwrap();
    let mut kept = b.take_seek_index(Slot::TwoChannel).unwrap();
    assert_eq!(kept.pending_claim_count(), 1);
    kept.merge(&loaded).unwrap();
    b.set_seek_index(Slot::TwoChannel, kept).unwrap();
    while !b.verify_speculation(Slot::TwoChannel, 1 << 20).unwrap() {}
    assert!(matches!(c.speculation(), Speculation::Unverified { .. }), "{:?}", c.speculation());
    let events = b.take_seek_events(Slot::TwoChannel);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].speculation, c.speculation());
}

/// Another pressing of the image: the same frames (numbers, kinds, sizes), other bytes.
fn other_pressing(n: u32) -> Vec<PFrame> {
    (0..n)
        .map(|k| PFrame { number: k, dst: true, data: vec![k as u8 ^ 0x5A; 4000 + (k as usize * 37) % 6000] })
        .collect()
}

/// A caller-kept index that took a provisional reading of image A is A's: extending or
/// verifying it with another image B of the same size is refused, and the reading is never
/// confirmed against B (#102).
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn a_caller_kept_index_does_not_check_against_another_image() {
    let mut a = Disc::open(Cursor::new(image(&frames(1000), 1000))).unwrap();
    let mut b = Disc::open(Cursor::new(image(&other_pressing(1000), 1000))).unwrap();
    let area = a.toc().areas[0].clone();
    let n = a.toc().sectors;
    assert_eq!((area.clone(), n), (b.toc().areas[0].clone(), b.toc().sectors), "same key");
    let mut index = AreaIndex::new(&area, n, IndexOptions::new()).unwrap();
    let c = TrackCursor::open_range(
        &area,
        700,
        710,
        n,
        CursorOptions::new().with_index(&index).with_policy(SeekPolicy::Speculative(options())),
    )
    .unwrap();
    let (_, mut c) = read(&mut a, c);
    assert!(matches!(c.speculation(), Speculation::Pending { .. }));
    index.absorb(&mut c).unwrap();
    assert_eq!(index.pending_claim_count(), 1);
    assert!(matches!(index.extend(b.image_mut(), 1 << 20), Err(scarletbook::Error::InvalidArgument(_))));
    assert!(matches!(index.verify(b.image_mut()), Err(scarletbook::Error::InvalidArgument(_))));
    assert!(matches!(c.speculation(), Speculation::Pending { .. }));
    // A itself checks it.
    while !index.extend(a.image_mut(), 1 << 20).unwrap() {}
    assert!(matches!(c.speculation(), Speculation::Confirmed { .. }));
}

/// A disc's index holding a provisional reading of its image, set on a disc of another image
/// (which clears the index's image: it is trusted as that disc's), does not check the
/// reading against that other image: it is unverified (#102).
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn a_reading_moves_to_another_disc_unverified() {
    let mut a = speculative_disc(image(&frames(1000), 1000), 64);
    let mut b = Disc::open(Cursor::new(image(&other_pressing(1000), 1000))).unwrap();
    let c = a.frames_cursor(Slot::TwoChannel, FrameSpan::Range(800, 810), CursorOptions::new()).unwrap();
    let (_, c) = read(&mut a, c);
    let kept = a.take_seek_index(Slot::TwoChannel).unwrap();
    assert_eq!(kept.pending_claim_count(), 1);
    b.set_seek_index(Slot::TwoChannel, kept).unwrap();
    while !b.verify_speculation(Slot::TwoChannel, 1 << 20).unwrap() {}
    assert!(matches!(c.speculation(), Speculation::Unverified { .. }), "{:?}", c.speculation());
}

/// A reader that runs a hook before each read, passing whether the read goes back from the
/// farthest byte read so far: a test's way to act at a chosen moment during a pass, on the
/// pass's own thread, with no sleeps. A pass from the first sector only goes forward; going
/// back is the check of a provisional reading.
struct Hooked<F> {
    inner: Cursor<Vec<u8>>,
    farthest: u64,
    hook: F,
}

impl<F: FnMut(bool)> std::io::Read for Hooked<F> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let at = self.inner.position();
        (self.hook)(at < self.farthest);
        let n = self.inner.read(buf)?;
        self.farthest = self.farthest.max(at + n as u64);
        Ok(n)
    }
}

impl<F> std::io::Seek for Hooked<F> {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// An image small enough for Miri, with its speculative disc: an interval of 4 sectors and
/// no minimum gap, so that a range near the end reads provisionally.
fn small_disc() -> (Vec<u8>, Disc<Cursor<Vec<u8>>>) {
    let fs: Vec<PFrame> =
        (0..48).map(|k| PFrame { number: k, dst: true, data: vec![k as u8; 1500] }).collect();
    let tracks = vec![TrackSpec { start: 0, duration: 48 }];
    let img = Spec { channels: 2, playtime: 48, tracks, audio: pack(&fs) }.build();
    let mut disc = Disc::open(Cursor::new(img.clone())).unwrap();
    disc.enable_seek_index(Slot::TwoChannel, IndexOptions::new().with_interval(4)).unwrap();
    let options = SpeculativeOptions::new().with_min_gap_sectors(0);
    disc.set_seek_policy(Slot::TwoChannel, SeekPolicy::Speculative(options)).unwrap();
    (img, disc)
}

/// Reads frames 40 to 44 through `disc`: a provisional reading that the shared index holds
/// from then on, without a verdict yet.
fn small_reading(disc: &mut Disc<Cursor<Vec<u8>>>, cursor: TrackCursor) -> TrackCursor {
    let (got, c) = read(disc, cursor);
    assert_eq!(got.iter().map(|f| f.number).collect::<Vec<_>>(), [40, 41, 42, 43, 44]);
    assert!(
        matches!(c.resumed_from(), ResumedFrom::Speculative { lsn, .. } if lsn > 0),
        "{:?}",
        c.resumed_from()
    );
    assert!(matches!(c.speculation(), Speculation::Pending { .. }), "{:?}", c.speculation());
    c
}

/// A reading registered through a disc sharing the index while the pass that completes the
/// index is reading (#310) is not in the copy that pass checks: the call reports not complete,
/// although the index is, and the next call settles it even with no budget at all.
#[test]
fn a_reading_registered_during_the_final_pass_is_not_reported_complete() {
    let (img, disc) = small_disc();
    let mut other = disc.share(Cursor::new(img.clone())).unwrap();
    let reading = std::cell::RefCell::new(None);
    let armed = std::cell::Cell::new(false);
    let hook = |_| {
        if armed.replace(false) {
            assert!(!other.seek_index(Slot::TwoChannel).unwrap().is_complete(), "during the pass");
            let c = other
                .frames_cursor(Slot::TwoChannel, FrameSpan::Range(40, 45), CursorOptions::new())
                .unwrap();
            reading.replace(Some(small_reading(&mut other, c)));
        }
    };
    let mut verifier = disc.share(Hooked { inner: Cursor::new(img), farthest: 0, hook }).unwrap();
    armed.set(true);
    // One call over the whole area, so that it is the final pass.
    assert!(!verifier.verify_speculation(Slot::TwoChannel, u64::MAX).unwrap());
    let reading = reading.take().expect("registered during the pass");
    assert!(matches!(reading.speculation(), Speculation::Pending { .. }), "{:?}", reading.speculation());
    assert!(disc.seek_index(Slot::TwoChannel).unwrap().is_complete());
    assert!(disc.take_seek_events(Slot::TwoChannel).is_empty());
    assert!(verifier.verify_speculation(Slot::TwoChannel, 0).unwrap());
    assert!(matches!(reading.speculation(), Speculation::Confirmed { .. }), "{:?}", reading.speculation());
    assert_eq!(disc.seek_index(Slot::TwoChannel).unwrap().pending_claim_count(), 0);
    let events = disc.take_seek_events(Slot::TwoChannel);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].speculation, reading.speculation());
}

/// One round per call: a reading registered during the check of an earlier one takes a call
/// of its own, and the loop ends once no disc registers more. It comes from a cursor made
/// before the pass, which is sure to read provisionally.
#[test]
#[cfg_attr(
    miri,
    ignore = "the same path as a_reading_registered_during_the_final_pass_is_not_reported_complete"
)]
fn each_call_makes_one_round_and_the_loop_ends() {
    let (img, disc) = small_disc();
    let mut other = disc.share(Cursor::new(img.clone())).unwrap();
    let mut made_before =
        Some(other.frames_cursor(Slot::TwoChannel, FrameSpan::Range(40, 45), CursorOptions::new()).unwrap());
    let readings = std::cell::RefCell::new(Vec::new());
    let stage = std::cell::Cell::new(0);
    let hook = |back| match stage.get() {
        1 => {
            stage.set(2);
            let c = other
                .frames_cursor(Slot::TwoChannel, FrameSpan::Range(40, 45), CursorOptions::new())
                .unwrap();
            readings.borrow_mut().push(small_reading(&mut other, c));
        }
        2 if back => {
            stage.set(3);
            assert!(other.seek_index(Slot::TwoChannel).unwrap().is_complete(), "during the check");
            let c = made_before.take().unwrap();
            readings.borrow_mut().push(small_reading(&mut other, c));
        }
        _ => {}
    };
    let mut verifier = disc.share(Hooked { inner: Cursor::new(img), farthest: 0, hook }).unwrap();
    stage.set(1);
    let mut verify = || verifier.verify_speculation(Slot::TwoChannel, u64::MAX).unwrap();
    assert_eq!([verify(), verify(), verify()], [false, false, true]);
    assert_eq!(stage.get(), 3);
    let readings = readings.take();
    assert!(matches!(readings[0].speculation(), Speculation::Confirmed { .. }));
    assert!(matches!(readings[1].speculation(), Speculation::Confirmed { .. }));
    assert_eq!(disc.take_seek_events(Slot::TwoChannel).len(), 2);
}

/// What `true` does not cover (#310): a reading that joins the index after the call that
/// reported it stays pending, with no event, until the next call. It comes from a cursor made
/// before the pass, which is sure to read provisionally.
#[test]
#[cfg_attr(
    miri,
    ignore = "the same path as a_reading_registered_during_the_final_pass_is_not_reported_complete"
)]
fn a_reading_registered_after_the_pass_waits_for_the_next_call() {
    let (_, mut disc) = small_disc();
    let c = disc.frames_cursor(Slot::TwoChannel, FrameSpan::Range(40, 45), CursorOptions::new()).unwrap();
    assert!(disc.verify_speculation(Slot::TwoChannel, u64::MAX).unwrap());
    let reading = small_reading(&mut disc, c);
    assert_eq!(disc.seek_index(Slot::TwoChannel).unwrap().pending_claim_count(), 1);
    assert!(disc.take_seek_events(Slot::TwoChannel).is_empty());
    assert!(disc.verify_speculation(Slot::TwoChannel, u64::MAX).unwrap());
    assert!(matches!(reading.speculation(), Speculation::Confirmed { .. }), "{:?}", reading.speculation());
    assert_eq!(disc.take_seek_events(Slot::TwoChannel).len(), 1);
}

/// A budget of 0 moves the pass on by exactly one checkpoint a call, from an empty index on
/// (`merge` used to drop the first, so that such a loop never ended): the loop ends, in one call
/// per boundary and one more, and settles the reading.
#[test]
#[cfg_attr(
    miri,
    ignore = "the same path as a_reading_registered_during_the_final_pass_is_not_reported_complete"
)]
fn no_budget_still_finishes() {
    let (_, mut disc) = small_disc();
    let c = disc.frames_cursor(Slot::TwoChannel, FrameSpan::Range(40, 45), CursorOptions::new()).unwrap();
    let reading = small_reading(&mut disc, c);
    let area = disc.toc().area(Slot::TwoChannel).unwrap();
    let boundaries = usize::try_from(area.audio_end_lsn - area.audio_start_lsn).unwrap() / 4 + 2;
    let mut calls = 0;
    while !disc.verify_speculation(Slot::TwoChannel, 0).unwrap() {
        calls += 1;
        assert_eq!(disc.seek_index(Slot::TwoChannel).unwrap().checkpoint_count(), calls);
        assert!(calls <= boundaries, "{calls} calls");
    }
    assert!(calls > 1, "a call of no budget does not do the whole pass: {calls} calls");
    assert!(matches!(reading.speculation(), Speculation::Confirmed { .. }), "{:?}", reading.speculation());
}

/// A caller-kept index with `count` more provisional readings of frames 40 to 44 of `disc`.
fn caller_readings(
    disc: &mut Disc<Cursor<Vec<u8>>>,
    index: &mut AreaIndex,
    count: usize,
) -> Vec<TrackCursor> {
    let area = disc.toc().area(Slot::TwoChannel).unwrap().clone();
    let n = disc.toc().sectors;
    let options = SpeculativeOptions::new().with_min_gap_sectors(0);
    (0..count)
        .map(|_| {
            let mut c = TrackCursor::for_range_speculative(&area, 40, 45, n, Some(&*index), options).unwrap();
            // Not through the disc's iterators, which would give the reading to the disc's index.
            while let Some(frame) = c.next_frame(disc.image_mut()) {
                frame.unwrap();
            }
            assert!(matches!(c.speculation(), Speculation::Pending { .. }), "{:?}", c.speculation());
            index.absorb(&mut c).unwrap();
            c
        })
        .collect()
}

/// An empty caller-kept index that merges with `disc`'s.
fn caller_index(disc: &Disc<Cursor<Vec<u8>>>) -> AreaIndex {
    let area = disc.toc().area(Slot::TwoChannel).unwrap();
    AreaIndex::new(area, disc.toc().sectors, IndexOptions::new().with_interval(4)).unwrap()
}

/// `count` more provisional readings of frames 40 to 44, held by `disc`'s own index.
fn kept_readings(disc: &mut Disc<Cursor<Vec<u8>>>, count: usize) -> Vec<TrackCursor> {
    (0..count)
        .map(|_| {
            let c = disc.range_frames_cursor(Slot::TwoChannel, 40, 45).unwrap();
            small_reading(disc, c)
        })
        .collect()
}

/// Each of `readings` has exactly one event in `events`, which says what its cursor says, and
/// there are no others.
fn assert_each_once(events: &[scarletbook::SeekEvent], readings: &[TrackCursor]) {
    assert_eq!(events.len(), readings.len());
    for c in readings {
        let id = c.claim().unwrap();
        let mine: Vec<_> = events.iter().filter(|e| e.claim == id).collect();
        assert_eq!(mine.len(), 1, "reading {id}");
        assert_eq!(mine[0].speculation, c.speculation());
    }
}

/// `AreaIndex::merge` past the cap with pending and already settled readings (#310): the oldest
/// pending ones are settled as unverified, each with one event on the index merged into, and
/// nothing is reported again by a second merge, by the other index, or by the check.
#[test]
#[cfg_attr(miri, ignore = "covered by the library's merging_past_the_cap_settles_the_oldest_once")]
fn merging_past_the_cap_reports_every_reading_once() {
    const MINE: usize = 200;
    const SETTLED: usize = 40;
    const THEIRS: usize = 160;
    let (_, mut disc) = small_disc();
    let mut a = caller_index(&disc);
    let mut b = caller_index(&disc);
    let mine = caller_readings(&mut disc, &mut a, MINE);
    let settled = caller_readings(&mut disc, &mut b, SETTLED);
    // A copy of B settles B's first readings: they are shared.
    let mut copy = b.clone();
    while !copy.extend(disc.image_mut(), u64::MAX).unwrap() {}
    let copy_events = copy.take_events();
    assert_each_once(&copy_events, &settled);
    let theirs = caller_readings(&mut disc, &mut b, THEIRS);

    a.merge(&b).unwrap();
    let dropped = MINE + THEIRS - scarletbook::seek::MAX_CLAIMS;
    assert_eq!(a.pending_claim_count(), scarletbook::seek::MAX_CLAIMS);
    let early = a.take_events();
    assert_each_once(&early, &mine[..dropped]);
    assert!(early.iter().all(|e| matches!(e.speculation, Speculation::Unverified { .. })));
    a.merge(&b).unwrap();
    assert!(a.take_events().is_empty(), "a second merge settles nothing");
    assert!(b.take_events().is_empty(), "merging leaves the other index's queue alone");

    while !a.extend(disc.image_mut(), u64::MAX).unwrap() {}
    while !b.extend(disc.image_mut(), u64::MAX).unwrap() {}
    assert!(b.take_events().is_empty(), "A settled all that B held");
    let mut seen = copy_events;
    seen.extend(early);
    seen.extend(a.take_events());
    let every: Vec<_> = mine.into_iter().chain(settled).chain(theirs).collect();
    assert_each_once(&seen, &every);
}

/// A verdict still on an index when a disc takes it over (#310) goes to the disc's queue at once
/// and only once. It used to stay on the kept index, so that it did not reach the queue until
/// the next `extend_seek_index`, which queued it from its copy, as did each later one.
#[test]
fn a_verdict_left_on_a_set_index_is_reported_once() {
    let (_, mut disc) = small_disc();
    // Made before the pass: sure to read provisionally.
    let made_before = disc.range_frames_cursor(Slot::TwoChannel, 40, 45).unwrap();
    let mut index = caller_index(&disc);
    let first = caller_readings(&mut disc, &mut index, 1);
    while !index.extend(disc.image_mut(), u64::MAX).unwrap() {}
    disc.set_seek_index(Slot::TwoChannel, index).unwrap();
    let mut seen = disc.take_seek_events(Slot::TwoChannel);
    assert_each_once(&seen, &first);
    assert!(disc.seek_index(Slot::TwoChannel).unwrap().take_events().is_empty(), "nor on a copy");
    assert!(disc.verify_speculation(Slot::TwoChannel, u64::MAX).unwrap());
    let second = small_reading(&mut disc, made_before);
    assert!(disc.verify_speculation(Slot::TwoChannel, u64::MAX).unwrap());
    assert!(disc.verify_speculation(Slot::TwoChannel, u64::MAX).unwrap());
    seen.extend(disc.take_seek_events(Slot::TwoChannel));
    let every: Vec<_> = first.into_iter().chain([second]).collect();
    assert_each_once(&seen, &every);
}

/// The same past the cap: a disc's index taken, merged with a caller's past the cap and set back
/// without taking its events. The repeats also used to crowd real verdicts out of the queue
/// ([`scarletbook::seek::MAX_EVENTS`]), here 256 of the 400, for a caller taking them at the end.
#[test]
#[cfg_attr(miri, ignore = "covered by a_verdict_left_on_a_set_index_is_reported_once")]
fn a_merge_past_the_cap_set_on_a_disc_reports_every_reading_once() {
    const EACH: usize = 200;
    let (_, mut disc) = small_disc();
    let kept = kept_readings(&mut disc, EACH);
    let mut other = caller_index(&disc);
    let caller = caller_readings(&mut disc, &mut other, EACH);
    let mut index = disc.take_seek_index(Slot::TwoChannel).unwrap();
    index.merge(&other).unwrap();
    disc.set_seek_index(Slot::TwoChannel, index).unwrap();
    while !disc.verify_speculation(Slot::TwoChannel, 8).unwrap() {}
    for _ in 0..8 {
        assert!(disc.verify_speculation(Slot::TwoChannel, 8).unwrap());
    }
    let seen = disc.take_seek_events(Slot::TwoChannel);
    let dropped = 2 * EACH - scarletbook::seek::MAX_CLAIMS;
    let unverified = seen.iter().filter(|e| matches!(e.speculation, Speculation::Unverified { .. }));
    assert_eq!(unverified.count(), dropped);
    let every: Vec<_> = kept.into_iter().chain(caller).collect();
    assert_each_once(&seen, &every);
}

/// `extend_seek_index` merging past the cap: possible only when another disc replaces the index
/// during the pass, so that the copy's readings are not all in the index it merges into (#310).
/// The call that dropped them hands their events over, once.
#[test]
#[cfg_attr(miri, ignore = "covered by a_verdict_left_on_a_set_index_is_reported_once")]
fn a_merge_past_the_cap_during_a_pass_reports_every_reading_once() {
    const EACH: usize = 200;
    let (img, mut disc) = small_disc();
    let kept = kept_readings(&mut disc, EACH);
    let mut replacement = caller_index(&disc);
    let caller = caller_readings(&mut disc, &mut replacement, EACH);
    let mut replacement = Some(replacement);
    let mut other = disc.share(Cursor::new(img.clone())).unwrap();
    let armed = std::cell::Cell::new(false);
    let hook = |_| {
        if armed.replace(false) {
            other.set_seek_index(Slot::TwoChannel, replacement.take().unwrap()).unwrap();
        }
    };
    let mut verifier = disc.share(Hooked { inner: Cursor::new(img), farthest: 0, hook }).unwrap();
    armed.set(true);
    // No budget: the pass does not reach the readings, so that the copy's stay pending.
    assert!(!verifier.verify_speculation(Slot::TwoChannel, 0).unwrap());
    assert!(!armed.get());
    let index = disc.seek_index(Slot::TwoChannel).unwrap();
    assert_eq!(index.pending_claim_count(), scarletbook::seek::MAX_CLAIMS);
    let mut seen = disc.take_seek_events(Slot::TwoChannel);
    assert_eq!(seen.len(), 2 * EACH - scarletbook::seek::MAX_CLAIMS);
    assert!(seen.iter().all(|e| matches!(e.speculation, Speculation::Unverified { .. })));
    while !verifier.verify_speculation(Slot::TwoChannel, u64::MAX).unwrap() {}
    assert!(verifier.verify_speculation(Slot::TwoChannel, u64::MAX).unwrap());
    seen.extend(disc.take_seek_events(Slot::TwoChannel));
    let every: Vec<_> = kept.into_iter().chain(caller).collect();
    assert_each_once(&seen, &every);
}

/// With the index itself in place, the merge of `extend_seek_index` never passes the cap: what
/// the copy holds pending the index holds too, or has settled (here: past the cap, while the
/// pass reads). Those events are the index's own, handed over when the reading registers.
#[test]
#[cfg_attr(miri, ignore = "covered by the library's merging_past_the_cap_settles_the_oldest_once")]
fn a_full_index_does_not_pass_the_cap_during_a_pass() {
    const MORE: usize = 3;
    let (img, mut disc) = small_disc();
    let kept = kept_readings(&mut disc, scarletbook::seek::MAX_CLAIMS);
    let mut other = disc.share(Cursor::new(img.clone())).unwrap();
    let during = std::cell::RefCell::new(Vec::new());
    let armed = std::cell::Cell::new(false);
    let hook = |_| {
        if armed.replace(false) {
            during.replace(kept_readings(&mut other, MORE));
        }
    };
    let mut verifier = disc.share(Hooked { inner: Cursor::new(img), farthest: 0, hook }).unwrap();
    armed.set(true);
    assert!(!verifier.verify_speculation(Slot::TwoChannel, 0).unwrap());
    let mut seen = disc.take_seek_events(Slot::TwoChannel);
    assert_each_once(&seen, &kept[..MORE]);
    while !verifier.verify_speculation(Slot::TwoChannel, u64::MAX).unwrap() {}
    seen.extend(disc.take_seek_events(Slot::TwoChannel));
    let every: Vec<_> = kept.into_iter().chain(during.take()).collect();
    assert_each_once(&seen, &every);
}
