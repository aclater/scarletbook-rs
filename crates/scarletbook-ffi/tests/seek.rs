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
    let mut done = 0u8;
    // SAFETY: live handles and a valid out-pointer.
    match unsafe { sb_seek_index_extend(disc, index, sectors, &mut done) } {
        SbStatus::Ok => Ok(done != 0),
        s => Err(s),
    }
}

fn frontier(index: *const SbSeekIndex) -> (u64, bool) {
    let (mut lsn, mut done) = (0, 0u8);
    // SAFETY: a live handle and valid out-pointers.
    assert_eq!(unsafe { sb_seek_index_frontier(index, &mut lsn, &mut done) }, SbStatus::Ok);
    (lsn, done != 0)
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
            SbStatus::Ok => frames.push((fi.number, fi.status, buf[..fi.size as usize].to_vec())),
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
    let mut done = 0u8;
    // SAFETY: a live handle and a valid out-pointer.
    assert_eq!(unsafe { sb_disc_seek_index_extend(disc, 0, 10, &mut done) }, SbStatus::Ok);
    assert_eq!(done, 1);
    index_free(kept);
    free(disc);
    free(plain);
}

/// Two images of the same size and layout, different contents (#90): an index built from
/// one refuses the other's handles, reads and indexes. Small enough for Miri.
#[test]
fn an_index_is_of_one_image() {
    let x_bytes = small_image(5);
    let mut y_bytes = x_bytes.clone();
    // Sector 2 of Y's audio: well-formed (no packets) where X's is malformed.
    let at = (546 + 2) * 2048;
    y_bytes[at..at + 2048].fill(0);
    let (x, y) = (open(&x_bytes), open(&y_bytes));
    let index = index_new(x, 0, 1).unwrap();
    assert!(!extend(x, index, 1).unwrap());
    let (lsn, _) = frontier(index);
    // Reads of Y into it: refused, the index unchanged.
    assert_eq!(extend(y, index, 1).unwrap_err(), SbStatus::InvalidArgument);
    assert_eq!(frontier(index).0, lsn);
    // A handle of Y that recorded checkpoints (from an empty index): refused.
    let empty = index_new(y, 0, 1).unwrap();
    let a = open_indexed(y, 0, (0, 7), empty).unwrap();
    read_all(a, y);
    assert_eq!(absorb(a, index), SbStatus::InvalidArgument);
    assert!(last_error().contains("another image"), "{}", last_error());
    assert_eq!(frontier(index).0, lsn);
    audio_free(a);
    // An index built from Y: not merged into X's.
    let of_y = index_new(y, 0, 1).unwrap();
    while !extend(y, of_y, 5).unwrap() {}
    // SAFETY: live handles.
    assert_eq!(unsafe { sb_seek_index_merge(index, of_y) }, SbStatus::InvalidArgument);
    assert_eq!(frontier(index).0, lsn);
    // X's own reads go on; a loaded copy takes the image of its first read.
    while !extend(x, index, 5).unwrap() {}
    let loaded = load(y, 0, &save(index)).unwrap();
    assert!(extend(y, loaded, 5).unwrap());
    assert_eq!(extend(x, loaded, 5).unwrap_err(), SbStatus::InvalidArgument);
    for i in [index, empty, of_y, loaded] {
        index_free(i);
    }
    free(x);
    free(y);
}

fn speculation(audio: *const SbTrackAudio) -> SbSpeculation {
    let mut s = SbSpeculation::default();
    // SAFETY: a live handle and a valid out-pointer.
    assert_eq!(unsafe { sb_track_audio_speculation(audio, &mut s) }, SbStatus::Ok);
    s
}

fn policy(disc: *const SbDisc, slot: u32, policy: u32, gap: u64) -> SbStatus {
    // SAFETY: a live handle.
    unsafe { sb_disc_seek_policy_set(disc, slot, policy, gap) }
}

fn next_event(disc: *const SbDisc, slot: u32) -> Option<SbSpeculation> {
    let mut e = SbSpeculation::default();
    // SAFETY: a live handle and a valid out-pointer.
    match unsafe { sb_disc_seek_event_next(disc, slot, &mut e) } {
        SbStatus::Ok => Some(e),
        SbStatus::End => None,
        s => panic!("{s:?}: {}", last_error()),
    }
}

/// Provisional reading through the C interface (#94; specification 4, 4.2 item 8): only
/// once opted in; the handle reports it; with a seek index kept, it is checked and its
/// verdict returned once; the positions equal a straight read on this well-formed image.
#[test]
#[cfg_attr(miri, ignore = "reads a corpus image")]
fn provisional_reading_is_opt_in_marked_and_checked() {
    let bytes = image("dst-features-2ch-6ch");
    let plain = open(&bytes);
    let disc = open(&bytes);
    let p = playtime(disc, 0);
    let (first, end) = (p / 2, p / 2 + 3);
    // The default: exact.
    let a = range_open(disc, 0, first, end);
    read_all(a, disc);
    let s = speculation(a);
    assert_eq!((s.state, s.claim, s.lsn), (SB_SPECULATION_EXACT, 0, 0));
    audio_free(a);
    // Opted in (any gap), with a seek index kept to check the readings.
    assert_eq!(policy(disc, 0, 7, 0), SbStatus::InvalidArgument);
    assert_eq!(policy(disc, 9, SB_SEEK_POLICY_SPECULATIVE, 0), SbStatus::InvalidArgument);
    assert_eq!(policy(disc, 0, SB_SEEK_POLICY_SPECULATIVE, 0), SbStatus::Ok);
    // SAFETY: a live handle.
    assert_eq!(unsafe { sb_disc_seek_index_enable(disc, 0, 4, SB_SEEK_DEFAULT_INLINE_BUDGET) }, SbStatus::Ok);
    let a = range_open(disc, 0, first, end);
    let got = read_all(a, disc);
    let s = speculation(a);
    assert_eq!(s.state, SB_SPECULATION_PENDING);
    assert!(s.claim > 0 && s.lsn > 0 && s.first == first && s.slot == 0, "{s:?}");
    let b = range_open(plain, 0, first, end);
    assert_eq!(read_all(b, plain).0, got.0);
    audio_free(b);
    assert!(next_event(disc, 0).is_none());
    let mut done = 0u8;
    // SAFETY: a live handle and a valid out-pointer.
    while unsafe { sb_disc_seek_verify(disc, 0, 1 << 20, &mut done) } == SbStatus::Ok && done == 0 {}
    assert_eq!(done, 1, "{}", last_error());
    let e = next_event(disc, 0).unwrap();
    assert_eq!((e.claim, e.state, e.lsn, e.first), (s.claim, SB_SPECULATION_CONFIRMED, s.lsn, first));
    assert!(next_event(disc, 0).is_none(), "returned once");
    assert_eq!(speculation(a).state, SB_SPECULATION_CONFIRMED);
    audio_free(a);
    free(disc);
    free(plain);
}

/// A caller-kept index takes the reading with `sb_track_audio_absorb` and checks it with
/// `sb_seek_index_verify`.
#[test]
#[cfg_attr(miri, ignore = "reads a corpus image")]
fn caller_kept_index_checks_readings() {
    let bytes = image("dst-features-2ch-6ch");
    let disc = open(&bytes);
    let p = playtime(disc, 0);
    assert_eq!(policy(disc, 0, SB_SEEK_POLICY_SPECULATIVE, 0), SbStatus::Ok);
    let index = index_new(disc, 0, 4).unwrap();
    let a = open_indexed(disc, 0, (p / 2, p / 2 + 2), index).unwrap();
    read_all(a, disc);
    let s = speculation(a);
    assert_eq!(s.state, SB_SPECULATION_PENDING, "{s:?}");
    assert_eq!(absorb(a, index), SbStatus::Ok);
    let mut e = SbSpeculation::default();
    // SAFETY: a live handle and a valid out-pointer: nothing checked yet.
    assert_eq!(unsafe { sb_seek_index_event_next(index, &mut e) }, SbStatus::End);
    while !extend(disc, index, 1 << 20).unwrap() {}
    let mut settled = u32::MAX;
    // SAFETY: live handles and a valid out-pointer (extend checked it already).
    assert_eq!(unsafe { sb_seek_index_verify(disc, index, &mut settled) }, SbStatus::Ok);
    assert_eq!(settled, 0);
    // SAFETY: a live handle and a valid out-pointer.
    assert_eq!(unsafe { sb_seek_index_event_next(index, &mut e) }, SbStatus::Ok);
    assert_eq!((e.claim, e.state), (s.claim, SB_SPECULATION_CONFIRMED));
    assert_eq!(speculation(a).state, SB_SPECULATION_CONFIRMED);
    audio_free(a);
    index_free(index);
    free(disc);
}

/// Arguments of the provisional reading functions. Small enough for Miri.
#[test]
fn provisional_reading_arguments() {
    let bytes = small_image(5);
    let disc = open(&bytes);
    let a = range_open(disc, 0, 0, 1);
    // SAFETY: NULL arguments are refused.
    let st = unsafe { sb_track_audio_speculation(a, ptr::null_mut()) };
    assert_eq!(st, SbStatus::InvalidArgument);
    // SAFETY: as above.
    let st = unsafe { sb_track_audio_speculation(ptr::null(), &mut SbSpeculation::default()) };
    assert_eq!(st, SbStatus::InvalidArgument);
    // SAFETY: as above.
    let st = unsafe { sb_disc_seek_event_next(disc, 0, ptr::null_mut()) };
    assert_eq!(st, SbStatus::InvalidArgument);
    // SAFETY: as above.
    let st = unsafe { sb_disc_seek_policy_set(ptr::null(), 0, SB_SEEK_POLICY_EXACT, 0) };
    assert_eq!(st, SbStatus::InvalidArgument);
    assert_eq!(speculation(a).state, SB_SPECULATION_EXACT);
    assert!(next_event(disc, 0).is_none());
    assert_eq!(policy(disc, 1, SB_SEEK_POLICY_EXACT, 0), SbStatus::NoSuchArea);
    let mut done = 0;
    // SAFETY: a live handle; no seek index is kept.
    assert_eq!(unsafe { sb_disc_seek_verify(disc, 0, 10, &mut done) }, SbStatus::InvalidArgument);
    audio_free(a);
    free(disc);
}

/// Two images of the same size and layout, different contents (#102): a caller-kept index
/// that took a provisional reading of one refuses to be extended or verified with the
/// other, and the reading is not confirmed against it.
#[test]
#[cfg_attr(miri, ignore = "reads a corpus image")]
fn a_reading_is_not_checked_against_another_image() {
    let a_bytes = image("dst-features-2ch-6ch");
    let a = open(&a_bytes);
    let mut info = SbAreaInfo::default();
    // SAFETY: a live handle and a valid out-pointer.
    assert_eq!(unsafe { sb_disc_area(a, 0, &mut info) }, SbStatus::Ok);
    // B: every audio sector of A with its bytes past the headers (at most 43 bytes) changed.
    let mut b_bytes = a_bytes.clone();
    for lsn in info.audio_start_lsn as usize..=info.audio_end_lsn as usize {
        for byte in &mut b_bytes[lsn * 2048 + 64..(lsn + 1) * 2048] {
            *byte ^= 0x5A;
        }
    }
    let b = open(&b_bytes);
    assert_eq!(policy(a, 0, SB_SEEK_POLICY_SPECULATIVE, 0), SbStatus::Ok);
    let index = index_new(a, 0, 4).unwrap();
    let p = playtime(a, 0);
    let h = open_indexed(a, 0, (p / 2, p / 2 + 2), index).unwrap();
    read_all(h, a);
    assert_eq!(speculation(h).state, SB_SPECULATION_PENDING);
    assert_eq!(absorb(h, index), SbStatus::Ok);
    assert_eq!(extend(b, index, 1 << 20).unwrap_err(), SbStatus::InvalidArgument);
    let mut settled = 0;
    // SAFETY: live handles and a valid out-pointer.
    assert_eq!(unsafe { sb_seek_index_verify(b, index, &mut settled) }, SbStatus::InvalidArgument);
    assert_eq!(speculation(h).state, SB_SPECULATION_PENDING);
    while !extend(a, index, 1 << 20).unwrap() {}
    assert_eq!(speculation(h).state, SB_SPECULATION_CONFIRMED);
    audio_free(h);
    index_free(index);
    free(a);
    free(b);
}

/// The ids of the corpus's well-formed images.
fn well_formed() -> Vec<String> {
    let corpus = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/manifest.json");
    let m: serde_json::Value = serde_json::from_slice(&std::fs::read(corpus).unwrap()).unwrap();
    let ids: Vec<String> = m["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["category"] == "valid")
        .map(|c| c["id"].as_str().unwrap().to_owned())
        .collect();
    assert!(ids.len() > 20, "{ids:?}");
    ids
}

fn verify_all(disc: *const SbDisc, slot: u32) {
    let mut done = 0u8;
    // SAFETY: a live handle and a valid out-pointer.
    while unsafe { sb_disc_seek_verify(disc, slot, 1 << 20, &mut done) } == SbStatus::Ok && done == 0 {}
    assert_eq!(done, 1, "{}", last_error());
}

/// On every well-formed corpus image, in each area, a provisional range read through the
/// C interface gives the frames, statuses, and missing and invalid counts of an exact one,
/// and a disc-kept seek index confirms it. (Malformed sector counts begin at `L` and are
/// not compared.)
#[test]
#[cfg_attr(miri, ignore = "reads every well-formed corpus image")]
fn provisional_reads_equal_exact_reads_on_the_corpus() {
    let mut provisional = 0;
    for id in well_formed() {
        let bytes = image(&id);
        let exact = open(&bytes);
        let disc = open(&bytes);
        for area in [0, 1] {
            let mut info = SbAreaInfo::default();
            // SAFETY: a live handle and a valid out-pointer.
            if unsafe { sb_disc_area(disc, area, &mut info) } != SbStatus::Ok {
                continue;
            }
            let (slot, p) = (info.slot, info.playtime_frames);
            assert_eq!(policy(disc, slot, SB_SEEK_POLICY_SPECULATIVE, 0), SbStatus::Ok);
            // SAFETY: a live handle.
            let s = unsafe { sb_disc_seek_index_enable(disc, slot, 4, SB_SEEK_DEFAULT_INLINE_BUDGET) };
            assert_eq!(s, SbStatus::Ok);
            let mut handles = Vec::new();
            for first in [p / 3, p / 2, p * 9 / 10, p.saturating_sub(2)] {
                let end = p.min(first + 4);
                let a = range_open(disc, slot, first, end);
                let got = read_all(a, disc);
                let b = range_open(exact, slot, first, end);
                let want = read_all(b, exact);
                assert_eq!(speculation(b).state, SB_SPECULATION_EXACT);
                assert_eq!(
                    (&got.0, got.1, got.2),
                    (&want.0, want.1, want.2),
                    "{id} slot {slot} first {first}"
                );
                audio_free(b);
                handles.push(a);
            }
            verify_all(disc, slot);
            for a in handles {
                let s = speculation(a);
                if s.state != SB_SPECULATION_EXACT {
                    assert_eq!(s.state, SB_SPECULATION_CONFIRMED, "{id} slot {slot}: {s:?}");
                    provisional += 1;
                }
                audio_free(a);
            }
            while next_event(disc, slot).is_some() {}
        }
        free(disc);
        free(exact);
    }
    assert!(provisional >= 10, "{provisional} provisional reads");
}

/// A disc of one 2-channel area of one-sector DST frames, frame `k` at audio sector `k`,
/// except that sector `at` holds a frame numbered `number` instead.
fn one_sector_frames(frames: u32, at: u32, number: u32) -> Vec<u8> {
    let s = 2048;
    let mut img = small_image(frames as usize);
    let a = 540 * s;
    let tc = |n: u32| [(n / 4500) as u8, (n / 75 % 60) as u8, (n % 75) as u8];
    img[a + 64..a + 67].copy_from_slice(&tc(frames));
    for k in 0..frames {
        let n = if k == at { number } else { k };
        let at = (546 + k as usize) * s;
        let sector = &mut img[at..at + s];
        sector.fill(0);
        // DST, one frame start, one packet: an audio packet starting the frame, of 2000 bytes.
        sector[0] = 1 | (1 << 2) | (1 << 5);
        sector[1..3].copy_from_slice(&((1u16 << 15) | (2 << 11) | 2000).to_be_bytes());
        sector[3..6].copy_from_slice(&tc(n));
        sector[6] = 1 << 2;
        sector[7..2007].fill(k as u8);
    }
    img
}

/// A frame numbered within the range, early in the area, refutes a provisional reading of
/// that range (specification 4, 4.2 item 8): the C caller sees REFUTED with `exact_from`,
/// positions from there on equal a straight read, and an exact re-read of the positions
/// below it completes a read equal to a straight one.
#[test]
#[cfg_attr(miri, ignore = "reads a thousand sectors")]
fn a_refutation_reaches_the_c_caller() {
    let bytes = one_sector_frames(1000, 100, 505);
    let plain = open(&bytes);
    let disc = open(&bytes);
    assert_eq!(playtime(disc, 0), 1000);
    let (first, end) = (300, 600);
    let b = range_open(plain, 0, first, end);
    let want = read_all(b, plain);
    audio_free(b);
    assert_eq!(policy(disc, 0, SB_SEEK_POLICY_SPECULATIVE, 0), SbStatus::Ok);
    // SAFETY: a live handle.
    let st = unsafe { sb_disc_seek_index_enable(disc, 0, 64, SB_SEEK_DEFAULT_INLINE_BUDGET) };
    assert_eq!(st, SbStatus::Ok);
    let a = range_open(disc, 0, first, end);
    let got = read_all(a, disc);
    assert_eq!(speculation(a).state, SB_SPECULATION_PENDING);
    assert_ne!(got.0, want.0, "the provisional positions differ");
    verify_all(disc, 0);
    let e = next_event(disc, 0).unwrap();
    let s = speculation(a);
    let fields = |x: SbSpeculation| (x.claim, x.lsn, x.slot, x.state, x.first, x.last_returned, x.exact_from);
    assert_eq!(fields(s), fields(e));
    assert_eq!((s.state, s.first, s.last_returned), (SB_SPECULATION_REFUTED, first, end - 1), "{s:?}");
    assert!((506..end).contains(&s.exact_from), "at least a* = 506: {s:?}");
    for (g, w) in got.0.iter().zip(&want.0).filter(|(g, _)| g.0 >= s.exact_from) {
        assert_eq!(g, w);
    }
    audio_free(a);
    // As pysacd does: re-read [first, exact_from) exactly and keep the rest.
    assert_eq!(policy(disc, 0, SB_SEEK_POLICY_EXACT, 0), SbStatus::Ok);
    let r = range_open(disc, 0, first, s.exact_from);
    let mut joined = read_all(r, disc).0;
    assert_eq!(speculation(r).state, SB_SPECULATION_EXACT);
    audio_free(r);
    joined.extend(got.0.into_iter().filter(|g| g.0 >= s.exact_from));
    assert_eq!(joined, want.0);
    free(disc);
    free(plain);
}

/// A provisional reading made through a disc is checked through its clone, on another
/// thread; the verdict reaches the reading's handle and the disc's own event queue.
#[test]
#[cfg_attr(miri, ignore = "reads a corpus image")]
fn a_clone_checks_readings_on_another_thread() {
    let bytes = image("dst-features-2ch-6ch");
    let disc = open(&bytes);
    let p = playtime(disc, 0);
    assert_eq!(policy(disc, 0, SB_SEEK_POLICY_SPECULATIVE, 0), SbStatus::Ok);
    // SAFETY: a live handle.
    assert_eq!(unsafe { sb_disc_seek_index_enable(disc, 0, 4, SB_SEEK_DEFAULT_INLINE_BUDGET) }, SbStatus::Ok);
    let a = range_open(disc, 0, p * 2 / 3, p * 2 / 3 + 3);
    read_all(a, disc);
    let s = speculation(a);
    assert_eq!(s.state, SB_SPECULATION_PENDING);
    let c = clone(disc).expose_provenance();
    std::thread::spawn(move || {
        let c: *mut SbDisc = ptr::with_exposed_provenance_mut(c);
        verify_all(c, 0);
        free(c);
    })
    .join()
    .unwrap();
    assert_eq!(speculation(a).state, SB_SPECULATION_CONFIRMED);
    let e = next_event(disc, 0).unwrap();
    assert_eq!((e.claim, e.state, e.first), (s.claim, SB_SPECULATION_CONFIRMED, s.first));
    assert!(next_event(disc, 0).is_none());
    audio_free(a);
    free(disc);
}
