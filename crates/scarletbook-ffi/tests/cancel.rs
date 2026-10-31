// SPDX-License-Identifier: MIT OR Apache-2.0

//! Cancelling and observing long calls through the C ABI (issue #106): a slow read
//! callback, a cancellation from another thread answered with `SB_STATUS_CANCELLED` within
//! 32 sectors, handles that resume or free, and progress polled from another thread.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::ffi::{CStr, c_void};
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};

use scarletbook_ffi::*;

const S: u64 = 2048;
/// Sectors a handle reads at most once cancellation is requested.
const BOUND: u64 = 32;
const ID: &str = "dst-features-2ch-6ch";

fn image(id: &str) -> Vec<u8> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/images").join(format!("{id}.iso"));
    std::fs::read(path).unwrap()
}

/// Handles moved to the reading thread; each is used by one thread at a time.
struct Handles<T>(T);
// SAFETY: the conventions let handles move between threads; the test uses each on one
// thread at a time (joining before the other uses it again).
unsafe impl<T> Send for Handles<T> {}

/// A slow positional source: sleeps on every read and counts the bytes read. Once armed
/// with a byte count ([`Slow::arm`]), the read that brings the total past it waits, before
/// copying, until the test releases it ([`Slow::release`]): the test then requests
/// cancellation while a read is known to be in progress, however the threads are scheduled.
struct Slow {
    bytes: Vec<u8>,
    read: AtomicU64,
    delay_us: u64,
    gate: Gate,
}

#[derive(Default)]
struct Gate {
    /// Armed at this total of bytes read; waiting; released.
    state: Mutex<(Option<u64>, bool, bool)>,
    changed: Condvar,
}

impl Slow {
    fn new(bytes: Vec<u8>, delay_us: u64) -> Self {
        Slow { bytes, read: AtomicU64::new(0), delay_us, gate: Gate::default() }
    }

    /// Holds the read that takes the total past `more` bytes from now.
    fn arm(&self, more: u64) {
        let mut s = self.gate.state.lock().unwrap();
        *s = (Some(self.read.load(Ordering::SeqCst) + more), false, false);
    }

    /// Waits until a read is held.
    fn wait_held(&self) {
        let mut s = self.gate.state.lock().unwrap();
        while !s.1 {
            s = self.gate.changed.wait(s).unwrap();
        }
    }

    /// Lets the held read (and every later one) go on.
    fn release(&self) {
        let mut s = self.gate.state.lock().unwrap();
        *s = (None, s.1, true);
        self.gate.changed.notify_all();
    }

    fn hold_if_armed(&self, len: usize) {
        let mut s = self.gate.state.lock().unwrap();
        let Some(at) = s.0 else { return };
        if self.read.load(Ordering::SeqCst) + len as u64 <= at {
            return;
        }
        s.1 = true;
        self.gate.changed.notify_all();
        while !s.2 {
            s = self.gate.changed.wait(s).unwrap();
        }
    }
}

unsafe extern "C" fn slow_read_at(user: *mut c_void, offset: u64, buf: *mut u8, len: usize) -> i64 {
    // SAFETY: `user` is the `Slow` given to sb_disc_open_callbacks_at, alive for the disc's
    // life; only shared references to it are made (its counter is atomic).
    let s = unsafe { &*user.cast::<Slow>() };
    std::thread::sleep(std::time::Duration::from_micros(s.delay_us));
    s.hold_if_armed(len);
    let start = (offset as usize).min(s.bytes.len());
    let n = len.min(s.bytes.len() - start);
    // SAFETY: the library passes a buffer valid for `len >= n` bytes of writes, which
    // does not overlap the source (owned by `s`).
    let out = unsafe { std::slice::from_raw_parts_mut(buf, n) };
    out.copy_from_slice(&s.bytes[start..start + n]);
    s.read.fetch_add(n as u64, Ordering::SeqCst);
    n as i64
}

fn open_slow(slow: &Slow) -> *mut SbDisc {
    let mut d = ptr::null_mut();
    let user = ptr::from_ref(slow).cast_mut().cast::<c_void>();
    // SAFETY: the callback copies at most `len` bytes and does not unwind; `slow` outlives
    // the disc in every test; `&mut d` is a valid out-pointer.
    let s = unsafe { sb_disc_open_callbacks_at(Some(slow_read_at), slow.bytes.len() as u64, user, &mut d) };
    assert_eq!(s, SbStatus::Ok);
    d
}

fn open(bytes: &[u8]) -> *mut SbDisc {
    let mut d = ptr::null_mut();
    // SAFETY: `bytes` is valid for its length; `&mut d` is a valid out-pointer.
    assert_eq!(unsafe { sb_disc_open_memory(bytes.as_ptr(), bytes.len(), &mut d) }, SbStatus::Ok);
    d
}

fn token(disc: *const SbDisc) -> *mut SbCancel {
    let mut t = ptr::dangling_mut();
    // SAFETY: `disc` is a live handle; `&mut t` is a valid out-pointer.
    assert_eq!(unsafe { sb_disc_cancel_token(disc, &mut t) }, SbStatus::Ok);
    t
}

fn request(t: *const SbCancel) {
    // SAFETY: `t` is a live token.
    assert_eq!(unsafe { sb_cancel_request(t) }, SbStatus::Ok);
}

fn withdraw(t: *const SbCancel) {
    // SAFETY: `t` is a live token.
    assert_eq!(unsafe { sb_cancel_withdraw(t) }, SbStatus::Ok);
}

fn requested(t: *const SbCancel) -> bool {
    let mut r = 9u8;
    // SAFETY: `t` is a live token; `&mut r` is a valid out-pointer.
    assert_eq!(unsafe { sb_cancel_requested(t, &mut r) }, SbStatus::Ok);
    r == 1
}

fn playtime(disc: *const SbDisc, slot: u32) -> u32 {
    let mut info = SbAreaInfo::default();
    // SAFETY: `disc` is a live handle; `&mut info` is a valid out-pointer.
    assert_eq!(unsafe { sb_disc_area(disc, slot, &mut info) }, SbStatus::Ok);
    info.playtime_frames
}

fn range_open(disc: *const SbDisc, slot: u32, first: u32, end: u32) -> *mut SbTrackAudio {
    let mut a = ptr::null_mut();
    // SAFETY: `disc` is a live handle; `&mut a` is a valid out-pointer.
    assert_eq!(unsafe { sb_range_audio_open(disc, slot, first, end, &mut a) }, SbStatus::Ok);
    a
}

fn raw_open(disc: *const SbDisc, slot: u32, first: u32, end: u32) -> *mut SbRawFrames {
    let mut r = ptr::null_mut();
    // SAFETY: `disc` is a live handle; `&mut r` is a valid out-pointer.
    assert_eq!(unsafe { sb_raw_frames_open(disc, slot, first, end, &mut r) }, SbStatus::Ok);
    r
}

fn audio_progress(a: *const SbTrackAudio) -> *mut SbProgress {
    let mut p = ptr::dangling_mut();
    // SAFETY: `a` is a live handle; `&mut p` is a valid out-pointer.
    assert_eq!(unsafe { sb_track_audio_progress(a, &mut p) }, SbStatus::Ok);
    p
}

fn raw_progress(r: *const SbRawFrames) -> *mut SbProgress {
    let mut p = ptr::dangling_mut();
    // SAFETY: `r` is a live handle; `&mut p` is a valid out-pointer.
    assert_eq!(unsafe { sb_raw_frames_progress(r, &mut p) }, SbStatus::Ok);
    p
}

fn progress(p: *const SbProgress) -> (u64, u64) {
    let (mut read, mut expected) = (0, 0);
    // SAFETY: `p` is a live progress handle; the outputs are valid.
    assert_eq!(unsafe { sb_progress_get(p, &mut read, &mut expected) }, SbStatus::Ok);
    (read, expected)
}

/// The next decoded frame: its status, number and bytes.
fn read(a: *const SbTrackAudio, disc: *const SbDisc) -> (SbStatus, u32, Vec<u8>) {
    let mut buf = vec![0u8; 6 * 4704];
    let mut fi = SbFrameInfo::default();
    // SAFETY: the handles are live; `buf` is valid for its length; `&mut fi` is valid.
    let s = unsafe { sb_track_audio_read(a, disc, buf.as_mut_ptr(), buf.len(), &mut fi) };
    buf.truncate(fi.size as usize);
    (s, fi.number, buf)
}

fn raw_read(r: *const SbRawFrames, disc: *const SbDisc) -> SbStatus {
    let mut buf = vec![0u8; SB_MAX_FRAME_BYTES];
    let mut fi = SbRawFrameInfo::default();
    // SAFETY: the handles are live; `buf` is valid for its length; `&mut fi` is valid.
    unsafe { sb_raw_frames_read(r, disc, buf.as_mut_ptr(), buf.len(), &mut fi) }
}

/// Every frame of `a` to its end, failing on any status but OK and END.
fn rest(a: *const SbTrackAudio, disc: *const SbDisc) -> Vec<(u32, Vec<u8>)> {
    let mut out = Vec::new();
    loop {
        match read(a, disc) {
            (SbStatus::Ok, n, b) => out.push((n, b)),
            (SbStatus::End, ..) => return out,
            (s, ..) => panic!("{s:?}"),
        }
    }
}

fn audio_free(a: *mut SbTrackAudio) {
    // SAFETY: `a` is NULL or a live handle not used again.
    unsafe { sb_track_audio_free(a) };
}

/// Another thread cancels a read through a slow positional callback: the reading thread's
/// call returns SB_STATUS_CANCELLED within the bound; once withdrawn, the same handle
/// gives the rest of the frames exactly; a handle cancelled and freed, then a new one,
/// reads correctly too. Progress is polled meanwhile from the cancelling thread.
#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn cancelled_from_another_thread() {
    let bytes = image(ID);
    let reference = open(&bytes);
    for slot in [0u32, 1] {
        let p = playtime(reference, slot);
        let first = p * 9 / 10;
        let ra = range_open(reference, slot, first, p);
        let want = rest(ra, reference);
        audio_free(ra);
        assert!(!want.is_empty());

        let slow = Slow::new(bytes.clone(), 1000);
        let disc = open_slow(&slow);
        let t = token(disc);
        assert!(!requested(t));
        let a = range_open(disc, slot, first, p);
        let pr = audio_progress(a);
        let (r0, e0) = progress(pr);
        assert_eq!(r0, 0);
        assert!(e0 > 0);
        // The read past the first BOUND sectors waits until the request is made (#121).
        slow.arm(BOUND * S);
        let worker = {
            let h = Handles((a, disc));
            std::thread::spawn(move || {
                let h = h;
                let (a, disc) = h.0;
                let mut got = Vec::new();
                let status = loop {
                    match read(a, disc) {
                        (SbStatus::Ok, n, b) => got.push((n, b)),
                        (s, ..) => break s,
                    }
                };
                Handles((status, got))
            })
        };
        // The scan before the range's first frame advances in steps of at most 32 sectors,
        // polled until the read past them is held.
        let mut last = 0;
        while !slow.gate.state.lock().unwrap().1 {
            let (read, expected) = progress(pr);
            assert!(read >= last && read <= expected);
            last = read;
            std::thread::yield_now();
        }
        slow.wait_held();
        assert!(!worker.is_finished(), "slot {slot}: read to the end before the request");
        assert!(progress(pr).0 > 0, "slot {slot}: no progress at the held read");
        request(t);
        let at = slow.read.load(Ordering::SeqCst);
        slow.release();
        let (status, mut got) = worker.join().unwrap().0;
        assert_eq!(status, SbStatus::Cancelled);
        let after = slow.read.load(Ordering::SeqCst) - at;
        assert!(after <= BOUND * S, "{after} bytes read after the request");
        // Every read of the disc is refused while the request stands, at once.
        let before = slow.read.load(Ordering::SeqCst);
        assert_eq!(read(a, disc).0, SbStatus::Cancelled);
        assert_eq!(slow.read.load(Ordering::SeqCst), before);
        // Withdrawn: the same handle continues exactly.
        withdraw(t);
        got.extend(rest(a, disc));
        assert_eq!(got.len(), want.len());
        assert!(got == want, "slot {slot}: frames differ after resuming");
        let (read_all, expected) = progress(pr);
        assert_eq!(read_all, expected);
        // A handle cancelled and freed, and a new one: reads correctly.
        let b = range_open(disc, slot, first, p);
        request(t);
        assert_eq!(read(b, disc).0, SbStatus::Cancelled);
        audio_free(b);
        withdraw(t);
        let b = range_open(disc, slot, first, p);
        assert!(rest(b, disc) == want);
        audio_free(b);
        audio_free(a);
        // The progress and token handles outlive the handles they came from.
        assert_eq!(progress(pr), (read_all, expected));
        // SAFETY: live handles, not used again.
        unsafe { sb_progress_free(pr) };
        // SAFETY: `disc` is live and not used again.
        unsafe { sb_disc_free(disc) };
        request(t);
        assert!(requested(t));
        // SAFETY: a live token, not used again.
        unsafe { sb_cancel_free(t) };
    }
    // SAFETY: `reference` is live and not used again.
    unsafe { sb_disc_free(reference) };
}

/// A clone's token is the original's; raw frames handles and seek index extension are
/// cancelled as well; progress of a raw frames handle completes.
#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn clones_raw_frames_and_seek_indexes() {
    let bytes = image(ID);
    let disc = open(&bytes);
    let mut clone = ptr::null_mut();
    // SAFETY: `disc` is live; `&mut clone` is a valid out-pointer.
    assert_eq!(unsafe { sb_disc_clone(disc, &mut clone) }, SbStatus::Ok);
    let t = token(clone);
    let p = playtime(disc, 0);
    let r = raw_open(disc, 0, p / 2, p);
    let pr = raw_progress(r);
    request(t);
    assert_eq!(raw_read(r, disc), SbStatus::Cancelled);
    // SAFETY: `disc` is live.
    assert_eq!(unsafe { sb_disc_seek_index_enable(disc, 0, SB_SEEK_DEFAULT_INTERVAL, 0) }, SbStatus::Ok);
    let mut complete = 0u8;
    // SAFETY: `disc` is live; `&mut complete` is a valid out-pointer.
    let s = unsafe { sb_disc_seek_index_extend(disc, 0, u64::MAX, &mut complete) };
    assert_eq!(s, SbStatus::Cancelled);
    withdraw(t);
    let mut n = 0;
    while raw_read(r, disc) == SbStatus::Ok {
        n += 1;
    }
    assert_eq!(n, p - p / 2);
    let (read, expected) = progress(pr);
    assert!(read > 0 && read == expected);
    // SAFETY: live handles, each freed once and not used again.
    unsafe {
        sb_raw_frames_free(r);
    }
    // SAFETY: as above.
    unsafe { sb_progress_free(pr) };
    // SAFETY: as above.
    unsafe { sb_cancel_free(t) };
    // SAFETY: as above.
    unsafe { sb_disc_free(clone) };
    // SAFETY: as above.
    unsafe { sb_disc_free(disc) };
}

/// The status, and NULL arguments, without any image (also under Miri).
#[test]
fn status_and_null_arguments() {
    assert_eq!(SbStatus::Cancelled as i32, -19);
    // SAFETY: sb_status_name returns a static NUL-terminated string.
    assert_eq!(unsafe { CStr::from_ptr(sb_status_name(-19)) }, c"cancelled");
    // SAFETY: NULL handles and out-pointers are allowed and refused.
    unsafe {
        assert_eq!(sb_cancel_request(ptr::null()), SbStatus::InvalidArgument);
    }
    // SAFETY: as above.
    unsafe { assert_eq!(sb_cancel_withdraw(ptr::null()), SbStatus::InvalidArgument) };
    let mut r = 0u8;
    // SAFETY: as above.
    unsafe { assert_eq!(sb_cancel_requested(ptr::null(), &mut r), SbStatus::InvalidArgument) };
    let mut t: *mut SbCancel = ptr::dangling_mut();
    // SAFETY: as above.
    unsafe { assert_eq!(sb_disc_cancel_token(ptr::null(), &mut t), SbStatus::InvalidArgument) };
    assert!(t.is_null());
    let mut p: *mut SbProgress = ptr::dangling_mut();
    // SAFETY: as above.
    unsafe { assert_eq!(sb_track_audio_progress(ptr::null(), &mut p), SbStatus::InvalidArgument) };
    assert!(p.is_null());
    p = ptr::dangling_mut();
    // SAFETY: as above.
    unsafe { assert_eq!(sb_raw_frames_progress(ptr::null(), &mut p), SbStatus::InvalidArgument) };
    assert!(p.is_null());
    let (mut a, mut b) = (0u64, 0u64);
    // SAFETY: as above.
    unsafe { assert_eq!(sb_progress_get(ptr::null(), &mut a, &mut b), SbStatus::InvalidArgument) };
    // SAFETY: NULL is ignored.
    unsafe { sb_cancel_free(ptr::null_mut()) };
    // SAFETY: NULL is ignored.
    unsafe { sb_progress_free(ptr::null_mut()) };
}
