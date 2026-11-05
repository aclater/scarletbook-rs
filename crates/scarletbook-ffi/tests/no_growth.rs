// SPDX-License-Identifier: MIT OR Apache-2.0

//! The heap does not grow over repeated open → seek → decode → close cycles (#238), through
//! the Rust API and the C interface, on a DST and a plain DSD corpus disc.
//!
//! A counting global allocator, which is why this is a test binary of its own, keeps the
//! bytes live. After a few warm-up cycles (one-time allocations: the thread's last error
//! message, the DST decoder's tables and so on) the count must reach a plateau: the same value
//! after many cycles in a row. LeakSanitizer (CI's `lsan` job) finds memory nothing points to;
//! this finds memory still reachable that keeps growing, such as a cache or a list without a
//! bound. The hour-long soak is in the scheduled workflow.
//!
//! The plateau may start late, as not every one-time allocation happens on the cycles' own
//! thread (#266). The idle pool thread goes to sleep after 33 rounds of `yield_now`, the first
//! use of the `Mutex` and `Condvar` rayon keeps for that. On macOS std boxes the pthread objects
//! of both on first use, 64 and 48 bytes, so that first sleep adds 112 bytes for good, at a
//! time that depends on the scheduler and may fall after the warm-up.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Cursor;
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

use scarletbook::Disc;
use scarletbook_ffi::*;

/// The system allocator, counting the bytes live.
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call is forwarded unchanged to `System`, which upholds `GlobalAlloc`'s
// contract; the counter has no effect on the memory returned.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded contract (`layout` has a non-zero size).
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            LIVE.fetch_add(layout.size(), Ordering::Relaxed);
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded contract.
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            LIVE.fetch_add(layout.size(), Ordering::Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        // SAFETY: forwarded contract (`p` was allocated by `System` with `layout`).
        unsafe { System.dealloc(p, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
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

/// Frames decoded after each seek.
const FRAMES: u32 = 3;

/// Cycles in a row, after the warm-up, over which the bytes live must not move.
const PLATEAU: usize = 20;

/// Cycles allowed to reach the plateau: room for a late one-time allocation, none for a list
/// that keeps growing.
const MAX_CYCLES: usize = 3 * PLATEAU;

fn image(id: &str) -> Vec<u8> {
    let corpus = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus");
    std::fs::read(corpus.join("images").join(format!("{id}.iso"))).unwrap()
}

/// Rust API: opens the disc and, in every area, decodes `FRAMES` frames from the middle of
/// its play time.
fn rust_cycle(bytes: &[u8]) {
    let mut disc = Disc::open(Cursor::new(bytes)).unwrap();
    let areas: Vec<_> = disc.toc().areas.iter().map(|a| (a.slot, a.playtime_frames())).collect();
    for (slot, playtime) in areas {
        let first = playtime / 2;
        let end = (first + FRAMES).min(playtime);
        let mut decoded = 0;
        for frame in disc.range_audio(slot, first, end).unwrap() {
            assert!(!frame.unwrap().data.is_empty());
            decoded += 1;
        }
        assert_eq!(decoded, end - first);
    }
}

/// C interface: the same as `rust_cycle`, with every handle freed.
fn c_cycle(bytes: &[u8]) {
    let mut disc = ptr::null_mut();
    // SAFETY: `bytes` is valid for its length and outlives the disc; `&mut disc` is a valid
    // out-pointer.
    assert_eq!(unsafe { sb_disc_open_memory(bytes.as_ptr(), bytes.len(), &mut disc) }, SbStatus::Ok);
    let mut info = SbDiscInfo::default();
    // SAFETY: a live handle and a valid out-pointer.
    assert_eq!(unsafe { sb_disc_info(disc, &mut info) }, SbStatus::Ok);
    for i in 0..info.area_count {
        let mut area = SbAreaInfo::default();
        // SAFETY: a live handle and a valid out-pointer.
        assert_eq!(unsafe { sb_disc_area(disc, i, &mut area) }, SbStatus::Ok);
        let first = area.playtime_frames / 2;
        let end = (first + FRAMES).min(area.playtime_frames);
        let mut audio = ptr::null_mut();
        // SAFETY: a live handle and a valid out-pointer.
        assert_eq!(unsafe { sb_range_audio_open(disc, area.slot, first, end, &mut audio) }, SbStatus::Ok);
        // SAFETY: a live handle.
        let mut buf = vec![0u8; unsafe { sb_track_audio_channels(audio) } as usize * 4704];
        let mut frame = SbFrameInfo::default();
        let mut decoded = 0;
        loop {
            // SAFETY: live handles, the audio opened on this disc; `buf` is valid for its
            // length and `frame` for a write.
            match unsafe { sb_track_audio_read(audio, disc, buf.as_mut_ptr(), buf.len(), &mut frame) } {
                SbStatus::Ok => decoded += 1,
                SbStatus::End => break,
                s => panic!("{s:?}"),
            }
        }
        assert_eq!(decoded, end - first);
        // SAFETY: a live handle, not used again.
        unsafe { sb_track_audio_free(audio) };
    }
    // SAFETY: a live handle, not used again.
    unsafe { sb_disc_free(disc) };
}

/// Miri reports any leak at the end of every test of this crate on its own, and this one's
/// decoding is far too slow for it; the `lsan` CI job runs it as a leak check too.
#[test]
#[cfg_attr(miri, ignore = "Miri checks every other test of this crate for leaks itself")]
fn the_heap_does_not_grow_over_open_seek_decode_close_cycles() {
    let images = [image("dst-features-2ch-6ch"), image("dsd-stereo-mch-2048")];
    let cycle = || {
        for bytes in &images {
            rust_cycle(bytes);
            c_cycle(bytes);
        }
    };
    // Each thread that decodes keeps its decoder's state (about 320 KiB for DST), so the
    // number of threads must not change between cycles: one pool thread, warmed up with the
    // rest.
    rayon::ThreadPoolBuilder::new().num_threads(1).build_global().unwrap();
    for _ in 0..3 {
        cycle();
    }
    if let Err(after) = plateau(cycle) {
        panic!("no plateau of {PLATEAU} cycles in {MAX_CYCLES}; bytes live after each cycle: {after:?}");
    }
    the_plateau_check_works();
}

/// Runs `cycle` until the bytes live have not moved over `PLATEAU` cycles in a row and returns
/// how many cycles that took, or, if `MAX_CYCLES` were not enough, the count after each.
fn plateau(mut cycle: impl FnMut()) -> Result<usize, Vec<usize>> {
    // Allocated before the count is read: its own growth would count.
    let mut after = Vec::with_capacity(MAX_CYCLES);
    let mut level = LIVE.load(Ordering::Relaxed);
    let mut flat = 0;
    for n in 1..=MAX_CYCLES {
        cycle();
        let live = LIVE.load(Ordering::Relaxed);
        after.push(live);
        flat = if live == level { flat + 1 } else { 0 };
        level = live;
        if flat == PLATEAU {
            return Ok(n);
        }
    }
    Err(after)
}

/// The check itself: a list that grows on every cycle, however little, never passes, and one
/// late step passes once the plateau after it is complete.
fn the_plateau_check_works() {
    let mut list = Vec::new();
    let after = plateau(|| list.push(Box::new(0u8))).unwrap_err();
    assert_eq!(after.len(), MAX_CYCLES);
    drop(list);

    let mut step = None;
    let mut cycles = 0;
    let taken = plateau(|| {
        cycles += 1;
        if cycles == PLATEAU {
            step = Some(vec![0u8; 112]);
        }
    });
    // The pool's thread may still allocate during this (see the module documentation), which
    // can only make it take longer.
    assert!(matches!(taken, Ok(n) if n >= 2 * PLATEAU), "{taken:?}");
    drop(step);
}
