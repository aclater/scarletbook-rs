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
