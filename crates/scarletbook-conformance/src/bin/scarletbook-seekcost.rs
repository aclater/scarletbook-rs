// SPDX-License-Identifier: MIT OR Apache-2.0

//! Measures what opening a frame range costs against where it starts, without and with a
//! seek index (issue #68), and as a cold seek by provisional reading (issue #94):
//!
//! ```text
//! scarletbook-seekcost IMAGE [--slot 2ch|mc] [--interval K] [--frames N] [--repeat R]
//!     [--rtt-ms MS] [--mbps MB]
//! ```
//!
//! For ranges of `N` frames (default 14) starting at 0 %, 16 %, 42 %, 72 % and 99 % of the
//! area's play time, it decodes the range from a fresh cursor and from a complete seek
//! index of interval `K` (default 64), and prints the best time of `R` runs (default 3) and
//! the sectors read (counted at the reader), and the index's build time, size in memory
//! and saved size. The decoded frames are compared: they must be equal.
//!
//! A second table measures cold seeks: a disc without a seek index and with
//! `SeekPolicy::Speculative` (default options) decodes the same ranges by provisional
//! reading (specification 4, 4.2 item 8), with its read calls and sectors, a network model
//! (`calls x MS + bytes / MB per second`, default 0.5 ms and 110 MB/s) and whether its
//! frames equal the exact ones.

use std::cell::Cell;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::process::ExitCode;
use std::rc::Rc;
use std::time::{Duration, Instant};

use scarletbook::{
    AreaIndex, AudioFrame, Disc, IndexOptions, ResumedFrom, SeekPolicy, Slot, SpeculativeOptions,
    TrackAudioCursor,
};

const USAGE: &str = "usage: scarletbook-seekcost IMAGE [--slot 2ch|mc] [--interval K] [--frames N] [--repeat R] \
                     [--rtt-ms MS] [--mbps MB]";

/// A reader that counts the bytes read through it, and its read calls.
struct Counting<R> {
    inner: R,
    read: Rc<Cell<u64>>,
    calls: Rc<Cell<u64>>,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read.set(self.read.get() + n as u64);
        self.calls.set(self.calls.get() + 1);
        Ok(n)
    }
}

impl<R: Seek> Seek for Counting<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

type Image = Disc<Counting<BufReader<File>>>;

struct Args {
    image: String,
    slot: Slot,
    interval: u32,
    frames: u32,
    repeat: u32,
    rtt_ms: f64,
    mbps: f64,
}

fn parse() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let image = it.next().ok_or(USAGE)?;
    let mut args =
        Args { image, slot: Slot::TwoChannel, interval: 64, frames: 14, repeat: 3, rtt_ms: 0.5, mbps: 110.0 };
    while let Some(flag) = it.next() {
        let value = it.next().ok_or_else(|| format!("{flag} needs a value\n{USAGE}"))?;
        let number = |v: &str| v.parse::<u32>().map_err(|_| format!("{flag} must be a number, not {v}"));
        match flag.as_str() {
            "--slot" => {
                args.slot = match value.as_str() {
                    "2ch" => Slot::TwoChannel,
                    "mc" => Slot::Multichannel,
                    v => return Err(format!("--slot must be 2ch or mc, not {v}")),
                }
            }
            "--interval" => args.interval = number(&value)?,
            "--frames" => args.frames = number(&value)?.max(1),
            "--repeat" => args.repeat = number(&value)?.max(1),
            "--rtt-ms" | "--mbps" => {
                let v: f64 = value.parse().map_err(|_| format!("{flag} must be a number, not {value}"))?;
                if !(v.is_finite() && v > 0.0) {
                    return Err(format!("{flag} must be positive"));
                }
                if flag == "--rtt-ms" { args.rtt_ms = v } else { args.mbps = v }
            }
            _ => return Err(format!("unknown option {flag}\n{USAGE}")),
        }
    }
    Ok(args)
}

fn decode(disc: &mut Image, cursor: TrackAudioCursor) -> Result<Vec<AudioFrame>, String> {
    disc.read_track_audio(cursor).map(|f| f.map_err(|e| e.to_string())).collect()
}

/// The best time of `repeat` runs of `open` then decoding, the sectors the last run read,
/// and its frames.
fn measure(
    disc: &mut Image,
    read: &Cell<u64>,
    repeat: u32,
    open: &dyn Fn(&Image) -> Result<TrackAudioCursor, String>,
) -> Result<(Duration, u64, Vec<AudioFrame>), String> {
    let mut best = Duration::MAX;
    let mut sectors = 0;
    let mut frames = Vec::new();
    for _ in 0..repeat {
        let before = read.get();
        let start = Instant::now();
        let cursor = open(disc)?;
        frames = decode(disc, cursor)?;
        best = best.min(start.elapsed());
        sectors = (read.get() - before) / 2048;
    }
    Ok((best, sectors, frames))
}

fn ms(d: Duration) -> String {
    format!("{:.2} ms", d.as_secs_f64() * 1000.0)
}

fn run(args: &Args) -> Result<(), String> {
    let read = Rc::new(Cell::new(0u64));
    let calls = Rc::new(Cell::new(0u64));
    let file = File::open(&args.image).map_err(|e| format!("{}: {e}", args.image))?;
    let reader = Counting { inner: BufReader::new(file), read: Rc::clone(&read), calls: Rc::clone(&calls) };
    let mut disc = Disc::open(reader).map_err(|e| e.to_string())?;
    let area = disc.toc().area(args.slot).ok_or("no such area")?.clone();
    let n = disc.toc().sectors;
    let p = area.playtime_frames();
    let (s0, s1) = scarletbook::audio::audio_sector_range(&area, n);
    println!(
        "{}: {} audio sectors, play time {p} frames, {} frames per range",
        args.image,
        s1 - s0,
        args.frames
    );

    let options = IndexOptions::new().with_interval(args.interval);
    let mut index = AreaIndex::new(&area, n, options).map_err(|e| e.to_string())?;
    let before = read.get();
    let start = Instant::now();
    while !index.extend(disc.image_mut(), 1 << 20).map_err(|e| e.to_string())? {}
    let built = start.elapsed();
    println!(
        "index (K = {}): built in {} reading {} sectors; {} checkpoints, {} bytes in memory, {} bytes saved",
        args.interval,
        ms(built),
        (read.get() - before) / 2048,
        index.checkpoints(),
        index.memory(),
        index.to_bytes().len()
    );
    println!();
    println!("| Range starts at | Without index | Sectors read | With index | Sectors read |");
    println!("|---|---|---|---|---|");
    let mut exact = Vec::new();
    for percent in [0u32, 16, 42, 72, 99] {
        let first = (u64::from(p) * u64::from(percent) / 100) as u32;
        let first = first.min(p.saturating_sub(args.frames));
        let end = (first + args.frames).min(p);
        let slot = args.slot;
        let plain = measure(&mut disc, &read, args.repeat, &|d| {
            d.range_audio_cursor(slot, first, end).map_err(|e| e.to_string())
        })?;
        let indexed = measure(&mut disc, &read, args.repeat, &|d| {
            d.range_audio_cursor_indexed(slot, first, end, &index).map_err(|e| e.to_string())
        })?;
        if plain.2 != indexed.2 {
            return Err(format!("frames {first}..{end} differ with the index"));
        }
        println!(
            "| {percent} % (frame {first}) | {} | {} | {} | {} |",
            ms(plain.0),
            plain.1,
            ms(indexed.0),
            indexed.1
        );
        exact.push((first, end, plain.2));
    }

    // Cold seeks: a disc of its own, no seek index, provisional reading allowed.
    let file = File::open(&args.image).map_err(|e| format!("{}: {e}", args.image))?;
    let reader = Counting { inner: BufReader::new(file), read: Rc::clone(&read), calls: Rc::clone(&calls) };
    let mut cold = Disc::open(reader).map_err(|e| e.to_string())?;
    let policy = SeekPolicy::Speculative(SpeculativeOptions::new());
    cold.set_seek_policy(args.slot, policy).map_err(|e| e.to_string())?;
    println!();
    println!(
        "Cold seeks (no index, provisional reading); network model {} ms per read call, {} MB/s:",
        args.rtt_ms, args.mbps
    );
    println!();
    println!(
        "| Range starts at | Start | Read calls | Sectors read | Time | Network model | Equal to exact |"
    );
    println!("|---|---|---|---|---|---|---|");
    for (percent, (first, end, want)) in [0u32, 16, 42, 72, 99].into_iter().zip(exact) {
        let slot = args.slot;
        let mut best = Duration::MAX;
        let (mut ncalls, mut sectors, mut start, mut equal) = (0, 0, String::new(), false);
        for _ in 0..args.repeat {
            let (r0, c0) = (read.get(), calls.get());
            let t = Instant::now();
            let cursor = cold.range_audio_cursor(slot, first, end).map_err(|e| e.to_string())?;
            let mut audio = cold.read_track_audio(cursor);
            let got: Vec<AudioFrame> =
                audio.by_ref().map(|f| f.map_err(|e| e.to_string())).collect::<Result<_, _>>()?;
            best = best.min(t.elapsed());
            let cursor = audio.into_cursor();
            ncalls = calls.get() - c0;
            sectors = (read.get() - r0) / 2048;
            start = match cursor.resumed_from() {
                ResumedFrom::Speculative { lsn, probes } => {
                    format!("provisional at LSN {lsn}, {probes} probes")
                }
                other => format!("{other:?}"),
            };
            equal = got == want;
        }
        let model = ncalls as f64 * args.rtt_ms + (sectors * 2048) as f64 / (args.mbps * 1e6) * 1e3;
        println!(
            "| {percent} % (frame {first}) | {start} | {ncalls} | {sectors} | {} | {model:.1} ms | {} |",
            ms(best),
            if equal { "yes" } else { "no" }
        );
    }
    Ok(())
}

fn main() -> ExitCode {
    match parse().and_then(|a| run(&a)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
