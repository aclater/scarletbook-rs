// SPDX-License-Identifier: MIT OR Apache-2.0

//! A filesystem's call pattern (issue #216): a stereo area converted to 176.4 kHz `Default`
//! WAV samples (`PcmOut::S24le`), in calls of `N` frames each, every call with its frames in
//! separate buffers (`Frames::Separate`, as a frame cache holds them), both neighbours and
//! per-frame clip counts. Prints the CPU time per second of audio for each call size, the
//! median of the timed passes, single-threaded.
//!
//! `cargo run --release -p scarletbook-pcm --example callpattern [SECONDS [PASSES [SIZES]]]`
//! (defaults 60 s, 7 passes, sizes `1,2,4,8,16,32,64,128,256`).

use std::time::Instant;

use scarletbook_pcm::{Converter, Cutoff, FRAME_BYTES_PER_CHANNEL, Frames, PcmConfig, PcmOut, PcmRate, Run};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let seconds: usize = args.next().map(|s| s.parse()).transpose()?.unwrap_or(60);
    let passes: usize = args.next().map(|s| s.parse()).transpose()?.unwrap_or(7);
    let sizes: Vec<usize> = match args.next() {
        Some(s) => s.split(',').map(str::parse).collect::<Result<_, _>>()?,
        None => vec![1, 2, 4, 8, 16, 32, 64, 128, 256],
    };
    let ch = 2;
    let total = 75 * seconds;
    let frame = ch * FRAME_BYTES_PER_CHANNEL;
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let area: Vec<Vec<u8>> = (0..total)
        .map(|_| {
            (0..frame)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    state as u8
                })
                .collect()
        })
        .collect();
    let frames: Vec<&[u8]> = area.iter().map(Vec::as_slice).collect();
    let mut conv = Converter::new(PcmConfig::new(PcmRate::Hz176400, Cutoff::Standard, 2))?;
    let bytes_per_frame = 3 * conv.samples_per_frame() * ch;
    let mut wav = vec![0u8; total * bytes_per_frame];
    let mut clipped = vec![0u64; total];
    println!("| Frames per call | ms CPU per s of audio | x real time |");
    println!("|---|---|---|");
    for &n in &sizes {
        let mut times = Vec::new();
        // One warm-up pass, then the timed ones.
        for pass in 0..=passes {
            let t = Instant::now();
            let mut f = 0;
            while f < total {
                let k = n.min(total - f);
                let run = Run::new(Frames::Separate(&frames[f..f + k]))
                    .with_before(f.checked_sub(1).map(|p| frames[p]))
                    .with_after(frames.get(f + k).copied());
                conv.convert(
                    run,
                    PcmOut::S24le(&mut wav[f * bytes_per_frame..(f + k) * bytes_per_frame]),
                    Some(&mut clipped[f..f + k]),
                )?;
                f += k;
            }
            if pass > 0 {
                times.push(t.elapsed().as_secs_f64());
            }
        }
        times.sort_by(f64::total_cmp);
        let ms = times[times.len() / 2] * 1000.0 / seconds as f64;
        println!("| {n} | {ms:.2} | {:.0} |", 1000.0 / ms);
    }
    // A checksum of the output, so that runs can be compared for identity.
    let sum =
        wav.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3));
    println!("\noutput FNV-1a: {sum:016x}");
    Ok(())
}
