// SPDX-License-Identifier: MIT OR Apache-2.0

//! The whole read path, `Disc::open` then `TrackAudio` over every track of an area
//! (issue #30), on small synthetic images held in memory (`synth::write_image`): sector
//! reading, demultiplexing, frame reassembly and, for DST, decoding on one thread.
//!
//! Each iteration reads one area of `FRAMES` frames; the throughput is in symbols
//! (37 632 x channels per frame), as in `dst_frames`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::hint::black_box;
use std::io::Cursor;

use criterion::{Criterion, Throughput};
use scarletbook::dst::NB;
use scarletbook::{Disc, SectorFormat, Slot};
use scarletbook_conformance::corpus_dir;
use scarletbook_conformance::synth::{Config, DstSource, Kind, write_image};

/// Frames per area (4 s).
const FRAMES: u32 = 300;

fn image(kind: Kind, format: SectorFormat, src: &DstSource) -> Vec<u8> {
    let cfg = Config {
        frames: FRAMES,
        two_channel: true,
        multichannel: true,
        mc_channels: 6,
        kind,
        format,
        track_frames: 75,
    };
    let mut out = Cursor::new(Vec::new());
    write_image(&mut out, &cfg, src).unwrap();
    out.into_inner()
}

/// Reads every track of the area in `slot`; returns the frames read.
fn read_area(img: &[u8], slot: Slot) -> u32 {
    let mut disc = Disc::open(Cursor::new(img)).unwrap();
    let tracks = disc.toc().area(slot).unwrap().tracks.len() as u32;
    let mut frames = 0;
    for t in 1..=tracks {
        for f in disc.track_audio(slot, t).unwrap() {
            black_box(f.unwrap().data);
            frames += 1;
        }
    }
    frames
}

fn main() {
    let src = DstSource::from_corpus(&corpus_dir()).unwrap();
    let images = [
        ("dsd", image(Kind::Dsd, SectorFormat::Plain, &src)),
        ("dst", image(Kind::Dst, SectorFormat::Plain, &src)),
        ("dst-2064", image(Kind::Dst, SectorFormat::Raw2064, &src)),
    ];
    let mut c = Criterion::default().configure_from_args();
    let mut g = c.benchmark_group("pipeline");
    g.sample_size(20);
    for (name, img) in &images {
        for (slot, area, channels) in [(Slot::TwoChannel, "2ch", 2u64), (Slot::Multichannel, "6ch", 6)] {
            assert_eq!(read_area(img, slot), FRAMES, "{name} {area}");
            g.throughput(Throughput::Elements(u64::from(FRAMES) * NB as u64 * channels));
            g.bench_function(format!("{name}/{area}"), |b| b.iter(|| read_area(black_box(img), slot)));
        }
    }
    g.finish();
    c.final_summary();
}
