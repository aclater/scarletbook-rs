// SPDX-License-Identifier: MIT OR Apache-2.0

//! The benchmark program of sacd-spec `benchmarks/README.md`:
//!
//! ```text
//! bench --version
//! bench decode IMAGE --area 2ch|mc --threads N --sink hash|null|raw DIR
//! bench dsf IMAGE --area 2ch|mc --threads N --out FILE [--via stream|read-at] [--chunk BYTES]
//! ```
//!
//! It decodes every track of one area, in track order, in one pass over the area
//! (continuing one decoding cursor from track to track). With `--threads N` all decoding
//! runs on a pool of `N` threads (`0`: one per logical CPU); with `N = 1` it runs on a
//! single thread.
//!
//! `dsf` is not part of that interface (issue #202): it writes the area's whole play range
//! `[0, P)` as one DSF file through `output::Layout`, `--chunk` bytes per call (default
//! 4 MiB). With `--via stream` (the default) one source reads the area once, in order;
//! with `--via read-at` every chunk is a `Layout::read_at` call of its own, as a
//! filesystem serving the file would make, with the area's seek index enabled. `FILE` may
//! be `/dev/null`, to leave the cost of writing out.
//!
//! Errors: the first line on stderr is one error code: a code of specification 7, 7.7.1
//! for a rejected image, `no-such-area` when the slot yields no area, and `io-error`,
//! `usage-error` or `internal-error` otherwise. A message follows on the next line.

#![forbid(unsafe_code)]

use std::borrow::Cow;
use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use scarletbook::audio::{AudioFrame, FRAME_BYTES_PER_CHANNEL, TrackAudio};
use scarletbook::output::{Format, OutputOptions, PositionSource};
use scarletbook::seek::IndexOptions;
use scarletbook::{Disc, Error, Slot};
use sha2::{Digest, Sha256};

const USAGE: &str = "usage: bench --version
       bench decode IMAGE --area 2ch|mc --threads N --sink hash|null|raw DIR
       bench dsf IMAGE --area 2ch|mc --threads N --out FILE [--via stream|read-at] [--chunk BYTES]";

/// Bytes per `Layout::fill` or `Layout::read_at` call of `dsf`, by default.
const DSF_CHUNK: usize = 4 << 20;

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
}

#[derive(Debug)]
enum Command {
    Decode(Sink),
    Dsf { out: PathBuf, via: Via, chunk: usize },
}

#[derive(Debug)]
struct Args {
    image: PathBuf,
    slot: Slot,
    threads: usize,
    command: Command,
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
    let dsf = match args.next().as_deref() {
        Some("--version") => return Ok(None),
        Some("decode") => false,
        Some("dsf") => true,
        _ => return Err(USAGE.into()),
    };
    let image = PathBuf::from(args.next().ok_or(USAGE)?);
    let (mut slot, mut threads, mut sink) = (None, None, None);
    let (mut out, mut via, mut chunk) = (None, Via::Stream, DSF_CHUNK);
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
                    other => return Err(format!("--via must be stream or read-at, not {other}")),
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
            other => return Err(format!("unknown argument {other}\n{USAGE}")),
        }
    }
    let command = match (dsf, sink, out) {
        (false, Some(sink), None) => Command::Decode(sink),
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
fn decode_area(path: &Path, slot: Slot, out: &mut dyn Output) -> Result<(), Failure> {
    let file = File::open(path)
        .map_err(|e| Failure { class: "io-error", message: format!("{}: {e}", path.display()) })?;
    let mut disc = Disc::open(BufReader::with_capacity(1 << 20, file))?;
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
            None => disc.track_audio_cursor(slot, n)?,
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

/// Writes the area's play range as one DSF file (see the module documentation).
fn write_dsf(path: &Path, slot: Slot, out: &Path, via: Via, chunk: usize) -> Result<(), Failure> {
    let file = File::open(path)
        .map_err(|e| Failure { class: "io-error", message: format!("{}: {e}", path.display()) })?;
    let mut disc = Disc::open(BufReader::with_capacity(1 << 20, file))?;
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
        Command::Dsf { out, via, chunk } => write_dsf(&args.image, args.slot, out, *via, *chunk),
        Command::Decode(Sink::Null) => decode_area(&args.image, args.slot, &mut NullOut),
        Command::Decode(Sink::Hash) => {
            let stdout = std::io::stdout().lock();
            let mut out =
                HashOut { out: BufWriter::new(stdout), number: 0, hashers: Vec::new(), chan: Vec::new() };
            decode_area(&args.image, args.slot, &mut out)?;
            out.out.flush()?;
            Ok(())
        }
        Command::Decode(Sink::Raw(dir)) => {
            std::fs::create_dir_all(dir)?;
            decode_area(
                &args.image,
                args.slot,
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
        assert!(matches!(a.command, Command::Decode(Sink::Raw(ref d))  if d == Path::new("out")));
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
