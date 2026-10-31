// SPDX-License-Identifier: MIT OR Apache-2.0

//! Conversion throughput (issue #130): one iteration converts one second of audio (75
//! frames), frame by frame with neighbours as a filesystem would, so criterion's time per
//! iteration is the CPU time per second of audio. One benchmark per rate, cutoff and 1, 2
//! and 6 channels: `cargo bench -p scarletbook-pcm --bench convert -- 176400/Default/2ch`.
//! `examples/throughput.rs` prints the same as the README's table.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::hint::black_box;

use criterion::Criterion;
use scarletbook_pcm::{Converter, Cutoff, FRAME_BYTES_PER_CHANNEL, FrameRun, PcmConfig, PcmRate};

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
                            let run = FrameRun { before, frames: &input[f * frame..(f + 1) * frame], after };
                            conv.convert_run(run, &mut out[f * spf..(f + 1) * spf]).unwrap();
                        }
                        black_box(&out);
                    })
                });
            }
        }
    }
    c.final_summary();
}
