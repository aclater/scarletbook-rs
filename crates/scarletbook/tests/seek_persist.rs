// SPDX-License-Identifier: MIT OR Apache-2.0

//! Saved seek indexes (issue #68): saving and loading, merging, and the checks that keep a
//! stale, foreign or hostile index from doing harm.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;
mod harness;

use std::io::Cursor;

use common::{PFrame, Spec, TrackSpec, pack};
use proptest::prelude::*;
use scarletbook::{
    AreaIndex, CursorOptions, Disc, Error, FrameSpan, IndexOptions, ResumedFrom, Slot, TrackCursor,
    TrackFrame,
};

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
    let c = disc.frames_cursor(Slot::TwoChannel, FrameSpan::Range(first, end), CursorOptions::new()).unwrap();
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
        let loaded = AreaIndex::from_bytes(&area, n, &bytes, usize::MAX).unwrap();
        assert_eq!(loaded, index);
        assert_eq!(loaded.to_bytes(), bytes);
        for first in [0, 17, 100, 199] {
            let c = TrackCursor::open_range(&area, first, p, n, CursorOptions::new().with_index(&loaded))
                .unwrap();
            assert_eq!(frames(&mut d, c).0, straight(&mut d, first, p), "K {k}, first {first}");
        }
        // Every truncation is refused.
        for len in 0..bytes.len() {
            assert!(AreaIndex::from_bytes(&area, n, &bytes[..len], usize::MAX).is_err(), "{len}");
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
    assert!(matches!(
        AreaIndex::from_bytes(&area, n + 1, &bytes, usize::MAX),
        Err(Error::InvalidArgument(_))
    ));
    let mut other = area.clone();
    other.slot = Slot::Multichannel;
    assert!(matches!(AreaIndex::from_bytes(&other, n, &bytes, usize::MAX), Err(Error::InvalidArgument(_))));
    other = area.clone();
    other.channel_count = 6;
    assert!(matches!(AreaIndex::from_bytes(&other, n, &bytes, usize::MAX), Err(Error::InvalidArgument(_))));
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
        let c = TrackCursor::open_range(&area, first, p, n, CursorOptions::new().with_index(&index)).unwrap();
        let (got, mut c) = frames(&mut b, c);
        assert!(matches!(c.resumed_from(), ResumedFrom::Discarded { .. }), "{:?}", c.resumed_from());
        assert_eq!(got, straight(&mut b, first, p));
        // A discarded cursor records nothing.
        let before = index.clone();
        index.absorb(&mut c).unwrap();
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
    let loaded = AreaIndex::from_bytes(&area, n, &index.to_bytes(), usize::MAX).unwrap();
    assert_eq!(loaded, index);
    // Frame 4 starts at sector 16, where `a` is 3 (frame 3 is delivered there).
    let mut c = TrackCursor::open_range(&area, 4, 5, n, CursorOptions::new().with_index(&loaded)).unwrap();
    let f = c.next_frame(&mut image).unwrap().unwrap();
    assert_eq!(f.number, 4);
    assert!(matches!(f.content, scarletbook::FrameContent::Dst(_)), "{f:?}");
    assert!(matches!(c.resumed_from(), ResumedFrom::Checkpoint { lsn: 16, .. }), "{:?}", c.resumed_from());
}

/// Small enough for Miri: more saved confirmations than the saved maximum of hints would be
/// dropped on loading, and the index would then save differently, so they are refused (#253).
/// The same bytes with room for the confirmation load and save unchanged.
#[test]
fn confirmations_beyond_the_saved_maximum_are_refused() {
    let (image, area) = harness::seek_area();
    let n = image.sector_count();
    let mut bytes =
        include_bytes!("regressions/seek_index/crash-122027e767ea9c1aba1588f02d41d45af9969961").to_vec();
    let refused = AreaIndex::from_bytes(&area, n, &harness::with_checksum(&bytes), usize::MAX);
    assert!(matches!(refused, Err(Error::InvalidArgument("seek index confirmations invalid"))));
    // The most seek hints kept (format version 2): one instead of none.
    bytes[92] = 1;
    let summed = harness::with_checksum(&bytes);
    let loaded = AreaIndex::from_bytes(&area, n, &summed, usize::MAX).unwrap();
    assert_eq!(loaded.confirmation_count(), 1);
    assert_eq!(loaded.to_bytes(), summed);
}

/// The FNV-1a (64-bit) checksum of saved bytes, rewritten after they were changed.
fn resum(bytes: &mut [u8]) {
    let body = bytes.len() - 8;
    let sum = bytes[..body]
        .iter()
        .fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    bytes[body..].copy_from_slice(&sum.to_le_bytes());
}

/// Small enough for Miri (#458): merging never joins a loaded index's forged checkpoint
/// with genuine ones. Checkpoints both indexes hold must be equal, so a merge takes all of
/// one index's, which loading checked: here both orders are refused, the indexes
/// unchanged; and an empty index takes the forged one whole, which saves as it loaded.
#[test]
fn merging_never_joins_a_forged_checkpoint() {
    let (mut image, area) = harness::seek_area();
    let n = image.sector_count();
    // Two checkpoints, the second's `a` (0) made 13: the #458 forgery past the area's first
    // sector, where loading accepts it.
    let mut start = AreaIndex::new(&area, n, IndexOptions::new().with_interval(1)).unwrap();
    while start.checkpoint_count() < 2 {
        start.extend(&mut image, 0).unwrap();
    }
    let mut bytes = start.to_bytes();
    assert_eq!(bytes[104..108], 0u32.to_le_bytes());
    bytes[104..108].copy_from_slice(&13u32.to_le_bytes());
    resum(&mut bytes);
    let forged = AreaIndex::from_bytes(&area, n, &bytes, usize::MAX).unwrap();
    let mut genuine = start.clone();
    while !genuine.extend(&mut image, 1000).unwrap() {}
    let genuine_bytes = genuine.to_bytes();
    let mut a = forged.clone();
    assert!(matches!(a.merge(&genuine), Err(Error::InvalidArgument("seek indexes differ"))));
    assert_eq!(a.to_bytes(), bytes);
    let mut b = genuine.clone();
    assert!(matches!(b.merge(&forged), Err(Error::InvalidArgument("seek indexes differ"))));
    assert_eq!(b.to_bytes(), genuine_bytes);
    let mut empty = AreaIndex::new(&area, n, IndexOptions::new().with_interval(1)).unwrap();
    empty.merge(&forged).unwrap();
    assert_eq!(empty.to_bytes(), bytes);
}

/// The damage record of saved bytes (#441): from its header to the checksum, found from
/// the end, and the offset of its header.
fn damage_record(bytes: &[u8]) -> (usize, &[u8]) {
    let u = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap()) as usize;
    let end = bytes.len() - 8;
    let at = (80..end - 16)
        .rev()
        .find(|&o| {
            let (r, d, m) = (u(o), u(o + 4), u(o + 8));
            u(o + 12) == 0
                && (1..64).contains(&r)
                && (1..=64).contains(&d)
                && m < 64
                && o + 16 + 8 * r + 40 * d + 4 * m == end
        })
        .unwrap();
    (at, &bytes[at..end])
}

/// Small enough for Miri (#458): a loaded damage record whose last checkpoint claims a
/// larger `a` than its scan had. Reads from before it record a genuine scan's checkpoints
/// past it, with a smaller `a`: neither absorbing them nor merging an index that holds them
/// joins them to the forged one, so the record stays as loaded and the index saves to
/// bytes that load. The genuine record takes the same reads and merge.
#[test]
fn damage_checkpoints_that_do_not_continue_a_forged_record_are_not_joined() {
    let (mut image, area) = harness::seek_area();
    let n = image.sector_count();
    let p = area.playtime_frames();
    let options = IndexOptions::new().with_interval(2);
    let mut partial = AreaIndex::new(&area, n, options).unwrap();
    while !partial.extend(&mut image, 1000).unwrap() {}
    let mut full = partial.clone();
    image.unreadable_sectors().insert(10, 1);
    partial.extend_past_marks(&mut image, 6).unwrap();
    while !full.extend_past_marks(&mut image, 64).unwrap() {}
    let mut bytes = partial.to_bytes();
    let (at, record) = damage_record(&bytes);
    let (r, d) = (
        u32::from_le_bytes(record[..4].try_into().unwrap()),
        u32::from_le_bytes(record[4..8].try_into().unwrap()),
    );
    assert_eq!((r, d), (1, 2), "one run, two checkpoints");
    // The last checkpoint's `a`, 2, made 19.
    let a = at + 16 + 8 + 40 + 8;
    assert_eq!(bytes[a..a + 4], 2u32.to_le_bytes());
    bytes[a..a + 4].copy_from_slice(&19u32.to_le_bytes());
    resum(&mut bytes);
    let forged = AreaIndex::from_bytes(&area, n, &bytes, usize::MAX).unwrap();
    let forged_record = damage_record(&bytes).1.to_vec();
    let check = |index: &AreaIndex, what: &str| {
        let saved = index.to_bytes();
        let loaded = AreaIndex::from_bytes(&area, n, &saved, usize::MAX);
        assert!(loaded.is_ok_and(|l| l.to_bytes() == saved), "{what}: saves to bytes that do not load");
        damage_record(&saved).1.to_vec()
    };
    for start in [&forged, &partial] {
        let mut copy = start.clone();
        for first in [0, 5, p / 2] {
            let mut c =
                TrackCursor::open_range(&area, first, p, n, CursorOptions::new().with_index(start)).unwrap();
            while let Some(f) = c.next_frame(&mut image) {
                f.unwrap();
            }
            copy.absorb(&mut c).unwrap();
        }
        let absorbed = check(&copy, "absorbed");
        let mut merged = start.clone();
        merged.merge(&full).unwrap();
        let merged = check(&merged, "merged");
        if std::ptr::eq(start, &forged) {
            assert_eq!(absorbed, forged_record, "a forged record took a scan's checkpoints");
            assert_eq!(merged, forged_record, "a forged record took a merged index's checkpoints");
        } else {
            assert!(absorbed.len() > forged_record.len(), "the genuine record did not grow");
            assert_eq!(
                merged,
                damage_record(&full.to_bytes()).1,
                "the genuine record did not take the merge"
            );
        }
    }
}

/// `bytes` of a saved index of the area of [`harness::seek_area`] without damage record
/// or seek hints (its malformed LSNs last), with every checkpoint's and the end state's
/// malformed count made 0 and no malformed LSN kept: consistent, so loading accepts it,
/// but not of the area, whose sector 10 is malformed (#461).
fn without_malformed(bytes: &[u8]) -> Vec<u8> {
    let u32_at = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap()) as usize;
    let (n, m) = (u32_at(36), u32_at(48));
    assert_eq!(m, 1, "the area's malformed sector is kept");
    let mut out = bytes[..bytes.len() - 8 - 4 * m].to_vec();
    out[48..52].copy_from_slice(&0u32.to_le_bytes());
    out[72..80].copy_from_slice(&0u64.to_le_bytes());
    for j in 0..n {
        let at = 80 + j * 24 + 8;
        out[at..at + 8].copy_from_slice(&0u64.to_le_bytes());
    }
    out.extend_from_slice(&[0; 8]);
    resum(&mut out);
    out
}

/// Reads frames `[first, p)` of the area of [`harness::seek_area`] through `index`, and
/// absorbs the cursor into `into`: whether it was taken. Either way `into` saves to bytes
/// that load to it; refused, it is unchanged.
fn read_and_absorb<R: std::io::Read + std::io::Seek>(
    image: &mut scarletbook::Image<R>,
    area: &scarletbook::Area,
    first: u32,
    index: &AreaIndex,
    into: &mut AreaIndex,
) -> bool {
    let (n, p) = (image.sector_count(), area.playtime_frames());
    let mut c = TrackCursor::open_range(area, first, p, n, CursorOptions::new().with_index(index)).unwrap();
    while let Some(f) = c.next_frame(image) {
        f.unwrap();
    }
    let before = into.to_bytes();
    let taken = into.absorb(&mut c);
    if let Err(e) = &taken {
        assert!(
            matches!(e, Error::InvalidArgument("cursor's checkpoints differ from the seek index")),
            "{e:?}"
        );
        assert_eq!(into.to_bytes(), before, "a refused absorb changed the index");
    }
    let saved = into.to_bytes();
    let loaded = AreaIndex::from_bytes(area, n, &saved, usize::MAX);
    assert!(loaded.is_ok_and(|l| l.to_bytes() == saved), "[{first}, {p}): saves to bytes that do not load");
    taken.is_ok()
}

/// Small enough for Miri (#461): loading refuses a checkpoint whose malformed count is not
/// that of the malformed sectors kept below it (here 1 at LSN 8, below which none is kept),
/// and a checkpoint 0 whose `a` is not 0. A loaded index whose counts are consistent but
/// not of the image (none counted) still meets a scan that finds sector 10 malformed: the
/// checkpoints a read from frame 0 records past the frontier would keep it below
/// checkpoints that count none, so absorbing them is refused, with the index unchanged,
/// instead of leaving an index that saves to bytes loading refuses. The genuine index takes
/// the same read.
#[test]
fn checkpoints_whose_malformed_sectors_differ_from_their_count_are_refused() {
    let (mut image, area) = harness::seek_area();
    let n = image.sector_count();
    let mut genuine = AreaIndex::new(&area, n, IndexOptions::new().with_interval(4)).unwrap();
    genuine.extend(&mut image, 13).unwrap();
    assert_eq!(genuine.checkpoint_count(), 4);
    let bytes = genuine.to_bytes();
    // Checkpoint 2's malformed count (after its `a` and `c - r`) made 1; checkpoint 0's `a`
    // made 1.
    for (at, value) in [(80 + 2 * 24 + 8, 1u8), (80, 1)] {
        let mut forged = bytes.clone();
        assert_eq!(forged[at], 0);
        forged[at] = value;
        resum(&mut forged);
        assert!(AreaIndex::from_bytes(&area, n, &forged, usize::MAX).is_err(), "offset {at}");
    }
    let forged = AreaIndex::from_bytes(&area, n, &without_malformed(&bytes), usize::MAX).unwrap();
    let mut copy = forged.clone();
    assert!(!read_and_absorb(&mut image, &area, 0, &forged, &mut copy), "a scan not of the index was taken");
    let mut copy = genuine.clone();
    assert!(read_and_absorb(&mut image, &area, 0, &genuine, &mut copy));
    assert!(copy.is_complete());
}

/// Small enough for Miri (#461): the same for a damage record. A record whose
/// checkpoints count no malformed sector (consistent, so loading accepts it) meets a read
/// from frame 0 that finds sector 10 malformed before the unreadable sector 12: the
/// record does not take the checkpoints it records past its last, which would keep sector
/// 10 below checkpoints that count none, and the index saves to bytes that load. The
/// genuine record takes them.
#[test]
fn damage_checkpoints_whose_malformed_sectors_differ_from_their_count_are_not_joined() {
    let (mut image, area) = harness::seek_area();
    let n = image.sector_count();
    let mut partial = AreaIndex::new(&area, n, IndexOptions::new().with_interval(2)).unwrap();
    while !partial.extend(&mut image, 1000).unwrap() {}
    image.unreadable_sectors().insert(12, 1);
    partial.extend_past_marks(&mut image, 6).unwrap();
    let mut bytes = partial.to_bytes();
    let (at, record) = damage_record(&bytes);
    let u = |o: usize| u32::from_le_bytes(record[o..o + 4].try_into().unwrap()) as usize;
    let (r, d, m) = (u(0), u(4), u(8));
    assert_eq!((r, d, m), (1, 2, 1), "one run, two checkpoints, sector 10");
    // Each checkpoint's malformed count (after its LSN, runs, `a` and `c - r`) made 0, and
    // no malformed LSN kept.
    for j in 0..d {
        let c = at + 16 + 8 * r + 40 * j + 16;
        assert_eq!(bytes[c..c + 8], 1u64.to_le_bytes());
        bytes[c..c + 8].copy_from_slice(&0u64.to_le_bytes());
    }
    bytes[at + 8..at + 12].copy_from_slice(&0u32.to_le_bytes());
    let end = bytes.len() - 8;
    bytes.drain(end - 4..end);
    resum(&mut bytes);
    let forged = AreaIndex::from_bytes(&area, n, &bytes, usize::MAX).unwrap();
    let held = damage_record(&bytes).1.to_vec();
    let mut copy = forged.clone();
    read_and_absorb(&mut image, &area, 0, &forged, &mut copy);
    assert_eq!(damage_record(&copy.to_bytes()).1, held, "a scan not of the record was joined");
    let mut copy = partial.clone();
    read_and_absorb(&mut image, &area, 0, &partial, &mut copy);
    let grown = damage_record(&copy.to_bytes()).1.len();
    assert!(grown > damage_record(&partial.to_bytes()).1.len(), "the genuine record did not grow");
}

/// Small enough for Miri (#461): checkpoints that hold different frames under assembly
/// never share one stored buffer. A loaded index whose last checkpoint (LSN 56, inside
/// frame 5, which starts at sector 20) holds that frame with another number (6) meets a
/// read from frame 4, which records the next checkpoint holding frame 5 from the same
/// sector: it is taken without storing that frame (replayed instead), so the index saves
/// to bytes that load, which require the checkpoints of one buffer to hold the same frame.
#[test]
fn checkpoints_holding_other_frames_do_not_share_a_stored_frame() {
    let (mut image, area) = harness::seek_area();
    let n = image.sector_count();
    let mut genuine = AreaIndex::new(&area, n, IndexOptions::new().with_interval(4)).unwrap();
    while genuine.checkpoint_count() < 15 {
        genuine.extend(&mut image, 0).unwrap();
    }
    assert_eq!(genuine.checkpoint_count(), 15);
    let mut bytes = genuine.to_bytes();
    let u = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize;
    let (count, f, h) = (u(&bytes, 36), u(&bytes, 40), u(&bytes, 44));
    let mut at = 80 + 24 * count;
    for _ in 0..f {
        at += 8 + u(&bytes, at + 4);
    }
    // The last held frame: checkpoint 14's, frame 5 from sector 20.
    let held = at + 24 * (h - 1);
    assert_eq!((u(&bytes, held), u(&bytes, held + 12)), (14, 5));
    bytes[held + 12..held + 16].copy_from_slice(&6u32.to_le_bytes());
    resum(&mut bytes);
    let forged = AreaIndex::from_bytes(&area, n, &bytes, usize::MAX).unwrap();
    let mut copy = forged.clone();
    assert!(read_and_absorb(&mut image, &area, 4, &forged, &mut copy));
    assert!(copy.checkpoint_count() > 15, "the read's checkpoints were not taken");
    let mut copy = genuine.clone();
    assert!(read_and_absorb(&mut image, &area, 4, &genuine, &mut copy));
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
    assert!(index.memory() < 2 * 70_000 + 40 * index.checkpoint_count(), "{}", index.memory());
    let c = TrackCursor::open_range(&area, 1, playtime, n, CursorOptions::new().with_index(&index)).unwrap();
    let (got, c) = frames(&mut d, c);
    let ResumedFrom::Checkpoint { lsn, replay_from, .. } = c.resumed_from() else {
        panic!("{:?}", c.resumed_from())
    };
    assert!(lsn > common::AUDIO_LSN as u64 + 300, "{lsn}");
    assert_eq!(replay_from, lsn, "stored, not replayed");
    assert_eq!(got, straight(&mut d, 1, playtime));
    // Saved and loaded, the buffer stays shared.
    let loaded = AreaIndex::from_bytes(&area, n, &index.to_bytes(), usize::MAX).unwrap();
    assert_eq!(loaded, index);
    // Loaded within a caller's smaller budget, the frame is dropped and replayed instead,
    // with the same frames.
    let small = AreaIndex::from_bytes(&area, n, &index.to_bytes(), 1000).unwrap();
    assert_eq!(small.options().inline_budget, 1000);
    assert!(small.memory() < index.memory());
    let c = TrackCursor::open_range(&area, 1, playtime, n, CursorOptions::new().with_index(&small)).unwrap();
    let (got, c) = frames(&mut d, c);
    let ResumedFrom::Checkpoint { lsn, replay_from, .. } = c.resumed_from() else { panic!() };
    assert!(replay_from < lsn, "replayed");
    assert_eq!(got, straight(&mut d, 1, playtime));
    assert!(index.to_bytes().len() < 70_000 + 60 * index.checkpoint_count());
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
    let c = TrackCursor::open_range(&area, 1, playtime, n, CursorOptions::new().with_index(&index)).unwrap();
    let ResumedFrom::Checkpoint { lsn, replay_from, .. } = c.resumed_from() else { panic!() };
    assert_eq!((lsn - s0, replay_from - s0), (80, 10));
    let mut it = failing.read_frames(c);
    assert!(matches!(it.next(), Some(Err(Error::Io(_)))));
    assert_eq!(it.malformed_sectors(), 20, "the checkpoint's count, nothing replayed added");
    assert_eq!(it.warnings().len(), 20);
}

/// Two images of the same size and layout, different contents (#90): an index built from
/// one refuses the other's cursors (which stop recording), its reads (`extend`) and an
/// index built from it (`merge`); a loaded index takes the image of its first absorb.
#[test]
#[cfg_attr(miri, ignore = "reads a few hundred sectors")]
fn an_index_is_of_one_image() {
    let (mut x, p) = disc(200, 0);
    let (mut y, _) = disc(200, 0x5A);
    let area = x.toc().areas[0].clone();
    let n = x.toc().sectors;
    assert_eq!((area.clone(), n), (y.toc().areas[0].clone(), y.toc().sectors), "same key");
    let options = IndexOptions::new().with_interval(8);
    let mut index = AreaIndex::new(&area, n, options).unwrap();
    index.extend(x.image_mut(), 40).unwrap();
    let before = index.clone();
    // A cursor of Y that recorded checkpoints (made from an empty index, so no digest
    // check stops it), read through Y and absorbed: refused, and it stops recording
    // (absorbing it again does nothing).
    let empty = AreaIndex::new(&area, n, options).unwrap();
    let mut c = TrackCursor::open_range(&area, 150, p, n, CursorOptions::new().with_index(&empty)).unwrap();
    while let Some(f) = c.next_frame(y.image_mut()) {
        f.unwrap();
    }
    assert!(matches!(index.absorb(&mut c), Err(Error::InvalidArgument(_))));
    assert_eq!(index, before);
    let mut fresh = AreaIndex::new(&area, n, options).unwrap();
    fresh.absorb(&mut c).unwrap();
    assert_eq!(fresh.checkpoint_count(), 0, "the refused cursor recorded nothing more");
    // Extending with Y: refused.
    assert!(matches!(index.extend(y.image_mut(), 40), Err(Error::InvalidArgument(_))));
    assert_eq!(index, before);
    // Merging an index of Y: refused, both unchanged.
    let mut of_y = AreaIndex::new(&area, n, options).unwrap();
    of_y.extend(y.image_mut(), 100).unwrap();
    let mut merged = index.clone();
    assert!(matches!(merged.merge(&of_y), Err(Error::InvalidArgument(_))));
    assert_eq!(merged, index);
    // X's own cursors and reads go on.
    let mut c = TrackCursor::open_range(&area, 0, p, n, CursorOptions::new().with_index(&index)).unwrap();
    while let Some(f) = c.next_frame(x.image_mut()) {
        f.unwrap();
    }
    index.absorb(&mut c).unwrap();
    assert!(index.is_complete());
    // Saved and loaded, the index has no image until its first absorb or extend: then
    // it is of that one.
    let mut loaded = AreaIndex::from_bytes(&area, n, &index.to_bytes(), usize::MAX).unwrap();
    assert_eq!(loaded, index, "the image is not part of the saved state");
    let mut c = TrackCursor::open_range(
        &area,
        0,
        1,
        n,
        CursorOptions::new().with_index(&AreaIndex::new(&area, n, options).unwrap()),
    )
    .unwrap();
    while let Some(f) = c.next_frame(y.image_mut()) {
        f.unwrap();
    }
    loaded.absorb(&mut c).unwrap();
    let mut cx = TrackCursor::open_range(
        &area,
        0,
        1,
        n,
        CursorOptions::new().with_index(&AreaIndex::new(&area, n, options).unwrap()),
    )
    .unwrap();
    while let Some(f) = cx.next_frame(x.image_mut()) {
        f.unwrap();
    }
    assert!(matches!(loaded.absorb(&mut cx), Err(Error::InvalidArgument(_))));
}

/// Discs made from one by `share` and `clone` read the same image: their cursors feed one
/// caller-kept index.
#[test]
#[cfg_attr(miri, ignore = "reads a few hundred sectors")]
fn shares_feed_one_index() {
    let (x, p) = disc(200, 0);
    let bytes = x.into_inner().into_inner();
    let mut a =
        Disc::open(scarletbook::ReadAtReader::new(std::sync::Arc::<[u8]>::from(bytes.clone()))).unwrap();
    let mut b = a.clone();
    let mut s = a.share(Cursor::new(bytes)).unwrap();
    let area = a.toc().areas[0].clone();
    let n = a.toc().sectors;
    let mut index = AreaIndex::new(&area, n, IndexOptions::new().with_interval(8)).unwrap();
    index.extend(a.image_mut(), 30).unwrap();
    index.extend(b.image_mut(), 30).unwrap();
    let mut c = TrackCursor::open_range(&area, 100, p, n, CursorOptions::new().with_index(&index)).unwrap();
    while let Some(f) = c.next_frame(s.image_mut()) {
        f.unwrap();
    }
    index.absorb(&mut c).unwrap();
    assert!(index.is_complete());
}
