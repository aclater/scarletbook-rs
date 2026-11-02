// SPDX-License-Identifier: MIT OR Apache-2.0

//! A long-running memory soak (#238): `soak [--minutes M] [--sample-secs S] [--csv FILE]
//! [--leak N] [IMAGE...]`.
//!
//! Until M minutes (default 60) have passed, it repeats one cycle over the images (default:
//! a set of corpus discs, DST and plain DSD, valid and damaged). For each image, it goes
//! through the Rust API and then through the C interface:
//! - opens the disc, from the file;
//! - in every area, enables the seek index, seeks to a pseudo-random position and decodes
//!   up to 16 frames, which are converted to PCM;
//! - lays out the range as a DSF and a DSDIFF (plain DSD) file and reads 64 KiB of each at
//!   a pseudo-random offset (the C side decodes on a pool of its own, made per image);
//! - closes everything.
//!
//! Errors from damaged images are expected and only counted. Every S seconds (default 60)
//! it prints one CSV row: elapsed seconds, cycles, resident memory (`/proc/self/statm`,
//! Linux) and the heap's bytes live (a counting global allocator), sampled between cycles.
//!
//! The verdict is automatic. The first quarter of the samples (at least one) is warm-up;
//! the rest is split into an earlier and a later half. Memory has plateaued when the later
//! half's maximum is at most the earlier half's maximum plus a tolerance: 256 KiB plus 2 %
//! of the heap, and 4 MiB plus 2 % of resident memory. Every thread that decodes keeps its
//! decoder's state (about 2.3 MiB with these images), so the heap rises while the global
//! pool's threads first take work, and moves by a few hundred KiB with what each kept last;
//! the resident memory moves with the allocator's fragmentation too. It prints
//! `verdict: plateau` and exits 0, or `verdict: growth` with the figures and exits 1. A
//! steady leak shows as a later half above the earlier one, however small the leak, given
//! enough cycles. `--leak N` leaks N bytes per cycle on purpose, to check the check. Run it
//! in release mode: `cargo run --release -p scarletbook-ffi --example
//! soak -- --minutes 60`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::alloc::{GlobalAlloc, Layout as AllocLayout, System};
use std::ffi::CString;
use std::fs::File;
use std::io::{BufReader, Write};
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use scarletbook::Disc;
use scarletbook::output::{Format, OutputOptions};
use scarletbook::seek::IndexOptions;
use scarletbook_ffi::*;
use scarletbook_pcm::{Converter, Cutoff, FRAME_BYTES_PER_CHANNEL, FrameRun, PcmConfig, PcmRate};

/// The system allocator, counting the bytes live.
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call is forwarded unchanged to `System`, which upholds `GlobalAlloc`'s
// contract; the counter has no effect on the memory returned.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: AllocLayout) -> *mut u8 {
        // SAFETY: forwarded contract (`layout` has a non-zero size).
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            LIVE.fetch_add(layout.size(), Ordering::Relaxed);
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: AllocLayout) -> *mut u8 {
        // SAFETY: forwarded contract.
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            LIVE.fetch_add(layout.size(), Ordering::Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: AllocLayout) {
        // SAFETY: forwarded contract (`p` was allocated by `System` with `layout`).
        unsafe { System.dealloc(p, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, p: *mut u8, layout: AllocLayout, new_size: usize) -> *mut u8 {
        // SAFETY: forwarded contract.
        let q = unsafe { System.realloc(p, layout, new_size) };
        if !q.is_null() {
            LIVE.fetch_add(new_size, Ordering::Relaxed);
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        }
        q
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// The corpus discs of a cycle by default: DST and plain DSD, stereo and multichannel,
/// and damaged ones for the error paths.
const IMAGES: [&str; 8] = [
    "dst-features-2ch-6ch",
    "dst-stereo-5ch-2064",
    "dst-invalid-frames-6ch",
    "dst-truncated-frames",
    "dsd-stereo-mch-2048",
    "dsd-stereo-mch-2054",
    "truncated-in-audio",
    "hostile-base",
];

/// Frames decoded after each seek.
const FRAMES: u32 = 16;

/// Bytes read from each output layout.
const READ: usize = 64 * 1024;

/// A small deterministic generator (xorshift64) for the seek positions and offsets.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        if n == 0 { 0 } else { self.0 % n }
    }
}

/// Converts `frames` (whole frames of `channels` channels) to 176.4 kHz PCM.
fn to_pcm(channels: u8, frames: &[u8]) {
    let Some(rate) = PcmRate::from_hz(176_400) else { return };
    let Ok(mut conv) = Converter::new(PcmConfig::new(rate, Cutoff::Default, channels)) else { return };
    let mut out = vec![0i32; frames.len() / FRAME_BYTES_PER_CHANNEL * conv.samples_per_frame()];
    let _ = conv.convert_run(FrameRun::new(frames), &mut out);
}

/// One image through the Rust API; the number of errors met.
fn rust_cycle(path: &PathBuf, rng: &mut Rng) -> u64 {
    let mut errors = 0;
    let Ok(file) = File::open(path) else { return 1 };
    let Ok(mut disc) = Disc::open(BufReader::new(file)) else { return 1 };
    let areas: Vec<_> =
        disc.toc().areas.iter().map(|a| (a.slot, a.channel_count, a.playtime_frames())).collect();
    for (slot, channels, playtime) in areas {
        if disc.enable_seek_index(slot, IndexOptions::new()).is_err() {
            errors += 1;
        }
        let first = rng.below(u64::from(playtime)) as u32;
        let end = first.saturating_add(FRAMES).min(playtime);
        let mut data = Vec::new();
        match disc.range_audio(slot, first, end) {
            Ok(frames) => {
                for frame in frames {
                    match frame {
                        Ok(f) => data.extend_from_slice(&f.data),
                        Err(_) => errors += 1,
                    }
                }
            }
            Err(_) => errors += 1,
        }
        to_pcm(channels, &data);
        for format in [Format::Dsf, Format::DsdiffDsd] {
            let layout = match disc.output_layout(slot, first, end, format, &OutputOptions::default()) {
                Ok(l) => l,
                Err(_) => {
                    errors += 1;
                    continue;
                }
            };
            let mut buf = vec![0u8; READ];
            let offset = rng.below(layout.size());
            if layout.read_at(&mut disc, offset, &mut buf).is_err() {
                errors += 1;
            }
        }
    }
    errors
}

/// One image through the C interface, every handle freed; the number of errors met.
fn c_cycle(path: &CString, rng: &mut Rng) -> u64 {
    let mut errors = 0;
    let mut disc = ptr::null_mut();
    // SAFETY: `path` is a NUL-terminated string; `&mut disc` is a valid out-pointer.
    if unsafe { sb_disc_open_file(path.as_ptr(), &mut disc) } != SbStatus::Ok {
        return 1;
    }
    let mut pool = ptr::null_mut();
    // SAFETY: `&mut pool` is a valid out-pointer.
    if unsafe { sb_pool_new(2, &mut pool) } != SbStatus::Ok {
        errors += 1;
    }
    let mut info = SbDiscInfo::default();
    // SAFETY: a live handle and a valid out-pointer.
    if unsafe { sb_disc_info(disc, &mut info) } != SbStatus::Ok {
        errors += 1;
    }
    for i in 0..info.area_count {
        let mut area = SbAreaInfo::default();
        // SAFETY: a live handle and a valid out-pointer.
        if unsafe { sb_disc_area(disc, i, &mut area) } != SbStatus::Ok {
            errors += 1;
            continue;
        }
        let slot = area.slot;
        // SAFETY: a live handle.
        let s = unsafe {
            sb_disc_seek_index_enable(disc, slot, SB_SEEK_DEFAULT_INTERVAL, SB_SEEK_DEFAULT_INLINE_BUDGET)
        };
        if s != SbStatus::Ok {
            errors += 1;
        }
        let playtime = area.playtime_frames;
        let first = rng.below(u64::from(playtime)) as u32;
        let end = first.saturating_add(FRAMES).min(playtime);
        let mut data = Vec::new();
        let mut audio = ptr::null_mut();
        // SAFETY: a live handle and a valid out-pointer.
        if unsafe { sb_range_audio_open(disc, slot, first, end, &mut audio) } == SbStatus::Ok {
            // SAFETY: live handles (`pool` may be NULL).
            if !pool.is_null() && unsafe { sb_track_audio_set_pool(audio, pool) } != SbStatus::Ok {
                errors += 1;
            }
            // SAFETY: a live handle.
            let mut buf = vec![0u8; unsafe { sb_track_audio_channels(audio) } as usize * 4704];
            let mut frame = SbFrameInfo::default();
            loop {
                // SAFETY: live handles, the audio opened on this disc; `buf` is valid for its
                // length and `frame` for a write.
                match unsafe { sb_track_audio_read(audio, disc, buf.as_mut_ptr(), buf.len(), &mut frame) } {
                    SbStatus::Ok => data.extend_from_slice(&buf[..(frame.size as usize).min(buf.len())]),
                    SbStatus::End => break,
                    _ => {
                        errors += 1;
                        break;
                    }
                }
            }
            // SAFETY: a live handle, not used again.
            unsafe { sb_track_audio_free(audio) };
        } else {
            errors += 1;
        }
        let channels = area.channel_count;
        let mut conv = ptr::null_mut();
        // SAFETY: `&mut conv` is a valid out-pointer.
        if unsafe { sb_pcm_converter_new(176_400, SB_PCM_CUTOFF_DEFAULT, channels, 0.0, &mut conv) }
            == SbStatus::Ok
        {
            let frame = channels as usize * FRAME_BYTES_PER_CHANNEL;
            let mut per_channel = 0u32;
            // SAFETY: a live handle; `per_channel` is valid for a write.
            unsafe { sb_pcm_samples_per_frame(conv, &mut per_channel) };
            let whole = data.len() / frame.max(1) * frame;
            let mut out = vec![0i32; whole / FRAME_BYTES_PER_CHANNEL * per_channel as usize];
            let mut clipped = [0u64; 6];
            // SAFETY: a live handle; no neighbours; `data` holds `whole` bytes of whole frames;
            // `out` is valid for its length in bytes; `clipped` holds 6 counts.
            let s = unsafe {
                sb_pcm_convert_run(
                    conv,
                    ptr::null(),
                    data.as_ptr(),
                    whole,
                    ptr::null(),
                    SB_PCM_S32,
                    out.as_mut_ptr().cast(),
                    out.len() * 4,
                    clipped.as_mut_ptr(),
                )
            };
            if s != SbStatus::Ok {
                errors += 1;
            }
            // SAFETY: a live handle, not used again.
            unsafe { sb_pcm_converter_free(conv) };
        } else {
            errors += 1;
        }
        let mut options = ptr::null_mut();
        // SAFETY: `&mut options` is a valid out-pointer.
        if unsafe { sb_output_options_new(&mut options) } != SbStatus::Ok {
            errors += 1;
            continue;
        }
        for format in [SbOutputFormat::Dsf, SbOutputFormat::DsdiffDsd] {
            let mut layout = ptr::null_mut();
            // SAFETY: live handles and a valid out-pointer.
            if unsafe { sb_output_layout_new(disc, slot, first, end, format as u32, options, &mut layout) }
                != SbStatus::Ok
            {
                errors += 1;
                continue;
            }
            let mut li = SbOutputLayoutInfo::default();
            // SAFETY: a live handle and a valid out-pointer.
            unsafe { sb_output_layout_info(layout, &mut li) };
            let mut buf = vec![0u8; READ];
            let mut written = 0usize;
            let offset = rng.below(li.size);
            // SAFETY: live handles; `buf` is valid for its length; `written` for a write.
            if unsafe {
                sb_output_layout_read(layout, disc, offset, buf.as_mut_ptr(), buf.len(), &mut written)
            } != SbStatus::Ok
            {
                errors += 1;
            }
            // SAFETY: a live handle, not used again.
            unsafe { sb_output_layout_free(layout) };
        }
        // SAFETY: a live handle, not used again.
        unsafe { sb_output_options_free(options) };
    }
    // SAFETY: live handles (or NULL), not used again.
    unsafe {
        sb_disc_free(disc);
    }
    // SAFETY: as above.
    unsafe { sb_pool_free(pool) };
    errors
}

/// Resident memory in bytes, where `/proc/self/statm` gives it (Linux, 4 KiB pages).
fn rss() -> u64 {
    std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1).and_then(|p| p.parse::<u64>().ok()))
        .map_or(0, |pages| pages * 4096)
}

/// One sample: elapsed seconds, cycles done, resident bytes, heap bytes live.
type Sample = (u64, u64, u64, u64);

/// Whether the later half of the samples past warm-up stays within tolerance of the
/// earlier half (see the module documentation); a line of figures either way.
fn verdict(samples: &[Sample]) -> (bool, String) {
    let warm = (samples.len() / 4).max(1);
    let rest = samples.get(warm..).unwrap_or(&[]);
    if rest.len() < 2 {
        return (false, format!("{} samples: too few to judge", samples.len()));
    }
    let (early, late) = rest.split_at(rest.len() / 2);
    let max = |s: &[Sample], f: fn(&Sample) -> u64| s.iter().map(f).max().unwrap_or(0);
    let (rss_e, rss_l) = (max(early, |s| s.2), max(late, |s| s.2));
    let (heap_e, heap_l) = (max(early, |s| s.3), max(late, |s| s.3));
    let rss_ok = rss_l <= rss_e + 4 * 1024 * 1024 + rss_e / 50;
    let heap_ok = heap_l <= heap_e + 256 * 1024 + heap_e / 50;
    let line = format!(
        "warm-up {warm} samples; max RSS {rss_e} then {rss_l} bytes ({}); max heap {heap_e} then {heap_l} bytes ({})",
        if rss_ok { "ok" } else { "grew" },
        if heap_ok { "ok" } else { "grew" },
    );
    (rss_ok && heap_ok, line)
}

fn main() {
    let mut minutes = 60.0f64;
    let mut sample_secs = 60u64;
    let mut csv_path = None;
    let mut leak = 0usize;
    let mut images = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--minutes" => minutes = args.next().and_then(|v| v.parse().ok()).expect("--minutes M"),
            "--sample-secs" => {
                sample_secs = args.next().and_then(|v| v.parse().ok()).expect("--sample-secs S")
            }
            "--csv" => csv_path = Some(args.next().expect("--csv FILE")),
            "--leak" => leak = args.next().and_then(|v| v.parse().ok()).expect("--leak N"),
            _ => images.push(PathBuf::from(a)),
        }
    }
    if images.is_empty() {
        let corpus = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/images");
        images = IMAGES.iter().map(|id| corpus.join(format!("{id}.iso"))).collect();
    }
    let c_paths: Vec<CString> =
        images.iter().map(|p| CString::new(p.to_string_lossy().into_owned()).unwrap()).collect();
    let mut csv: Box<dyn Write> = match &csv_path {
        Some(p) => Box::new(File::create(p).expect("the CSV file")),
        None => Box::new(std::io::sink()),
    };
    let header = "elapsed_s,cycles,rss_bytes,heap_bytes";
    println!("{header}");
    writeln!(csv, "{header}").unwrap();
    let start = Instant::now();
    let total = Duration::from_secs_f64(minutes * 60.0);
    let every = Duration::from_secs(sample_secs.max(1));
    let mut next = every;
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let (mut cycles, mut errors) = (0u64, 0u64);
    let mut samples: Vec<Sample> = Vec::with_capacity((total.as_secs() / every.as_secs() + 2) as usize);
    loop {
        for (path, c_path) in images.iter().zip(&c_paths) {
            errors += rust_cycle(path, &mut rng);
            errors += c_cycle(c_path, &mut rng);
        }
        cycles += 1;
        if leak > 0 {
            std::hint::black_box(Box::leak(vec![0u8; leak].into_boxed_slice()));
        }
        let elapsed = start.elapsed();
        if elapsed >= next {
            let s = (elapsed.as_secs(), cycles, rss(), LIVE.load(Ordering::Relaxed) as u64);
            let row = format!("{},{},{},{}", s.0, s.1, s.2, s.3);
            println!("{row}");
            writeln!(csv, "{row}").unwrap();
            csv.flush().unwrap();
            samples.push(s);
            next += every;
        }
        if elapsed >= total {
            break;
        }
    }
    let (ok, line) = verdict(&samples);
    println!("{cycles} cycles over {} images, {errors} errors from damaged images", images.len());
    println!("{line}");
    println!("verdict: {}", if ok { "plateau" } else { "growth" });
    std::process::exit(if ok { 0 } else { 1 });
}
