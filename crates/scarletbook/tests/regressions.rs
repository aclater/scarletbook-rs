// SPDX-License-Identifier: MIT OR Apache-2.0

//! Minimised fuzz findings, replayed through the fuzz harness. Each file in
//! `tests/regressions/<target>/` is one input.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod harness;

use std::path::Path;

fn replay(target: &str, run: fn(&[u8])) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/regressions").join(target);
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    let mut paths: Vec<_> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for p in paths.iter().filter(|p| p.is_file() && p.extension().is_none_or(|e| e != "md")) {
        eprintln!("replaying {}", p.display());
        run(&std::fs::read(p).unwrap());
    }
}

#[test]
#[cfg_attr(miri, ignore = "slow under Miri")]
fn toc_regressions() {
    replay("toc", harness::toc);
}

#[test]
#[cfg_attr(miri, ignore = "slow under Miri")]
fn demux_regressions() {
    replay("demux", harness::demux);
}

#[test]
#[cfg_attr(miri, ignore = "slow under Miri")]
fn dst_regressions() {
    replay("dst", harness::dst);
}

#[test]
#[cfg_attr(miri, ignore = "slow under Miri")]
fn seek_index_regressions() {
    replay("seek_index", harness::seek_index);
}

/// The fuzz seeds load, with their checksum, and go through the harness.
#[test]
#[cfg_attr(miri, ignore = "slow under Miri")]
fn seek_index_seeds_load() {
    let (image, area) = harness::seek_area();
    let (mut stored, mut hinted, mut confirmed, mut damaged) = (false, false, false, false);
    for seed in harness::seek_index_seeds() {
        let mut bytes = seed.clone();
        let sum = seed
            .iter()
            .fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
        bytes.extend_from_slice(&sum.to_le_bytes());
        let full =
            scarletbook::AreaIndex::from_bytes(&area, image.sector_count(), &bytes, usize::MAX).unwrap();
        let bare = scarletbook::AreaIndex::from_bytes(&area, image.sector_count(), &bytes, 0).unwrap();
        stored |= bare.memory() < full.memory();
        // Format version 2 (#124): seek hints recorded by extend.
        hinted |= full.hint_count() > 0;
        confirmed |= full.confirmation_count() > 0;
        // Flags bit 2 (#441): a damage record.
        damaged |= bytes[14] & 4 != 0;
        harness::seek_index(&seed);
    }
    assert!(stored, "no seed stores a frame under assembly");
    assert!(hinted, "no seed holds seek hints");
    assert!(confirmed, "no seed holds a saved confirmation");
    assert!(damaged, "no seed holds a damage record");
}

/// The disc seeds of the fuzz target (#124): a disc of the harness takes them (their TOC
/// fingerprint is its own), one holds seek hints alone, so that its readings are compared
/// with reads without an index, and one holds saved confirmations.
#[test]
#[cfg_attr(miri, ignore = "slow under Miri")]
fn seek_index_disc_seeds_load() {
    let d = harness::seek_disc();
    let (mut hints_only, mut confirmed) = (false, false);
    for seed in harness::seek_index_disc_seeds() {
        let index = scarletbook::AreaIndex::from_bytes(
            &d.area,
            d.sectors,
            &harness::with_checksum(&seed),
            usize::MAX,
        )
        .unwrap();
        hints_only |=
            index.hint_count() > 0 && index.checkpoint_count() == 0 && index.confirmation_count() == 0;
        confirmed |= index.confirmation_count() > 0;
        let mut disc = scarletbook::Disc::open(std::io::Cursor::new(&d.image[..])).unwrap();
        disc.set_seek_index(d.area.slot, index).unwrap();
        harness::seek_index(&seed);
    }
    assert!(hints_only, "no disc seed holds seek hints alone");
    assert!(confirmed, "no disc seed holds a saved confirmation");
}

/// Small enough for Miri (#458): a loaded index whose checkpoint 1 claims `a = 13`. A read
/// from frame 0 resumes at checkpoint 0, before it, so it never checks it, and its scan's
/// checkpoints have a smaller `a`: absorbing them is refused, with the index unchanged,
/// instead of leaving an index that saves to bytes loading refuses. A genuine index takes
/// the same read's checkpoints. The fuzz input of #458, whose checkpoint 0 claims it, is
/// refused when loaded since #461 (`a` is 0 at the area's first sector).
#[test]
fn checkpoints_that_do_not_continue_a_forged_one_are_refused() {
    let (mut image, area) = harness::seek_area();
    let n = image.sector_count();
    let input = harness::with_checksum(include_bytes!(
        "regressions/seek_index/crash-3e61048c68db8489411a14adbf2156488778f96e"
    ));
    assert!(matches!(
        scarletbook::AreaIndex::from_bytes(&area, n, &input, usize::MAX),
        Err(scarletbook::Error::InvalidArgument("seek index checkpoint invalid"))
    ));
    let mut genuine =
        scarletbook::AreaIndex::new(&area, n, scarletbook::IndexOptions::new().with_interval(1)).unwrap();
    while genuine.checkpoint_count() < 2 {
        genuine.extend(&mut image, 0).unwrap();
    }
    assert_eq!(genuine.checkpoint_count(), 2);
    // Checkpoint 1's `a`, then the checksum again.
    let mut bytes = genuine.to_bytes();
    bytes[104..108].copy_from_slice(&13u32.to_le_bytes());
    let bytes = harness::with_checksum(&bytes[..bytes.len() - 8]);
    let mut forged = scarletbook::AreaIndex::from_bytes(&area, n, &bytes, usize::MAX).unwrap();
    let p = area.playtime_frames();
    let read = |image: &mut _, index: &scarletbook::AreaIndex| {
        let mut c = scarletbook::TrackCursor::open_range(
            &area,
            0,
            p,
            n,
            scarletbook::CursorOptions::new().with_index(index),
        )
        .unwrap();
        while let Some(f) = c.next_frame(image) {
            f.unwrap();
        }
        c
    };
    let mut cursor = read(&mut image, &forged);
    assert!(matches!(
        forged.absorb(&mut cursor),
        Err(scarletbook::Error::InvalidArgument("cursor's checkpoints differ from the seek index"))
    ));
    assert_eq!(forged.to_bytes(), bytes, "a refused absorb changed the index");
    // The same read through the genuine index.
    let mut cursor = read(&mut image, &genuine);
    genuine.absorb(&mut cursor).unwrap();
    assert!(genuine.is_complete());
    let saved = genuine.to_bytes();
    assert_eq!(scarletbook::AreaIndex::from_bytes(&area, n, &saved, usize::MAX).unwrap().to_bytes(), saved);
}

/// Indexes of the area of [`harness::seek_area`] saved by 1.3.1, from genuine scans: the
/// fuzz seeds (`area-seed-*`), and complete indexes at intervals 2 and 4 with damage
/// records from reads and `extend_past_marks` past sectors 11 to 13, 12 or 57 (`area-k*`).
/// The disc seeds (`disc-seed-*`) are of the disc of [`harness::seek_disc`].
const SAVED_BY_1_3_1: [(&str, &[u8]); 13] = [
    ("area-seed-0", include_bytes!("fixtures/seek_index_v1_3_1/area-seed-0.bin")),
    ("area-seed-1", include_bytes!("fixtures/seek_index_v1_3_1/area-seed-1.bin")),
    ("area-seed-2", include_bytes!("fixtures/seek_index_v1_3_1/area-seed-2.bin")),
    ("area-seed-3", include_bytes!("fixtures/seek_index_v1_3_1/area-seed-3.bin")),
    ("area-seed-4", include_bytes!("fixtures/seek_index_v1_3_1/area-seed-4.bin")),
    ("area-seed-5", include_bytes!("fixtures/seek_index_v1_3_1/area-seed-5.bin")),
    ("area-seed-6", include_bytes!("fixtures/seek_index_v1_3_1/area-seed-6.bin")),
    ("area-k4-mark12", include_bytes!("fixtures/seek_index_v1_3_1/area-k4-mark12.bin")),
    ("area-k4-mark57", include_bytes!("fixtures/seek_index_v1_3_1/area-k4-mark57.bin")),
    ("area-k2-mark11-13", include_bytes!("fixtures/seek_index_v1_3_1/area-k2-mark11-13.bin")),
    ("disc-seed-0", include_bytes!("fixtures/seek_index_v1_3_1/disc-seed-0.bin")),
    ("disc-seed-1", include_bytes!("fixtures/seek_index_v1_3_1/disc-seed-1.bin")),
    ("disc-seed-2", include_bytes!("fixtures/seek_index_v1_3_1/disc-seed-2.bin")),
];

/// Indexes 1.3.1 saved from genuine scans load and save unchanged, with the checks loading
/// added in 1.3.2 (#461: each checkpoint counts the malformed sectors kept below it, `a` is
/// 0 at the area's first sector). Small enough for Miri.
#[test]
fn indexes_of_1_3_1_load_and_save_unchanged() {
    let (image, area) = harness::seek_area();
    let disc = harness::seek_disc();
    for (name, bytes) in SAVED_BY_1_3_1 {
        let (area, n) =
            if name.starts_with("disc") { (&disc.area, disc.sectors) } else { (&area, image.sector_count()) };
        let index = scarletbook::AreaIndex::from_bytes(area, n, bytes, usize::MAX)
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(index.to_bytes(), bytes, "{name}");
    }
}

/// The offset of the damage record's header in `body` (saved bytes without the checksum),
/// found from the end, if it has one.
fn damage_header(body: &[u8]) -> Option<usize> {
    let u = |o: usize| u32::from_le_bytes(body[o..o + 4].try_into().unwrap()) as usize;
    (80..body.len().checked_sub(16)?).rev().find(|&o| {
        let (r, d, m) = (u(o), u(o + 4), u(o + 8));
        u(o + 12) == 0
            && (1..64).contains(&r)
            && (1..=64).contains(&d)
            && m < 64
            && o + 16 + 8 * r + 40 * d + 4 * m == body.len()
    })
}

/// The fixture `seed` of [`SAVED_BY_1_3_1`] without its checksum, with `edits`: each
/// writes `value` (or adds it, or writes it to that field of every checkpoint up to the
/// one chosen) into a field of the index's checkpoints (`a`, `c - r`, malformed count, or
/// all three), of the damage record's (`a`, `c - r`, malformed count, lost stretch), of a
/// stored frame's (length, frame number), or anywhere.
fn mutated(seed: usize, edits: &[(u8, usize, u8, u8, u64)]) -> Vec<u8> {
    let (_, bytes) = SAVED_BY_1_3_1[seed];
    let mut body = bytes[..bytes.len() - 8].to_vec();
    let u = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize;
    for &(target, j, field, op, value) in edits {
        // (offset of checkpoint j's field, width), for every checkpoint the edit touches.
        let fields: Vec<(usize, usize)> = match target % 4 {
            0 => {
                let n = u(&body, 36).max(1);
                // Field 3: all three.
                let offs: &[(usize, usize)] = match field % 4 {
                    3 => &[(0, 4), (4, 4), (8, 8)],
                    f => &[[(0, 4), (4, 4), (8, 8)][usize::from(f)]],
                };
                let js = if op % 3 == 2 { 0..=j % n } else { j % n..=j % n };
                js.flat_map(|j| offs.iter().map(move |&(off, w)| (80 + 24 * j + off, w)))
                    .filter(|&(o, w)| o + w <= body.len())
                    .collect()
            }
            1 => {
                let Some(at) = damage_header(&body) else { continue };
                let (r, d) = (u(&body, at), u(&body, at + 4));
                let (off, w) = [(8, 4), (12, 4), (16, 8), (32, 4)][usize::from(field) % 4];
                let js = if op % 3 == 2 { 0..=j % d } else { j % d..=j % d };
                js.map(|j| (at + 16 + 8 * r + 40 * j + off, w)).collect()
            }
            2 => {
                // A stored frame under assembly's checkpoint: its length or frame number.
                let get =
                    |o: usize| body.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize);
                let (count, f, h) = (u(&body, 36), u(&body, 40), u(&body, 44).max(1));
                let mut at = Some(80usize.saturating_add(count.saturating_mul(24)));
                for _ in 0..f.min(64) {
                    at = at.and_then(|a| Some(a.saturating_add(8).saturating_add(get(a.checked_add(4)?)?)));
                }
                let Some(o) = at.map(|a| a.saturating_add(24 * (j % h) + [8, 12][usize::from(field) % 2]))
                else {
                    continue;
                };
                if o.saturating_add(4) > body.len() {
                    continue;
                }
                vec![(o, 4)]
            }
            _ => {
                let w = [1, 4, 8][usize::from(field) % 3];
                vec![(j % body.len().saturating_sub(w - 1).max(1), w.min(body.len()))]
            }
        };
        for (o, w) in fields {
            let mut old = [0u8; 8];
            old[..w].copy_from_slice(&body[o..o + w]);
            let new = if op % 3 == 1 { u64::from_le_bytes(old).wrapping_add(value) } else { value };
            body[o..o + w].copy_from_slice(&new.to_le_bytes()[..w]);
        }
    }
    body
}

/// `index` saves to bytes that load to an index saving the same bytes.
fn saves(index: &scarletbook::AreaIndex, area: &scarletbook::Area, n: u64, what: &str) {
    let saved = index.to_bytes();
    let loaded = scarletbook::AreaIndex::from_bytes(area, n, &saved, usize::MAX);
    assert!(loaded.is_ok_and(|l| l.to_bytes() == saved), "{what}: saves to bytes that do not load");
}

/// The oracle of [`mutated_indexes_are_refused_or_stay_saveable`] for one input: `bytes`
/// (with their checksum) of an index of the area of [`harness::seek_area`] are refused, or
/// load to an index that saves them unchanged, and that stays saveable after reads of
/// `[first, p)` for each of `firsts` absorbed into it, `extend_past_marks`, `extend`, and
/// merges with a genuine index both ways, the image's sectors `marks` (LSN, count) marked
/// unreadable.
fn stays_saveable(bytes: &[u8], marks: &[(u64, u64)], firsts: &[u32], eager: bool) {
    use scarletbook::{AreaIndex, CursorOptions, IndexOptions, TrackCursor};
    let (mut image, area) = harness::seek_area();
    let (n, p) = (image.sector_count(), area.playtime_frames());
    let Ok(loaded) = AreaIndex::from_bytes(&area, n, bytes, usize::MAX) else { return };
    assert_eq!(loaded.to_bytes(), bytes, "saving a loaded index changes it");
    let mut genuine =
        AreaIndex::new(&area, n, IndexOptions::new().with_interval(loaded.options().interval)).unwrap();
    while !genuine.extend(&mut image, 1000).unwrap() {}
    for &(lsn, count) in marks {
        image.unreadable_sectors().insert(lsn, count);
    }
    let mut index = loaded.clone();
    if eager {
        for _ in 0..64 {
            if index.extend_past_marks(&mut image, 8).unwrap_or(true) {
                break;
            }
        }
        saves(&index, &area, n, "built past the marks");
    }
    for &first in firsts {
        let first = first % p;
        let mut c =
            TrackCursor::open_range(&area, first, p, n, CursorOptions::new().with_index(&loaded)).unwrap();
        while let Some(Ok(_)) = c.next_frame(&mut image) {}
        let _ = index.absorb(&mut c);
        saves(&index, &area, n, "after a read");
    }
    let _ = index.extend(&mut image, 1000);
    saves(&index, &area, n, "extended");
    let mut merged = index.clone();
    let _ = merged.merge(&genuine);
    saves(&merged, &area, n, "merged with a genuine index");
    let _ = genuine.merge(&index);
    saves(&genuine, &area, n, "a genuine index merged with it");
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig {
        cases: std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(512),
        failure_persistence: None,
        ..proptest::prelude::ProptestConfig::default()
    })]

    /// Mutations of saved genuine indexes, with their checksum, through the oracle of the
    /// `seek_index` fuzz target (#461): each is refused when loaded, or saves to the same
    /// bytes and stays saveable after reads, `absorb`, `extend_past_marks`, `extend` and
    /// `merge` with real scans, with sectors marked unreadable.
    #[test]
    #[cfg_attr(miri, ignore = "slow under Miri; indexes_of_1_3_1_load_and_save_unchanged and the #458 and #461 tests cover loading and joins under Miri")]
    fn mutated_indexes_are_refused_or_stay_saveable(
        seed in 0usize..10,
        marks in proptest::collection::vec((0u64..64, 1u64..4), 0..3),
        firsts in proptest::collection::vec(0u32..20, 1..4),
        eager: bool,
        edits in proptest::collection::vec(
            (
                0u8..4,
                0usize..4096,
                0u8..4,
                0u8..3,
                proptest::prop_oneof![0u64..3, proptest::strategy::Strategy::prop_map(proptest::num::u64::ANY, |v| v % 64), proptest::num::u64::ANY],
            ),
            1..4,
        ),
    ) {
        let body = mutated(seed, &edits);
        stays_saveable(&harness::with_checksum(&body), &marks, &firsts, eager);
    }
}
