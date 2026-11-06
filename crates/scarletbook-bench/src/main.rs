// SPDX-License-Identifier: MIT OR Apache-2.0

//! The benchmark program of sacd-spec `benchmarks/README.md`:
//!
//! ```text
//! bench --version
//! bench decode IMAGE --area 2ch|mc --threads N --sink hash|null|raw DIR [--lanes K]
//! bench dsf IMAGE --area 2ch|mc --threads N --out FILE [--via stream|read-at|parallel] [--chunk BYTES]
//! bench realtime IMAGE --area 2ch|mc [--seeks N]
//! bench open IMAGE [--iterations N] [--evict PROGRAM]
//! bench first IMAGE --area 2ch|mc --threads N [--track N] [--bytes N] [--iterations N] [--evict PROGRAM]
//! ```
//!
//! It decodes every track of one area, in track order, in one pass over the area
//! (continuing one decoding cursor from track to track). With `--threads N` all decoding
//! runs on a pool of `N` threads (`0`: one per logical CPU); with `N = 1` it runs on a
//! single thread. `--lanes K` (1 to 4, not part of that interface; issue #203) decodes `K`
//! DST frames together on each thread, `TrackAudioCursor::with_dst_lanes`, instead of the
//! library's default for the channel count; the output is the same.
//!
//! `dsf` is not part of that interface (issue #202): it writes the area's whole play range
//! `[0, P)` as one DSF file through `output::Layout`, `--chunk` bytes per call (default
//! 4 MiB). With `--via stream` (the default) one source reads the area once, in order;
//! with `--via read-at` every chunk is a `Layout::read_at` call of its own, as a
//! filesystem serving the file would make, with the area's seek index enabled. With `--via
//! parallel` (Unix) the calling thread reads and demultiplexes the area, and `N` worker
//! threads (`--threads`, as for the pool) each fill whole DSF rows, about `--chunk` bytes
//! at a time, and write them at their offset (`write_at`): a prototype of a parallel
//! writer, from the public `Layout` API only (#202, #224). `FILE` may be `/dev/null`, to
//! leave the cost of writing out.
//!
//! `realtime` is not part of that interface either (issue #255): what a real-time player
//! needs from one thread. It reads the area's whole play range through `Disc::range_audio`,
//! timing each call that yields a frame (demultiplexing and decoding), then builds a complete
//! seek index (untimed) and times `--seeks` jumps (default 200) to pseudo-random positions
//! from a fixed generator, from before `range_audio` until its first frame. It prints one
//! `name<TAB>value` line per figure. A real-time factor is the seconds of audio per second
//! of decoding; a window is 75 frames, one second of audio, sliding by one frame. Put the
//! image in the page cache first, to leave disc reads out, and pin the process to one core.
//!
//! `open` and `first` are not part of that interface either (issue #224): latencies far
//! below the time a process takes to start, so they are timed inside the process, `N`
//! times (`--iterations`, default 100), and printed as `name<TAB>value` lines. `open` times
//! from before opening the file until `Disc::open` has parsed the TOC; closing is not
//! timed. `first` times the first `--bytes` (default 128 KiB) of a track's decoded audio,
//! from before `Disc::track_audio` until the frame that completes them, on a disc that is
//! already open and has a complete seek index (built untimed, as for `realtime`), through a
//! new file handle per iteration. `--track` defaults to the area's middle track.
//!
//! By default the image stays in the page cache. `--evict PROGRAM` runs `PROGRAM IMAGE`,
//! untimed, before each iteration, and a failure of it fails the run: this crate has no
//! `unsafe`, and the standard library has no call that evicts a file's pages, so that is
//! left to a helper (`benchmarks/evict.py`).
//!
//! Errors: the first line on stderr is one error code: a code of specification 7, 7.7.1
//! for a rejected image, `no-such-area` when the slot yields no area, and `io-error`,
//! `usage-error` or `internal-error` otherwise. A message follows on the next line.

#![forbid(unsafe_code)]

use std::borrow::Cow;
use std::collections::VecDeque;
use std::fs::File;
use std::hint::black_box;
use std::io::{BufReader, BufWriter, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use scarletbook::audio::{AudioFrame, FRAME_BYTES_PER_CHANNEL, TrackAudio};
use scarletbook::output::{Format, OutputOptions, PositionSource, RegionKind};
use scarletbook::seek::IndexOptions;
use scarletbook::{Disc, Error, Slot};
use sha2::{Digest, Sha256};

const USAGE: &str = "usage: bench --version
       bench decode IMAGE --area 2ch|mc --threads N --sink hash|null|raw DIR [--lanes K]
       bench dsf IMAGE --area 2ch|mc --threads N --out FILE [--via stream|read-at|parallel] [--chunk BYTES]
       bench realtime IMAGE --area 2ch|mc [--seeks N]
       bench open IMAGE [--iterations N] [--evict PROGRAM]
       bench first IMAGE --area 2ch|mc --threads N [--track N] [--bytes N] [--iterations N] [--evict PROGRAM]";

/// Bytes per `Layout::fill` or `Layout::read_at` call of `dsf`, by default.
const DSF_CHUNK: usize = 4 << 20;

/// Frames per second of DSD64 audio, and so the frames of `realtime`'s sliding window.
const FRAMES_PER_SECOND: usize = 75;

/// `realtime`'s seeks, by default.
const SEEKS: usize = 200;

/// Timed iterations of `open` and `first`, by default.
const ITERATIONS: usize = 100;

/// The most timed iterations of `open` and `first`: their times are kept in memory.
const MAX_ITERATIONS: usize = 1_000_000;

/// The bytes of decoded audio `first` times, by default.
const FIRST_BYTES: usize = 128 << 10;

/// A frame slower than this misses a player's bound: half a frame period, as #255 rounds it.
const SLOW_FRAME_NANOS: u64 = 6_700_000;

/// Bytes buffered per output file of the raw sink.
const OUT_BUFFER: usize = 1 << 20;

#[derive(Debug)]
enum Sink {
    Hash,
    Null,
    Raw(PathBuf),
}

/// How `dsf` reads the area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Via {
    Stream,
    ReadAt,
    Parallel,
}

#[derive(Debug)]
enum Command {
    Decode(Sink, Option<usize>),
    Dsf { out: PathBuf, via: Via, chunk: usize },
    Realtime { seeks: usize },
    Open(Samples),
    First { track: Option<u32>, bytes: usize, samples: Samples },
}

/// How `open` and `first` repeat what they time.
#[derive(Debug)]
struct Samples {
    iterations: usize,
    /// Run with the image as its argument, untimed, before each iteration.
    evict: Option<PathBuf>,
}

#[derive(Debug)]
struct Args {
    image: PathBuf,
    slot: Slot,
    threads: usize,
    command: Command,
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
    let (dsf, realtime, opening, first) = match args.next().as_deref() {
        Some("--version") => return Ok(None),
        Some("decode") => (false, false, false, false),
        Some("dsf") => (true, false, false, false),
        Some("realtime") => (false, true, false, false),
        Some("open") => (false, false, true, false),
        Some("first") => (false, false, false, true),
        _ => return Err(USAGE.into()),
    };
    let image = PathBuf::from(args.next().ok_or(USAGE)?);
    let (mut slot, mut threads, mut sink) = (None, None, None);
    let (mut out, mut via, mut chunk) = (None, Via::Stream, DSF_CHUNK);
    let mut seeks = SEEKS;
    let mut lanes = None;
    let (mut iterations, mut evict, mut track, mut bytes) = (ITERATIONS, None, None, FIRST_BYTES);
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value\n{USAGE}"));
        match flag.as_str() {
            // `--area` selects by master TOC slot, not by area kind.
            "--area" => {
                slot = Some(match value()?.as_str() {
                    "2ch" => Slot::TwoChannel,
                    "mc" => Slot::Multichannel,
                    other => return Err(format!("--area must be 2ch or mc, not {other}")),
                })
            }
            "--threads" => {
                let v = value()?;
                threads =
                    Some(v.parse::<usize>().map_err(|_| format!("--threads must be a number, not {v}"))?);
            }
            "--sink" => {
                sink = Some(match value()?.as_str() {
                    "hash" => Sink::Hash,
                    "null" => Sink::Null,
                    "raw" => Sink::Raw(PathBuf::from(value()?)),
                    other => return Err(format!("--sink must be hash, null or raw DIR, not {other}")),
                })
            }
            "--out" if dsf => out = Some(PathBuf::from(value()?)),
            "--via" if dsf => {
                via = match value()?.as_str() {
                    "stream" => Via::Stream,
                    "read-at" => Via::ReadAt,
                    "parallel" => Via::Parallel,
                    other => return Err(format!("--via must be stream, read-at or parallel, not {other}")),
                }
            }
            "--chunk" if dsf => {
                let v = value()?;
                chunk = v
                    .parse::<usize>()
                    .ok()
                    .filter(|&n| n > 0)
                    .ok_or_else(|| format!("--chunk must be a positive number, not {v}"))?;
            }
            "--seeks" if realtime => {
                let v = value()?;
                seeks = v.parse::<usize>().map_err(|_| format!("--seeks must be a number, not {v}"))?;
            }
            "--iterations" if opening || first => {
                let v = value()?;
                iterations = v
                    .parse::<usize>()
                    .ok()
                    .filter(|n| (1..=MAX_ITERATIONS).contains(n))
                    .ok_or_else(|| format!("--iterations must be from 1 to {MAX_ITERATIONS}, not {v}"))?;
            }
            "--lanes" if !(dsf || realtime || opening || first) => {
                let v = value()?;
                let max = scarletbook::dst::MAX_LANES;
                lanes = Some(
                    v.parse::<usize>()
                        .ok()
                        .filter(|n| (1..=max).contains(n))
                        .ok_or_else(|| format!("--lanes must be from 1 to {max}, not {v}"))?,
                );
            }
            "--evict" if opening || first => evict = Some(PathBuf::from(value()?)),
            "--track" if first => {
                let v = value()?;
                track = Some(v.parse::<u32>().map_err(|_| format!("--track must be a number, not {v}"))?);
            }
            "--bytes" if first => {
                let v = value()?;
                bytes = v
                    .parse::<usize>()
                    .ok()
                    .filter(|&n| n > 0)
                    .ok_or_else(|| format!("--bytes must be a positive number, not {v}"))?;
            }
            other => return Err(format!("unknown argument {other}\n{USAGE}")),
        }
    }
    if realtime {
        // Always one thread: that is what it measures.
        return match (slot, threads, sink) {
            (Some(slot), None, None) => {
                Ok(Some(Args { image, slot, threads: 1, command: Command::Realtime { seeks } }))
            }
            _ => Err(format!("realtime takes --area and --seeks only, and needs --area\n{USAGE}")),
        };
    }
    let samples = Samples { iterations, evict };
    if opening {
        // Reads no area: the slot is not used.
        return match (slot, threads, sink) {
            (None, None, None) => {
                Ok(Some(Args { image, slot: Slot::TwoChannel, threads: 1, command: Command::Open(samples) }))
            }
            _ => Err(format!("open takes --iterations and --evict only\n{USAGE}")),
        };
    }
    if first {
        return match (slot, threads, sink) {
            (Some(slot), Some(threads), None) => {
                Ok(Some(Args { image, slot, threads, command: Command::First { track, bytes, samples } }))
            }
            _ => Err(format!("first needs --area and --threads, and takes no --sink\n{USAGE}")),
        };
    }
    let command = match (dsf, sink, out) {
        (false, Some(sink), None) => Command::Decode(sink, lanes),
        (true, None, Some(out)) => Command::Dsf { out, via, chunk },
        (false, ..) => return Err(format!("--area, --threads and --sink are all required\n{USAGE}")),
        (true, ..) => return Err(format!("--area, --threads and --out are all required\n{USAGE}")),
    };
    match (slot, threads) {
        (Some(slot), Some(threads)) => Ok(Some(Args { image, slot, threads, command })),
        _ => Err(format!("--area and --threads are required\n{USAGE}")),
    }
}

/// A failure: the error class printed on stderr, and a message.
struct Failure {
    class: &'static str,
    message: String,
}

impl From<Error> for Failure {
    fn from(e: Error) -> Self {
        let class = e.code().unwrap_or(match e {
            Error::Io(_) => "io-error",
            Error::NoSuchArea(_) => "no-such-area",
            _ => "internal-error",
        });
        Failure { class, message: e.to_string() }
    }
}

impl From<std::io::Error> for Failure {
    fn from(e: std::io::Error) -> Self {
        Failure { class: "io-error", message: e.to_string() }
    }
}

/// Receives each track's audio, frame by frame.
trait Output {
    fn start_track(&mut self, number: u32, channels: u8) -> Result<(), Failure>;
    fn frame(&mut self, data: &[u8]) -> Result<(), Failure>;
    fn end_track(&mut self) -> Result<(), Failure>;
    fn no_audio(&mut self, number: u32, code: &str) -> Result<(), Failure>;
}

struct NullOut;

impl Output for NullOut {
    fn start_track(&mut self, _: u32, _: u8) -> Result<(), Failure> {
        Ok(())
    }
    fn frame(&mut self, data: &[u8]) -> Result<(), Failure> {
        std::hint::black_box(data);
        Ok(())
    }
    fn end_track(&mut self) -> Result<(), Failure> {
        Ok(())
    }
    fn no_audio(&mut self, _: u32, _: &str) -> Result<(), Failure> {
        Ok(())
    }
}

/// Splits an interleaved frame into its channels' 4704-byte runs.
fn split(data: &[u8], channels: usize, chan: &mut [Vec<u8>]) {
    // A constant channel count lets the compiler vectorise the strided copy; a generic
    // iterator chain over `step_by` was most of the time of the raw sink.
    match channels {
        1 => split_n::<1>(data, chan),
        2 => split_2(data, chan),
        3 => split_n::<3>(data, chan),
        4 => split_n::<4>(data, chan),
        5 => split_n::<5>(data, chan),
        6 => split_n::<6>(data, chan),
        _ => {
            for (c, out) in chan.iter_mut().enumerate().take(channels) {
                out.clear();
                out.extend(data.iter().skip(c).step_by(channels));
            }
        }
    }
}

fn split_2(data: &[u8], chan: &mut [Vec<u8>]) {
    let [l, r] = chan else { return split_n::<2>(data, chan) };
    let n = data.len() / 2;
    l.clear();
    l.resize(n, 0);
    r.clear();
    r.resize(n, 0);
    // Eight interleaved bytes at a time: the even bytes go left, the odd ones right.
    fn pack(v: u64) -> [u8; 4] {
        let v = (v | (v >> 8)) & 0x0000_FFFF_0000_FFFF;
        (((v | (v >> 16)) & 0xFFFF_FFFF) as u32).to_le_bytes()
    }
    let words = data.chunks_exact(8);
    let tail = words.remainder();
    for ((l, r), w) in l.chunks_exact_mut(4).zip(r.chunks_exact_mut(4)).zip(words) {
        let mut b = [0u8; 8];
        b.copy_from_slice(w);
        let x = u64::from_le_bytes(b);
        l.copy_from_slice(&pack(x & 0x00FF_00FF_00FF_00FF));
        r.copy_from_slice(&pack((x >> 8) & 0x00FF_00FF_00FF_00FF));
    }
    let done = (n / 4) * 4;
    for (i, f) in tail.chunks_exact(2).enumerate() {
        l[done + i] = f[0];
        r[done + i] = f[1];
    }
}

fn split_n<const C: usize>(data: &[u8], chan: &mut [Vec<u8>]) {
    let n = data.len() / C;
    for (c, out) in chan.iter_mut().enumerate().take(C) {
        out.clear();
        out.resize(n, 0);
        for (o, f) in out.iter_mut().zip(data.chunks_exact(C)) {
            *o = f[c];
        }
    }
}

struct HashOut<W: Write> {
    out: W,
    number: u32,
    hashers: Vec<Sha256>,
    chan: Vec<Vec<u8>>,
}

impl<W: Write> Output for HashOut<W> {
    fn start_track(&mut self, number: u32, channels: u8) -> Result<(), Failure> {
        self.number = number;
        self.hashers = (0..channels).map(|_| Sha256::new()).collect();
        self.chan = vec![Vec::with_capacity(FRAME_BYTES_PER_CHANNEL); usize::from(channels)];
        Ok(())
    }
    fn frame(&mut self, data: &[u8]) -> Result<(), Failure> {
        split(data, self.hashers.len(), &mut self.chan);
        for (h, c) in self.hashers.iter_mut().zip(&self.chan) {
            h.update(c);
        }
        Ok(())
    }
    fn end_track(&mut self) -> Result<(), Failure> {
        let mut line = self.number.to_string();
        for h in std::mem::take(&mut self.hashers) {
            line.push('\t');
            line.extend(h.finalize().iter().map(|b| format!("{b:02x}")));
        }
        writeln!(self.out, "{line}")?;
        Ok(())
    }
    fn no_audio(&mut self, number: u32, code: &str) -> Result<(), Failure> {
        // A track without audio: its number and its outcome code (7.7.4); not an error.
        writeln!(self.out, "{number}\t{code}")?;
        Ok(())
    }
}

struct RawOut {
    dir: PathBuf,
    files: Vec<BufWriter<File>>,
    chan: Vec<Vec<u8>>,
}

impl Output for RawOut {
    fn start_track(&mut self, number: u32, channels: u8) -> Result<(), Failure> {
        // TT: the track number zero-padded to at least two digits (01 to 99, 100 to 255).
        self.files = (0..channels)
            .map(|c| {
                // Large buffers: one write call per 256 frames of a channel, not one per frame.
                File::create(self.dir.join(format!("{number:02}-{c}.dsd")))
                    .map(|f| BufWriter::with_capacity(OUT_BUFFER, f))
            })
            .collect::<Result<_, _>>()?;
        self.chan = vec![Vec::with_capacity(FRAME_BYTES_PER_CHANNEL); usize::from(channels)];
        Ok(())
    }
    fn frame(&mut self, data: &[u8]) -> Result<(), Failure> {
        split(data, self.files.len(), &mut self.chan);
        for (f, c) in self.files.iter_mut().zip(&self.chan) {
            f.write_all(c)?;
        }
        Ok(())
    }
    fn end_track(&mut self) -> Result<(), Failure> {
        for mut f in std::mem::take(&mut self.files) {
            f.flush()?;
        }
        Ok(())
    }
    fn no_audio(&mut self, _: u32, _: &str) -> Result<(), Failure> {
        Ok(())
    }
}

/// Decodes every track of the area in one pass, feeding `out`.
/// Opens the image at `path` as the library's documentation does, with
/// `BufReader::new`: its 8 KiB buffer is smaller than the area reads (32 sectors, 64 KiB),
/// which therefore go straight into the cursor's buffer, one copy from the kernel, as with
/// `sb_disc_open` (#202). A buffer larger than those reads, such as 1 MiB, copies every
/// byte twice: on lennon (3950X) it made a 78-minute plain-DSD stereo area 0.10 s slower to
/// read.
fn open(path: &Path) -> Result<Disc<BufReader<File>>, Failure> {
    let file = File::open(path)
        .map_err(|e| Failure { class: "io-error", message: format!("{}: {e}", path.display()) })?;
    Ok(Disc::open(BufReader::new(file))?)
}

fn decode_area(path: &Path, slot: Slot, lanes: Option<usize>, out: &mut dyn Output) -> Result<(), Failure> {
    let mut disc = open(path)?;
    let area = disc.toc().area(slot).ok_or(Error::NoSuchArea(slot))?;
    let numbers: Vec<u32> = area.tracks.iter().map(|t| u32::from(t.number)).collect();
    let mut cursor = None;
    for n in numbers {
        if let Err(e) = disc.check_track(slot, n) {
            out.no_audio(n, e.code().unwrap_or("internal-error"))?;
            continue;
        }
        let next = match cursor.take() {
            Some(prev) => disc.continue_track_audio(prev, slot, n)?,
            None => match lanes {
                Some(k) => disc.track_audio_cursor(slot, n)?.with_dst_lanes(k)?,
                None => disc.track_audio_cursor(slot, n)?,
            },
        };
        let mut track = disc.read_track_audio(next);
        out.start_track(n, track.channels())?;
        for frame in track.by_ref() {
            out.frame(&frame?.data)?;
        }
        out.end_track()?;
        cursor = Some(track.into_cursor());
    }
    Ok(())
}

/// The positions of one range read in order, the last few kept: `Layout::fill` reads a
/// DSF row's positions again for each channel.
struct Stream<'a, R> {
    frames: TrackAudio<'a, R>,
    window: VecDeque<AudioFrame>,
}

/// Positions kept behind the latest one asked for.
const KEPT: u32 = 4;

impl<R: Read + Seek> PositionSource for Stream<'_, R> {
    fn content(&mut self, t: u32) -> Result<Cow<'_, [u8]>, Error> {
        while self.window.back().is_none_or(|f| f.number < t) {
            let frame = self.frames.next().ok_or(Error::InvalidArgument("position past the range"))??;
            self.window.push_back(frame);
        }
        while self.window.front().is_some_and(|f| f.number.saturating_add(KEPT) < t) {
            self.window.pop_front();
        }
        let front = self.window.front().map_or(t, |f| f.number);
        let i = t.checked_sub(front).ok_or(Error::InvalidArgument("position no longer held"))?;
        let frame = self.window.get(i as usize).ok_or(Error::InvalidArgument("position not held"))?;
        Ok(Cow::Borrowed(&frame.data))
    }
}

/// What `realtime` reports of one pass, from the time of each frame in nanoseconds.
#[derive(Debug, PartialEq)]
struct FrameTimes {
    mean_factor: f64,
    slowest_window_factor: f64,
    p999_nanos: u64,
    max_nanos: u64,
    slow_frames: usize,
}

/// The real-time factor of `frames` frames decoded in `nanos` nanoseconds.
fn factor(frames: usize, nanos: u64) -> f64 {
    (frames as f64 / FRAMES_PER_SECOND as f64) / (nanos.max(1) as f64 / 1e9)
}

/// The figures of a pass, `None` for no frames. A pass shorter than a window is its own
/// slowest window.
fn frame_times(times: &[u64]) -> Option<FrameTimes> {
    let mut sorted = times.to_vec();
    sorted.sort_unstable();
    let max_nanos = *sorted.last()?;
    // Nearest rank: the smallest time that at least 99.9 % of the frames do not exceed.
    let p999_nanos = sorted[(sorted.len() * 999).div_ceil(1000) - 1];
    let window = FRAMES_PER_SECOND.min(times.len());
    let mut sum: u64 = times[..window].iter().sum();
    let mut slowest = sum;
    for (gone, added) in times.iter().zip(&times[window..]) {
        sum = sum - gone + added;
        slowest = slowest.max(sum);
    }
    Some(FrameTimes {
        mean_factor: factor(times.len(), times.iter().sum()),
        slowest_window_factor: factor(window, slowest),
        p999_nanos,
        max_nanos,
        slow_frames: times.iter().filter(|&&t| t > SLOW_FRAME_NANOS).count(),
    })
}

/// The next value of a fixed pseudo-random sequence (xorshift64), for repeatable seeks.
fn next_random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn millis(nanos: u64) -> f64 {
    nanos as f64 / 1e6
}

/// Prints what a real-time player needs from one thread (see the module documentation).
fn realtime(path: &Path, slot: Slot, seeks: usize, out: &mut impl Write) -> Result<(), Failure> {
    let mut disc = open(path)?;
    let area = disc.toc().area(slot).ok_or(Error::NoSuchArea(slot))?;
    let end = area.playtime_frames();
    let mut times = Vec::with_capacity(end as usize);
    let mut frames = disc.range_audio(slot, 0, end)?;
    loop {
        let start = Instant::now();
        let frame = frames.next();
        let nanos = start.elapsed().as_nanos();
        let Some(frame) = frame else { break };
        std::hint::black_box(&frame?.data);
        times.push(u64::try_from(nanos).unwrap_or(u64::MAX));
    }
    drop(frames);
    writeln!(out, "frames\t{}", times.len())?;
    let Some(t) = frame_times(&times) else { return Ok(()) };
    writeln!(out, "mean_realtime_factor\t{:.2}", t.mean_factor)?;
    writeln!(out, "slowest_window_realtime_factor\t{:.2}", t.slowest_window_factor)?;
    writeln!(out, "p999_frame_ms\t{:.3}", millis(t.p999_nanos))?;
    writeln!(out, "max_frame_ms\t{:.3}", millis(t.max_nanos))?;
    writeln!(out, "frames_over_6.7_ms\t{}", t.slow_frames)?;

    disc.enable_seek_index(slot, IndexOptions::new())?;
    while !disc.extend_seek_index(slot, 1 << 20)? {}
    let mut random = 0x9E37_79B9_7F4A_7C15;
    let mut jumps = Vec::with_capacity(seeks);
    for _ in 0..seeks {
        let position = (next_random(&mut random) % u64::from(end)) as u32;
        let start = Instant::now();
        let first = disc.range_audio(slot, position, end)?.next();
        let nanos = start.elapsed().as_nanos();
        let first = first.ok_or(Error::InvalidArgument("no frame after a seek"))??;
        if first.number != position {
            return Err(Error::InvalidArgument("a seek reached another frame").into());
        }
        jumps.push(u64::try_from(nanos).unwrap_or(u64::MAX));
    }
    jumps.sort_unstable();
    writeln!(out, "seeks\t{}", jumps.len())?;
    if let (Some(median), Some(max)) = (jumps.get(jumps.len() / 2), jumps.last()) {
        writeln!(out, "seek_median_ms\t{:.3}", millis(*median))?;
        writeln!(out, "seek_max_ms\t{:.3}", millis(*max))?;
    }
    Ok(())
}

/// Runs `--evict`'s program on the image, if there is one.
fn evict(program: Option<&Path>, image: &Path) -> Result<(), Failure> {
    let Some(program) = program else { return Ok(()) };
    let run = std::process::Command::new(program)
        .arg(image)
        .output()
        .map_err(|e| Failure { class: "io-error", message: format!("{}: {e}", program.display()) })?;
    if run.status.success() {
        return Ok(());
    }
    let message =
        format!("{}: {}: {}", program.display(), run.status, String::from_utf8_lossy(&run.stderr).trim());
    Err(Failure { class: "io-error", message })
}

/// Prints the figures of `open` and `first`, from each iteration's time in nanoseconds.
fn print_times(times: &mut [u64], out: &mut impl Write) -> Result<(), Failure> {
    times.sort_unstable();
    writeln!(out, "iterations\t{}", times.len())?;
    if let (Some(median), Some(min), Some(max)) = (times.get(times.len() / 2), times.first(), times.last()) {
        writeln!(out, "median_us\t{:.2}", *median as f64 / 1e3)?;
        writeln!(out, "min_us\t{:.2}", *min as f64 / 1e3)?;
        writeln!(out, "max_us\t{:.2}", *max as f64 / 1e3)?;
    }
    Ok(())
}

/// Times opening the image up to its parsed TOC (see the module documentation).
fn open_times(path: &Path, samples: &Samples, out: &mut impl Write) -> Result<(), Failure> {
    let mut times = Vec::with_capacity(samples.iterations);
    for _ in 0..samples.iterations {
        evict(samples.evict.as_deref(), path)?;
        let start = Instant::now();
        let disc = open(path)?;
        black_box(disc.toc());
        let nanos = start.elapsed().as_nanos();
        times.push(u64::try_from(nanos).unwrap_or(u64::MAX));
    }
    print_times(&mut times, out)
}

/// Times the first bytes of a track's audio (see the module documentation).
fn first_times(
    path: &Path,
    slot: Slot,
    (track, bytes): (Option<u32>, usize),
    samples: &Samples,
    out: &mut impl Write,
) -> Result<(), Failure> {
    let mut base = open(path)?;
    let area = base.toc().area(slot).ok_or(Error::NoSuchArea(slot))?;
    let usage = |message: String| Failure { class: "usage-error", message };
    let middle = area.tracks.get(area.tracks.len() / 2).map(|t| u32::from(t.number));
    let number = track.or(middle).ok_or_else(|| usage("the area has no tracks".into()))?;
    if !area.tracks.iter().any(|t| u32::from(t.number) == number) {
        return Err(usage(format!("--track {number}: the area has no such track")));
    }
    base.check_track(slot, number)?;
    base.enable_seek_index(slot, IndexOptions::new())?;
    while !base.extend_seek_index(slot, 1 << 20)? {}
    let mut times = Vec::with_capacity(samples.iterations);
    for _ in 0..samples.iterations {
        let mut disc = base.share(BufReader::new(File::open(path)?))?;
        evict(samples.evict.as_deref(), path)?;
        let start = Instant::now();
        let mut frames = disc.track_audio(slot, number)?;
        let mut got = 0;
        while got < bytes {
            let Some(frame) = frames.next() else {
                return Err(usage(format!("--bytes {bytes}: past the end of track {number}")));
            };
            let frame = frame?;
            got += frame.data.len();
            black_box(&frame.data);
        }
        let nanos = start.elapsed().as_nanos();
        times.push(u64::try_from(nanos).unwrap_or(u64::MAX));
    }
    writeln!(out, "track\t{number}")?;
    print_times(&mut times, out)
}

/// Writes the area's play range as one DSF file (see the module documentation).
fn write_dsf(path: &Path, slot: Slot, out: &Path, via: Via, chunk: usize) -> Result<(), Failure> {
    let mut disc = open(path)?;
    let area = disc.toc().area(slot).ok_or(Error::NoSuchArea(slot))?;
    let end = area.playtime_frames();
    let layout = disc.output_layout(slot, 0, end, Format::Dsf, &OutputOptions::default())?;
    let mut file = File::create(out)?;
    let mut buf = vec![0u8; chunk];
    let mut offset = 0u64;
    match via {
        Via::Stream => {
            let frames = disc.range_audio(slot, 0, end)?;
            let mut source = Stream { frames, window: VecDeque::new() };
            while offset < layout.size() {
                let n = layout.fill(offset, &mut buf, &mut source)?;
                file.write_all(&buf[..n])?;
                offset += n as u64;
            }
        }
        Via::Parallel => {
            return Err(Failure {
                class: "internal-error",
                message: "--via parallel is write_dsf_parallel".into(),
            });
        }
        Via::ReadAt => {
            disc.enable_seek_index(slot, IndexOptions::new())?;
            while offset < layout.size() {
                let n = layout.read_at(&mut disc, offset, &mut buf)?;
                file.write_all(&buf[..n])?;
                offset += n as u64;
            }
        }
    }
    file.flush()?;
    Ok(())
}

/// Positions of one piece of the file, in memory: shared with the next piece where a DSF
/// row reads a position on both sides of their boundary.
struct Piece {
    first: u32,
    frames: Vec<Arc<AudioFrame>>,
}

impl PositionSource for Piece {
    fn content(&mut self, t: u32) -> Result<Cow<'_, [u8]>, Error> {
        let i = t.checked_sub(self.first).ok_or(Error::InvalidArgument("position before the piece"))?;
        let f = self.frames.get(i as usize).ok_or(Error::InvalidArgument("position past the piece"))?;
        Ok(Cow::Borrowed(&f.data))
    }
}

/// `--via parallel`: the file in pieces of whole DSF rows (the first with the header, the
/// last with the tag), each filled and written at its offset by one of `workers` threads,
/// while this thread reads the area and hands each piece the positions it needs.
#[cfg(unix)]
fn write_dsf_parallel(
    path: &Path,
    slot: Slot,
    out: &Path,
    workers: usize,
    chunk: usize,
) -> Result<(), Failure> {
    use std::os::unix::fs::FileExt;
    use std::sync::{Mutex, mpsc};

    let mut disc = open(path)?;
    let area = disc.toc().area(slot).ok_or(Error::NoSuchArea(slot))?;
    let end = area.playtime_frames();
    let layout = disc.output_layout(slot, 0, end, Format::Dsf, &OutputOptions::default())?;
    let size = layout.size();
    let sound = layout
        .regions()
        .iter()
        .find(|r| r.kind == RegionKind::SoundData)
        .copied()
        .ok_or(Failure { class: "internal-error", message: "a DSF file without sound data".into() })?;
    // Piece boundaries: the start of the sound data plus whole rows, and the file's end.
    let row = 4096 * u64::from(layout.channels());
    let step = (chunk as u64 / row).max(1) * row;
    let mut bounds: Vec<u64> = vec![0];
    bounds.extend((1..).map(|k| sound.offset + k * step).take_while(|&b| b < sound.offset + sound.len));
    bounds.push(size);

    let file = File::create(out)?;
    let (send, receive) = mpsc::sync_channel::<(u64, u64, Piece)>(2 * workers);
    let receive = Mutex::new(receive);
    let failed: Mutex<Option<Failure>> = Mutex::new(None);
    std::thread::scope(|scope| -> Result<(), Failure> {
        // Dropped when the reader returns, early or not, so that the workers stop.
        let send = send;
        for _ in 0..workers {
            scope.spawn(|| {
                let mut buf = Vec::new();
                loop {
                    let job = receive.lock().map(|r| r.recv());
                    let Ok(Ok((at, to, mut piece))) = job else { return };
                    // After a failure, the pieces still sent are taken and dropped, so that
                    // the reader never waits on a full channel.
                    if failed.lock().map_or(true, |f| f.is_some()) {
                        continue;
                    }
                    buf.resize((to - at) as usize, 0);
                    let result = layout
                        .fill(at, &mut buf, &mut piece)
                        .map_err(Failure::from)
                        .and_then(|n| file.write_all_at(&buf[..n], at).map_err(Failure::from));
                    if let (Err(e), Ok(mut f)) = (result, failed.lock()) {
                        f.get_or_insert(e);
                    }
                }
            });
        }
        // The reader: positions in order, each piece taking those of `positions_for`.
        let mut frames = disc.range_audio(slot, 0, end)?;
        let mut held: VecDeque<Arc<AudioFrame>> = VecDeque::new();
        for w in bounds.windows(2) {
            let need = layout.positions_for(w[0]..w[1]);
            while held.front().is_some_and(|f| f.number < need.start) {
                held.pop_front();
            }
            while held.back().is_none_or(|f| f.number + 1 < need.end) && !need.is_empty() {
                let f = frames.next().ok_or(Error::InvalidArgument("position past the range"))??;
                held.push_back(Arc::new(f));
            }
            let first = held.front().map_or(need.start, |f| f.number);
            let piece = Piece { first, frames: held.iter().cloned().collect() };
            if failed.lock().map_or(true, |f| f.is_some()) || send.send((w[0], w[1], piece)).is_err() {
                break;
            }
        }
        Ok(())
    })?;
    if let Some(e) = failed.into_inner().ok().flatten() {
        return Err(e);
    }
    Ok(())
}

#[cfg(not(unix))]
fn write_dsf_parallel(_: &Path, _: Slot, _: &Path, _: usize, _: usize) -> Result<(), Failure> {
    Err(Failure { class: "usage-error", message: "--via parallel needs a Unix write_at".into() })
}

fn run(args: Args) -> Result<(), Failure> {
    // `--threads 0` is one thread per logical CPU, whatever RAYON_NUM_THREADS says.
    let threads = match args.threads {
        0 => std::thread::available_parallelism().map_or(1, usize::from),
        n => n,
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .map_err(|e| Failure { class: "internal-error", message: e.to_string() })?;
    pool.install(|| match &args.command {
        Command::Dsf { out, via: Via::Parallel, chunk } => {
            write_dsf_parallel(&args.image, args.slot, out, threads, *chunk)
        }
        Command::Dsf { out, via, chunk } => write_dsf(&args.image, args.slot, out, *via, *chunk),
        Command::Realtime { seeks } => {
            let mut out = std::io::stdout().lock();
            realtime(&args.image, args.slot, *seeks, &mut out)
        }
        Command::Open(samples) => open_times(&args.image, samples, &mut std::io::stdout().lock()),
        Command::First { track, bytes, samples } => {
            first_times(&args.image, args.slot, (*track, *bytes), samples, &mut std::io::stdout().lock())
        }
        Command::Decode(Sink::Null, lanes) => decode_area(&args.image, args.slot, *lanes, &mut NullOut),
        Command::Decode(Sink::Hash, lanes) => {
            let stdout = std::io::stdout().lock();
            let mut out =
                HashOut { out: BufWriter::new(stdout), number: 0, hashers: Vec::new(), chan: Vec::new() };
            decode_area(&args.image, args.slot, *lanes, &mut out)?;
            out.out.flush()?;
            Ok(())
        }
        Command::Decode(Sink::Raw(dir), lanes) => {
            std::fs::create_dir_all(dir)?;
            decode_area(
                &args.image,
                args.slot,
                *lanes,
                &mut RawOut { dir: dir.clone(), files: Vec::new(), chan: Vec::new() },
            )
        }
    })
}

fn main() -> ExitCode {
    let args = match parse(std::env::args().skip(1)) {
        Ok(Some(a)) => a,
        Ok(None) => {
            println!("scarletbook {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Err(usage) => {
            // The first stderr line is the error code alone (benchmarks/README.md, Errors).
            eprintln!("usage-error\n{usage}");
            return ExitCode::from(2);
        }
    };
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(f) => {
            // The first stderr line is the error code alone; a message follows.
            eprintln!("{}\n{}", f.class, f.message);
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn args(s: &str) -> Result<Option<Args>, String> {
        parse(s.split_whitespace().map(str::to_owned))
    }

    #[test]
    fn parses_the_interface() {
        assert!(args("--version").unwrap().is_none());
        let a = args("decode x.iso --area mc --threads 0 --sink raw out").unwrap().unwrap();
        assert_eq!((a.slot, a.threads), (Slot::Multichannel, 0));
        assert!(matches!(a.command, Command::Decode(Sink::Raw(ref d), None)  if d == Path::new("out")));
        for k in 1..=4 {
            let a = args(&format!("decode x.iso --area 2ch --threads 0 --sink null --lanes {k}"))
                .unwrap()
                .unwrap();
            assert!(matches!(a.command, Command::Decode(Sink::Null, Some(n)) if n == k));
        }
        for k in ["0", "5", "x"] {
            assert!(args(&format!("decode x.iso --area 2ch --threads 1 --sink null --lanes {k}")).is_err());
        }
        assert!(args("dsf x.iso --area 2ch --threads 1 --out f.dsf --lanes 1").is_err());
        let a =
            args("dsf x.iso --area 2ch --threads 1 --out f.dsf --via read-at --chunk 4096").unwrap().unwrap();
        assert!(
            matches!(a.command, Command::Dsf { ref out, via: Via::ReadAt, chunk: 4096 } if out == Path::new("f.dsf"))
        );
        assert!(args("dsf x.iso --area 2ch --threads 1 --sink null").is_err());
        assert!(args("decode x.iso --area 2ch --threads 1 --out f.dsf").is_err());
        assert!(args("dsf x.iso --area 2ch --threads 1 --out f.dsf --chunk 0").is_err());
        assert!(args("decode x.iso --area 5ch --threads 1 --sink null").is_err());
        assert!(args("decode x.iso --area 2ch --sink null").is_err());
        assert!(args("decode x.iso --area 2ch --threads x --sink null").is_err());
        assert!(args("").is_err());
        let a = args("realtime x.iso --area mc --seeks 5").unwrap().unwrap();
        assert_eq!((a.slot, a.threads), (Slot::Multichannel, 1));
        assert!(matches!(a.command, Command::Realtime { seeks: 5 }));
        assert!(matches!(
            args("realtime x.iso --area 2ch").unwrap().unwrap().command,
            Command::Realtime { seeks: SEEKS }
        ));
        assert!(args("realtime x.iso").is_err());
        assert!(args("realtime x.iso --area 2ch --threads 2").is_err());
        assert!(args("realtime x.iso --area 2ch --sink null").is_err());
        assert!(args("decode x.iso --area 2ch --threads 1 --sink null --seeks 5").is_err());
        let a = args("open x.iso --iterations 7 --evict e.py").unwrap().unwrap();
        assert!(matches!(
            a.command,
            Command::Open(Samples { iterations: 7, evict: Some(ref e) }) if e == Path::new("e.py")
        ));
        assert!(matches!(
            args("open x.iso").unwrap().unwrap().command,
            Command::Open(Samples { iterations: ITERATIONS, evict: None })
        ));
        assert!(args("open x.iso --area 2ch").is_err());
        assert!(args("open x.iso --iterations 0").is_err());
        assert!(args("open x.iso --iterations 1000000").is_ok());
        assert!(args("open x.iso --iterations 1000001").is_err());
        assert!(args("open x.iso --track 1").is_err());
        let a = args("first x.iso --area mc --threads 0 --track 3 --bytes 4096").unwrap().unwrap();
        assert_eq!((a.slot, a.threads), (Slot::Multichannel, 0));
        assert!(matches!(a.command, Command::First { track: Some(3), bytes: 4096, .. }));
        assert!(matches!(
            args("first x.iso --area 2ch --threads 1").unwrap().unwrap().command,
            Command::First { track: None, bytes: FIRST_BYTES, .. }
        ));
        assert!(args("first x.iso --area 2ch").is_err());
        assert!(args("first x.iso --threads 1").is_err());
        assert!(args("first x.iso --area 2ch --threads 1 --bytes 0").is_err());
        assert!(args("first x.iso --area 2ch --threads 1 --sink null").is_err());
        assert!(args("decode x.iso --area 2ch --threads 1 --sink null --evict e").is_err());
    }

    #[test]
    fn frame_figures() {
        assert_eq!(frame_times(&[]), None);
        // 150 frames of 1 ms, then one window's worth with a slow frame in it.
        let mut times = vec![1_000_000u64; 150];
        times[100] = 8_000_000;
        let t = frame_times(&times).unwrap();
        assert_eq!((t.max_nanos, t.slow_frames), (8_000_000, 1));
        // The slowest second of audio holds the slow frame: 74 ms + 8 ms for 1 s.
        assert!((t.slowest_window_factor - 1.0 / 0.082).abs() < 1e-9, "{t:?}");
        // 149 ms + 8 ms for 2 s.
        assert!((t.mean_factor - 2.0 / 0.157).abs() < 1e-9, "{t:?}");
        // Rank 150 of 150: the slow frame.
        assert_eq!(t.p999_nanos, 8_000_000);
        // Shorter than a window: 3 frames in 3 ms, 1/25 s of audio.
        let t = frame_times(&[1_000_000; 3]).unwrap();
        assert!((t.slowest_window_factor - (0.04 / 0.003)).abs() < 1e-9, "{t:?}");
        assert_eq!(t.p999_nanos, 1_000_000);
        // Exactly at the bound is not over it.
        assert_eq!(frame_times(&[SLOW_FRAME_NANOS]).unwrap().slow_frames, 0);
    }

    #[test]
    fn split_deinterleaves() {
        let mut chan = vec![Vec::new(); 2];
        split(&[1, 2, 1, 2, 1, 2], 2, &mut chan);
        assert_eq!(chan, vec![vec![1, 1, 1], vec![2, 2, 2]]);
        let data: Vec<u8> = (0..60).collect();
        for c in 1..=7usize {
            let mut chan = vec![Vec::new(); c];
            let used = &data[..data.len() / c * c];
            split(used, c, &mut chan);
            for (k, out) in chan.iter().enumerate() {
                let want: Vec<u8> = used.iter().copied().skip(k).step_by(c).collect();
                assert_eq!(out, &want);
            }
        }
    }
}
