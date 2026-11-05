// SPDX-License-Identifier: MIT OR Apache-2.0

//! The heap does not grow over repeated open → seek → decode → close cycles (#238), through
//! the Rust API and the C interface, on a DST and a plain DSD corpus disc.
//!
//! A counting global allocator, which is why this is a test binary of its own, keeps the
//! bytes live. After a few warm-up cycles (one-time allocations: the thread's last error
//! message, the DST decoder's tables and so on) the count must come back to the same value
//! after every cycle. LeakSanitizer (CI's `lsan` job) finds memory nothing points to; this
//! finds memory still reachable that keeps growing, such as a cache or a list without a
//! bound. The hour-long soak is in the scheduled workflow.
//!
//! There is one exception, on macOS only (#266). The idle pool thread goes to sleep after 33
//! rounds of `yield_now`, the first use of the `Mutex` and `Condvar` rayon keeps for that. On
//! macOS std boxes the pthread objects of both on first use, 64 and 48 bytes, so that first
//! sleep adds up to 112 bytes for good, at a time that depends on the scheduler and may fall
//! after the warm-up: see [`FIRST_SLEEP`]. Waiting for it would not be a guarantee, as nothing
//! shows that the thread has slept. The other platforms stay exact, and code that grows does
//! so there as well.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashMap;
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

/// Cycles checked after the warm-up.
const CYCLES: usize = 20;

/// Rise over the settled count that the pool thread's first sleep can explain on macOS: none
/// yet, the `Condvar` alone (the settled count has the `Mutex`), the `Mutex` alone (the
/// `Condvar` is about to follow) or both.
const FIRST_SLEEP: [usize; 4] = [0, 48, 64, 112];

/// The rises allowed on this platform.
const ALLOWED: &[usize] = if cfg!(target_os = "macos") { &FIRST_SLEEP } else { &[0] };

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
    let (settled, after) = counts(cycle);
    assert!(
        holds(settled, &after, ALLOWED),
        "bytes live after warm-up: {settled}; after each further cycle: {after:?}"
    );
    the_check_works();
}

/// The bytes live before `CYCLES` runs of `cycle`, and after each.
fn counts(mut cycle: impl FnMut()) -> (usize, Vec<usize>) {
    // Allocated before the count is read: its own growth would count.
    let mut after = Vec::with_capacity(CYCLES);
    let settled = LIVE.load(Ordering::Relaxed);
    for _ in 0..CYCLES {
        cycle();
        after.push(LIVE.load(Ordering::Relaxed));
    }
    (settled, after)
}

/// Whether every count is `settled` plus one of the `allowed` rises, and none falls back: one
/// late allocation stays.
fn holds(settled: usize, after: &[usize], allowed: &[usize]) -> bool {
    let mut last = 0;
    after.iter().all(|&n| {
        let Some(rise) = n.checked_sub(settled) else { return false };
        let ok = rise >= last && allowed.contains(&rise);
        last = rise;
        ok
    })
}

/// The check itself, with macOS's allowance, the looser one, on every platform.
///
/// Collections are the cases that matter: unlike a list of boxes they allocate only when they
/// double, so the gaps between allocations widen while they keep growing, and a check that
/// waits for a flat stretch would pass them. Their contents are in place before the count is
/// read, as they would be after the warm-up.
fn the_check_works() {
    for per_cycle in [1u64, 2, 4] {
        let mut list: Vec<u64> = (0..3 * per_cycle).collect();
        let (settled, after) = counts(|| list.extend(0..per_cycle));
        assert!(!holds(settled, &after, &FIRST_SLEEP), "Vec<u64>, {per_cycle} a cycle: {after:?}");
        drop(list);

        let mut map: HashMap<u64, u64> = (0..3 * per_cycle).map(|k| (k, k)).collect();
        let mut next = 3 * per_cycle;
        let (settled, after) = counts(|| {
            map.extend((next..next + per_cycle).map(|k| (k, k)));
            next += per_cycle;
        });
        assert!(!holds(settled, &after, &FIRST_SLEEP), "HashMap, {per_cycle} a cycle: {after:?}");
        drop(map);
    }

    let mut boxes = Vec::with_capacity(CYCLES);
    let (settled, after) = counts(|| boxes.push(Box::new(0u8)));
    assert!(!holds(settled, &after, &FIRST_SLEEP), "boxes: {after:?}");
    drop(boxes);

    // From here on no real counts: the pool's thread may still be allocating, and only these
    // cases would mind.
    assert!(holds(100, &[100, 100], &[0]));
    assert!(!holds(100, &[100, 212], &[0]));
    assert!(holds(100, &[100, 212, 212], &FIRST_SLEEP));
    assert!(holds(100, &[100, 164, 212, 212], &FIRST_SLEEP));
    assert!(holds(100, &[148, 148], &FIRST_SLEEP));
    assert!(!holds(100, &[100, 113], &FIRST_SLEEP));
    assert!(!holds(100, &[212, 100], &FIRST_SLEEP));
    assert!(!holds(100, &[212, 324], &FIRST_SLEEP));
    assert!(!holds(100, &[99], &FIRST_SLEEP));
}
