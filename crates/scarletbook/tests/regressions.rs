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

/// Small enough for Miri (#458): a loaded index whose only checkpoint, at the area's first
/// sector, claims `a = 13`. A read from frame 0 starts before it, so it never checks it,
/// and its scan's checkpoints have a smaller `a`: absorbing them is refused, with the index
/// unchanged, instead of leaving an index that saves to bytes loading refuses. A genuine
/// index takes the same read's checkpoints.
#[test]
fn checkpoints_that_do_not_continue_a_forged_one_are_refused() {
    let (mut image, area) = harness::seek_area();
    let n = image.sector_count();
    let bytes = harness::with_checksum(include_bytes!(
        "regressions/seek_index/crash-3e61048c68db8489411a14adbf2156488778f96e"
    ));
    let mut forged = scarletbook::AreaIndex::from_bytes(&area, n, &bytes, usize::MAX).unwrap();
    assert_eq!(forged.checkpoint_count(), 1);
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
    // The same read through a genuine index of one checkpoint.
    let mut genuine =
        scarletbook::AreaIndex::new(&area, n, scarletbook::IndexOptions::new().with_interval(1)).unwrap();
    genuine.extend(&mut image, 0).unwrap();
    assert_eq!(genuine.checkpoint_count(), 1);
    let mut cursor = read(&mut image, &genuine);
    genuine.absorb(&mut cursor).unwrap();
    assert!(genuine.is_complete());
    let saved = genuine.to_bytes();
    assert_eq!(scarletbook::AreaIndex::from_bytes(&area, n, &saved, usize::MAX).unwrap().to_bytes(), saved);
}
