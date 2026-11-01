// SPDX-License-Identifier: MIT OR Apache-2.0
//! Scratch profiler for #173 (not for commit): prof173 RATE CUTOFF CH SECONDS RUN PASSES
use std::time::Instant;
use scarletbook_pcm::{Converter, Cutoff, FRAME_BYTES_PER_CHANNEL, FrameRun, PcmConfig, PcmRate};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let rate = PcmRate::from_hz(a.get(1).map_or(176400, |s| s.parse().unwrap())).unwrap();
    let cutoff = match a.get(2).map_or("Default", |s| s.as_str()) { "Low" => Cutoff::Low, "Wide" => Cutoff::Wide, _ => Cutoff::Default };
    let ch: usize = a.get(3).map_or(2, |s| s.parse().unwrap());
    let seconds: usize = a.get(4).map_or(10, |s| s.parse().unwrap());
    let run: usize = a.get(5).map_or(1, |s| s.parse().unwrap());
    let passes: usize = a.get(6).map_or(4, |s| s.parse().unwrap());
    let frames = 75 * seconds;
    let mut st = 0x9E37_79B9_7F4A_7C15u64;
    let input: Vec<u8> = (0..frames * ch * FRAME_BYTES_PER_CHANNEL).map(|_| { st ^= st << 13; st ^= st >> 7; st ^= st << 17; st as u8 }).collect();
    let mut conv = Converter::new(PcmConfig::new(rate, cutoff, ch as u8)).unwrap();
    let frame = ch * FRAME_BYTES_PER_CHANNEL;
    let spf = conv.samples_per_frame() * ch;
    let mut out = vec![0i32; frames * spf];
    let mut best = f64::INFINITY;
    for pass in 0..passes {
        let t = Instant::now();
        let mut f = 0;
        while f < frames {
            let n = run.min(frames - f);
            let before = f.checked_sub(1).map(|p| &input[p * frame..(p + 1) * frame]);
            let after = (f + n < frames).then(|| &input[(f + n) * frame..(f + n + 1) * frame]);
            let r = FrameRun::new(&input[f * frame..(f + n) * frame]).with_before(before).with_after(after);
            conv.convert_run(r, &mut out[f * spf..(f + n) * spf]).unwrap();
            f += n;
        }
        if pass > 0 || passes == 1 { best = best.min(t.elapsed().as_secs_f64()); }
    }
    let ms = best * 1000.0 / seconds as f64;
    let mut h = 0u64; for &v in &out { h = h.wrapping_mul(0x100000001b3).wrapping_add(v as u32 as u64); }
    println!("{} {:?} {}ch run{}: {:.3} ms/s  {:.0}x  hash {:016x}", rate.hz(), cutoff, ch, run, ms, 1000.0 / ms, h);
}
