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
    for seed in harness::seek_index_seeds() {
        let mut bytes = seed.clone();
        let sum = seed
            .iter()
            .fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
        bytes.extend_from_slice(&sum.to_le_bytes());
        scarletbook::AreaIndex::from_bytes(&area, image.sector_count(), &bytes).unwrap();
        harness::seek_index(&seed);
    }
}
