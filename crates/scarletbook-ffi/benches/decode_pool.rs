// SPDX-License-Identifier: MIT OR Apache-2.0

//! Decoding a whole area's DST audio on a pool, the way a consumer of the C interface does
//! it, against the Rust API (issue #146). Not a criterion benchmark: one run decodes every
//! track of an area once (seconds on a 50-minute image), and the program prints its wall
//! time and the process's CPU time.
//!
//! ```text
//! cargo bench -p scarletbook-ffi --bench decode_pool -- IMAGE MODE [--area 2ch|mc] [--threads N] [--rounds R] [--sink null|raw DIR]
//! ```
//!
//! Every mode decodes every track of the area in order, continuing one cursor or handle from
//! track to track, into a sink on the calling thread: `null` (the default; `black_box`), or
//! `raw DIR`, the output of `bench decode --sink raw` (one file per track and channel,
//! `TT-C.dsd`, through 1 MiB buffers), so that a consumer's work per frame is part of the
//! time. Modes:
//!
//! - `rust-pool`: `TrackAudio` with `DecodeOn::Pool(pool)`, read on the calling thread,
//!   which is not one of the pool's;
//! - `rust-install`: `TrackAudio` inside `pool.install`, as `bench decode` does: the calling
//!   thread is one of the pool's and runs other decoding jobs while it waits;
//! - `c-read`: `sb_disc_open_file`, `sb_pool_new`, then per track `sb_track_audio_open_next`
//!   and `sb_track_audio_set_pool`, and `sb_track_audio_read` into one caller buffer per
//!   frame;
//! - `c-stored`: `c-read` with `sb_track_audio_keep_stored` and
//!   `sb_track_audio_read_with_stored` (decoded and stored content of every frame).
//!
//! `--threads 0` (the default) is one thread per logical CPU, as `bench decode --threads 0`.
//! `--rounds R` (default 3) repeats the run; each round prints a line, then the median.
//! CPU time is user plus system time of the whole process, from `/proc/self/stat` (Linux;
//! elsewhere it prints `-`). Without arguments (as under `cargo bench`), it prints the usage
//! and exits.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::ffi::CString;
use std::fs::File;
use std::hint::black_box;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use scarletbook::audio::FRAME_BYTES_PER_CHANNEL;
use scarletbook::{DecodeOn, Disc, Slot};
use scarletbook_ffi::*;

const USAGE: &str = "usage: decode_pool IMAGE rust-pool|rust-install|c-read|c-stored [--area 2ch|mc] [--threads N] \
                     [--rounds R] [--sink null|raw DIR]";

/// Where the frames go, on the calling thread.
enum Sink {
    Null,
    /// Per-channel files of the current track in `dir`, as `bench decode --sink raw`.
    Raw {
        dir: PathBuf,
        files: Vec<BufWriter<File>>,
        chan: Vec<u8>,
    },
}

impl Sink {
    fn start_track(&mut self, number: u32, channels: usize) {
        if let Sink::Raw { dir, files, .. } = self {
            *files = (0..channels)
                .map(|c| {
                    BufWriter::with_capacity(
                        1 << 20,
                        File::create(dir.join(format!("{number:02}-{c}.dsd"))).unwrap(),
                    )
                })
                .collect();
        }
    }

    fn frame(&mut self, data: &[u8]) {
        match self {
            Sink::Null => {
                black_box(data);
            }
            Sink::Raw { files, chan, .. } => {
                let n = files.len();
                for (c, f) in files.iter_mut().enumerate() {
                    chan.clear();
                    chan.extend(data.chunks_exact(n).map(|x| x[c]));
                    f.write_all(chan).unwrap();
                }
            }
        }
    }

    fn end_track(&mut self) {
        if let Sink::Raw { files, .. } = self {
            for mut f in files.drain(..) {
                f.flush().unwrap();
            }
        }
    }
}

/// Process CPU time (user + system), if the system reports it.
fn cpu_time() -> Option<Duration> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // The fields after the command name, which is in parentheses and may contain spaces.
    let rest = &stat[stat.rfind(')')? + 2..];
    let fields: Vec<&str> = rest.split(' ').collect();
    // utime and stime are fields 14 and 15 of the line, 12 and 13 after the state field.
    let ticks: u64 = fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?;
    // USER_HZ is 100 on Linux.
    Some(Duration::from_millis(ticks * 10))
}

/// Decodes every track of `slot` with the Rust API; returns the frames decoded.
fn rust(image: &Path, slot: Slot, pool: &Arc<rayon::ThreadPool>, install: bool, sink: &mut Sink) -> u64 {
    let mut run = || {
        let file = File::open(image).unwrap();
        let mut disc = Disc::open(BufReader::with_capacity(1 << 20, file)).unwrap();
        let numbers: Vec<u32> =
            disc.toc().area(slot).unwrap().tracks.iter().map(|t| u32::from(t.number)).collect();
        let mut frames = 0u64;
        let mut cursor = None;
        for n in numbers {
            if disc.check_track(slot, n).is_err() {
                continue;
            }
            let mut next = match cursor.take() {
                Some(prev) => disc.continue_track_audio(prev, slot, n).unwrap(),
                None => disc.track_audio_cursor(slot, n).unwrap(),
            };
            if !install {
                next.set_decode_on(DecodeOn::Pool(Arc::clone(pool)));
            }
            let mut track = disc.read_track_audio(next);
            sink.start_track(n, usize::from(track.channels()));
            for frame in track.by_ref() {
                sink.frame(&frame.unwrap().data);
                frames += 1;
            }
            sink.end_track();
            cursor = Some(track.into_cursor());
        }
        frames
    };
    if install { pool.install(run) } else { run() }
}

/// Decodes every track of `slot` through the C interface on a pool of `threads`; returns the
/// frames decoded.
fn c(image: &Path, slot: Slot, threads: u32, stored: bool, sink: &mut Sink) -> u64 {
    let slot = match slot {
        Slot::TwoChannel => 0,
        _ => 1,
    };
    let path = CString::new(image.to_str().unwrap()).unwrap();
    let mut disc: *mut SbDisc = ptr::null_mut();
    let mut pool: *mut SbPool = ptr::null_mut();
    // SAFETY: `path` is NUL-terminated; the out-pointers are valid.
    unsafe {
        assert_eq!(sb_disc_open_file(path.as_ptr(), &mut disc), SbStatus::Ok);
    }
    // SAFETY: the out-pointer is valid.
    unsafe {
        assert_eq!(sb_pool_new(threads, &mut pool), SbStatus::Ok);
    }
    let mut area = SbAreaInfo::default();
    let mut index = 0;
    loop {
        // SAFETY: `disc` is a live handle; `&mut area` is valid.
        assert_eq!(unsafe { sb_disc_area(disc, index, &mut area) }, SbStatus::Ok);
        if area.slot == slot {
            break;
        }
        index += 1;
    }
    let mut buf = vec![0u8; area.channel_count as usize * FRAME_BYTES_PER_CHANNEL];
    let mut raw = vec![0u8; SB_MAX_FRAME_BYTES + 1];
    let (mut info, mut raw_info) = (SbFrameInfo::default(), SbRawFrameInfo::default());
    let mut audio: *mut SbTrackAudio = ptr::null_mut();
    let mut frames = 0u64;
    for n in 1..=area.track_count {
        // SAFETY: `disc` is a live handle.
        if unsafe { sb_track_check(disc, slot, n) } != SbStatus::Ok {
            continue;
        }
        let mut next: *mut SbTrackAudio = ptr::null_mut();
        // SAFETY: `disc` is live; `audio` is NULL or a live handle, consumed on success.
        assert_eq!(unsafe { sb_track_audio_open_next(disc, audio, slot, n, &mut next) }, SbStatus::Ok);
        audio = next;
        // SAFETY: `audio` and `pool` are live handles.
        assert_eq!(unsafe { sb_track_audio_set_pool(audio, pool) }, SbStatus::Ok);
        if stored {
            // SAFETY: `audio` is a live handle.
            assert_eq!(unsafe { sb_track_audio_keep_stored(audio) }, SbStatus::Ok);
        }
        sink.start_track(n, area.channel_count as usize);
        loop {
            let s = if stored {
                // SAFETY: live handles; the buffers are valid for the capacities passed.
                unsafe {
                    sb_track_audio_read_with_stored(
                        audio,
                        disc,
                        buf.as_mut_ptr(),
                        buf.len(),
                        &mut info,
                        raw.as_mut_ptr(),
                        raw.len(),
                        &mut raw_info,
                        0,
                    )
                }
            } else {
                // SAFETY: live handles; `buf` is valid for its length.
                unsafe { sb_track_audio_read(audio, disc, buf.as_mut_ptr(), buf.len(), &mut info) }
            };
            match s {
                SbStatus::Ok => {
                    sink.frame(&buf[..info.size as usize]);
                    frames += 1;
                }
                SbStatus::End => break,
                s => panic!("{s:?}"),
            }
        }
        sink.end_track();
    }
    // SAFETY: live handles, not used again.
    unsafe {
        sb_track_audio_free(audio);
    }
    // SAFETY: as above.
    unsafe {
        sb_pool_free(pool);
    }
    // SAFETY: as above.
    unsafe {
        sb_disc_free(disc);
    }
    frames
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).filter(|a| a != "--bench").collect();
    let [image, mode, opts @ ..] = args.as_slice() else {
        println!("{USAGE}");
        return;
    };
    let (mut slot, mut threads, mut rounds) = (Slot::TwoChannel, 0usize, 3usize);
    let mut sink = Sink::Null;
    let mut opts = opts.iter();
    while let Some(o) = opts.next() {
        let v = opts.next().unwrap_or_else(|| panic!("{o} needs a value\n{USAGE}"));
        match o.as_str() {
            "--area" => slot = if v == "mc" { Slot::Multichannel } else { Slot::TwoChannel },
            "--threads" => threads = v.parse().expect("--threads N"),
            "--rounds" => rounds = v.parse().expect("--rounds R"),
            "--sink" if v == "null" => sink = Sink::Null,
            "--sink" if v == "raw" => {
                let dir =
                    PathBuf::from(opts.next().unwrap_or_else(|| panic!("--sink raw needs DIR\n{USAGE}")));
                std::fs::create_dir_all(&dir).unwrap();
                sink = Sink::Raw { dir, files: Vec::new(), chan: Vec::new() };
            }
            _ => panic!("{USAGE}"),
        }
    }
    let threads = match threads {
        0 => std::thread::available_parallelism().map_or(1, usize::from),
        n => n,
    };
    let image = Path::new(image);
    let pool = Arc::new(rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap());
    let mut walls = Vec::new();
    let mut cpus = Vec::new();
    for round in 1..=rounds {
        let (start, cpu0) = (Instant::now(), cpu_time());
        let frames = match mode.as_str() {
            "rust-pool" => rust(image, slot, &pool, false, &mut sink),
            "rust-install" => rust(image, slot, &pool, true, &mut sink),
            "c-read" => c(image, slot, threads as u32, false, &mut sink),
            "c-stored" => c(image, slot, threads as u32, true, &mut sink),
            _ => panic!("{USAGE}"),
        };
        let wall = start.elapsed();
        let cpu = cpu_time().zip(cpu0).map(|(b, a)| b - a);
        println!(
            "{mode} round {round}: {frames} frames, {:.3} s wall, {} CPU, {threads} threads",
            wall.as_secs_f64(),
            cpu.map_or("-".into(), |c| format!("{:.2} s", c.as_secs_f64()))
        );
        walls.push(wall);
        cpus.extend(cpu);
    }
    walls.sort();
    cpus.sort();
    println!(
        "{mode} median: {:.3} s wall, {} CPU",
        walls[walls.len() / 2].as_secs_f64(),
        cpus.get(cpus.len() / 2).map_or("-".into(), |c| format!("{:.2} s", c.as_secs_f64()))
    );
}
