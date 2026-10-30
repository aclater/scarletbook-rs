// SPDX-License-Identifier: MIT OR Apache-2.0

//! Single-frame `DstDecoder::decode` throughput (issue #30).
//!
//! Two sets of frames, each benchmarked per channel count:
//!
//! - `corpus/Nch`: every distinct coded DST frame of the conformance corpus images
//!   `corpus/images/dst-*.iso` that decodes, demultiplexed with `Disc::area_frames`;
//! - `stress/Nch-LAYOUT`: frames generated at start-up by the annex 5.A encoder of
//!   `tests/common/dst_enc.rs` from a seeded sigma-delta signal, with order-128 filters
//!   (`o128`), order-32 filters (`o32`) or several filter and ptable segments
//!   (`segmented`). They are identical on every run.
//!
//! One iteration decodes one frame (cycling through the set), so criterion's time is the
//! time per frame; the throughput is in symbols (37 632 x channels per frame).
//!
//! `cargo bench -p scarletbook --bench dst_frames -- stress/2ch` filters as usual. For
//! `perf` (see `perf/`), setting `SCARLETBOOK_PERF_BENCH=GROUP/ID` (or `list`) skips
//! criterion: the program decodes `SCARLETBOOK_PERF_FRAMES` frames (default 2000) of that
//! benchmark and prints the number of symbols decoded.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../tests/common/dst_enc.rs"]
mod dst_enc;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::hint::black_box;
use std::io::BufReader;
use std::path::Path;

use criterion::{Criterion, Throughput};
use scarletbook::dst::{FL, NB};
use scarletbook::{Disc, DstDecoder, FrameKind, Slot};

/// Frames of each stress layout (consecutive frames of one signal).
const STRESS_FRAMES: usize = 8;

/// One benchmark: frames of one channel count.
struct Set {
    /// `corpus/2ch`, `stress/6ch-o128`, ...
    name: String,
    channels: u8,
    frames: Vec<Vec<u8>>,
}

impl Set {
    fn symbols_per_frame(&self) -> u64 {
        (NB * usize::from(self.channels)) as u64
    }
}

/// The coded frames of the corpus images `dst-*.iso` that decode, by channel count.
fn corpus_sets() -> Vec<Set> {
    let images = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/images");
    let mut paths: Vec<_> = std::fs::read_dir(&images)
        .expect("corpus/images")
        .map(|e| e.unwrap().path())
        .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("dst-"))
        .collect();
    paths.sort();
    let mut by_channels: BTreeMap<u8, BTreeSet<Vec<u8>>> = BTreeMap::new();
    let mut dec = DstDecoder::new();
    for path in paths {
        let Ok(mut disc) = Disc::open(BufReader::new(File::open(&path).unwrap())) else { continue };
        for slot in [Slot::TwoChannel, Slot::Multichannel] {
            let Some(channels) = disc.toc().area(slot).map(|a| a.channel_count) else { continue };
            let Ok(frames) = disc.area_frames(slot) else { continue };
            let mut out = vec![0u8; FL * usize::from(channels)];
            for f in frames.flatten() {
                let coded = f.data.first().is_some_and(|b| b & 0x80 != 0);
                if f.kind == FrameKind::Dst && coded && dec.decode(&f.data, channels, &mut out).is_ok() {
                    by_channels.entry(channels).or_default().insert(f.data);
                }
            }
        }
    }
    by_channels
        .into_iter()
        .map(|(channels, frames)| Set {
            name: format!("corpus/{channels}ch"),
            channels,
            frames: frames.into_iter().collect(),
        })
        .collect()
}

/// The stress set, checked against its signal.
fn stress_sets() -> Vec<Set> {
    let mut sets: Vec<Set> = Vec::new();
    let mut dec = DstDecoder::new();
    for f in dst_enc::stress_set(STRESS_FRAMES) {
        let mut out = vec![0u8; FL * usize::from(f.channels)];
        dec.decode(&f.frame, f.channels, &mut out).unwrap();
        assert!(out == f.audio, "stress frame does not round-trip");
        let name = format!("stress/{}ch-{}", f.channels, f.layout);
        match sets.iter_mut().find(|s| s.name == name) {
            Some(s) => s.frames.push(f.frame),
            None => sets.push(Set { name, channels: f.channels, frames: vec![f.frame] }),
        }
    }
    sets
}

/// Decodes `count` frames of `set`, cycling; returns the number of symbols.
fn run(set: &Set, count: usize) -> u64 {
    let mut dec = DstDecoder::new();
    let mut out = vec![0u8; FL * usize::from(set.channels)];
    for i in 0..count {
        dec.decode(black_box(&set.frames[i % set.frames.len()]), set.channels, &mut out).unwrap();
        black_box(&out);
    }
    count as u64 * set.symbols_per_frame()
}

fn main() {
    let sets: Vec<Set> = corpus_sets().into_iter().chain(stress_sets()).collect();

    if let Ok(name) = std::env::var("SCARLETBOOK_PERF_BENCH") {
        if name == "list" {
            for s in &sets {
                let bytes: usize = s.frames.iter().map(Vec::len).sum();
                println!("{}\t{} frames\t{} bytes per frame", s.name, s.frames.len(), bytes / s.frames.len());
            }
            return;
        }
        let set = sets.iter().find(|s| s.name == name).unwrap_or_else(|| panic!("no benchmark {name}"));
        let count = std::env::var("SCARLETBOOK_PERF_FRAMES").map_or(2000, |v| v.parse().expect("a number"));
        println!("{}", run(set, count));
        return;
    }

    let mut c = Criterion::default().configure_from_args();
    for (group, prefix) in [("corpus", "corpus/"), ("stress", "stress/")] {
        let mut g = c.benchmark_group(group);
        for set in sets.iter().filter(|s| s.name.starts_with(prefix)) {
            g.throughput(Throughput::Elements(set.symbols_per_frame()));
            let mut dec = DstDecoder::new();
            let mut out = vec![0u8; FL * usize::from(set.channels)];
            let mut i = 0;
            g.bench_function(&set.name[prefix.len()..], |b| {
                b.iter(|| {
                    dec.decode(black_box(&set.frames[i]), set.channels, &mut out).unwrap();
                    i = (i + 1) % set.frames.len();
                })
            });
        }
        g.finish();
    }
    c.final_summary();
}
