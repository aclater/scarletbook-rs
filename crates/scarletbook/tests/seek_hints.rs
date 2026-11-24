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
use common::{AREA_TOC_LSN, AUDIO_LSN, PFrame, Spec, TrackSpec, pack};
use scarletbook::seek::DEFAULT_INLINE_BUDGET;
use scarletbook::{
    AreaIndex, CursorOptions, Disc, FrameSpan, IndexOptions, ResumedFrom, SeekPolicy, Slot, Speculation,
    SpeculativeOptions, TrackFrame,
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
    image_with(offset, tracks, 0)
}

/// As [`image`], with `fill` mixed into the frames' bytes: the same layout and records.
fn image_with(offset: u32, tracks: u32, fill: u8) -> Vec<u8> {
    let fs: Vec<PFrame> = (0..2000u32)
        .map(|k| PFrame {
            number: k + offset,
            dst: true,
            data: vec![k as u8 ^ fill; 3000 + (k as usize * 53) % 6000],
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
    let c = disc.frames_cursor(Slot::TwoChannel, FrameSpan::Range(first, end), CursorOptions::new()).unwrap();
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
    assert!(index.hint_count() >= RANGES.len(), "{} hints", index.hint_count());
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
        // The first session did not verify its readings: provisional.
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
    assert!(index.hint_count() < 20, "{} hints", index.hint_count());
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
    assert!(small.hint_count() <= 2 && small.hint_count() > 0, "{}", small.hint_count());
    // And as it grows: the bound is the caller's.
    assert_eq!(small.options().max_hints, 2);
    assert!(index.hint_count() <= IndexOptions::new().max_hints);
}

/// A first session that verified its readings (one pass) saves their confirmations: after a
/// remount, a reading from a confirmed `L` for the same or a later first frame is reported
/// confirmed at once (4.2 item 9), with one read call; on an image with the same TOC and
/// records but other bytes at `L`, it stays provisional.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn confirmations_after_a_remount() {
    let img = image(0, 4);
    let (mut disc, _, _) = open(img.clone());
    disc.enable_seek_index(Slot::TwoChannel, IndexOptions::new()).unwrap();
    speculative(&mut disc);
    for r in RANGES {
        read(&mut disc, r);
    }
    while !disc.verify_speculation(Slot::TwoChannel, 1 << 20).unwrap() {}
    let index = disc.seek_index(Slot::TwoChannel).unwrap();
    assert_eq!(index.confirmation_count(), RANGES.len());
    // Saved small: hints and confirmations, not the exact checkpoints.
    let full = index.to_bytes().len();
    let saved = index.without_checkpoints().to_bytes();
    assert!(saved.len() < full, "{} of {full} bytes", saved.len());
    assert_eq!(u32::from_le_bytes(saved[8..12].try_into().unwrap()), 3, "version 3");
    let mut plain = Disc::open(Cursor::new(img.clone())).unwrap();
    for (first, end) in RANGES.iter().flat_map(|&(f, e)| [(f, e), (f + 2, e + 2)]) {
        let (want, _) = read(&mut plain, (first, end));
        let (mut disc, _, calls) = open(img.clone());
        let area = disc.toc().areas[0].clone();
        let index = AreaIndex::from_bytes(&area, disc.toc().sectors, &saved, DEFAULT_INLINE_BUDGET).unwrap();
        assert_eq!(
            AreaIndex::from_bytes(&area, disc.toc().sectors, &saved, usize::MAX).unwrap().to_bytes(),
            saved
        );
        disc.set_seek_index(Slot::TwoChannel, index).unwrap();
        speculative(&mut disc);
        let c0 = calls.get();
        let (got, c) = read(&mut disc, (first, end));
        assert_eq!(got, want, "[{first}, {end})");
        assert_eq!(calls.get() - c0, 1, "[{first}, {end})");
        assert!(matches!(c.speculation(), Speculation::Confirmed { .. }), "{:?}", c.speculation());
    }
    // Other bytes at `L`, the same TOC and records: provisional, and the frames are that
    // image's.
    let other = image_with(0, 4, 0x5A);
    let mut plain = Disc::open(Cursor::new(other.clone())).unwrap();
    let (mut disc, _, _) = open(other);
    let area = disc.toc().areas[0].clone();
    let index = AreaIndex::from_bytes(&area, disc.toc().sectors, &saved, DEFAULT_INLINE_BUDGET).unwrap();
    disc.set_seek_index(Slot::TwoChannel, index).unwrap();
    speculative(&mut disc);
    let (got, c) = read(&mut disc, RANGES[1]);
    assert_eq!(got, read(&mut plain, RANGES[1]).0);
    assert!(matches!(c.speculation(), Speculation::Pending { .. }), "{:?}", c.speculation());
}

/// An image with A's area (the same key) but another TOC (5 tracks) and other bytes in the
/// first 100 frames: the sectors of the ranges read are A's, those before them are not.
fn image_b() -> Vec<u8> {
    let fs: Vec<PFrame> = (0..2000u32)
        .map(|k| PFrame {
            number: k,
            dst: true,
            data: vec![if k < 100 { k as u8 ^ 0x77 } else { k as u8 }; 3000 + (k as usize * 53) % 6000],
        })
        .collect();
    let tracks = (0..5).map(|i| TrackSpec { start: i * 400, duration: 400 }).collect();
    Spec { channels: 2, playtime: 2000, tracks, audio: pack(&fs) }.build()
}

/// The saved state of a first session on image A that verified its readings: hints and
/// confirmations, without checkpoints.
fn confirmed_session(img: &[u8]) -> Vec<u8> {
    let (mut disc, _, _) = open(img.to_vec());
    disc.enable_seek_index(Slot::TwoChannel, IndexOptions::new()).unwrap();
    speculative(&mut disc);
    for r in RANGES {
        read(&mut disc, r);
    }
    while !disc.verify_speculation(Slot::TwoChannel, 1 << 20).unwrap() {}
    disc.seek_index(Slot::TwoChannel).unwrap().without_checkpoints().to_bytes()
}

/// 4.2 item 9: saved confirmations hold only for the image the pass read, which a disc
/// establishes by checking the index's TOC fingerprint against its own. An index loaded for
/// image A and used on image B through the low-level path (no disc checks its fingerprint),
/// whose sectors at `L` are A's: its readings stay provisional, and verifying them on B
/// keeps no confirmation; through a disc of B, the index is refused; through a disc of A,
/// it confirms.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn confirmations_need_a_disc_that_checked_the_fingerprint() {
    let a = image(0, 4);
    let saved = confirmed_session(&a);
    let b = image_b();
    let (mut disc_b, _, _) = open(b.clone());
    let area = disc_b.toc().areas[0].clone();
    let n = disc_b.toc().sectors;
    let mut index = AreaIndex::from_bytes(&area, n, &saved, DEFAULT_INLINE_BUDGET).unwrap();
    let kept = index.confirmation_count();
    assert_eq!(kept, RANGES.len());
    let options = SpeculativeOptions::new().with_min_gap_sectors(64);
    let (first, end) = RANGES[1];
    let mut cursor = scarletbook::TrackCursor::open_range(
        &area,
        first,
        end,
        n,
        CursorOptions::new().with_index(&index).with_policy(SeekPolicy::Speculative(options)),
    )
    .unwrap();
    while let Some(f) = cursor.next_frame(disc_b.image_mut()) {
        f.unwrap();
    }
    assert!(matches!(cursor.resumed_from(), ResumedFrom::Speculative { probes: 0, .. }), "from a hint");
    assert!(matches!(cursor.speculation(), Speculation::Pending { .. }), "{:?}", cursor.speculation());
    index.absorb(&mut cursor).unwrap();
    while !index.extend(disc_b.image_mut(), 1 << 20).unwrap() {}
    assert!(matches!(cursor.speculation(), Speculation::Confirmed { .. }), "verified on B itself");
    assert_eq!(index.confirmation_count(), kept, "nothing kept for B under A's fingerprint");
    // A disc of B refuses the index (another TOC); a disc of A takes it and confirms.
    let loaded = AreaIndex::from_bytes(&area, n, &saved, DEFAULT_INLINE_BUDGET).unwrap();
    assert!(disc_b.set_seek_index(Slot::TwoChannel, loaded).is_err());
    let (mut disc_a, _, _) = open(a);
    let loaded = AreaIndex::from_bytes(&area, n, &saved, DEFAULT_INLINE_BUDGET).unwrap();
    disc_a.set_seek_index(Slot::TwoChannel, loaded).unwrap();
    speculative(&mut disc_a);
    let (_, c) = read(&mut disc_a, (first, end));
    assert!(matches!(c.speculation(), Speculation::Confirmed { .. }), "{:?}", c.speculation());
}

/// A 2-channel area of `n` frames in `tracks` tracks of equal length, plain DSD or DST
/// frames of varying sizes, whose TOC gives each track's start LSN as the sector where the
/// track's first frame starts (as on a well-formed disc), moved `shift` sectors later for
/// every track but the first; and those sectors, unmoved.
fn image_tracked(dst: bool, n: u32, tracks: u32, shift: u32) -> (Vec<u8>, Vec<u64>) {
    let fs: Vec<PFrame> = (0..n)
        .map(|k| PFrame {
            number: k,
            dst,
            data: vec![k as u8; if dst { 3000 + (k as usize * 53) % 6000 } else { 2 * 4704 }],
        })
        .collect();
    let audio = pack(&fs);
    // The sector each frame starts in, from the sectors' frame records (4.3.1, 4.3.3).
    let mut at = vec![0u64; n as usize];
    for (i, s) in audio.iter().enumerate() {
        let (f, k, r) =
            (usize::from((s[0] >> 2) & 7), usize::from(s[0] >> 5), if s[0] & 1 == 1 { 4 } else { 3 });
        for j in 0..f {
            let p = 1 + 2 * k + j * r;
            let t = (u32::from(s[p]) * 60 + u32::from(s[p + 1])) * 75 + u32::from(s[p + 2]);
            at[t as usize] = (AUDIO_LSN + i) as u64;
        }
    }
    let per = n / tracks;
    let starts: Vec<u64> = (0..tracks).map(|i| at[(i * per) as usize]).collect();
    let specs = (0..tracks).map(|i| TrackSpec { start: i * per, duration: per }).collect();
    let mut img = Spec { channels: 2, playtime: n, tracks: specs, audio }.build();
    let trl1 = (AREA_TOC_LSN + 4) * S;
    for (i, &l) in starts.iter().enumerate() {
        let l = if i == 0 { l } else { l + u64::from(shift) };
        img[trl1 + 8 + 4 * i..][..4].copy_from_slice(&(l as u32).to_be_bytes());
    }
    (img, starts)
}

/// A reader that logs each read call's image offset and length.
struct Logged<R> {
    inner: R,
    pos: u64,
    log: Rc<std::cell::RefCell<Vec<(u64, usize)>>>,
}

impl<R: std::io::Read> std::io::Read for Logged<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.log.borrow_mut().push((self.pos, n));
        self.pos += n as u64;
        Ok(n)
    }
}

impl<R: std::io::Seek> std::io::Seek for Logged<R> {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.pos = self.inner.seek(pos)?;
        Ok(self.pos)
    }
}

/// The reads of one file system's head and tail reads (issue #431): for each track in
/// turn, its first `head` positions, then its last `tail`, each from a new cursor, the
/// cursor before absorbed once the next is open; every range's frames, and each range's
/// reads (offset, length), after the disc is open and `saved` (if any) loaded.
#[allow(clippy::type_complexity)]
fn head_then_tail(
    img: &[u8],
    saved: Option<&[u8]>,
    (head, tail): (u32, u32),
) -> (Vec<Vec<TrackFrame>>, Vec<Vec<(u64, usize)>>, Vec<u8>) {
    let log = Rc::new(std::cell::RefCell::new(Vec::new()));
    let reader = Logged { inner: Cursor::new(img.to_vec()), pos: 0, log: Rc::clone(&log) };
    let mut disc = Disc::open(reader).unwrap();
    let area = disc.toc().areas[0].clone();
    match saved {
        Some(saved) => {
            let index =
                AreaIndex::from_bytes(&area, disc.toc().sectors, saved, DEFAULT_INLINE_BUDGET).unwrap();
            disc.set_seek_index(Slot::TwoChannel, index).unwrap();
        }
        None => disc.enable_seek_index(Slot::TwoChannel, IndexOptions::new()).unwrap(),
    }
    let options = SpeculativeOptions::new().with_min_gap_sectors(16);
    disc.set_seek_policy(Slot::TwoChannel, SeekPolicy::Speculative(options)).unwrap();
    let (mut frames, mut reads, mut prev) = (Vec::new(), Vec::new(), None);
    for t in &area.tracks {
        let (s, e) = (t.start_frame, t.end_frame());
        for (first, end) in [(s, s + head), (e - tail, e)] {
            log.borrow_mut().clear();
            let c = disc
                .frames_cursor(Slot::TwoChannel, FrameSpan::Range(first, end), CursorOptions::new())
                .unwrap();
            if let Some(mut p) = prev.take() {
                disc.absorb_seek_checkpoints(&mut p);
            }
            let mut it = disc.read_frames(c);
            frames.push(it.by_ref().map(Result::unwrap).collect());
            prev = Some(it.into_cursor());
            reads.push(log.borrow().clone());
        }
    }
    if let Some(mut p) = prev.take() {
        disc.absorb_seek_checkpoints(&mut p);
    }
    (frames, reads, disc.seek_index(Slot::TwoChannel).unwrap().to_bytes())
}

/// Issue #431: after a remount, the head of a track read after the tail of the one before
/// begins at the track's start sector, not at the seek hint where that tail's reading
/// began, with one read call; the frames are an exact read's. The reads are pinned.
#[test]
fn a_track_head_after_a_tail_starts_at_the_track() {
    let (n, tracks) = if cfg!(miri) { (90, 3) } else { (240, 4) };
    let (img, starts) = image_tracked(false, n, tracks, 0);
    let (_, _, saved) = head_then_tail(&img, None, (14, 7));
    let (got, reads, _) = head_then_tail(&img, Some(&saved), (14, 7));
    let mut plain = Disc::open(Cursor::new(img.clone())).unwrap();
    let area = plain.toc().areas[0].clone();
    let ranges = area
        .tracks
        .iter()
        .flat_map(|t| [(t.start_frame, t.start_frame + 14), (t.end_frame() - 7, t.end_frame())]);
    for ((first, end), got) in ranges.zip(&got) {
        assert_eq!(*got, read(&mut plain, (first, end)).0, "[{first}, {end})");
    }
    for (i, &l) in starts.iter().enumerate().skip(1) {
        let head = &reads[2 * i];
        assert_eq!(head.len(), 1, "track {}: {head:?}", i + 1);
        assert_eq!(head[0].0, l * S as u64, "track {} head from its start sector: {head:?}", i + 1);
    }
    if !cfg!(miri) {
        // One read per range: track 1's head from the area's first sector, each tail from
        // its hint, each later head from its track's start sector.
        let pinned: &[&[(u64, usize)]] = &[
            &[(1_118_208, 262_144)],
            &[(1_607_680, 77_824)],
            &[(1_683_456, 133_120)],
            &[(2_115_584, 135_168)],
            &[(2_248_704, 133_120)],
            &[(2_682_880, 133_120)],
            &[(2_813_952, 133_120)],
            &[(3_248_128, 133_120)],
        ];
        assert_eq!(reads, pinned, "reads");
    }
}

/// Speculative reads equal exact ones for every first frame, with seek hints from a first
/// session, on plain DSD and DST areas; also where the TOC's track start LSNs lie a few
/// sectors past the tracks' first frames (the reading then begins at the hint).
#[test]
fn speculative_equals_exact_from_every_frame_with_track_starts() {
    let (n, tracks, step) = if cfg!(miri) { (60, 2, 29) } else { (240, 4, 1) };
    for (dst, shift) in [(false, 0), (true, 0), (false, 3), (true, 2)] {
        let (img, _) = image_tracked(dst, n, tracks, shift);
        let (_, _, saved) = head_then_tail(&img, None, (14, 7));
        let mut plain = Disc::open(Cursor::new(img.clone())).unwrap();
        for first in (0..n).step_by(step) {
            let end = (first + 3).min(n);
            let (mut disc, _, _) = open(img.clone());
            let area = disc.toc().areas[0].clone();
            let index =
                AreaIndex::from_bytes(&area, disc.toc().sectors, &saved, DEFAULT_INLINE_BUDGET).unwrap();
            disc.set_seek_index(Slot::TwoChannel, index).unwrap();
            let options = SpeculativeOptions::new().with_min_gap_sectors(16);
            disc.set_seek_policy(Slot::TwoChannel, SeekPolicy::Speculative(options)).unwrap();
            let (got, _) = read(&mut disc, (first, end));
            assert_eq!(got, read(&mut plain, (first, end)).0, "dst {dst}, shift {shift}: [{first}, {end})");
        }
    }
}
