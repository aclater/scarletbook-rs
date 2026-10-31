// SPDX-License-Identifier: MIT OR Apache-2.0

//! Measures what opening a frame range costs against where it starts, without and with a
//! seek index (issue #68), and as a cold seek by provisional reading (issue #94):
//!
//! ```text
//! scarletbook-seekcost IMAGE [--slot 2ch|mc] [--interval K] [--frames N] [--repeat R]
//!     [--rtt-ms MS] [--mbps MB] [--positions P]
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
//!
//! A third table measures cold seeks as a consumer that opens the disc afresh for each one
//! sees them (issue #123): for `P` positions spread over the area (`--positions P`, default
//! 200; 0 skips it), a newly opened disc with `SeekPolicy::Speculative` opens a range there,
//! and the table gives the read calls and KiB up to its first frame (stored, before
//! decoding) and for the whole range decoded, as mean and maximum, and how many ranges
//! equal the exact ones.
//!
//! A fourth table measures seeks after a remount (issue #124): the same ranges read in a
//! first session with a seek index kept, its saved form (seek hints) loaded on a newly
//! opened disc for each range and for one 3 frames later; and the same from a complete
//! saved index with exact reads.

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
                     [--rtt-ms MS] [--mbps MB] [--positions P]";

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
    positions: u32,
}

fn parse() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let image = it.next().ok_or(USAGE)?;
    let mut args = Args {
        image,
        slot: Slot::TwoChannel,
        interval: 64,
        frames: 14,
        repeat: 3,
        rtt_ms: 0.5,
        mbps: 110.0,
        positions: 200,
    };
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
            "--positions" => args.positions = number(&value)?,
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
    if args.positions > 0 {
        fresh(args, &mut disc, &index, p)?;
        warm(args, &mut disc, &index, p)?;
    }
    Ok(())
}

/// The ranges of the third and fourth tables: `args.positions` starts spread over the area
/// (Weyl sequence of the golden ratio), `args.frames` frames each.
fn positions(args: &Args, p: u32) -> Vec<(u32, u32)> {
    let span = p.saturating_sub(args.frames).max(1);
    let mut x = 0.5f64;
    (0..args.positions)
        .map(|_| {
            x = (x + 0.618_033_988_749_895) % 1.0;
            let first = (f64::from(span) * x) as u32;
            (first, (first + args.frames).min(p))
        })
        .collect()
}

/// The fourth table: seeks after a remount (issue #124). A first session reads every range
/// provisionally with a seek index kept and saves it (its seek hints); then each range, and
/// one 3 frames later, is read on a newly opened disc that loads the saved index. The same
/// with a complete index (one full pass, saved) and exact reads.
fn warm(args: &Args, exact: &mut Image, complete: &AreaIndex, p: u32) -> Result<(), String> {
    let slot = args.slot;
    let ranges = positions(args, p);
    let (mut first, _, _) = open_cold(args)?;
    first
        .enable_seek_index(slot, IndexOptions::new().with_interval(args.interval))
        .map_err(|e| e.to_string())?;
    for &(a, b) in &ranges {
        let cursor = first.range_frames_cursor(slot, a, b).map_err(|e| e.to_string())?;
        first.read_frames(cursor).try_for_each(|f| f.map(drop)).map_err(|e| e.to_string())?;
    }
    let hinted = first.seek_index(slot).ok_or("no index")?.to_bytes();
    let mut full = complete.clone();
    // Kept by a disc: with its TOC fingerprint, as a consumer saves it.
    let (mut d, _, _) = open_cold(args)?;
    d.set_seek_index(slot, full.clone()).map_err(|e| e.to_string())?;
    full = d.seek_index(slot).ok_or("no index")?;
    let full_bytes = full.to_bytes();
    println!();
    println!(
        "Seeks after a remount, {} ranges read before and each 3 frames later, {} frames each \
         (mean / max): saved hints {} bytes ({} hints), complete index {} bytes",
        ranges.len(),
        args.frames,
        hinted.len(),
        first.seek_index(slot).map_or(0, |i| i.hints()),
        full_bytes.len()
    );
    println!();
    println!("| Saved state | Read calls | KiB | Equal to exact | Provisional |");
    println!("|---|---|---|---|---|");
    for (name, saved, policy) in [
        ("seek hints, provisional", &hinted, SeekPolicy::Speculative(SpeculativeOptions::new())),
        ("complete index, exact", &full_bytes, SeekPolicy::Exact),
    ] {
        let (mut calls_v, mut kib_v, mut equal, mut provisional) = (Vec::new(), Vec::new(), 0, 0);
        for &(a, b) in ranges
            .iter()
            .flat_map(|&(a, b)| [(a, b), ((a + 3).min(p - 1), (b + 3).min(p))])
            .collect::<Vec<_>>()
            .iter()
        {
            let (mut disc, read, calls) = open_cold(args)?;
            let area = disc.toc().area(slot).ok_or("no such area")?.clone();
            let n = disc.toc().sectors;
            let index = AreaIndex::from_bytes(&area, n, saved, scarletbook::seek::DEFAULT_INLINE_BUDGET)
                .map_err(|e| e.to_string())?;
            disc.set_seek_index(slot, index).map_err(|e| e.to_string())?;
            disc.set_seek_policy(slot, policy).map_err(|e| e.to_string())?;
            let (r0, c0) = (read.get(), calls.get());
            let cursor = disc.range_audio_cursor(slot, a, b).map_err(|e| e.to_string())?;
            let mut audio = disc.read_track_audio(cursor);
            let got: Vec<AudioFrame> =
                audio.by_ref().map(|f| f.map_err(|e| e.to_string())).collect::<Result<_, _>>()?;
            if audio.into_cursor().speculation() != scarletbook::Speculation::Exact {
                provisional += 1;
            }
            calls_v.push((calls.get() - c0) as f64);
            kib_v.push((read.get() - r0) as f64 / 1024.0);
            let cursor = exact.range_audio_cursor_indexed(slot, a, b, complete).map_err(|e| e.to_string())?;
            if got == decode(exact, cursor)? {
                equal += 1;
            }
        }
        println!(
            "| {name} | {} | {} | {equal} of {} | {provisional} |",
            stats(&calls_v),
            stats(&kib_v),
            calls_v.len()
        );
    }
    Ok(())
}

/// Opens the image afresh, provisional reading allowed, with its counters at 0 once open.
/// Counts of bytes and of read calls, shared with a [`Counting`] reader.
type Counter = Rc<Cell<u64>>;

fn open_cold(args: &Args) -> Result<(Image, Counter, Counter), String> {
    let (read, calls) = (Rc::new(Cell::new(0u64)), Rc::new(Cell::new(0u64)));
    let file = File::open(&args.image).map_err(|e| format!("{}: {e}", args.image))?;
    let reader = Counting { inner: BufReader::new(file), read: Rc::clone(&read), calls: Rc::clone(&calls) };
    let mut disc = Disc::open(reader).map_err(|e| e.to_string())?;
    disc.set_seek_policy(args.slot, SeekPolicy::Speculative(SpeculativeOptions::new()))
        .map_err(|e| e.to_string())?;
    read.set(0);
    calls.set(0);
    Ok((disc, read, calls))
}

/// Mean and maximum of `v`.
fn stats(v: &[f64]) -> String {
    let mean = v.iter().sum::<f64>() / v.len().max(1) as f64;
    let max = v.iter().copied().fold(0.0, f64::max);
    format!("{mean:.2} / {max:.0}")
}

/// The third table: cold seeks to `args.positions` positions, each on a newly opened disc.
fn fresh(args: &Args, exact: &mut Image, index: &AreaIndex, p: u32) -> Result<(), String> {
    let slot = args.slot;
    // Read calls and KiB to the first frame and for the whole range, by kind of start.
    let mut costs: [[Vec<f64>; 4]; 2] = Default::default();
    let (mut equal, mut provisional, mut probes) = (0, 0, Vec::new());
    for (first, end) in positions(args, p) {
        // Up to the first stored frame.
        let (mut disc, read, calls) = open_cold(args)?;
        let cursor = disc.range_frames_cursor(slot, first, end).map_err(|e| e.to_string())?;
        let mut frames = disc.read_frames(cursor);
        frames.next().transpose().map_err(|e| e.to_string())?;
        let kind = match frames.into_cursor().resumed_from() {
            ResumedFrom::Speculative { probes: n, .. } => {
                provisional += 1;
                probes.push(f64::from(n));
                0
            }
            _ => 1,
        };
        let (first_calls, first_kib) = (calls.get() as f64, read.get() as f64 / 1024.0);
        // The whole range, decoded.
        let (mut disc, read, calls) = open_cold(args)?;
        let cursor = disc.range_audio_cursor(slot, first, end).map_err(|e| e.to_string())?;
        let got = decode(&mut disc, cursor)?;
        let row = [first_calls, first_kib, calls.get() as f64, read.get() as f64 / 1024.0];
        for (v, c) in costs[kind].iter_mut().zip(row) {
            v.push(c);
        }
        let cursor = exact.range_audio_cursor_indexed(slot, first, end, index).map_err(|e| e.to_string())?;
        if got == decode(exact, cursor)? {
            equal += 1;
        }
    }
    println!();
    println!(
        "Cold seeks on a newly opened disc, {} positions, {} frames each \
         (mean / max over the positions of each kind of start):",
        args.positions, args.frames
    );
    println!();
    println!(
        "| Start | Positions | Read calls to the first frame | KiB to the first frame | Read calls, whole range decoded | KiB, whole range decoded |"
    );
    println!("|---|---|---|---|---|---|");
    for (name, c) in ["provisional", "exact (near the exact start)"].into_iter().zip(&costs) {
        if !c[0].is_empty() {
            println!(
                "| {name} | {} | {} | {} | {} | {} |",
                c[0].len(),
                stats(&c[0]),
                stats(&c[1]),
                stats(&c[2]),
                stats(&c[3])
            );
        }
    }
    println!();
    println!(
        "Reads to choose a provisional start (mean / max): {} ({provisional} provisional)",
        stats(&probes)
    );
    println!("Equal to exact reads: {equal} of {}", args.positions);
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
