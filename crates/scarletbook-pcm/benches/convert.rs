// SPDX-License-Identifier: MIT OR Apache-2.0

//! Conversion throughput (issue #130): one iteration converts one second of audio (75
//! frames), frame by frame with neighbours as a filesystem would, so criterion's time per
//! iteration is the CPU time per second of audio. One benchmark per rate, cutoff and 1, 2
//! and 6 channels: `cargo bench -p scarletbook-pcm --bench convert -- 176400/Standard/2ch`.
//! `examples/throughput.rs` prints the same as the README's table.
//!
//! `run4/...` (issue #172): a stereo run of four frames at 176.4 kHz `Standard`, with both
//! neighbours, to 3-byte little-endian samples: `convert_run` + `pack_s24le` against
//! `convert` from one buffer and from four separate frames (with per-frame clip counts).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::hint::black_box;

use criterion::Criterion;
use scarletbook_pcm::{
    Converter, Cutoff, FRAME_BYTES_PER_CHANNEL, FrameRun, Frames, PcmConfig, PcmOut, PcmRate, Run, pack_s24le,
};

fn main() {
    let mut c = Criterion::default().sample_size(10).configure_from_args();
    let frames = 75;
    for rate in PcmRate::ALL {
        for cutoff in Cutoff::ALL.into_iter().filter(|c| c.supports(rate)) {
            for channels in [1u8, 2, 6] {
                let ch = usize::from(channels);
                let mut s = 0x2545_F491_4F6C_DD1Du64;
                let input: Vec<u8> = (0..frames * ch * FRAME_BYTES_PER_CHANNEL)
                    .map(|_| {
                        s ^= s << 13;
                        s ^= s >> 7;
                        s ^= s << 17;
                        s as u8
                    })
                    .collect();
                let mut conv = Converter::new(PcmConfig::new(rate, cutoff, channels)).unwrap();
                let frame = ch * FRAME_BYTES_PER_CHANNEL;
                let spf = conv.samples_per_frame() * ch;
                let mut out = vec![0i32; frames * spf];
                c.bench_function(&format!("{}/{cutoff:?}/{channels}ch", rate.hz()), |b| {
                    b.iter(|| {
                        for f in 0..frames {
                            let before = f.checked_sub(1).map(|p| &input[p * frame..(p + 1) * frame]);
                            let after = (f + 1 < frames).then(|| &input[(f + 1) * frame..(f + 2) * frame]);
                            let run = FrameRun::new(&input[f * frame..(f + 1) * frame])
                                .with_before(before)
                                .with_after(after);
                            conv.convert_run(run, &mut out[f * spf..(f + 1) * spf]).unwrap();
                        }
                        black_box(&out);
                    })
                });
            }
        }
    }
    run4(&mut c);
    c.final_summary();
}

/// A run of four frames (frames 1 to 4 of 6), stereo, 176.4 kHz `Standard`.
fn run4(c: &mut Criterion) {
    let ch = 2;
    let frame = ch * FRAME_BYTES_PER_CHANNEL;
    let mut s = 0x2545_F491_4F6C_DD1Du64;
    // Separate buffers, as a frame cache holds them.
    let area: Vec<Vec<u8>> = (0..6)
        .map(|_| {
            (0..frame)
                .map(|_| {
                    s ^= s << 13;
                    s ^= s >> 7;
                    s ^= s << 17;
                    s as u8
                })
                .collect()
        })
        .collect();
    let contiguous = area[1..5].concat();
    let separate: Vec<&[u8]> = area[1..5].iter().map(Vec::as_slice).collect();
    let (before, after) = (Some(&area[0][..]), Some(&area[5][..]));
    let mut conv = Converter::new(PcmConfig::new(PcmRate::Hz176400, Cutoff::Standard, 2)).unwrap();
    let samples = 4 * conv.samples_per_frame() * ch;
    let mut pcm = vec![0i32; samples];
    let mut wav = vec![0u8; 3 * samples];
    let mut clipped = [0u64; 4];
    c.bench_function("run4/176400/Standard/2ch/copy+convert_run+pack_s24le", |b| {
        b.iter(|| {
            // Today's path for separate frames: copy into one buffer, convert, pack.
            let mut joined = Vec::with_capacity(4 * frame);
            for f in &separate {
                joined.extend_from_slice(f);
            }
            let run = FrameRun::new(&joined).with_before(before).with_after(after);
            conv.convert_run(run, &mut pcm).unwrap();
            pack_s24le(&pcm, &mut wav);
            black_box(&wav);
        })
    });
    c.bench_function("run4/176400/Standard/2ch/convert_run+pack_s24le", |b| {
        b.iter(|| {
            let run = FrameRun::new(&contiguous).with_before(before).with_after(after);
            conv.convert_run(run, &mut pcm).unwrap();
            pack_s24le(&pcm, &mut wav);
            black_box(&wav);
        })
    });
    c.bench_function("run4/176400/Standard/2ch/convert-contiguous-s24le", |b| {
        b.iter(|| {
            let run = Run::new(&contiguous[..]).with_before(before).with_after(after);
            conv.convert(run, PcmOut::S24le(&mut wav), None).unwrap();
            black_box(&wav);
        })
    });
    c.bench_function("run4/176400/Standard/2ch/convert-separate-s24le-counts", |b| {
        b.iter(|| {
            let run = Run::new(Frames::Separate(&separate)).with_before(before).with_after(after);
            conv.convert(run, PcmOut::S24le(&mut wav), Some(&mut clipped)).unwrap();
            black_box((&wav, &clipped));
        })
    });
}
