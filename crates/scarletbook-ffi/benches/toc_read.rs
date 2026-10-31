// SPDX-License-Identifier: MIT OR Apache-2.0

//! Opening a disc and reading its whole TOC, as a consumer that only lists discs does
//! (issue #132).
//!
//! - `c/IMAGE`: `sb_disc_open_callbacks_at` with a positional read callback over the image
//!   in memory, then every TOC query of the C interface: the disc information, strings,
//!   stored catalogue numbers, genres, text channels, manufacturer sector and warnings;
//!   for each area its information, details, loudspeaker layout and channel speakers,
//!   strings, text channels and stored play time; for each track its information, strings
//!   and stored timecodes. Then `sb_disc_free`.
//! - `c-bulk/IMAGE`: the same open, then the whole TOC in one `sb_disc_toc` call, as the
//!   README recommends (a 64 KiB buffer reused from disc to disc, a second call if it is too
//!   small), and a walk over the layout's areas, tracks and string refs.
//! - `rust/IMAGE`: the same through the Rust API, `Disc::open` over a `ReadAtReader`.
//!
//! One iteration is one disc, so criterion's time is the time per disc. The images are
//! the corpus images with the most text and tracks, and a synthetic image of 2 areas of
//! 255 tracks (`scarletbook-synth`), each with a title.
//!
//! `cargo bench -p scarletbook-ffi --bench toc_read -- c/` filters as usual. Setting
//! `SCARLETBOOK_PERF_BENCH=GROUP/ID` (or `list`) skips criterion: the program reads that
//! disc `SCARLETBOOK_PERF_DISCS` times (default 20 000), for `perf`. `list` also prints,
//! for each image, the calls one disc makes, the read callbacks and bytes of its open, and
//! the size of its `sb_disc_toc` layout.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::cell::Cell;
use std::ffi::{c_char, c_void};
use std::hint::black_box;
use std::io::Cursor;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use criterion::Criterion;
use scarletbook::toc::{AreaTextItem, DiscTextItem, TrackTextItem};
use scarletbook::{Disc, ReadAtReader, SectorFormat};
use scarletbook_conformance::corpus_dir;
use scarletbook_conformance::synth::{Config, DstSource, Kind, write_image};
use scarletbook_ffi::*;

/// The corpus images read: the most text, invalid text, and the most tracks.
const CORPUS: &[&str] = &[
    "full-text",
    "text-sjis-garbage",
    "text-items-255-unterminated",
    "dst-features-2ch-6ch",
    "area-tracks-255",
];

/// An image in memory, with counters of the reads made through the callback.
struct Image {
    name: String,
    bytes: Vec<u8>,
    reads: AtomicU64,
    read_bytes: AtomicU64,
}

/// The positional read callback: copies from the image's bytes.
///
/// # Safety
///
/// `user` must point to a live `Image`; `buf` must be valid for `len` bytes of writes.
unsafe extern "C" fn read_at(user: *mut c_void, offset: u64, buf: *mut u8, len: usize) -> i64 {
    // SAFETY: `user` is the `Image` passed to sb_disc_open_callbacks_at, alive for the disc.
    let image = unsafe { &*user.cast::<Image>() };
    image.reads.fetch_add(1, Ordering::Relaxed);
    let Some(rest) = usize::try_from(offset).ok().and_then(|o| image.bytes.get(o..)) else { return 0 };
    let n = rest.len().min(len);
    image.read_bytes.fetch_add(n as u64, Ordering::Relaxed);
    // SAFETY: `buf` is valid for `len >= n` bytes of writes; the regions are distinct.
    unsafe { ptr::copy_nonoverlapping(rest.as_ptr(), buf, n) };
    n as i64
}

/// Reads a string field with a buffer large enough for any TOC string.
fn string(buf: &mut [u8], f: impl FnOnce(*mut c_char, usize, *mut usize) -> SbStatus) -> usize {
    let mut len = 0usize;
    let status = f(buf.as_mut_ptr().cast(), buf.len(), &mut len);
    assert!(matches!(status, SbStatus::Ok | SbStatus::Absent), "{status:?}");
    len
}

/// Opens `image` through the C interface and makes every TOC query; returns the number of
/// calls made (open and free included).
fn read_c(image: &Image) -> u32 {
    let mut buf = [0u8; 4096];
    let calls = Cell::new(0u32);
    let mut sum = 0usize;
    let mut disc: *mut SbDisc = ptr::null_mut();
    let user = ptr::from_ref(image).cast_mut().cast::<c_void>();
    // Every call gets the live handle (or, for the open, `read_at`, which is safe to call with
    // `user`, a live `Image`, for the disc's lifetime), valid pointers to locals, and buffers
    // of the length passed.
    macro_rules! c {
        ($call:expr) => {{
            // SAFETY: as stated above, for each call of this function.
            unsafe { $call }
        }};
    }
    let call = |s: SbStatus| {
        assert!(matches!(s, SbStatus::Ok | SbStatus::Absent), "{s:?}");
        calls.set(calls.get() + 1);
    };
    call(c!(sb_disc_open_callbacks_at(Some(read_at), image.bytes.len() as u64, user, &mut disc)));
    let mut info = SbDiscInfo::default();
    call(c!(sb_disc_info(disc, &mut info)));
    for field in 0..DiscTextItem::ALL.len() as u32 + 2 {
        sum += string(&mut buf, |b, c, l| c!(sb_disc_string(disc, field, b, c, l)));
        calls.set(calls.get() + 1);
    }
    for field in [SbDiscString::AlbumCatalogNumber as u32, SbDiscString::DiscCatalogNumber as u32] {
        sum += string(&mut buf, |b, c, l| c!(sb_disc_catalog_number_stored(disc, field, b, c, l)));
        calls.set(calls.get() + 1);
    }
    let mut genre = SbGenre::default();
    for list in [SB_GENRE_LIST_ALBUM, SB_GENRE_LIST_DISC] {
        for i in 0..4 {
            call(c!(sb_disc_genre(disc, list, i, &mut genre)));
            sum += usize::from(genre.index);
        }
    }
    let mut channels = 0u32;
    call(c!(sb_disc_text_channel_count(disc, &mut channels)));
    let mut channel = SbTextChannel::default();
    for i in 0..channels.min(8) {
        call(c!(sb_disc_text_channel(disc, i, &mut channel)));
    }
    if info.manufacturer_sector != 0 {
        sum += string(&mut buf, |b, c, l| c!(sb_disc_manufacturer(disc, b.cast(), c, l)));
        calls.set(calls.get() + 1);
    }
    let mut warning = SbWarning { lsn: 0, slot: 0, track: 0, code: ptr::null() };
    for i in 0..info.warning_count {
        call(c!(sb_disc_warning(disc, i, &mut warning)));
    }
    for index in 0..info.area_count {
        let mut area = SbAreaInfo::default();
        call(c!(sb_disc_area(disc, index, &mut area)));
        let slot = area.slot;
        let mut details = SbAreaDetails::default();
        call(c!(sb_area_details(disc, slot, &mut details)));
        let mut layout = 0u32;
        call(c!(sb_area_loudspeaker_layout(disc, slot, &mut layout)));
        for i in 0..area.channel_count {
            call(c!(sb_area_channel_speaker(disc, slot, i, &mut layout)));
        }
        for field in 0..AreaTextItem::ALL.len() as u32 {
            sum += string(&mut buf, |b, c, l| c!(sb_area_string(disc, slot, field, b, c, l)));
            calls.set(calls.get() + 1);
        }
        for i in 0..u32::from(details.text_channel_count.min(8)) {
            call(c!(sb_area_text_channel(disc, slot, i, &mut channel)));
        }
        let mut playtime = SbTimecode::default();
        call(c!(sb_area_playtime_stored(disc, slot, &mut playtime)));
        for number in 1..=area.track_count {
            let mut track = SbTrackInfo::default();
            call(c!(sb_track_info(disc, slot, number, &mut track)));
            for field in 0..TrackTextItem::ALL.len() as u32 + 1 {
                sum += string(&mut buf, |b, c, l| c!(sb_track_string(disc, slot, number, field, b, c, l)));
                calls.set(calls.get() + 1);
            }
            let (mut start, mut duration) = (SbTimecode::default(), SbTimecode::default());
            call(c!(sb_track_timecodes_stored(disc, slot, number, &mut start, &mut duration)));
        }
    }
    c!(sb_disc_free(disc));
    black_box(sum);
    calls.get() + 1
}

/// Opens `image` through the C interface and reads its whole TOC with one `sb_disc_toc`
/// call, as the README recommends: into `buf` (64 KiB, reused from disc to disc), again
/// with the size given if that is too small. Then walks the layout as a consumer does: every
/// area, every track and every string ref. Returns the layout's size.
fn read_c_bulk(image: &Image, buf: &mut Vec<u8>) -> usize {
    let mut disc: *mut SbDisc = ptr::null_mut();
    let user = ptr::from_ref(image).cast_mut().cast::<c_void>();
    // SAFETY: `read_at` is safe to call with `user`, a live `Image`, for the disc's lifetime;
    // `disc` is a valid out-pointer.
    let s = unsafe { sb_disc_open_callbacks_at(Some(read_at), image.bytes.len() as u64, user, &mut disc) };
    assert_eq!(s, SbStatus::Ok);
    let mut len = 0usize;
    // SAFETY: `disc` is a live handle; `buf` is valid for `buf.len()` bytes; `len` for a write.
    let mut s = unsafe { sb_disc_toc(disc, 0, buf.as_mut_ptr(), buf.len(), &mut len) };
    if s == SbStatus::BufferTooSmall {
        buf.resize(len, 0);
        // SAFETY: as above.
        s = unsafe { sb_disc_toc(disc, 0, buf.as_mut_ptr(), buf.len(), &mut len) };
    }
    assert_eq!(s, SbStatus::Ok);
    // SAFETY: `disc` is a live handle, not used again.
    unsafe { sb_disc_free(disc) };

    let b = &buf[..len];
    let u32_at = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize;
    let str_len = |at: usize| if u32_at(at) == SB_TOC_ABSENT as usize { 0 } else { u32_at(at + 4) };
    let disc_at = u32_at(16);
    let mut sum = (0..21).map(|i| str_len(disc_at + 88 + 8 * i)).sum::<usize>();
    for i in 0..u32_at(32) {
        let a = u32_at(28) + 152 * i;
        sum += (0..4).map(|k| str_len(a + 80 + 8 * k)).sum::<usize>();
        for t in 0..u32_at(a + 48) {
            let t = u32_at(a + 76) + 168 * t;
            sum += u32_at(t + 4) + (0..15).map(|k| str_len(t + 36 + 8 * k)).sum::<usize>();
        }
    }
    black_box(sum);
    len
}

/// Opens `image` through the Rust API and reads what `read_c` does.
fn read_rust(image: &Arc<[u8]>) {
    let disc = Disc::open(ReadAtReader::new(Arc::clone(image))).unwrap();
    let toc = disc.toc();
    let m = &toc.master;
    let mut sum = m.album_catalog_number.len()
        + m.disc_catalog_number.len()
        + m.album_catalog_number_stored.len()
        + m.disc_catalog_number_stored.len()
        + m.locales.len()
        + m.manufacturer.as_ref().map_or(0, Vec::len)
        + disc.warnings().len();
    sum += m.album_genres.iter().chain(&m.disc_genres).map(|g| usize::from(g.index)).sum::<usize>();
    sum += DiscTextItem::ALL.iter().filter_map(|&i| m.text.get(i)).map(str::len).sum::<usize>();
    for area in &toc.areas {
        sum += usize::from(matches!(area.loudspeaker_layout(), scarletbook::LoudspeakerLayout::Stereo))
            + area.channel_speakers().map_or(0, |s| s.len());
        sum += AreaTextItem::ALL.iter().filter_map(|&i| area.text.get(i)).map(str::len).sum::<usize>();
        sum += area.locales.len() + usize::from(area.playtime.frames);
        for track in &area.tracks {
            sum += track.isrc.as_ref().map_or(0, String::len) + track.start_lsn as usize;
            sum += TrackTextItem::ALL.iter().filter_map(|&i| track.text.get(i)).map(str::len).sum::<usize>();
            let (start, duration) = disc.track_timecodes_stored(area.slot, u32::from(track.number)).unwrap();
            sum += usize::from(start.frames + duration.frames);
        }
    }
    black_box(sum);
}

/// 2-channel and multichannel areas of 255 one-frame DSD tracks, each with a title.
fn synthetic() -> Vec<u8> {
    let cfg = Config {
        frames: 255,
        two_channel: true,
        multichannel: true,
        mc_channels: 6,
        kind: Kind::Dsd,
        format: SectorFormat::Plain,
        track_frames: 1,
    };
    let mut out = Cursor::new(Vec::new());
    write_image(&mut out, &cfg, &DstSource::default()).unwrap();
    out.into_inner()
}

fn images() -> Vec<Image> {
    let dir = corpus_dir().join("images");
    let mut v: Vec<(String, Vec<u8>)> = CORPUS
        .iter()
        .map(|id| (id.to_string(), std::fs::read(dir.join(format!("{id}.iso"))).unwrap()))
        .collect();
    v.push(("synth-255-tracks".into(), synthetic()));
    v.into_iter()
        .map(|(name, bytes)| Image { name, bytes, reads: AtomicU64::new(0), read_bytes: AtomicU64::new(0) })
        .collect()
}

fn main() {
    let images = images();

    if let Ok(name) = std::env::var("SCARLETBOOK_PERF_BENCH") {
        if name == "list" {
            let mut buf = vec![0u8; 64 * 1024];
            for image in &images {
                let layout = read_c_bulk(image, &mut buf);
                image.reads.store(0, Ordering::Relaxed);
                image.read_bytes.store(0, Ordering::Relaxed);
                let calls = read_c(image);
                println!(
                    "{}\t{calls} calls\t{} read callbacks\t{} bytes read\t{layout} bytes of sb_disc_toc",
                    image.name,
                    image.reads.load(Ordering::Relaxed),
                    image.read_bytes.load(Ordering::Relaxed)
                );
            }
            return;
        }
        let (group, id) = name.split_once('/').unwrap_or_else(|| panic!("no benchmark {name}"));
        let image = images.iter().find(|i| i.name == id).unwrap_or_else(|| panic!("no benchmark {name}"));
        let count = std::env::var("SCARLETBOOK_PERF_DISCS").map_or(20_000, |v| v.parse().expect("a number"));
        let shared: Arc<[u8]> = image.bytes.clone().into();
        let mut buf = vec![0u8; 64 * 1024];
        for _ in 0..count {
            match group {
                "c" => {
                    read_c(image);
                }
                "c-bulk" => {
                    read_c_bulk(image, &mut buf);
                }
                "rust" => read_rust(&shared),
                _ => panic!("no benchmark {name}"),
            }
        }
        println!("{count}");
        return;
    }

    let mut c = Criterion::default().configure_from_args();
    let mut g = c.benchmark_group("c");
    for image in &images {
        g.bench_function(&image.name, |b| b.iter(|| read_c(image)));
    }
    g.finish();
    let mut g = c.benchmark_group("c-bulk");
    for image in &images {
        let mut buf = vec![0u8; 64 * 1024];
        g.bench_function(&image.name, |b| b.iter(|| read_c_bulk(image, &mut buf)));
    }
    g.finish();
    let mut g = c.benchmark_group("rust");
    for image in &images {
        let shared: Arc<[u8]> = image.bytes.clone().into();
        g.bench_function(&image.name, |b| b.iter(|| read_rust(&shared)));
    }
    g.finish();
    c.final_summary();
}
