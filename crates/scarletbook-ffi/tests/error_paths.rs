// SPDX-License-Identifier: MIT OR Apache-2.0

//! The C ABI's less travelled paths: NULL output pointers, handles of another disc,
//! buffers too small, log callbacks dropped on a failure, stored frame list handles in the
//! queries written for ranges, and the TOC warnings and speculation states the corpus
//! does not reach. Each case states the status the caller gets; none may panic.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::ffi::{CStr, c_char, c_void};
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

use scarletbook_ffi::*;

fn image(id: &str) -> Vec<u8> {
    let corpus = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus");
    std::fs::read(corpus.join("images").join(format!("{id}.iso"))).unwrap()
}

/// A disc with one 2-channel area of `sectors` audio sectors (each one packet past its
/// end, so malformed) from LSN 546, play time 7 frames: small enough for Miri. As in
/// `seek.rs`.
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

/// A disc of one 2-channel area of one-sector DST frames, frame `k` at audio sector `k`,
/// except that sector `at` holds a frame numbered `number` instead. As in `seek.rs`.
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

fn last_error() -> String {
    // SAFETY: a valid NUL-terminated string, copied before this thread's next call.
    unsafe { CStr::from_ptr(sb_last_error_message()) }.to_string_lossy().into_owned()
}

fn open(bytes: &[u8]) -> *mut SbDisc {
    let mut d = ptr::null_mut();
    // SAFETY: `bytes` is valid for its length; `&mut d` is a valid out-pointer.
    let s = unsafe { sb_disc_open_memory(bytes.as_ptr(), bytes.len(), &mut d) };
    assert_eq!(s, SbStatus::Ok, "{}", last_error());
    d
}

fn free(disc: *mut SbDisc) {
    // SAFETY: a live handle that the caller does not use again.
    unsafe { sb_disc_free(disc) };
}

fn range_open(disc: *const SbDisc, slot: u32, first: u32, end: u32) -> *mut SbTrackAudio {
    let mut a = ptr::null_mut();
    // SAFETY: a live handle and a valid out-pointer.
    let s = unsafe { sb_range_audio_open(disc, slot, first, end, &mut a) };
    assert_eq!(s, SbStatus::Ok, "{}", last_error());
    a
}

fn audio_free(audio: *mut SbTrackAudio) {
    // SAFETY: a live handle that the caller does not use again.
    unsafe { sb_track_audio_free(audio) };
}

fn raw_open(disc: *const SbDisc, slot: u32, first: u32, end: u32) -> *mut SbRawFrames {
    let mut r = ptr::null_mut();
    // SAFETY: a live handle and a valid out-pointer.
    let s = unsafe { sb_raw_frames_open(disc, slot, first, end, &mut r) };
    assert_eq!(s, SbStatus::Ok, "{}", last_error());
    r
}

fn list_open(disc: *const SbDisc, slot: u32) -> *mut SbRawFrames {
    let mut r = ptr::null_mut();
    // SAFETY: a live handle and a valid out-pointer.
    let s = unsafe { sb_stored_frame_list_open(disc, slot, &mut r) };
    assert_eq!(s, SbStatus::Ok, "{}", last_error());
    r
}

fn raw_free(raw: *mut SbRawFrames) {
    // SAFETY: NULL or a live handle that the caller does not use again.
    unsafe { sb_raw_frames_free(raw) };
}

/// Reads the next stored frame: its status and kind.
fn raw_read(raw: *const SbRawFrames, disc: *const SbDisc) -> (SbStatus, u32) {
    let mut buf = vec![0u8; SB_MAX_FRAME_BYTES];
    let mut fi = SbRawFrameInfo::default();
    // SAFETY: live handles; `buf` is valid for its length; `&mut fi` is a valid out-pointer.
    let s = unsafe { sb_raw_frames_read(raw, disc, buf.as_mut_ptr(), buf.len(), &mut fi) };
    (s, fi.kind)
}

/// Reads the next decoded frame of `audio`.
fn audio_read(audio: *const SbTrackAudio, disc: *const SbDisc) -> SbStatus {
    let mut buf = vec![0u8; 6 * 4704];
    let mut fi = SbFrameInfo::default();
    // SAFETY: live handles; `buf` is valid for its length; `&mut fi` is a valid out-pointer.
    unsafe { sb_track_audio_read(audio, disc, buf.as_mut_ptr(), buf.len(), &mut fi) }
}

fn index_new(disc: *const SbDisc, slot: u32, interval: u32) -> *mut SbSeekIndex {
    let mut i = ptr::null_mut();
    // SAFETY: a live handle and a valid out-pointer.
    let s = unsafe { sb_seek_index_new(disc, slot, interval, SB_SEEK_DEFAULT_INLINE_BUDGET, &mut i) };
    assert_eq!(s, SbStatus::Ok, "{}", last_error());
    i
}

fn index_free(index: *mut SbSeekIndex) {
    // SAFETY: a live handle that the caller does not use again.
    unsafe { sb_seek_index_free(index) };
}

fn policy(disc: *const SbDisc, slot: u32, policy: u32) {
    // SAFETY: a live handle.
    assert_eq!(unsafe { sb_disc_seek_policy_set(disc, slot, policy, 0) }, SbStatus::Ok);
}

fn status_name(status: i32) -> String {
    // SAFETY: sb_status_name returns a static NUL-terminated string.
    unsafe { CStr::from_ptr(sb_status_name(status)) }.to_str().unwrap().to_owned()
}

/// Every status has its own name, and the integers between and around them are "unknown".
#[test]
fn every_status_has_a_name() {
    let names: Vec<String> = (-21..=2).map(status_name).collect();
    assert!(names.iter().all(|n| n != "unknown"), "{names:?}");
    let distinct: std::collections::BTreeSet<_> = names.iter().collect();
    assert_eq!(distinct.len(), names.len(), "{names:?}");
    for (status, name) in [
        (SbStatus::NotScarletbook, "not-scarletbook"),
        (SbStatus::BufferTooSmall, "buffer-too-small"),
        (SbStatus::Panic, "internal-error"),
        (SbStatus::NotEditedMaster, "not-edited-master"),
    ] {
        assert_eq!(status_name(status as i32), name);
    }
    assert_eq!(status_name(3), "unknown");
    assert_eq!(status_name(-22), "unknown");
}

/// A disc handle's `Debug` names it without its contents.
#[test]
fn a_disc_handle_debug_prints() {
    let bytes = small_image(5);
    let disc = open(&bytes);
    // SAFETY: NULL or a live handle, only read here.
    let handle = unsafe { disc.as_ref() }.expect("a live handle");
    let text = format!("{handle:?}");
    assert!(text.starts_with("SbDisc") && text.contains("id"), "{text}");
    free(disc);
}

/// Every call that writes a result through a pointer refuses a NULL one with
/// `SB_STATUS_INVALID_ARGUMENT`, and says so.
#[test]
fn null_output_pointers() {
    let bytes = small_image(5);
    let disc = open(&bytes);
    let null_arg = |s: SbStatus| {
        assert_eq!(s, SbStatus::InvalidArgument);
        assert!(last_error().contains("NULL"), "{}", last_error());
    };
    let mut tc = SbTimecode::default();
    // SAFETY: a live handle; NULL outputs are refused before any write.
    null_arg(unsafe { sb_track_timecodes_stored(disc, 0, 1, ptr::null_mut(), &mut tc) });
    // SAFETY: as above.
    null_arg(unsafe { sb_track_timecodes_stored(disc, 0, 1, &mut tc, ptr::null_mut()) });
    // SAFETY: as above.
    null_arg(unsafe { sb_track_audio_open(disc, 0, 1, ptr::null_mut()) });
    // SAFETY: as above.
    null_arg(unsafe { sb_stored_frame_list_open(disc, 0, ptr::null_mut()) });
    // SAFETY: as above.
    null_arg(unsafe { sb_disc_seek_index_get(disc, 0, ptr::null_mut()) });
    // SAFETY: as above.
    null_arg(unsafe { sb_disc_cancel_token(disc, ptr::null_mut()) });
    let saved = [0u8; 16];
    // SAFETY: as above; `saved` is valid for its length.
    null_arg(unsafe { sb_seek_index_load(disc, 0, saved.as_ptr(), saved.len(), 0, ptr::null_mut()) });
    let index = index_new(disc, 0, 4);
    // SAFETY: as above.
    null_arg(unsafe { sb_seek_index_event_next(index, ptr::null_mut()) });
    index_free(index);
    let a = range_open(disc, 0, 0, 1);
    // SAFETY: as above.
    null_arg(unsafe { sb_track_audio_progress(a, ptr::null_mut()) });
    let mut p = ptr::null_mut();
    // SAFETY: a live handle and a valid out-pointer.
    assert_eq!(unsafe { sb_track_audio_progress(a, &mut p) }, SbStatus::Ok);
    let mut n = 0u64;
    // SAFETY: a live progress handle; NULL outputs are refused before any write.
    null_arg(unsafe { sb_progress_get(p, ptr::null_mut(), &mut n) });
    // SAFETY: as above.
    null_arg(unsafe { sb_progress_get(p, &mut n, ptr::null_mut()) });
    // SAFETY: a live progress handle, not used again.
    unsafe { sb_progress_free(p) };
    audio_free(a);
    let r = raw_open(disc, 0, 0, 1);
    // SAFETY: as above.
    null_arg(unsafe { sb_raw_frames_progress(r, ptr::null_mut()) });
    raw_free(r);
    free(disc);
}

/// `sb_track_audio_read_with_stored` refuses a disc the handle is not of and a decoded
/// buffer too small for a frame; `sb_track_audio_counts` leaves out a missing position
/// held back for a too small stored buffer until it is returned.
#[test]
#[cfg_attr(miri, ignore = "reads twenty DST sectors")]
fn read_with_stored_refusals_and_a_held_missing_frame() {
    // Sector 5 holds a second frame 6: position 5 is missing.
    let bytes = one_sector_frames(20, 5, 6);
    let disc = open(&bytes);
    let other = open(&bytes);
    let a = range_open(disc, 0, 0, 20);
    // SAFETY: a live handle.
    assert_eq!(unsafe { sb_track_audio_keep_stored(a) }, SbStatus::Ok);
    // Reads one position with a decoded buffer of `cap` bytes and a stored buffer of
    // `raw_cap` bytes (NULL for 0): the status, frame number and stored kind.
    let read = |d: *const SbDisc, cap: usize, raw_cap: usize, equivalent: u8| {
        let mut buf = vec![0u8; cap];
        let mut raw = vec![0u8; raw_cap];
        let raw_ptr = if raw_cap == 0 { ptr::null_mut() } else { raw.as_mut_ptr() };
        let mut fi = SbFrameInfo::default();
        let mut ri = SbRawFrameInfo::default();
        // SAFETY: live handles; `buf` and `raw` are valid for `cap` and `raw_cap` bytes (or
        // NULL); `&mut fi` and `&mut ri` are valid out-pointers.
        let s = unsafe {
            sb_track_audio_read_with_stored(
                a,
                d,
                buf.as_mut_ptr(),
                cap,
                &mut fi,
                raw_ptr,
                raw_cap,
                &mut ri,
                equivalent,
            )
        };
        (s, fi.number, ri.kind)
    };
    let (full, raw_full) = (2 * 4704, SB_MAX_FRAME_BYTES);
    assert_eq!(read(other, full, raw_full, 0).0, SbStatus::InvalidArgument);
    assert!(last_error().contains("another disc"), "{}", last_error());
    assert_eq!(read(disc, full - 1, raw_full, 0).0, SbStatus::BufferTooSmall);
    let counts = || {
        let (mut missing, mut invalid) = (u64::MAX, u64::MAX);
        // SAFETY: a live handle and valid out-pointers.
        assert_eq!(unsafe { sb_track_audio_counts(a, &mut missing, &mut invalid) }, SbStatus::Ok);
        (missing, invalid)
    };
    for k in 0..5 {
        let (s, number, _) = read(disc, full, raw_full, 1);
        assert_eq!((s, number), (SbStatus::Ok, k), "{}", last_error());
    }
    // Frames 1 to 4 are not valid DST (their bytes are filler).
    let before = counts();
    // Position 5's DST equivalent needs bytes: held back, and not counted yet.
    let missing = SbFrameKind::Missing as u32;
    assert_eq!(read(disc, full, 0, 1), (SbStatus::BufferTooSmall, 5, missing));
    assert_eq!(counts(), before);
    assert_eq!(read(disc, full, raw_full, 1), (SbStatus::Ok, 5, missing));
    assert_eq!(counts(), (before.0 + 1, before.1));
    audio_free(a);
    free(disc);
    free(other);
}

/// A log callback that counts its calls.
unsafe extern "C" fn count_log(user: *mut c_void, _level: i32, _message: *const c_char) {
    // SAFETY: `user` is the test's `AtomicU32`, alive while the disc can call this.
    unsafe { &*user.cast::<AtomicU32>() }.fetch_add(1, Ordering::SeqCst);
}

/// The `SB_LOG_ERROR` and `SB_LOG_WARN` levels are accepted; a log callback given to a call
/// that then fails for a NULL callback or output pointer is never called.
#[test]
fn log_levels_and_failed_opens_with_a_log() {
    let bytes = small_image(5);
    let calls = AtomicU32::new(0);
    let user = ptr::from_ref(&calls).cast_mut().cast::<c_void>();
    for level in [SB_LOG_ERROR, SB_LOG_WARN] {
        let mut d = ptr::null_mut();
        // SAFETY: `bytes` is valid for its length; `count_log` may be called with `user`
        // until the disc is freed, which happens before `calls` is dropped.
        let s = unsafe {
            sb_disc_open_memory_with_log(bytes.as_ptr(), bytes.len(), Some(count_log), user, level, &mut d)
        };
        assert_eq!(s, SbStatus::Ok, "{}", last_error());
        free(d);
    }
    let opened = calls.load(Ordering::SeqCst);
    let mut d: *mut SbDisc = ptr::dangling_mut();
    // SAFETY: a NULL read callback is refused; `&mut d` is a valid out-pointer.
    let s = unsafe {
        sb_disc_open_callbacks_at_with_log(
            None,
            0,
            ptr::null_mut(),
            Some(count_log),
            user,
            SB_LOG_TRACE,
            &mut d,
        )
    };
    assert_eq!((s, d.is_null()), (SbStatus::InvalidArgument, true));
    let disc = open(&bytes);
    // SAFETY: a live handle; the NULL output pointer is refused.
    let s = unsafe { sb_disc_clone_with_log(disc, Some(count_log), user, SB_LOG_TRACE, ptr::null_mut()) };
    assert_eq!(s, SbStatus::InvalidArgument);
    let mut c: *mut SbDisc = ptr::dangling_mut();
    // SAFETY: a live handle; NULL callbacks are refused; `&mut c` is a valid out-pointer.
    let s = unsafe {
        sb_disc_clone_callbacks_with_log(
            disc,
            None,
            None,
            ptr::null_mut(),
            Some(count_log),
            user,
            SB_LOG_TRACE,
            &mut c,
        )
    };
    assert_eq!((s, c.is_null()), (SbStatus::InvalidArgument, true));
    free(disc);
    assert_eq!(calls.load(Ordering::SeqCst), opened, "a refused call's callback was called");
}

/// `sb_dst_decode` takes an empty frame without a pointer, and refuses a NULL frame with
/// a length.
#[test]
fn dst_decode_frame_pointers() {
    let mut out = vec![0u8; 4704];
    let mut rule = u32::MAX;
    // SAFETY: an empty frame needs no pointer; `out` is valid for its length.
    let s = unsafe { sb_dst_decode(ptr::null(), 0, 1, out.as_mut_ptr(), out.len(), &mut rule) };
    assert!(matches!(s, SbStatus::Ok | SbStatus::InvalidDstFrame), "{s:?}");
    // SAFETY: as above; a NULL frame with a length is refused before it is read.
    let s = unsafe { sb_dst_decode(ptr::null(), 8, 1, out.as_mut_ptr(), out.len(), &mut rule) };
    assert_eq!(s, SbStatus::InvalidArgument);
    assert!(last_error().contains("NULL frame"), "{}", last_error());
}

fn warnings(disc: *const SbDisc) -> Vec<(i32, String)> {
    let mut info = SbDiscInfo::default();
    // SAFETY: a live handle and a valid out-pointer.
    assert_eq!(unsafe { sb_disc_info(disc, &mut info) }, SbStatus::Ok);
    (0..info.warning_count)
        .map(|i| {
            let mut w = SbWarning { lsn: 0, slot: 0, track: 0, code: ptr::null() };
            // SAFETY: a live handle and a valid out-pointer.
            assert_eq!(unsafe { sb_disc_warning(disc, i, &mut w) }, SbStatus::Ok);
            // SAFETY: `code` is a static NUL-terminated string.
            (w.slot, unsafe { CStr::from_ptr(w.code) }.to_str().unwrap().to_owned())
        })
        .collect()
}

/// TOC warnings the corpus does not raise, through `sb_disc_warning` and `sb_disc_toc`
/// (specification 7): no manufacturer sector, and a multichannel area skipped for a TOC
/// longer than `MAX_AREA_TOC_SECTORS` or for a track list whose ISRC and genre structure
/// runs past the area TOC's end.
#[test]
fn toc_warnings_reach_the_c_caller() {
    let s = 2048;
    let m = 510 * s;
    let mut no_manufacturer = small_image(5);
    no_manufacturer[m + 9 * s..m + 9 * s + 8].fill(0);
    let mut too_large = small_image(5);
    too_large[m + 72..m + 76].copy_from_slice(&540u32.to_be_bytes());
    too_large[m + 86..m + 88].copy_from_slice(&129u16.to_be_bytes());
    // A 4-sector multichannel TOC after the audio: TRL1, TRL2, then SACD_IGL in its last
    // sector, which needs two.
    let mut malformed = small_image(5);
    let x = malformed.len() / s;
    malformed.resize((x + 4) * s, 0);
    malformed[m + 72..m + 76].copy_from_slice(&(x as u32).to_be_bytes());
    malformed[m + 86..m + 88].copy_from_slice(&4u16.to_be_bytes());
    let z = x * s;
    malformed[z..z + 8].copy_from_slice(b"MULCHTOC");
    malformed[z + 69] = 1;
    malformed[z + s..z + s + 8].copy_from_slice(b"SACDTRL1");
    malformed[z + 2 * s..z + 2 * s + 8].copy_from_slice(b"SACDTRL2");
    malformed[z + 3 * s..z + 3 * s + 8].copy_from_slice(b"SACD_IGL");
    for (bytes, want) in [
        (no_manufacturer, (-1, "manufacturer-missing")),
        (too_large, (1, "area-toc-too-large")),
        (malformed, (1, "area-toc-malformed")),
    ] {
        let disc = open(&bytes);
        let got = warnings(disc);
        assert!(got.iter().any(|(slot, code)| (*slot, code.as_str()) == want), "{got:?}");
        let mut len = 0usize;
        // SAFETY: a live handle; a NULL buffer only sizes the layout.
        assert_eq!(unsafe { sb_disc_toc(disc, 0, ptr::null_mut(), 0, &mut len) }, SbStatus::BufferTooSmall);
        let mut buf = vec![0u8; len];
        // SAFETY: `buf` is valid for `len` bytes.
        assert_eq!(unsafe { sb_disc_toc(disc, 0, buf.as_mut_ptr(), len, &mut len) }, SbStatus::Ok);
        assert!(buf.windows(want.1.len()).any(|w| w == want.1.as_bytes()), "the layout holds the code");
        free(disc);
    }
}

/// A stored frame list handle (specification 4, 4.6.1 item 2) in the raw frames queries
/// written for ranges: plain DSD frames as stored, its malformed count, warnings and
/// progress, exact positions (no speculation, not suspect), nothing to give a seek index,
/// and, as `prev` with an index, a range that starts from that index.
#[test]
#[cfg_attr(miri, ignore = "reads a corpus image")]
fn a_stored_frame_list_in_range_queries() {
    let bytes = image("dsd-1ch");
    let disc = open(&bytes);
    let mut a = SbAreaInfo::default();
    // SAFETY: a live handle and a valid out-pointer.
    assert_eq!(unsafe { sb_disc_area(disc, 0, &mut a) }, SbStatus::Ok);
    let slot = a.slot;
    let index = index_new(disc, slot, 4);
    let list = list_open(disc, slot);
    let mut frames = 0;
    loop {
        match raw_read(list, disc) {
            (SbStatus::Ok, kind) => {
                assert_eq!(kind, SbFrameKind::Dsd as u32);
                frames += 1;
            }
            (SbStatus::End, _) => break,
            (s, _) => panic!("{s:?}: {}", last_error()),
        }
    }
    assert!(frames > 0);
    let mut n = u64::MAX;
    // SAFETY: a live handle and a valid out-pointer.
    assert_eq!(unsafe { sb_raw_frames_malformed_sectors(list, &mut n) }, SbStatus::Ok);
    assert_eq!(n, 0);
    let mut w = SbWarning { lsn: 0, slot: 0, track: 0, code: ptr::null() };
    // SAFETY: a live handle and a valid out-pointer; a well-formed area has no warning.
    assert_eq!(unsafe { sb_raw_frames_warning(list, 0, &mut w) }, SbStatus::InvalidArgument);
    let mut p = ptr::null_mut();
    // SAFETY: a live handle and a valid out-pointer.
    assert_eq!(unsafe { sb_raw_frames_progress(list, &mut p) }, SbStatus::Ok);
    let (mut read, mut expected) = (0u64, 0u64);
    // SAFETY: a live progress handle and valid out-pointers.
    assert_eq!(unsafe { sb_progress_get(p, &mut read, &mut expected) }, SbStatus::Ok);
    assert!(read > 0 && read == expected, "{read} {expected}");
    // SAFETY: a live progress handle, not used again.
    unsafe { sb_progress_free(p) };
    let mut spec = SbSpeculation { claim: 9, state: u32::MAX, ..SbSpeculation::default() };
    // SAFETY: a live handle and a valid out-pointer.
    assert_eq!(unsafe { sb_raw_frames_speculation(list, &mut spec) }, SbStatus::Ok);
    assert_eq!((spec.claim, spec.state), (0, SbSpeculation::default().state));
    let mut suspect = u32::MAX;
    // SAFETY: a live handle and a valid out-pointer.
    assert_eq!(unsafe { sb_raw_frames_suspect(list, &mut suspect) }, SbStatus::Ok);
    assert_eq!(suspect, SB_SUSPECT_NONE);
    // SAFETY: live handles.
    assert_eq!(unsafe { sb_raw_frames_absorb(list, index) }, SbStatus::Ok);
    // A range after the list, from the index: the same frames as a range opened alone.
    let (first, end) = (a.playtime_frames / 2, a.playtime_frames / 2 + 2);
    let mut next = ptr::null_mut();
    // SAFETY: live handles; `list` is consumed on success; `&mut next` is a valid out-pointer.
    let s = unsafe { sb_raw_frames_open_next_indexed(disc, list, slot, first, end, index, &mut next) };
    assert_eq!(s, SbStatus::Ok, "{}", last_error());
    let alone = raw_open(disc, slot, first, end);
    for _ in first..end {
        assert_eq!(raw_read(next, disc), raw_read(alone, disc));
    }
    assert_eq!(raw_read(next, disc).0, SbStatus::End);
    raw_free(next);
    raw_free(alone);
    index_free(index);
    free(disc);
}

/// `sb_raw_frames_speculation` and `sb_raw_frames_suspect` on a range read provisionally.
#[test]
#[cfg_attr(miri, ignore = "reads a corpus image")]
fn a_raw_range_read_provisionally() {
    let bytes = image("dst-features-2ch-6ch");
    let disc = open(&bytes);
    let mut a = SbAreaInfo::default();
    // SAFETY: a live handle and a valid out-pointer.
    assert_eq!(unsafe { sb_disc_area(disc, 0, &mut a) }, SbStatus::Ok);
    policy(disc, a.slot, SB_SEEK_POLICY_SPECULATIVE);
    let p = a.playtime_frames;
    let r = raw_open(disc, a.slot, p / 2, p / 2 + 2);
    assert_eq!(raw_read(r, disc).0, SbStatus::Ok);
    let mut spec = SbSpeculation::default();
    // SAFETY: a live handle and a valid out-pointer.
    assert_eq!(unsafe { sb_raw_frames_speculation(r, &mut spec) }, SbStatus::Ok);
    assert_eq!((spec.state, spec.first, spec.slot), (SB_SPECULATION_PENDING, p / 2, a.slot), "{spec:?}");
    assert!(spec.claim > 0 && spec.lsn > 0, "{spec:?}");
    let mut suspect = u32::MAX;
    // SAFETY: a live handle and a valid out-pointer.
    assert_eq!(unsafe { sb_raw_frames_suspect(r, &mut suspect) }, SbStatus::Ok);
    assert_eq!(suspect, SB_SUSPECT_NONE);
    // SAFETY: NULL arguments are refused.
    assert_eq!(unsafe { sb_raw_frames_speculation(r, ptr::null_mut()) }, SbStatus::InvalidArgument);
    // SAFETY: as above.
    assert_eq!(unsafe { sb_raw_frames_speculation(ptr::null(), &mut spec) }, SbStatus::InvalidArgument);
    raw_free(r);
    free(disc);
}

/// The decreasing frame numbers mark through the C interface (specification 4, 4.2 item
/// 8, sacd-spec `eafaaba`): a sector just before the range holding the area's last frame
/// makes the first frame records read while choosing the start decrease.
#[test]
#[cfg_attr(miri, ignore = "reads two hundred sectors")]
fn decreasing_frame_numbers_are_suspect() {
    let bytes = one_sector_frames(200, 98, 199);
    let disc = open(&bytes);
    policy(disc, 0, SB_SEEK_POLICY_SPECULATIVE);
    let a = range_open(disc, 0, 100, 102);
    assert_eq!(audio_read(a, disc), SbStatus::Ok);
    let mut s = u32::MAX;
    // SAFETY: a live handle and a valid out-pointer.
    assert_eq!(unsafe { sb_track_audio_suspect(a, &mut s) }, SbStatus::Ok);
    assert_eq!(s, SB_SUSPECT_FRAME_NUMBERS_DECREASE);
    audio_free(a);
    let r = raw_open(disc, 0, 100, 102);
    assert_eq!(raw_read(r, disc).0, SbStatus::Ok);
    let mut s = u32::MAX;
    // SAFETY: a live handle and a valid out-pointer.
    assert_eq!(unsafe { sb_raw_frames_suspect(r, &mut s) }, SbStatus::Ok);
    assert_eq!(s, SB_SUSPECT_FRAME_NUMBERS_DECREASE);
    raw_free(r);
    free(disc);
}

/// A caller-kept index holds at most 256 provisional readings waiting to be checked: two
/// more drop the two oldest as `SB_SPECULATION_UNVERIFIED`, returned in order by
/// `sb_seek_index_event_next`.
#[test]
#[cfg_attr(miri, ignore = "opens 258 handles")]
fn readings_past_the_limit_are_unverified() {
    let bytes = one_sector_frames(200, 200, 0);
    let disc = open(&bytes);
    policy(disc, 0, SB_SEEK_POLICY_SPECULATIVE);
    let index = index_new(disc, 0, 4);
    let mut claims = Vec::new();
    for _ in 0..258 {
        let mut a = ptr::null_mut();
        // SAFETY: live handles and a valid out-pointer.
        let s = unsafe { sb_range_audio_open_indexed(disc, 0, 150, 152, index, &mut a) };
        assert_eq!(s, SbStatus::Ok, "{}", last_error());
        assert_eq!(audio_read(a, disc), SbStatus::Ok);
        let mut spec = SbSpeculation::default();
        // SAFETY: a live handle and a valid out-pointer.
        assert_eq!(unsafe { sb_track_audio_speculation(a, &mut spec) }, SbStatus::Ok);
        assert_eq!(spec.state, SB_SPECULATION_PENDING, "{spec:?}");
        claims.push(spec.claim);
        // SAFETY: live handles.
        assert_eq!(unsafe { sb_track_audio_absorb(a, index) }, SbStatus::Ok);
        audio_free(a);
    }
    let mut events = Vec::new();
    loop {
        let mut e = SbSpeculation::default();
        // SAFETY: a live handle and a valid out-pointer.
        match unsafe { sb_seek_index_event_next(index, &mut e) } {
            SbStatus::Ok => events.push((e.claim, e.state, e.first)),
            SbStatus::End => break,
            s => panic!("{s:?}: {}", last_error()),
        }
    }
    let unverified = SB_SPECULATION_UNVERIFIED;
    assert_eq!(events, [(claims[0], unverified, 150), (claims[1], unverified, 150)]);
    index_free(index);
    free(disc);
}

/// A positional source of `bytes`: reads past them return 0.
struct Short {
    bytes: Vec<u8>,
}

unsafe extern "C" fn short_read_at(user: *mut c_void, offset: u64, buf: *mut u8, len: usize) -> i64 {
    // SAFETY: `user` is the test's `Short`, alive for the disc's life; only read.
    let s = unsafe { &*user.cast::<Short>() };
    let start = usize::try_from(offset).unwrap_or(usize::MAX).min(s.bytes.len());
    let n = len.min(s.bytes.len() - start);
    // SAFETY: the library passes a buffer valid for `len >= n` bytes of writes, which does
    // not overlap the source.
    let out = unsafe { std::slice::from_raw_parts_mut(buf, n) };
    out.copy_from_slice(&s.bytes[start..start + n]);
    n as i64
}

/// Sectors within the size a source claims but past its end: `sb_disc_read_sectors`
/// fails with `SB_STATUS_IO` (a source shorter than it says), not as for sectors past the
/// size.
#[test]
fn sectors_the_source_does_not_have() {
    let short = Short { bytes: small_image(5) };
    let sectors = short.bytes.len() as u64 / 2048;
    let mut disc = ptr::null_mut();
    // SAFETY: `short_read_at` may be called with `user` (the `Short`, which outlives the
    // disc) from any thread; `&mut disc` is a valid out-pointer.
    let s = unsafe {
        sb_disc_open_callbacks_at(
            Some(short_read_at),
            (sectors + 4) * 2048,
            ptr::from_ref(&short).cast_mut().cast(),
            &mut disc,
        )
    };
    assert_eq!(s, SbStatus::Ok, "{}", last_error());
    let mut buf = vec![0u8; 2 * 2048];
    // SAFETY: a live handle; `buf` is valid for its length.
    let s = unsafe { sb_disc_read_sectors(disc, sectors - 1, 2, buf.as_mut_ptr(), buf.len()) };
    assert_eq!(s, SbStatus::Io, "{}", last_error());
    // SAFETY: as above.
    let s = unsafe { sb_disc_read_sectors(disc, sectors + 3, 2, buf.as_mut_ptr(), buf.len()) };
    assert_eq!(s, SbStatus::SectorRangeInvalid, "{}", last_error());
    free(disc);
}

/// The `toc` fuzz target's harness (`fuzz_disc_toc`), on an image it opens, one with a
/// prefix and one it does not; and `fuzz_pcm_convert_run` on an empty run.
#[cfg(feature = "fuzzing")]
#[test]
fn the_fuzzing_harnesses() {
    let bytes = small_image(5);
    scarletbook_ffi::fuzz_disc_toc(0, &bytes);
    scarletbook_ffi::fuzz_disc_toc(2048, &bytes[2048..]);
    scarletbook_ffi::fuzz_disc_toc(0, &bytes[..2048]);
    scarletbook_ffi::fuzz_pcm_convert_run(176_400, SB_PCM_CUTOFF_DEFAULT, 1, 0.0, None, &[], None);
}

/// A log callback's state: the first call from a pool thread for a group of frames other
/// than the first blocks until `released`; `late` counts
/// the calls after `replaced`.
#[derive(Default)]
struct Gate {
    state: Mutex<GateState>,
    changed: Condvar,
}

#[derive(Default)]
struct GateState {
    blocked: bool,
    released: bool,
    replaced: bool,
    late: u32,
}

impl Gate {
    fn lock(&self) -> MutexGuard<'_, GateState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits up to 10 s for `done`; whether it came.
    fn wait_until(&self, done: impl Fn(&GateState) -> bool) -> bool {
        let s = self.lock();
        let (s, _) = self
            .changed
            .wait_timeout_while(s, std::time::Duration::from_secs(10), |s| !done(s))
            .unwrap_or_else(PoisonError::into_inner);
        done(&s)
    }
}

unsafe extern "C" fn gate_log(user: *mut c_void, _level: i32, message: *const c_char) {
    if !std::thread::current().name().is_some_and(|n| n.starts_with("scarletbook-")) {
        return;
    }
    // SAFETY: a valid NUL-terminated string during the call.
    let message = unsafe { CStr::from_ptr(message) }.to_bytes();
    // Not the group the first read waits for.
    let first_group = message.starts_with(b"decoding DST frames 0..");
    // SAFETY: `user` is the test's `Gate`, alive until the callback is cleared.
    let gate = unsafe { &*user.cast::<Gate>() };
    let mut s = gate.lock();
    if s.replaced {
        s.late += 1;
    } else if !s.blocked && !first_group {
        s.blocked = true;
        gate.changed.notify_all();
        while !s.released {
            s = gate.changed.wait(s).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// Replacing a disc's log while a pool thread is inside its callback waits for that call
/// to return, and the decoding jobs already queued with the old log no longer call it.
#[test]
#[cfg_attr(miri, ignore = "decodes DST on a pool")]
fn replacing_the_log_waits_for_a_pool_threads_call() {
    let bytes = image("dst-features-2ch-6ch");
    let gate = Gate::default();
    let user = ptr::from_ref(&gate).cast_mut().cast::<c_void>();
    let mut disc = ptr::null_mut();
    // SAFETY: `bytes` is valid for its length; `gate_log` may be called with `user` from
    // any thread until the log is replaced below, before `gate` is dropped.
    let s = unsafe {
        sb_disc_open_memory_with_log(
            bytes.as_ptr(),
            bytes.len(),
            Some(gate_log),
            user,
            SB_LOG_TRACE,
            &mut disc,
        )
    };
    assert_eq!(s, SbStatus::Ok, "{}", last_error());
    let mut pool = ptr::null_mut();
    // SAFETY: a valid out-pointer.
    assert_eq!(unsafe { sb_pool_new(4, &mut pool) }, SbStatus::Ok);
    let mut a = SbAreaInfo::default();
    // SAFETY: a live handle and a valid out-pointer.
    assert_eq!(unsafe { sb_disc_area(disc, 0, &mut a) }, SbStatus::Ok);
    let audio = range_open(disc, a.slot, 0, a.playtime_frames);
    // SAFETY: live handles.
    assert_eq!(unsafe { sb_track_audio_set_pool(audio, pool) }, SbStatus::Ok);
    std::thread::scope(|scope| {
        // Releases the blocked call a while after it blocks: after sb_disc_set_log below
        // has started to wait for it, unless this thread is that late.
        scope.spawn(|| {
            assert!(gate.wait_until(|s| s.blocked), "no pool thread call");
            std::thread::sleep(std::time::Duration::from_millis(200));
            gate.lock().released = true;
            gate.changed.notify_all();
        });
        // The first read queues groups of frames on the pool and waits for the first.
        assert_eq!(audio_read(audio, disc), SbStatus::Ok);
        assert!(gate.wait_until(|s| s.blocked), "no pool thread call");
        // SAFETY: a live handle; removing the log.
        assert_eq!(unsafe { sb_disc_set_log(disc, None, ptr::null_mut(), SB_LOG_OFF) }, SbStatus::Ok);
        let mut s = gate.lock();
        assert!(s.released, "sb_disc_set_log returned while the callback ran");
        s.replaced = true;
    });
    while audio_read(audio, disc) == SbStatus::Ok {}
    assert_eq!(gate.lock().late, 0, "the old callback was called after sb_disc_set_log returned");
    audio_free(audio);
    // SAFETY: a live pool, not used again.
    unsafe { sb_pool_free(pool) };
    free(disc);
}
