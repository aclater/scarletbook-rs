// SPDX-License-Identifier: MIT OR Apache-2.0

//! Throughput table for the README: CPU time per second of audio and the real-time factor,
//! single-threaded, for every rate, cutoff and 1, 2 and 6 channels.
//!
//! `cargo run --release -p scarletbook-pcm --example throughput [SECONDS]` (default 20).
//! The input is pseudo-random bytes, which cost the same as music: the work per sample does
//! not depend on the data.

use std::time::Instant;

use scarletbook_pcm::{Converter, Cutoff, FRAME_BYTES_PER_CHANNEL, FrameRun, PcmConfig, PcmRate};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let seconds: usize = std::env::args().nth(1).map(|s| s.parse()).transpose()?.unwrap_or(20);
    let frames = 75 * seconds;
    println!("| Rate | Cutoff | Channels | ms CPU per s of audio | x real time |");
    println!("|---|---|---|---|---|");
    for rate in PcmRate::ALL {
        for cutoff in Cutoff::ALL {
            if !cutoff.supports(rate) {
                continue;
            }
            for channels in [1u8, 2, 6] {
                let ch = usize::from(channels);
                let mut state = 0x9E37_79B9_7F4A_7C15u64;
                let input: Vec<u8> = (0..frames * ch * FRAME_BYTES_PER_CHANNEL)
                    .map(|_| {
                        state ^= state << 13;
                        state ^= state >> 7;
                        state ^= state << 17;
                        state as u8
                    })
                    .collect();
                let mut conv = Converter::new(PcmConfig::new(rate, cutoff, channels))?;
                let mut out = vec![0i32; frames * conv.samples_per_frame() * ch];
                // Per-frame calls with neighbours, as a filesystem would make them, after one
                // warm-up pass; the best of three passes.
                let frame = ch * FRAME_BYTES_PER_CHANNEL;
                let spf = conv.samples_per_frame() * ch;
                let mut best = f64::INFINITY;
                for pass in 0..4 {
                    let t = Instant::now();
                    for f in 0..frames {
                        let before = f.checked_sub(1).map(|p| &input[p * frame..(p + 1) * frame]);
                        let after = (f + 1 < frames).then(|| &input[(f + 1) * frame..(f + 2) * frame]);
                        let run = FrameRun { before, frames: &input[f * frame..(f + 1) * frame], after };
                        conv.convert_run(run, &mut out[f * spf..(f + 1) * spf])?;
                    }
                    if pass > 0 {
                        best = best.min(t.elapsed().as_secs_f64());
                    }
                }
                let ms = best * 1000.0 / seconds as f64;
                println!("| {} | {cutoff:?} | {channels} | {ms:.2} | {:.0} |", rate.hz(), 1000.0 / ms);
            }
        }
    }
    Ok(())
}
