// SPDX-License-Identifier: MIT OR Apache-2.0

//! Seek indexes through the C ABI (issue #68), driven from Rust as a C caller would.
//!
//! Each unsafe call sits in a small wrapper that states why its arguments meet the
//! function's contract; the tests themselves are safe code.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::ffi::{CStr, c_char, c_void};
use std::path::PathBuf;
use std::ptr;
use std::sync::Mutex;

use scarletbook_ffi::*;

fn image(id: &str) -> Vec<u8> {
    let corpus = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus");
    std::fs::read(corpus.join("images").join(format!("{id}.iso"))).unwrap()
}

/// A disc with one 2-channel area of `sectors` audio sectors (each one packet past its
/// end, so malformed) from LSN 546, play time 7 frames: small enough for Miri.
fn small_image(sectors: usize) -> Vec<u8> {
    let s = 2048;
    let mut img = vec![0u8; (546 + sectors) * s];
    let m = 510 * s;
    img[m..m + 8].copy_from_slice(b"SACDMTOC");
    img[m + 8] = 1;
    img[m + 24..m + 29].copy_from_slice(b"CAT-1");
    img[m + 64..m + 68].copy_from_slice(&540u32.to_be_bytes());
    img[m + 84..m + 86].copy_from_slice(&6u16.to_be_bytes());
    img[m + 9 * s..m + 9 * s + 8].copy_from_slice(b"SACD_Man");
    let a = 540 * s;
    img[a..a + 8].copy_from_slice(b"TWOCHTOC");
    img[a + 32] = 2;
    img[a + 66] = 7;
    img[a + 69] = 1;
    img[a + 72..a + 76].copy_from_slice(&546u32.to_be_bytes());
    img[a + 76..a + 80].copy_from_slice(&(546 + sectors as u32 - 1).to_be_bytes());
    img[a + s..a + s + 8].copy_from_slice(b"SACDTRL1");
    let t2 = a + 2 * s;
    img[t2..t2 + 8].copy_from_slice(b"SACDTRL2");
    img[t2 + 1028 + 2] = 2;
    for i in 0..sectors {
        let at = (546 + i) * s;
        img[at] = 1 << 5;
        img[at + 1..at + 3].copy_from_slice(&((2u16 << 11) | 2047).to_be_bytes());
    }
    img
}

fn last_error() -> String {
    // SAFETY: a valid NUL-terminated string, copied before this thread's next call.
    unsafe { CStr::from_ptr(sb_last_error_message()) }.to_string_lossy().into_owned()
}

fn open(bytes: &[u8]) -> *mut SbDisc {
    let mut d = ptr::null_mut();
    // SAFETY: `bytes` is valid for its length; `&mut d` is a valid out-pointer.
    assert_eq!(unsafe { sb_disc_open_memory(bytes.as_ptr(), bytes.len(), &mut d) }, SbStatus::Ok);
    d
}

fn free(disc: *mut SbDisc) {
    // SAFETY: a live handle that the caller does not use again.
    unsafe { sb_disc_free(disc) };
}

fn playtime(disc: *const SbDisc, slot: u32) -> u32 {
    let mut a = SbAreaInfo::default();
    // SAFETY: a live handle and a valid out-pointer.
    assert_eq!(unsafe { sb_disc_area(disc, slot, &mut a) }, SbStatus::Ok);
    a.playtime_frames
}

fn index_new(disc: *const SbDisc, slot: u32, interval: u32) -> Result<*mut SbSeekIndex, SbStatus> {
    let mut i: *mut SbSeekIndex = ptr::dangling_mut();
    // SAFETY: `disc` is NULL or a live handle; `&mut i` is a valid out-pointer.
    match unsafe { sb_seek_index_new(disc, slot, interval, SB_SEEK_DEFAULT_INLINE_BUDGET, &mut i) } {
        SbStatus::Ok => Ok(i),
        s => {
            assert!(i.is_null(), "*out not cleared on failure");
            Err(s)
        }
    }
}

fn index_free(index: *mut SbSeekIndex) {
    // SAFETY: NULL or a live handle that the caller does not use again.
    unsafe { sb_seek_index_free(index) };
}

fn extend(disc: *const SbDisc, index: *mut SbSeekIndex, sectors: u64) -> Result<bool, SbStatus> {
    let mut done = false;
    // SAFETY: live handles and a valid out-pointer.
    match unsafe { sb_seek_index_extend(disc, index, sectors, &mut done) } {
        SbStatus::Ok => Ok(done),
        s => Err(s),
    }
}

fn frontier(index: *const SbSeekIndex) -> (u64, bool) {
    let (mut lsn, mut done) = (0, false);
    // SAFETY: a live handle and valid out-pointers.
    assert_eq!(unsafe { sb_seek_index_frontier(index, &mut lsn, &mut done) }, SbStatus::Ok);
    (lsn, done)
}

fn save(index: *const SbSeekIndex) -> Vec<u8> {
    let mut len = 0;
    // SAFETY: a live handle; a NULL buffer asks for the size; `&mut len` is valid.
    assert_eq!(unsafe { sb_seek_index_save(index, ptr::null_mut(), 0, &mut len) }, SbStatus::BufferTooSmall);
    let mut buf = vec![0u8; len];
    // SAFETY: `buf` is valid for `len` bytes of writes.
    assert_eq!(unsafe { sb_seek_index_save(index, buf.as_mut_ptr(), buf.len(), &mut len) }, SbStatus::Ok);
    assert_eq!(len, buf.len());
    buf
}

fn load(disc: *const SbDisc, slot: u32, bytes: &[u8]) -> Result<*mut SbSeekIndex, SbStatus> {
    let mut i: *mut SbSeekIndex = ptr::dangling_mut();
    // SAFETY: a live handle; `bytes` valid for its length; `&mut i` a valid out-pointer.
    match unsafe {
        sb_seek_index_load(disc, slot, bytes.as_ptr(), bytes.len(), SB_SEEK_DEFAULT_INLINE_BUDGET, &mut i)
    } {
        SbStatus::Ok => Ok(i),
        s => {
            assert!(i.is_null(), "*out not cleared on failure");
            Err(s)
        }
    }
}

fn open_indexed(
    disc: *const SbDisc,
    slot: u32,
    (first, end): (u32, u32),
    index: *const SbSeekIndex,
) -> Result<*mut SbTrackAudio, SbStatus> {
    let mut a: *mut SbTrackAudio = ptr::dangling_mut();
    // SAFETY: live handles (or a NULL index); `&mut a` is a valid out-pointer.
    match unsafe { sb_range_audio_open_indexed(disc, slot, first, end, index, &mut a) } {
        SbStatus::Ok => Ok(a),
        s => {
            assert!(a.is_null(), "*out not cleared on failure");
            Err(s)
        }
    }
}

fn open_next_indexed(
    disc: *const SbDisc,
    prev: *mut SbTrackAudio,
    slot: u32,
    (first, end): (u32, u32),
    index: *mut SbSeekIndex,
) -> Result<*mut SbTrackAudio, SbStatus> {
    let mut a: *mut SbTrackAudio = ptr::dangling_mut();
    // SAFETY: live handles; `prev` is not used again if the call succeeds; `&mut a` is a
    // valid out-pointer.
    match unsafe { sb_range_audio_open_next_indexed(disc, prev, slot, first, end, index, &mut a) } {
        SbStatus::Ok => Ok(a),
        s => {
            assert!(a.is_null(), "*out not cleared on failure");
            Err(s)
        }
    }
}

fn absorb(audio: *const SbTrackAudio, index: *mut SbSeekIndex) -> SbStatus {
    // SAFETY: live handles.
    unsafe { sb_track_audio_absorb(audio, index) }
}

fn audio_free(audio: *mut SbTrackAudio) {
    // SAFETY: a live handle that the caller does not use again.
    unsafe { sb_track_audio_free(audio) };
}

/// The rest of a handle: each frame's number, status and bytes, then the counts.
type Read = (Vec<(u32, u32, Vec<u8>)>, u64, u64, u64);

fn read_all(a: *const SbTrackAudio, disc: *const SbDisc) -> Read {
    // SAFETY: a live handle.
    let channels = unsafe { sb_track_audio_channels(a) } as usize;
    let mut buf = vec![0u8; channels * 4704];
    let mut frames = Vec::new();
    loop {
        let mut fi = SbFrameInfo::default();
        // SAFETY: live handles; `buf` and `fi` are valid.
        match unsafe { sb_track_audio_read(a, disc, buf.as_mut_ptr(), buf.len(), &mut fi) } {
            SbStatus::End => break,
            SbStatus::Ok => frames.push((fi.number, fi.status, buf[..fi.size].to_vec())),
            s => panic!("{s:?}: {}", last_error()),
        }
    }
    let (mut missing, mut invalid, mut malformed) = (0, 0, 0);
    // SAFETY: a live handle and valid out-pointers.
    assert_eq!(unsafe { sb_track_audio_counts(a, &mut missing, &mut invalid) }, SbStatus::Ok);
    // SAFETY: as above.
    assert_eq!(unsafe { sb_track_audio_malformed_sectors(a, &mut malformed) }, SbStatus::Ok);
    (frames, missing, invalid, malformed)
}

/// Ranges read through an index (fresh and continued, in any order) equal ranges read
/// without, on well-formed and hostile corpus images; the index survives save and load.
#[test]
#[cfg_attr(miri, ignore = "reads corpus images")]
fn indexed_reads_equal_plain_reads() {
    for id in ["dst-features-2ch-6ch", "audio-random", "audio-dst-endless-frame", "audio-packets-overrun"] {
        let bytes = image(id);
        let disc = open(&bytes);
        for slot in [0, 1] {
            let Ok(index) = index_new(disc, slot, 1) else { continue };
            let p = playtime(disc, slot);
            let ranges: Vec<(u32, u32)> =
                [(p * 3 / 4, p), (0, p), (p / 2, p / 2 + 1), (p / 3, p), (p - 1, p)]
                    .into_iter()
                    .filter(|r| r.0 < r.1)
                    .collect();
            let mut prev: *mut SbTrackAudio = ptr::null_mut();
            for &r in &ranges {
                let plain = open_indexed(disc, slot, r, ptr::null()).unwrap();
                let want = read_all(plain, disc);
                audio_free(plain);
                let fresh = open_indexed(disc, slot, r, index).unwrap();
                assert_eq!(read_all(fresh, disc), want, "{id} slot {slot} {r:?}");
                assert_eq!(absorb(fresh, index), SbStatus::Ok, "{}", last_error());
                audio_free(fresh);
                let next = open_next_indexed(disc, prev, slot, r, index).unwrap();
                assert_eq!(read_all(next, disc), want, "{id} slot {slot} {r:?} continued");
                prev = next;
            }
            assert_eq!(absorb(prev, index), SbStatus::Ok);
            audio_free(prev);
            while !extend(disc, index, 100).unwrap() {}
            assert!(frontier(index).1);
            // Saved and loaded, it reads alike.
            let saved = save(index);
            let loaded = load(disc, slot, &saved).unwrap();
            assert_eq!(save(loaded), saved);
            for &r in &ranges {
                let plain = open_indexed(disc, slot, r, ptr::null()).unwrap();
                let a = open_indexed(disc, slot, r, loaded).unwrap();
                assert_eq!(read_all(a, disc), read_all(plain, disc));
                audio_free(a);
                audio_free(plain);
            }
            index_free(loaded);
            index_free(index);
        }
        free(disc);
    }
}

/// Small enough for Miri: arguments, size queries, merging and refusals.
#[test]
fn index_arguments_and_refusals() {
    let bytes = small_image(5);
    let disc = open(&bytes);
    assert_eq!(index_new(ptr::null(), 0, 64).unwrap_err(), SbStatus::InvalidArgument);
    assert_eq!(index_new(disc, 7, 64).unwrap_err(), SbStatus::InvalidArgument);
    assert_eq!(index_new(disc, 0, 0).unwrap_err(), SbStatus::InvalidArgument);
    assert_eq!(index_new(disc, 0, SB_SEEK_MAX_INTERVAL + 1).unwrap_err(), SbStatus::InvalidArgument);
    // SAFETY: a NULL out-pointer is refused.
    assert_eq!(unsafe { sb_seek_index_new(disc, 0, 64, 0, ptr::null_mut()) }, SbStatus::InvalidArgument);
    let a = index_new(disc, 0, 2).unwrap();
    let b = index_new(disc, 0, 2).unwrap();
    let mut bytes_held = 0;
    // SAFETY: a live handle and a valid out-pointer.
    assert_eq!(unsafe { sb_seek_index_memory(a, &mut bytes_held) }, SbStatus::Ok);
    assert!(bytes_held > 0);
    assert!(!extend(disc, b, 1).unwrap() || frontier(b).1);
    while !extend(disc, b, 1).unwrap() {}
    // SAFETY: live handles.
    assert_eq!(unsafe { sb_seek_index_merge(a, b) }, SbStatus::Ok);
    // SAFETY: a live handle; merging into itself does nothing.
    assert_eq!(unsafe { sb_seek_index_merge(a, a) }, SbStatus::Ok);
    // SAFETY: NULL is refused.
    assert_eq!(unsafe { sb_seek_index_merge(ptr::null_mut(), b) }, SbStatus::InvalidArgument);
    assert_eq!(save(a), save(b));
    let other = index_new(disc, 0, 3).unwrap();
    // SAFETY: live handles.
    assert_eq!(unsafe { sb_seek_index_merge(a, other) }, SbStatus::InvalidArgument);
    // A small buffer: the size, and nothing written.
    let saved = save(a);
    let mut small = vec![0xAAu8; saved.len() - 1];
    let mut len = 0;
    assert_eq!(
        // SAFETY: `small` is valid for its length.
        unsafe { sb_seek_index_save(a, small.as_mut_ptr(), small.len(), &mut len) },
        SbStatus::BufferTooSmall
    );
    assert_eq!(len, saved.len());
    assert!(small.iter().all(|&x| x == 0xAA));
    // Loading: garbage, a truncation, another slot.
    assert_eq!(load(disc, 0, b"not an index").unwrap_err(), SbStatus::InvalidArgument);
    assert_eq!(load(disc, 0, &saved[..saved.len() - 1]).unwrap_err(), SbStatus::InvalidArgument);
    assert_eq!(load(disc, 1, &saved).unwrap_err(), SbStatus::NoSuchArea);
    assert_eq!(index_new(disc, 1, 2).unwrap_err(), SbStatus::NoSuchArea);
    let mut i: *mut SbSeekIndex = ptr::dangling_mut();
    assert_eq!(
        // SAFETY: a NULL buffer is refused, and the out-pointer cleared.
        unsafe { sb_seek_index_load(disc, 0, ptr::null(), 0, SB_SEEK_DEFAULT_INLINE_BUDGET, &mut i) },
        SbStatus::InvalidArgument
    );
    assert!(i.is_null());
    // An index of another image size is refused by the openers and by loading.
    let longer = small_image(6);
    let other_disc = open(&longer);
    let mch = index_new(other_disc, 0, 2).unwrap();
    assert_eq!(load(other_disc, 0, &saved).unwrap_err(), SbStatus::InvalidArgument);
    assert_eq!(open_indexed(disc, 0, (0, 1), mch).unwrap_err(), SbStatus::InvalidArgument);
    let prev = open_indexed(disc, 0, (0, 1), a).unwrap();
    assert_eq!(open_next_indexed(disc, prev, 0, (0, 2), mch).unwrap_err(), SbStatus::InvalidArgument);
    // `prev` was left untouched.
    let next = open_next_indexed(disc, prev, 0, (0, 2), a).unwrap();
    // A raw frames handle with an index.
    let mut raw: *mut SbRawFrames = ptr::dangling_mut();
    // SAFETY: live handles and a valid out-pointer.
    assert_eq!(unsafe { sb_raw_frames_open_indexed(disc, 0, 1, 2, a, &mut raw) }, SbStatus::Ok);
    // SAFETY: live handles.
    assert_eq!(unsafe { sb_raw_frames_absorb(raw, a) }, SbStatus::Ok);
    let mut raw2: *mut SbRawFrames = ptr::dangling_mut();
    // SAFETY: live handles; `raw` is consumed on success.
    assert_eq!(unsafe { sb_raw_frames_open_next_indexed(disc, raw, 0, 2, 3, a, &mut raw2) }, SbStatus::Ok);
    // SAFETY: live handles, freed once.
    unsafe { sb_raw_frames_free(raw2) };
    audio_free(next);
    for i in [a, b, other, mch] {
        index_free(i);
    }
    index_free(ptr::null_mut());
    free(other_disc);
    free(disc);
}

/// Log lines of a disc, and a seek index for the callback to reach.
#[derive(Default)]
struct LogUser {
    lines: Mutex<Vec<String>>,
    index: std::sync::atomic::AtomicPtr<SbSeekIndex>,
    victim: std::sync::atomic::AtomicPtr<SbTrackAudio>,
    probes: Mutex<Vec<SbStatus>>,
}

unsafe extern "C" fn cb_log(user: *mut c_void, _level: i32, message: *const c_char) {
    // SAFETY: `user` is the LogUser given with the callback, alive while the disc is.
    let user = unsafe { &*user.cast::<LogUser>() };
    // SAFETY: the library passes a valid NUL-terminated string for the call.
    let m = unsafe { CStr::from_ptr(message) }.to_string_lossy().into_owned();
    user.lines.lock().unwrap().push(m);
    let index = user.index.swap(ptr::null_mut(), std::sync::atomic::Ordering::Relaxed);
    if !index.is_null() {
        let mut lsn = 0;
        // SAFETY: `index` is live; the library must refuse it while in use, and ignore
        // its free.
        let s = unsafe { sb_seek_index_frontier(index, &mut lsn, ptr::null_mut()) };
        user.probes.lock().unwrap().push(s);
        // SAFETY: as above.
        unsafe { sb_seek_index_free(index) };
    }
    let victim = user.victim.swap(ptr::null_mut(), std::sync::atomic::Ordering::Relaxed);
    if !victim.is_null() {
        // SAFETY: `victim` is the `prev` handle of the call that invoked this callback;
        // the library must refuse it while in use, and ignore its free.
        let s = unsafe { sb_track_audio_absorb(victim, ptr::null_mut()) };
        user.probes.lock().unwrap().push(s);
        // SAFETY: as above.
        unsafe { sb_track_audio_free(victim) };
    }
}

/// While a call uses an index or consumes `prev`, a log callback cannot reach them.
#[test]
#[cfg_attr(miri, ignore = "reads a corpus image")]
fn log_callbacks_cannot_reach_handles_in_use() {
    let bytes = image("audio-random");
    let user = LogUser::default();
    let mut disc = ptr::null_mut();
    let u = ptr::from_ref(&user).cast_mut().cast::<c_void>();
    // SAFETY: `bytes` is valid; `cb_log` may be called with `u` while `user` lives, which
    // outlives the disc.
    let s = unsafe {
        sb_disc_open_memory_with_log(bytes.as_ptr(), bytes.len(), Some(cb_log), u, SB_LOG_TRACE, &mut disc)
    };
    assert_eq!(s, SbStatus::Ok);
    let index = index_new(disc, 0, 1).unwrap();
    // Extending logs (the resume and the scan) while the index is in use.
    user.index.store(index, std::sync::atomic::Ordering::Relaxed);
    while !extend(disc, index, 4).unwrap() {}
    assert_eq!(*user.probes.lock().unwrap(), [SbStatus::InvalidArgument]);
    // Opening with `prev` logs while `prev` is borrowed, and nothing after it is taken.
    let prev = open_indexed(disc, 0, (0, 1), index).unwrap();
    user.victim.store(prev, std::sync::atomic::Ordering::Relaxed);
    user.index.store(index, std::sync::atomic::Ordering::Relaxed);
    let next = open_next_indexed(disc, prev, 0, (5, 7), index).unwrap();
    let probes = user.probes.lock().unwrap().clone();
    assert_eq!(probes, [SbStatus::InvalidArgument; 3], "{probes:?}");
    assert!(
        user.lines
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("from the seek index") || l.contains("continuing"))
    );
    read_all(next, disc);
    audio_free(next);
    index_free(index);
    free(disc);
}

fn clone(disc: *const SbDisc) -> *mut SbDisc {
    let mut c = ptr::null_mut();
    // SAFETY: a live handle and a valid out-pointer.
    assert_eq!(unsafe { sb_disc_clone(disc, &mut c) }, SbStatus::Ok, "{}", last_error());
    c
}

fn range_open(disc: *const SbDisc, slot: u32, first: u32, end: u32) -> *mut SbTrackAudio {
    let mut a = ptr::null_mut();
    // SAFETY: a live handle and a valid out-pointer.
    let s = unsafe { sb_range_audio_open(disc, slot, first, end, &mut a) };
    assert_eq!(s, SbStatus::Ok, "{}", last_error());
    a
}

/// A disc and its clone keep one seek index: reads through the clone, on another thread,
/// build it; the disc's later reads start from it (logged), and every read equals a read
/// of a disc without an index.
#[test]
#[cfg_attr(miri, ignore = "reads a corpus image")]
fn a_clone_shares_the_discs_index() {
    let bytes = image("dst-features-2ch-6ch");
    let plain = open(&bytes);
    let user = LogUser::default();
    let mut disc = ptr::null_mut();
    let u = ptr::from_ref(&user).cast_mut().cast::<c_void>();
    // SAFETY: `bytes` is valid; `cb_log` may be called with `u` while `user` lives, which
    // outlives the disc.
    let s = unsafe {
        sb_disc_open_memory_with_log(bytes.as_ptr(), bytes.len(), Some(cb_log), u, SB_LOG_DEBUG, &mut disc)
    };
    assert_eq!(s, SbStatus::Ok);
    // SAFETY: a live handle.
    assert_eq!(unsafe { sb_disc_seek_index_enable(disc, 0, 1, SB_SEEK_DEFAULT_INLINE_BUDGET) }, SbStatus::Ok);
    let mut none = ptr::null_mut();
    // SAFETY: a live handle and a valid out-pointer.
    assert_eq!(unsafe { sb_disc_seek_index_get(disc, 1, &mut none) }, SbStatus::Absent, "none kept for mc");
    assert!(none.is_null());
    let p = playtime(disc, 0);
    // The clone reads the whole area on another thread, which builds the index.
    let c = clone(disc).expose_provenance();
    let whole = std::thread::spawn(move || {
        let c: *mut SbDisc = ptr::with_exposed_provenance_mut(c);
        let a = range_open(c, 0, 0, p);
        let got = read_all(a, c);
        audio_free(a);
        free(c);
        got
    })
    .join()
    .unwrap();
    let a = range_open(plain, 0, 0, p);
    assert_eq!(whole, read_all(a, plain));
    audio_free(a);
    // The disc sees the complete index its clone built.
    let mut kept = ptr::null_mut();
    // SAFETY: a live handle and a valid out-pointer.
    assert_eq!(unsafe { sb_disc_seek_index_get(disc, 0, &mut kept) }, SbStatus::Ok);
    assert!(frontier(kept).1, "complete");
    // Its seeks start from it, and read as without.
    user.lines.lock().unwrap().clear();
    for first in [p / 2, p - 1, p / 3] {
        let a = range_open(disc, 0, first, p);
        let b = range_open(plain, 0, first, p);
        assert_eq!(read_all(a, disc), read_all(b, plain), "first {first}");
        audio_free(a);
        audio_free(b);
    }
    let lines = user.lines.lock().unwrap().clone();
    assert!(lines.iter().any(|l| l.contains("from the seek index")), "{lines:?}");
    // Setting, disabling, and the copies being copies.
    // SAFETY: live handles.
    unsafe {
        assert_eq!(sb_disc_seek_index_disable(disc, 0), SbStatus::Ok);
    }
    let mut gone = ptr::dangling_mut();
    // SAFETY: a live handle and a valid out-pointer.
    assert_eq!(unsafe { sb_disc_seek_index_get(disc, 0, &mut gone) }, SbStatus::Absent);
    assert!(gone.is_null());
    // SAFETY: live handles.
    assert_eq!(unsafe { sb_disc_seek_index_set(disc, 0, kept) }, SbStatus::Ok);
    let mut done = false;
    // SAFETY: a live handle and a valid out-pointer.
    assert_eq!(unsafe { sb_disc_seek_index_extend(disc, 0, 10, &mut done) }, SbStatus::Ok);
    assert!(done);
    index_free(kept);
    free(disc);
    free(plain);
}
