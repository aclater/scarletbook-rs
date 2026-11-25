// SPDX-License-Identifier: MIT OR Apache-2.0

//! Seeking past unreadable sectors (#441): a seek index keeps the state of a scan that met
//! unreadable sectors, keyed by the runs it met, and a range opened past them starts there
//! instead of scanning the area from its first sector. Whatever the marks, added or removed
//! between reads, and whatever the image, a range read so returns exactly what a scan from
//! the first sector returns; the record survives saving and loading, and an index without
//! one saves as before (the bytes of 1.1.1).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::io::{self, Cursor, Read, Seek};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use common::hostile::{S, stream};
use common::{AUDIO_LSN, PFrame, Spec, TrackSpec, pack};
use proptest::prelude::*;
use proptest::strategy::ValueTree;
use scarletbook::seek::DEFAULT_INLINE_BUDGET;
use scarletbook::{
    Area, AreaIndex, AudioFrame, CursorOptions, Disc, FrameSpan, IndexOptions, ReadAt, ReadAtReader,
    ResumedFrom, Slot, TrackCursor, TrackFrame, Warning,
};

const SLOT: Slot = Slot::TwoChannel;

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

/// A 2-channel disc of `n` DST frames numbered 0 to `n - 1` of 1500 to 6000 bytes (one to
/// three sectors; under Miri 300 to 1200), and its number of audio sectors.
fn frames_disc(n: u32) -> (Vec<u8>, u32, usize) {
    let frames: Vec<PFrame> = (0..n)
        .map(|number| PFrame {
            number,
            dst: true,
            data: (0..if cfg!(miri) {
                300 + (number as usize * 977) % 900
            } else {
                1500 + (number as usize * 977) % 4500
            })
                .map(|i| (i as u32 ^ number) as u8)
                .collect(),
        })
        .collect();
    let audio = pack(&frames);
    let sectors = audio.len();
    let spec = Spec { channels: 2, playtime: n, tracks: vec![TrackSpec { start: 0, duration: n }], audio };
    (spec.build(), n, sectors)
}

/// Well-formed areas: frames of both kinds and any size, numbered one apart (under Miri,
/// a few short DST frames).
fn well_formed() -> impl Strategy<Value = Vec<[u8; S]>> {
    let (most, longest) = if cfg!(miri) { (6, 3000) } else { (40, 9000) };
    proptest::collection::vec((any::<bool>(), 1usize..longest, any::<u8>()), 1..most).prop_map(|kinds| {
        let frames: Vec<PFrame> = kinds
            .iter()
            .enumerate()
            .map(|(i, &(dst, len, seed))| {
                let dst = dst || cfg!(miri);
                let len = if dst { len } else { 2 * 4704 };
                PFrame { number: i as u32, dst, data: (0..len).map(|j| seed ^ j as u8).collect() }
            })
            .collect();
        pack(&frames)
    })
}

/// What a read of a range gives: positions (numbers, contents, lost or not), then counts
/// and warnings after them.
#[derive(Debug, PartialEq)]
struct Got {
    frames: Vec<TrackFrame>,
    missing: u64,
    lost: u64,
    malformed: u64,
    warnings: Vec<Warning>,
    /// The malformed count after each position returned.
    malformed_after: Vec<u64>,
}

fn read<R: Read + Seek>(disc: &mut Disc<R>, cursor: TrackCursor, take: usize) -> (Got, TrackCursor) {
    let mut it = disc.read_frames(cursor);
    let (mut frames, mut malformed_after) = (Vec::new(), Vec::new());
    while frames.len() < take {
        let Some(f) = it.next() else { break };
        frames.push(f.unwrap());
        malformed_after.push(it.malformed_sectors());
    }
    let cursor = it.into_cursor();
    let got = Got {
        frames,
        missing: cursor.missing_frames(),
        lost: cursor.lost_frames(),
        malformed: cursor.malformed_sectors(),
        warnings: cursor.warnings().to_vec(),
        malformed_after,
    };
    (got, cursor)
}

fn area<R: Read + Seek>(disc: &Disc<R>) -> (Area, u64) {
    (disc.toc().areas[0].clone(), disc.toc().sectors)
}

/// What a test does next.
#[derive(Clone, Debug)]
enum Op {
    /// Marks `count` sectors from the fraction `at` of the area.
    Mark(f64, u64),
    /// Unmarks them.
    Unmark(f64, u64),
    /// Reads `take` positions of the range between the two fractions of the play time.
    Read(f64, f64, usize),
    /// Saves the index and loads it again.
    Reload,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (0.0f64..1.0, 1u64..4).prop_map(|(a, k)| Op::Mark(a, k)),
        1 => (0.0f64..1.0, 1u64..6).prop_map(|(a, k)| Op::Unmark(a, k)),
        6 => (0.0f64..1.0, 0.0f64..1.0, 0usize..40).prop_map(|(a, b, t)| Op::Read(a, b, t)),
        1 => Just(Op::Reload),
    ]
}

/// Damage resumes seen by the property test, to show it reaches them.
static DAMAGE_RESUMES: AtomicU64 = AtomicU64::new(0);

/// Runs `ops` on a disc of `audio` with a seek index of `interval`, built complete before
/// any mark when `eager`: every read from the index gives exactly the read of a scan from
/// the first sector with the marks of that moment.
fn resumed_equals_rescan(
    audio: Vec<[u8; S]>,
    interval: u32,
    eager: bool,
    ops: &[Op],
) -> Result<(), TestCaseError> {
    let sectors = audio.len() as f64;
    let (img, playtime) = disc_of(audio);
    let mut disc = Disc::open(Cursor::new(img)).unwrap();
    let (area, n) = area(&disc);
    let mut index = AreaIndex::new(&area, n, IndexOptions::new().with_interval(interval)).unwrap();
    if eager {
        while !index.extend(disc.image_mut(), 64).unwrap() {}
    }
    let lsn = |a: f64| AUDIO_LSN as u64 + (a * sectors) as u64;
    for op in ops {
        match *op {
            Op::Mark(a, k) => disc.unreadable_sectors().insert(lsn(a), k),
            Op::Unmark(a, k) => disc.unreadable_sectors().remove(lsn(a), k),
            Op::Reload => {
                let bytes = index.to_bytes();
                let loaded = AreaIndex::from_bytes(&area, n, &bytes, DEFAULT_INLINE_BUDGET).unwrap();
                prop_assert_eq!(&loaded.to_bytes(), &bytes);
                prop_assert_eq!(&loaded, &index);
                index = loaded;
            }
            Op::Read(a, b, take) => {
                let first = (a * f64::from(playtime)) as u32;
                let end = first + 1 + (b * f64::from(playtime - first - 1)) as u32;
                let straight =
                    disc.frames_cursor(SLOT, FrameSpan::Range(first, end), CursorOptions::new()).unwrap();
                let (want, _) = read(&mut disc, straight, take);
                let options = CursorOptions::new().with_index(&index);
                let sought = TrackCursor::open_range(&area, first, end, n, options).unwrap();
                let (got, mut cursor) = read(&mut disc, sought, take);
                let from = cursor.resumed_from();
                let marks = disc.unreadable_sectors().ranges();
                if let (ResumedFrom::Checkpoint { lsn, .. }, Some(&(m, _))) = (from, marks.first()) {
                    if lsn > m {
                        DAMAGE_RESUMES.fetch_add(1, Ordering::Relaxed);
                    }
                }
                prop_assert_eq!(
                    &got,
                    &want,
                    "[{}, {}) take {} from {:?}, marks {:?}",
                    first,
                    end,
                    take,
                    from,
                    marks
                );
                index.absorb(&mut cursor).unwrap();
            }
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(config())]

    /// The gate of #441: on hostile and well-formed areas, with random marks added and
    /// removed between reads of random ranges, every read from the index (which keeps the
    /// state of the scans that met the marks, saved and loaded on the way) equals a scan
    /// from the first sector, position for position: numbers, contents, lost statuses,
    /// missing, lost and malformed counts (after every position) and warnings.
    ///
    /// Under Miri: two cases of a few short well-formed frames and a few steps.
    #[test]
    fn resumed_reads_equal_rescans(
        audio in if cfg!(miri) { well_formed().boxed() } else { prop_oneof![stream(), well_formed()].boxed() },
        interval in prop_oneof![Just(1u32), Just(2), Just(7), Just(64)],
        eager in any::<bool>(),
        ops in proptest::collection::vec(op(), 1..if cfg!(miri) { 6 } else { 24 }),
    ) {
        resumed_equals_rescan(audio, interval, eager, &ops)?;
    }
}

/// The property test reaches damage resumes: 64 generated cases, or under Miri one fixed
/// case that must (a mark early in a few frames, then two ranges past it).
#[test]
fn the_property_reaches_damage_resumes() {
    if cfg!(miri) {
        let frames: Vec<PFrame> =
            (0..8).map(|number| PFrame { number, dst: true, data: vec![number as u8; 2500] }).collect();
        let ops = [Op::Mark(0.1, 1), Op::Read(0.5, 1.0, 40), Op::Read(0.75, 1.0, 40)];
        resumed_equals_rescan(pack(&frames), 1, true, &ops).unwrap();
        assert!(DAMAGE_RESUMES.load(Ordering::Relaxed) > 0, "no read resumed past a mark");
        return;
    }
    let mut runner = proptest::test_runner::TestRunner::deterministic();
    for _ in 0..64 {
        let ops = proptest::collection::vec(op(), 8..24);
        let (audio, ops) = (well_formed(), ops).new_tree(&mut runner).unwrap().current();
        resumed_equals_rescan(audio, 2, true, &ops).unwrap();
    }
    assert!(DAMAGE_RESUMES.load(Ordering::Relaxed) > 0, "no read resumed past a mark");
}

/// A reader counting the calls and bytes the library asks for.
#[derive(Clone)]
struct Counting {
    bytes: Arc<Vec<u8>>,
    calls: Arc<AtomicU64>,
    read: Arc<AtomicU64>,
}

impl Counting {
    fn new(bytes: Vec<u8>) -> Self {
        Counting { bytes: Arc::new(bytes), calls: Arc::default(), read: Arc::default() }
    }

    /// The calls and sectors since the last time.
    fn take(&self) -> (u64, u64) {
        (self.calls.swap(0, Ordering::Relaxed), self.read.swap(0, Ordering::Relaxed) / S as u64)
    }
}

impl ReadAt for Counting {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.read.fetch_add(buf.len() as u64, Ordering::Relaxed);
        self.bytes.read_at(offset, buf)
    }

    fn size(&self) -> io::Result<u64> {
        Ok(self.bytes.len() as u64)
    }
}

fn raw<R: Read + Seek>(
    disc: &mut Disc<R>,
    (first, end): (u32, u32),
) -> (Vec<TrackFrame>, u64, u64, ResumedFrom) {
    let mut frames = disc.range_frames(SLOT, first, end).unwrap();
    let v: Vec<TrackFrame> = frames.by_ref().map(Result::unwrap).collect();
    let (missing, lost) = (frames.missing_frames(), frames.lost_frames());
    (v, missing, lost, frames.into_cursor().resumed_from())
}

fn decoded<R: Read + Seek>(disc: &mut Disc<R>, (first, end): (u32, u32)) -> (Vec<AudioFrame>, u64) {
    let mut audio = disc.range_audio(SLOT, first, end).unwrap();
    let v: Vec<AudioFrame> = audio.by_ref().map(Result::unwrap).collect();
    (v, audio.lost_frames())
}

/// What a disc without an index, with the marks of `disc`, reads of `range`.
fn straight<R: Read + Seek>(image: &[u8], disc: &Disc<R>, range: (u32, u32)) -> (Vec<TrackFrame>, u64, u64) {
    let mut plain = Disc::open(Cursor::new(image.to_vec())).unwrap();
    for (lsn, count) in disc.unreadable_sectors().ranges() {
        plain.unreadable_sectors().insert(lsn, count);
    }
    let (v, m, l, _) = raw(&mut plain, range);
    (v, m, l)
}

fn checkpoint(from: ResumedFrom) -> Option<u64> {
    match from {
        ResumedFrom::Checkpoint { lsn, .. } => Some(lsn),
        _ => None,
    }
}

/// The disc's own index, built complete before any mark, then a mark: the first range
/// opened past it scans from the first sector, which records the damage state; later ranges
/// past it start there. Marks added before that state, or removed, make it unused until a
/// scan records it again; marks past it change nothing. Saved and loaded, it serves a new
/// disc with the same marks at once.
#[test]
fn ranges_past_marks_start_from_the_damage_record() {
    let (image, playtime, sectors) = frames_disc(if cfg!(miri) { 16 } else { 150 });
    let lsn = |i: usize| (AUDIO_LSN + i) as u64;
    let counting = Counting::new(image.clone());
    let mut disc = Disc::open(ReadAtReader::new(counting.clone())).unwrap();
    disc.enable_seek_index(SLOT, IndexOptions::new().with_interval(2)).unwrap();
    while !disc.extend_seek_index(SLOT, 64).unwrap() {}
    // Under Miri the area is a few sectors: the mark lies before the first checkpoint past
    // the area's first sector.
    let mark = lsn(if cfg!(miri) { 1 } else { sectors / 4 });
    disc.unreadable_sectors().insert(mark, 2);
    counting.take();
    let check = |disc: &mut Disc<ReadAtReader<Counting>>, range: (u32, u32)| {
        let (got, missing, lost, from) = raw(disc, range);
        assert_eq!((got, missing, lost), straight(&image, disc, range), "{range:?} from {from:?}");
        from
    };
    let late = (playtime * 3 / 4, playtime);
    // The first range past the mark: from the first sector (or, in a small area, the
    // checkpoint there).
    let from = check(&mut disc, (playtime / 2, playtime));
    assert!(checkpoint(from).is_none_or(|l| l < mark), "{from:?}");
    let (calls, scanned) = counting.take();
    // The next: from the state past the mark, reading about an interval and the range.
    let from = check(&mut disc, late);
    assert!(checkpoint(from).is_some_and(|l| l > mark), "{from:?}");
    let (calls_after, read_after) = counting.take();
    if !cfg!(miri) {
        assert!(
            read_after * 2 < scanned,
            "{read_after} sectors against {scanned} ({calls_after} and {calls} calls)"
        );
    }
    // Decoded alike.
    let mut plain = Disc::open(Cursor::new(image.clone())).unwrap();
    plain.unreadable_sectors().insert(mark, 2);
    assert_eq!(decoded(&mut disc, late), decoded(&mut plain, late));
    // A mark past the damage state's checkpoint: still used (not in the few sectors of the
    // area under Miri, where no sector lies past it).
    if !cfg!(miri) {
        disc.unreadable_sectors().insert(lsn(sectors - 3), 1);
        assert!(checkpoint(check(&mut disc, late)).is_some_and(|l| l > mark));
    }
    // A mark before it: from the first sector, then from the new state.
    let early = lsn(if cfg!(miri) { 0 } else { sectors / 8 });
    disc.unreadable_sectors().insert(early, 1);
    assert_eq!(check(&mut disc, (playtime / 2, playtime)), ResumedFrom::Start);
    assert!(checkpoint(check(&mut disc, late)).is_some_and(|l| l > mark));
    // Marks removed: the index's own checkpoints, as without marks.
    disc.unreadable_sectors().clear();
    assert!(checkpoint(check(&mut disc, late)).is_some_and(|l| l > mark));
    disc.unreadable_sectors().insert(mark, 2);
    disc.unreadable_sectors().insert(early, 1);
    if !cfg!(miri) {
        disc.unreadable_sectors().insert(lsn(sectors - 3), 1);
    }
    // Saved with the damage state and loaded for a new disc of the image with the same
    // marks: no scan from the first sector.
    let bytes = disc.seek_index(SLOT).unwrap().to_bytes();
    let mut again = Disc::open(Cursor::new(image.clone())).unwrap();
    let (area, n) = area(&again);
    again
        .set_seek_index(SLOT, AreaIndex::from_bytes(&area, n, &bytes, DEFAULT_INLINE_BUDGET).unwrap())
        .unwrap();
    for (l, c) in disc.unreadable_sectors().ranges() {
        again.unreadable_sectors().insert(l, c);
    }
    let (got, missing, lost, from) = raw(&mut again, late);
    assert!(checkpoint(from).is_some_and(|l| l > mark), "{from:?}");
    assert_eq!((got, missing, lost), straight(&image, &again, late));
}

/// Reads and sectors to open a range past a mark, before and after the damage record
/// (#441), on a larger synthetic area: printed for the PR, not checked.
#[test]
#[ignore = "a measurement for #441, run by hand"]
fn cost_of_ranges_past_a_mark() {
    for frames in [1500u32, 6000] {
        let (image, playtime, sectors) = frames_disc(frames);
        for interval in [64u32] {
            let counting = Counting::new(image.clone());
            let mut disc = Disc::open(ReadAtReader::new(counting.clone())).unwrap();
            disc.enable_seek_index(SLOT, IndexOptions::new().with_interval(interval)).unwrap();
            while !disc.extend_seek_index(SLOT, 4096).unwrap() {}
            disc.unreadable_sectors().insert((AUDIO_LSN + sectors / 4) as u64, 1);
            counting.take();
            for (i, at) in [0.5, 0.6, 0.75, 0.9, 0.3, 0.55, 0.8].into_iter().enumerate() {
                let first = (at * f64::from(playtime)) as u32;
                let _ = raw(&mut disc, (first, (first + 75).min(playtime)));
                let (calls, read) = counting.take();
                println!(
                    "I441 frames {frames} sectors {sectors} K {interval} range {i} at {at}: {calls} calls, {read} sectors"
                );
            }
        }
    }
}

/// Indexes saved by 1.1.1 load and save unchanged, and an index built now without marks
/// saves exactly those bytes. The fixtures were saved by 1.1.1 (`b729d19`) for the disc
/// below: `complete-v1.bin` by `AreaIndex::extend` at `K = 4` (version 1),
/// `disc-v2.bin` by a disc's own index at `K = 8` after one range read and extending it
/// complete (version 2, with the TOC fingerprint and seek hints).
///
/// Under Miri, only loading and saving the fixtures: building indexes of 120 frames is left
/// to the native run.
#[test]
fn indexes_of_1_1_1_load_and_save_unchanged() {
    let v1: &[u8] = include_bytes!("fixtures/seek_index_v1_1_1/complete-v1.bin");
    let v2: &[u8] = include_bytes!("fixtures/seek_index_v1_1_1/disc-v2.bin");
    let fs: Vec<PFrame> = (0..120u32)
        .filter(|n| n % 11 != 5)
        .map(|n| PFrame { number: n, dst: true, data: vec![n as u8 ^ 0x5A; 1500 + (n as usize * 97) % 3000] })
        .collect();
    let image = Spec {
        channels: 2,
        playtime: 120,
        tracks: vec![TrackSpec { start: 0, duration: 1 }],
        audio: pack(&fs),
    }
    .build();
    let mut disc = Disc::open(Cursor::new(image.clone())).unwrap();
    let (area, n) = area(&disc);
    for bytes in [v1, v2] {
        let index = AreaIndex::from_bytes(&area, n, bytes, DEFAULT_INLINE_BUDGET).unwrap();
        assert_eq!(index.to_bytes(), bytes);
    }
    if cfg!(miri) {
        return;
    }
    let mut index = AreaIndex::new(&area, n, IndexOptions::new().with_interval(4)).unwrap();
    while !index.extend(disc.image_mut(), 7).unwrap() {}
    assert_eq!(index.to_bytes(), v1);
    disc.enable_seek_index(SLOT, IndexOptions::new().with_interval(8)).unwrap();
    let _ = disc.range_frames(SLOT, 40, 60).unwrap().count();
    while !disc.extend_seek_index(SLOT, 64).unwrap() {}
    assert_eq!(disc.seek_index(SLOT).unwrap().to_bytes(), v2);
    // A read past a mark adds a damage record, saved with flags bit 2; without it, the
    // index saves in a form 1.1.1 reads (see `damage_records_round_trip`).
    disc.unreadable_sectors().insert((AUDIO_LSN + 30) as u64, 1);
    let _ = disc.range_frames(SLOT, 60, 120).unwrap().count();
    let index = disc.seek_index(SLOT).unwrap();
    assert_eq!(index.to_bytes()[14] & 4, 4, "a damage record is saved");
    assert_eq!(index.without_damage().to_bytes()[14] & 4, 0);
}

/// The damage record's saved form: flags bit 2 and a section before the checksum. It
/// round-trips; without the bit and the section it is the index without the record (so a
/// reader that does not know the bit, as before 1.3.0, sees an unknown flag and refuses
/// it); and changing any byte of it (with the checksum made right) never panics.
#[test]
fn damage_records_round_trip() {
    let (image, playtime, sectors) = frames_disc(if cfg!(miri) { 16 } else { 60 });
    let mut disc = Disc::open(Cursor::new(image)).unwrap();
    disc.enable_seek_index(SLOT, IndexOptions::new().with_interval(1)).unwrap();
    disc.unreadable_sectors().insert((AUDIO_LSN + (sectors / 3).max(1)) as u64, 1);
    let _ = disc.range_frames(SLOT, 0, playtime).unwrap().count();
    let index = disc.seek_index(SLOT).unwrap();
    let (area, n) = area(&disc);
    let bytes = index.to_bytes();
    let loaded = AreaIndex::from_bytes(&area, n, &bytes, DEFAULT_INLINE_BUDGET).unwrap();
    assert_eq!(loaded, index);
    assert_eq!(loaded.to_bytes(), bytes);
    assert_eq!(bytes[14] & 4, 4, "flags bit 2");
    let plain = index.without_damage().to_bytes();
    assert_eq!(plain[14] & 4, 0);
    // The plain bytes are these without the bit, the section and the checksum, re-summed.
    let body = plain.len() - 8;
    let mut stripped = bytes[..body].to_vec();
    stripped[14] &= !4;
    assert_eq!(stripped[..], plain[..body]);
    let fnv = |b: &[u8]| {
        b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &x| (h ^ u64::from(x)).wrapping_mul(0x0100_0000_01b3))
    };
    assert_eq!(fnv(&stripped).to_le_bytes()[..], plain[body..]);
    // Every byte of the section changed, the checksum made right: refused or loaded, never a
    // panic; loaded, it saves as it was given.
    let step = if cfg!(miri) { 31 } else { 1 };
    for at in (body..bytes.len() - 8).step_by(step) {
        for delta in [1u8, 0x80] {
            let mut b = bytes.clone();
            b[at] = b[at].wrapping_add(delta);
            let sum = fnv(&b[..b.len() - 8]);
            let len = b.len();
            b[len - 8..].copy_from_slice(&sum.to_le_bytes());
            if let Ok(i) = AreaIndex::from_bytes(&area, n, &b, DEFAULT_INLINE_BUDGET) {
                assert_eq!(i.to_bytes(), b, "byte {at}");
            }
        }
    }
}

/// More runs of unreadable sectors than a damage record keeps (4096): the record keeps the
/// first 4096 and the checkpoints before the next, saves and loads, and reads from it
/// equal a scan from the first sector.
#[test]
#[cfg_attr(
    miri,
    ignore = "needs more than 4096 marked runs; damage_records_round_trip covers saving under Miri"
)]
fn more_runs_than_a_record_keeps() {
    let (image, playtime, sectors) = frames_disc(5200);
    assert!(sectors > 2 * 4100 + 64, "{sectors} sectors");
    let mut disc = Disc::open(Cursor::new(image.clone())).unwrap();
    disc.enable_seek_index(SLOT, IndexOptions::new().with_interval(64)).unwrap();
    // 4100 single sectors apart, from the second sector on: every boundary is readable.
    for i in 0..4100u64 {
        disc.unreadable_sectors().insert((AUDIO_LSN + 1) as u64 + 2 * i, 1);
    }
    let all = (0, playtime);
    let (got, missing, lost, _) = raw(&mut disc, all);
    assert_eq!((got, missing, lost), straight(&image, &disc, all));
    let index = disc.seek_index(SLOT).unwrap();
    let bytes = index.to_bytes();
    assert_eq!(bytes[14] & 4, 4, "a damage record is kept");
    let (area, n) = area(&disc);
    let loaded = AreaIndex::from_bytes(&area, n, &bytes, DEFAULT_INLINE_BUDGET).unwrap();
    assert_eq!(loaded, index);
    assert_eq!(loaded.to_bytes(), bytes);
    // A new disc with the loaded index and the same marks: ranges equal the scan, and one
    // past the marks starts from the record.
    let mut again = Disc::open(Cursor::new(image.clone())).unwrap();
    again.set_seek_index(SLOT, loaded).unwrap();
    for (l, c) in disc.unreadable_sectors().ranges() {
        again.unreadable_sectors().insert(l, c);
    }
    let mut resumed = false;
    for first in [playtime / 4, playtime / 2, playtime * 3 / 4, playtime - 1] {
        let range = (first, playtime);
        let (got, missing, lost, from) = raw(&mut again, range);
        assert_eq!((got, missing, lost), straight(&image, &again, range), "{range:?} from {from:?}");
        resumed |= checkpoint(from).is_some_and(|l| l > (AUDIO_LSN + 1) as u64);
    }
    assert!(resumed, "no range started from the record");
    let bytes = again.seek_index(SLOT).unwrap().to_bytes();
    AreaIndex::from_bytes(&area, n, &bytes, DEFAULT_INLINE_BUDGET).unwrap();
}
