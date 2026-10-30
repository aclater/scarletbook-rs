// SPDX-License-Identifier: MIT OR Apache-2.0

//! The C ABI, driven from Rust as a C caller would.
//!
//! Each unsafe call sits in a small wrapper that states why its arguments meet the
//! function's contract; the tests themselves are safe code.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::ffi::{CStr, CString, c_char, c_void};
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::ptr;

use scarletbook_ffi::*;
use sha2::{Digest, Sha256};

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus")
}

fn image(id: &str) -> Vec<u8> {
    std::fs::read(corpus().join("images").join(format!("{id}.iso"))).unwrap()
}

fn expected(id: &str) -> serde_json::Value {
    let m: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(corpus().join("manifest.json")).unwrap()).unwrap();
    m["cases"].as_array().unwrap().iter().find(|c| c["id"] == id).unwrap()["expected"].clone()
}

fn last_error() -> String {
    // SAFETY: sb_last_error_message returns a valid NUL-terminated string that lives until
    // this thread's next call into the library; it is copied before any such call.
    unsafe { CStr::from_ptr(sb_last_error_message()) }.to_string_lossy().into_owned()
}

fn status_name(status: i32) -> String {
    // SAFETY: sb_status_name returns a static NUL-terminated string.
    unsafe { CStr::from_ptr(sb_status_name(status)) }.to_str().unwrap().to_owned()
}

/// Opens an image from memory; the out-pointer is pre-set to a non-NULL garbage value to
/// check that it is cleared on failure.
fn open(bytes: &[u8]) -> Result<*mut SbDisc, SbStatus> {
    let mut d: *mut SbDisc = ptr::dangling_mut();
    // SAFETY: `bytes` is valid for its length; `&mut d` is a valid out-pointer.
    let s = unsafe { sb_disc_open_memory(bytes.as_ptr(), bytes.len(), &mut d) };
    match s {
        SbStatus::Ok => Ok(d),
        s => {
            assert!(d.is_null(), "*out not cleared on failure");
            Err(s)
        }
    }
}

fn free(disc: *mut SbDisc) {
    // SAFETY: `disc` is NULL or a live handle that the caller does not use again.
    unsafe { sb_disc_free(disc) };
}

fn info(disc: *const SbDisc) -> (SbStatus, SbDiscInfo) {
    let mut i = SbDiscInfo::default();
    // SAFETY: `disc` is NULL or a live handle; `&mut i` is a valid out-pointer.
    (unsafe { sb_disc_info(disc, &mut i) }, i)
}

fn area(disc: *const SbDisc, index: u32) -> (SbStatus, SbAreaInfo) {
    let mut a = SbAreaInfo::default();
    // SAFETY: `disc` is a live handle; `&mut a` is a valid out-pointer.
    (unsafe { sb_disc_area(disc, index, &mut a) }, a)
}

fn track(disc: *const SbDisc, slot: u32, number: u32) -> (SbStatus, SbTrackInfo) {
    let mut t = SbTrackInfo::default();
    // SAFETY: `disc` is a live handle; `&mut t` is a valid out-pointer.
    (unsafe { sb_track_info(disc, slot, number, &mut t) }, t)
}

fn check(disc: *const SbDisc, slot: u32, number: u32) -> SbStatus {
    // SAFETY: `disc` is a live handle.
    unsafe { sb_track_check(disc, slot, number) }
}

/// Fetches a string with a copy-out call `f(buf, cap, len)`: first sizing, then copying.
fn string(f: impl Fn(*mut c_char, usize, *mut usize) -> SbStatus) -> Option<String> {
    let mut len = 0usize;
    match f(ptr::null_mut(), 0, &mut len) {
        SbStatus::Absent => {
            assert_eq!(len, 0);
            return None;
        }
        SbStatus::BufferTooSmall => {}
        s => panic!("unexpected {s:?}"),
    }
    let mut buf = vec![0 as c_char; len + 1];
    assert_eq!(f(buf.as_mut_ptr(), buf.len(), &mut len), SbStatus::Ok);
    assert_eq!(buf[len], 0);
    Some(String::from_utf8(buf[..len].iter().map(|&b| b as u8).collect()).unwrap())
}

fn disc_string(disc: *const SbDisc, field: u32) -> Option<String> {
    // SAFETY: `disc` is a live handle; `string` passes a buffer valid for `c` bytes (or
    // NULL) and a valid length pointer.
    string(|b, c, l| unsafe { sb_disc_string(disc, field, b, c, l) })
}

fn track_string(disc: *const SbDisc, slot: u32, number: u32, field: u32) -> Option<String> {
    // SAFETY: as in `disc_string`.
    string(|b, c, l| unsafe { sb_track_string(disc, slot, number, field, b, c, l) })
}

fn audio_open(disc: *const SbDisc, slot: u32, number: u32) -> Result<*mut SbTrackAudio, SbStatus> {
    let mut a: *mut SbTrackAudio = ptr::dangling_mut();
    // SAFETY: `disc` is NULL or a live handle; `&mut a` is a valid out-pointer.
    match unsafe { sb_track_audio_open(disc, slot, number, &mut a) } {
        SbStatus::Ok => Ok(a),
        s => {
            assert!(a.is_null(), "*out not cleared on failure");
            Err(s)
        }
    }
}

fn audio_read(audio: *const SbTrackAudio, disc: *const SbDisc, buf: &mut [u8]) -> (SbStatus, SbFrameInfo) {
    let mut fi = SbFrameInfo::default();
    // SAFETY: the handles are NULL or live; `buf` is valid for its length; `&mut fi` is a
    // valid out-pointer.
    (unsafe { sb_track_audio_read(audio, disc, buf.as_mut_ptr(), buf.len(), &mut fi) }, fi)
}

fn audio_channels(audio: *const SbTrackAudio) -> u32 {
    // SAFETY: `audio` is a live handle.
    unsafe { sb_track_audio_channels(audio) }
}

fn audio_free(audio: *mut SbTrackAudio) {
    // SAFETY: `audio` is NULL or a live handle that the caller does not use again.
    unsafe { sb_track_audio_free(audio) };
}

/// Decodes a track through the C ABI and hashes each channel.
fn hash_track(disc: *const SbDisc, slot: u32, number: u32) -> (Vec<String>, u64, u64) {
    let a = audio_open(disc, slot, number).unwrap_or_else(|s| panic!("{s:?}: {}", last_error()));
    let ch = audio_channels(a) as usize;
    let mut hashers: Vec<Sha256> = (0..ch).map(|_| Sha256::new()).collect();
    let mut buf = vec![0u8; ch * 4704];
    loop {
        let (s, fi) = audio_read(a, disc, &mut buf);
        match s {
            SbStatus::End => break,
            SbStatus::Ok => assert_eq!(fi.size, buf.len()),
            s => panic!("{s:?}: {}", last_error()),
        }
        for (c, h) in hashers.iter_mut().enumerate() {
            let chan: Vec<u8> = buf.iter().skip(c).step_by(ch).copied().collect();
            h.update(&chan);
        }
    }
    let (mut missing, mut invalid) = (0u64, 0u64);
    // SAFETY: `a` is a live handle; both out-pointers are valid.
    assert_eq!(unsafe { sb_track_audio_counts(a, &mut missing, &mut invalid) }, SbStatus::Ok);
    audio_free(a);
    (hashers.into_iter().map(|h| format!("{:x}", h.finalize())).collect(), missing, invalid)
}

#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn decodes_corpus_tracks_through_the_abi() {
    for id in ["dst-features-2ch-6ch", "dst-invalid-frames-5ch", "hostile-base"] {
        let disc = open(&image(id)).unwrap();
        let exp = expected(id);
        for area in exp["audio"].as_array().unwrap() {
            let slot = if area["area"] == "2ch" { 0 } else { 1 };
            for t in area["tracks"].as_array().unwrap() {
                let n = t["number"].as_u64().unwrap() as u32;
                if t["outcome"] != "ok" {
                    assert_ne!(check(disc, slot, n), SbStatus::Ok);
                    continue;
                }
                let (sha, missing, invalid) = hash_track(disc, slot, n);
                let exp_sha: Vec<String> =
                    t["sha256"].as_array().unwrap().iter().map(|h| h.as_str().unwrap().to_owned()).collect();
                assert_eq!(sha, exp_sha, "{id} {slot} {n}");
                assert_eq!(missing, t["missing_frames"].as_u64().unwrap());
                assert_eq!(invalid, t["invalid_dst_frames"].as_u64().unwrap());
            }
        }
        free(disc);
    }
}

#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn toc_accessors() {
    let disc = open(&image("full-text")).unwrap();
    let exp = expected("full-text");
    let toc = &exp["toc"];
    let (s, i) = info(disc);
    assert_eq!(s, SbStatus::Ok);
    assert_eq!(u64::from(i.sector_size), toc["sector_size"].as_u64().unwrap());
    assert_eq!(i.sectors, toc["sectors"].as_u64().unwrap());
    assert_eq!(i.area_count as usize, toc["areas"].as_array().unwrap().len());
    assert_eq!(
        disc_string(disc, SbDiscString::AlbumTitle as u32).as_deref(),
        toc["text"]["album_title"].as_str()
    );
    assert_eq!(
        disc_string(disc, SbDiscString::AlbumCatalogNumber as u32).as_deref(),
        toc["album_catalog_number"].as_str()
    );
    let (s, a) = area(disc, 0);
    assert_eq!(s, SbStatus::Ok);
    let a0 = &toc["areas"][0];
    assert_eq!(u64::from(a.track_count), a0["tracks"].as_array().unwrap().len() as u64);
    let (s, t) = track(disc, a.slot, 1);
    assert_eq!(s, SbStatus::Ok);
    assert_eq!(u64::from(t.duration_frames), a0["tracks"][0]["duration_frames"].as_u64().unwrap());
    let title = track_string(disc, a.slot, 1, SbTrackString::Title as u32);
    assert_eq!(title.as_deref(), a0["tracks"][0]["text"]["title"].as_str());
    let isrc = track_string(disc, a.slot, 1, SbTrackString::Isrc as u32);
    assert_eq!(isrc.as_deref(), a0["tracks"][0]["isrc"].as_str());
    free(disc);
}

#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn warnings_and_rejections() {
    let disc = open(&image("track-start-past-playtime")).unwrap();
    let (_, i) = info(disc);
    let exp = expected("track-start-past-playtime");
    let mut got = Vec::new();
    for idx in 0..i.warning_count {
        let mut w = SbWarning { code: ptr::null(), slot: 0, track: 0, lsn: 0 };
        // SAFETY: `disc` is a live handle; `&mut w` is a valid out-pointer.
        assert_eq!(unsafe { sb_disc_warning(disc, idx, &mut w) }, SbStatus::Ok);
        // SAFETY: `w.code` is a static NUL-terminated string.
        got.push(unsafe { CStr::from_ptr(w.code) }.to_str().unwrap().to_owned());
    }
    for w in exp["warnings"].as_array().unwrap() {
        assert!(got.iter().any(|g| g == w["code"].as_str().unwrap()), "{got:?}");
    }
    free(disc);

    assert_eq!(open(&image("master-version-99")).unwrap_err(), SbStatus::MasterTocVersionUnsupported);
    assert!(last_error().contains("version"));
    assert_eq!(open(&[]).unwrap_err(), SbStatus::NotScarletbook);
    assert_eq!(open(&image("master-no-areas")).unwrap_err(), SbStatus::NoAudioAreas);
    assert_eq!(status_name(SbStatus::NoAudioAreas as i32), "no-audio-areas");
    assert_eq!(status_name(12345), "unknown");
}

#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn hostile_arguments() {
    let bytes = image("hostile-base");
    let disc = open(&bytes).unwrap();
    let other = open(&bytes).unwrap();
    let mut out: *mut SbDisc = ptr::dangling_mut();
    // SAFETY: NULL data with a non-zero length is rejected before any read; `&mut out` is
    // a valid out-pointer.
    assert_eq!(unsafe { sb_disc_open_memory(ptr::null(), 5, &mut out) }, SbStatus::InvalidArgument);
    assert!(out.is_null());
    // SAFETY: `bytes` is valid for its length; a NULL out-pointer is rejected.
    let s = unsafe { sb_disc_open_memory(bytes.as_ptr(), bytes.len(), ptr::null_mut()) };
    assert_eq!(s, SbStatus::InvalidArgument);
    // SAFETY: a NULL path is rejected; `&mut out` is a valid out-pointer.
    assert_eq!(unsafe { sb_disc_open_file(ptr::null(), &mut out) }, SbStatus::InvalidArgument);
    let missing = CString::new("/nonexistent/disc.iso").unwrap();
    out = ptr::dangling_mut();
    // SAFETY: `missing` is a valid NUL-terminated string; `&mut out` is a valid out-pointer.
    assert_eq!(unsafe { sb_disc_open_file(missing.as_ptr(), &mut out) }, SbStatus::Io);
    assert!(out.is_null());
    assert_eq!(info(ptr::null()).0, SbStatus::InvalidArgument);
    // SAFETY: `disc` is a live handle; a NULL out-pointer is rejected.
    assert_eq!(unsafe { sb_disc_info(disc, ptr::null_mut()) }, SbStatus::InvalidArgument);
    assert_eq!(area(disc, 7).0, SbStatus::InvalidArgument);
    assert_eq!(track(disc, 9, 1).0, SbStatus::InvalidArgument);
    assert_eq!(track(disc, 0, 99).0, SbStatus::NoSuchTrack);
    let mut buf = [0 as c_char; 64];
    // SAFETY: `disc` is a live handle; `buf` is valid for 64 bytes; a NULL `len` is
    // rejected.
    let s = unsafe { sb_disc_string(disc, 16, buf.as_mut_ptr(), 64, ptr::null_mut()) };
    assert_eq!(s, SbStatus::InvalidArgument);
    let mut len = 0;
    // SAFETY: as above, with a valid `len`; field 99 is rejected.
    let s = unsafe { sb_disc_string(disc, 99, buf.as_mut_ptr(), 64, &mut len) };
    assert_eq!(s, SbStatus::InvalidArgument);
    assert_eq!(audio_open(disc, 0, 99).unwrap_err(), SbStatus::NoSuchTrack);

    let a = audio_open(disc, 0, 1).unwrap();
    let mut small = [0u8; 16];
    assert_eq!(audio_read(a, disc, &mut small).0, SbStatus::BufferTooSmall);
    let mut frame = vec![0u8; 2 * 4704];
    assert_eq!(audio_read(a, other, &mut frame).0, SbStatus::InvalidArgument);
    assert_eq!(audio_read(a, disc, &mut frame).0, SbStatus::Ok);
    assert_eq!(audio_read(ptr::null(), disc, &mut frame).0, SbStatus::InvalidArgument);
    let mut missing_count = 7u64;
    // SAFETY: `a` is a live handle; one out-pointer is valid, the other NULL (rejected).
    let s = unsafe { sb_track_audio_counts(a, &mut missing_count, ptr::null_mut()) };
    assert_eq!(s, SbStatus::InvalidArgument);
    assert_eq!(missing_count, 7, "nothing is written unless both pointers are given");
    audio_free(a);
    audio_free(ptr::null_mut());
    free(ptr::null_mut());
    free(disc);
    free(other);
}

#[test]
fn dst_decode_reports_the_rule() {
    let mut out = vec![0u8; 3 * 4704];
    let mut rule = 0u32;
    let decode = |frame: &[u8], ch: u32, out: &mut [u8], rule: &mut u32| {
        // SAFETY: `frame` and `out` are valid for their lengths and do not overlap;
        // `rule` is a valid out-pointer.
        unsafe { sb_dst_decode(frame.as_ptr(), frame.len(), ch, out.as_mut_ptr(), out.len(), rule) }
    };
    assert_eq!(
        (decode(&[0x80, 0x00], 2, &mut out[..2 * 4704], &mut rule), rule),
        (SbStatus::InvalidDstFrame, 3)
    );
    // A buffer larger than needed is accepted; only the first channels x 4704 bytes change.
    out.fill(0xEE);
    assert_eq!(decode(&[0x00, 1, 2, 3], 2, &mut out, &mut rule), SbStatus::Ok);
    assert_eq!(&out[..4], &[1, 2, 3, 0]);
    assert!(out[2 * 4704..].iter().all(|&b| b == 0xEE));
    assert_eq!(decode(&[0x00, 1], 2, &mut out[..100], &mut rule), SbStatus::BufferTooSmall);
    assert_eq!(decode(&[0x00, 1], 7, &mut out, &mut rule), SbStatus::InvalidArgument);
}

/// State shared with the callbacks: the image, and (once set) a disc and track handle to
/// call back into from inside a read, recording what those calls return.
struct User {
    cursor: Cursor<Vec<u8>>,
    disc: *mut SbDisc,
    audio: *mut SbTrackAudio,
    probes: Vec<SbStatus>,
}

unsafe extern "C" fn cb_read(user: *mut c_void, buf: *mut u8, len: usize) -> i64 {
    // SAFETY: `user` is the `User` passed to sb_disc_open_callbacks, alive for the disc's
    // life, and the library calls back only from the thread using the disc.
    let u = unsafe { &mut *user.cast::<User>() };
    if !u.disc.is_null() {
        // Re-entrant calls with handles that are in use must fail cleanly.
        u.probes.push(info(u.disc).0);
        u.probes.push(audio_read(u.audio, u.disc, &mut [0u8; 2 * 4704]).0);
        u.probes.push(audio_open_next(u.disc, u.audio, 0, 1).map_or_else(|s| s, |_| SbStatus::Ok));
        free(u.disc); // must be a no-op
        audio_free(u.audio); // must be a no-op
    }
    // SAFETY: the library passes a buffer valid for `len` bytes of writes.
    let out = unsafe { std::slice::from_raw_parts_mut(buf, len) };
    u.cursor.read(out).map_or(-1, |n| n as i64)
}

unsafe extern "C" fn cb_seek(user: *mut c_void, offset: i64, whence: i32) -> i64 {
    // SAFETY: as in `cb_read`.
    let u = unsafe { &mut *user.cast::<User>() };
    let pos = match whence {
        0 => SeekFrom::Start(offset as u64),
        1 => SeekFrom::Current(offset),
        _ => SeekFrom::End(offset),
    };
    u.cursor.seek(pos).map_or(-1, |p| p as i64)
}

/// A read callback that claims to have read more than asked: must be an I/O error, not UB.
unsafe extern "C" fn cb_read_lies(_: *mut c_void, _: *mut u8, len: usize) -> i64 {
    len as i64 + 100
}

fn open_callbacks(read: SbReadFn, user: &mut User) -> Result<*mut SbDisc, SbStatus> {
    let mut d: *mut SbDisc = ptr::dangling_mut();
    let user = (user as *mut User).cast::<c_void>();
    // SAFETY: the callbacks follow the contract (no unwinding, at most `len` bytes);
    // `user` outlives the disc in every test; `&mut d` is a valid out-pointer.
    match unsafe { sb_disc_open_callbacks(read, Some(cb_seek), user, &mut d) } {
        SbStatus::Ok => Ok(d),
        s => {
            assert!(d.is_null());
            Err(s)
        }
    }
}

#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn callbacks() {
    let mut user = User {
        cursor: Cursor::new(image("dsd-stereo-mch-2064")),
        disc: ptr::null_mut(),
        audio: ptr::null_mut(),
        probes: Vec::new(),
    };
    let disc = open_callbacks(Some(cb_read), &mut user).unwrap();
    let (_, i) = info(disc);
    assert_eq!(i.sector_size, 2064);
    let exp = expected("dsd-stereo-mch-2064");
    let t = &exp["audio"][0]["tracks"][0];
    let (sha, _, _) = hash_track(disc, 0, 1);
    assert_eq!(sha[0], t["sha256"][0].as_str().unwrap());

    // Re-entrancy: call back into the library from inside a read.
    let a = audio_open(disc, 0, 1).unwrap();
    user.disc = disc;
    user.audio = a;
    let mut frame = vec![0u8; 2 * 4704];
    assert_eq!(audio_read(a, disc, &mut frame).0, SbStatus::Ok);
    user.disc = ptr::null_mut();
    assert!(!user.probes.is_empty());
    assert!(user.probes.iter().all(|&s| s == SbStatus::InvalidArgument), "{:?}", user.probes);
    // The handles survived the attempted frees.
    assert_eq!(info(disc).0, SbStatus::Ok);
    assert_eq!(audio_read(a, disc, &mut frame).0, SbStatus::Ok);
    audio_free(a);
    free(disc);

    assert_eq!(open_callbacks(None, &mut user).unwrap_err(), SbStatus::InvalidArgument);
    assert_eq!(open_callbacks(Some(cb_read_lies), &mut user).unwrap_err(), SbStatus::Io);
}

/// A tiny image (one area, no audio) so that Miri can check the handle and copy-out code.
fn tiny_image() -> Vec<u8> {
    let s = 2048;
    let mut img = vec![0u8; 547 * s];
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
    img[a + 66] = 10; // play time: 10 frames, beyond the capacity of 7
    img[a + 69] = 1;
    img[a + 72..a + 76].copy_from_slice(&546u32.to_be_bytes());
    img[a + 76..a + 80].copy_from_slice(&546u32.to_be_bytes());
    // The sub-structure chain starts at sector 1 and stops at the first unknown signature.
    img[a + s..a + s + 8].copy_from_slice(b"SACDTRL1");
    let t2 = a + 2 * s;
    img[t2..t2 + 8].copy_from_slice(b"SACDTRL2");
    img[t2 + 1028 + 2] = 2; // track 1: 2 frames
    img
}

#[test]
fn tiny_image_handles_and_strings() {
    let disc = open(&tiny_image()).unwrap();
    assert_eq!(disc_string(disc, SbDiscString::AlbumCatalogNumber as u32).as_deref(), Some("CAT-1"));
    assert_eq!(disc_string(disc, SbDiscString::AlbumTitle as u32), None);
    let mut exact = [0 as c_char; 5];
    let mut len = 0;
    // SAFETY: `disc` is a live handle; `exact` is valid for 5 bytes; `&mut len` is valid.
    let s = unsafe { sb_disc_string(disc, 16, exact.as_mut_ptr(), 5, &mut len) };
    // Exactly the string length: no room for the NUL.
    assert_eq!((s, len), (SbStatus::BufferTooSmall, 5));
    let a = audio_open(disc, 0, 1).unwrap();
    let mut buf = vec![0u8; 2 * 4704];
    let mut n = 0;
    loop {
        let (s, fi) = audio_read(a, disc, &mut buf);
        if s != SbStatus::Ok {
            assert_eq!(s, SbStatus::End);
            break;
        }
        assert_eq!(fi.status, SbFrameStatus::Missing as u32);
        assert!(buf.iter().all(|&b| b == 0x69));
        n += 1;
    }
    assert_eq!(n, 2);
    // The disc may be freed first; the track handle can then still be freed.
    free(disc);
    audio_free(a);
}

fn audio_open_next(
    disc: *const SbDisc,
    prev: *mut SbTrackAudio,
    slot: u32,
    number: u32,
) -> Result<*mut SbTrackAudio, SbStatus> {
    let mut a: *mut SbTrackAudio = ptr::dangling_mut();
    // SAFETY: `disc` is a live handle; `prev` is NULL or a live handle that the caller does
    // not use again if the call succeeds; `&mut a` is a valid out-pointer.
    match unsafe { sb_track_audio_open_next(disc, prev, slot, number, &mut a) } {
        SbStatus::Ok => Ok(a),
        s => {
            assert!(a.is_null(), "*out not cleared on failure");
            Err(s)
        }
    }
}

/// Hashes the rest of a track handle's audio per channel.
fn hash_handle(a: *mut SbTrackAudio, disc: *const SbDisc) -> Vec<String> {
    let ch = audio_channels(a) as usize;
    let mut hashers: Vec<Sha256> = (0..ch).map(|_| Sha256::new()).collect();
    let mut buf = vec![0u8; ch * 4704];
    while audio_read(a, disc, &mut buf).0 == SbStatus::Ok {
        for (c, h) in hashers.iter_mut().enumerate() {
            h.update(buf.iter().skip(c).step_by(ch).copied().collect::<Vec<u8>>());
        }
    }
    hashers.into_iter().map(|h| format!("{:x}", h.finalize())).collect()
}

#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn open_next_continues_across_tracks() {
    let id = "dst-features-2ch-6ch";
    let disc = open(&image(id)).unwrap();
    let exp = expected(id);
    for (slot, area) in exp["audio"].as_array().unwrap().iter().enumerate() {
        let slot = slot as u32;
        let mut prev: *mut SbTrackAudio = ptr::null_mut();
        for t in area["tracks"].as_array().unwrap() {
            let n = t["number"].as_u64().unwrap() as u32;
            if check(disc, slot, n) != SbStatus::Ok {
                continue;
            }
            let a = audio_open_next(disc, prev, slot, n).unwrap();
            let exp_sha: Vec<String> =
                t["sha256"].as_array().unwrap().iter().map(|h| h.as_str().unwrap().to_owned()).collect();
            assert_eq!(hash_handle(a, disc), exp_sha, "{id} {slot} {n}");
            prev = a;
        }
        // A failing call leaves `prev` untouched: it can still be freed.
        assert_eq!(audio_open_next(disc, prev, slot, 999).unwrap_err(), SbStatus::NoSuchTrack);
        audio_free(prev);
    }
    free(disc);
}

#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn malformed_sector_warnings() {
    // audio-random: an area of random audio sectors, many of them malformed.
    let disc = open(&image("audio-random")).unwrap();
    let a = audio_open(disc, 0, 1).unwrap();
    let ch = audio_channels(a) as usize;
    let mut buf = vec![0u8; ch * 4704];
    while audio_read(a, disc, &mut buf).0 == SbStatus::Ok {}
    let mut count = 0u64;
    // SAFETY: `a` is a live handle; `&mut count` is a valid out-pointer.
    assert_eq!(unsafe { sb_track_audio_malformed_sectors(a, &mut count) }, SbStatus::Ok);
    assert!(count > 0, "random audio sectors include malformed ones");
    let mut w = SbWarning { code: ptr::null(), slot: 0, track: 0, lsn: 0 };
    // SAFETY: as above.
    assert_eq!(unsafe { sb_track_audio_warning(a, 0, &mut w) }, SbStatus::Ok);
    // SAFETY: `w.code` is a static NUL-terminated string.
    assert_eq!(unsafe { CStr::from_ptr(w.code) }.to_str().unwrap(), "audio-sector-malformed");
    assert_eq!(w.slot, 0);
    assert!(w.lsn >= 0);
    // SAFETY: as above; the index is past the end.
    assert_eq!(unsafe { sb_track_audio_warning(a, u32::MAX, &mut w) }, SbStatus::InvalidArgument);
    audio_free(a);
    free(disc);
}

#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn open_next_refusals_leave_prev_usable() {
    let bytes = image("hostile-base");
    let disc = open(&bytes).unwrap();
    let other = open(&bytes).unwrap();
    let a = audio_open(disc, 0, 1).unwrap();
    // `prev` from another disc.
    assert_eq!(audio_open_next(other, a, 0, 2).unwrap_err(), SbStatus::InvalidArgument);
    // A bad slot, a missing track and a track without audio are refused before `prev`
    // is taken.
    assert_eq!(audio_open_next(disc, a, 9, 2).unwrap_err(), SbStatus::InvalidArgument);
    assert_eq!(audio_open_next(disc, a, 0, 99).unwrap_err(), SbStatus::NoSuchTrack);
    // `prev` is still the caller's after every refusal: it reads and continues normally.
    let mut frame = vec![0u8; 2 * 4704];
    assert_eq!(audio_read(a, disc, &mut frame).0, SbStatus::Ok);
    let b = audio_open_next(disc, a, 0, 2).unwrap();
    assert_eq!(audio_read(b, disc, &mut frame).0, SbStatus::Ok);
    audio_free(b);
    free(disc);
    free(other);
}

/// The ABI version and the layouts it covers: a change to a struct must come with a new
/// SB_ABI_VERSION, and with this test.
#[test]
fn abi_version_and_layouts() {
    assert_eq!(sb_abi_version(), SB_ABI_VERSION);
    assert_eq!(SB_ABI_VERSION, 1);
    #[cfg(target_pointer_width = "64")]
    {
        assert_eq!(std::mem::size_of::<SbWarning>(), 24);
        assert_eq!(std::mem::size_of::<SbFrameInfo>(), 24);
    }
    assert_eq!(std::mem::size_of::<SbStatus>(), 4);
}

#[test]
fn version_is_the_crate_version() {
    // SAFETY: sb_version returns a static NUL-terminated string.
    let v = unsafe { CStr::from_ptr(sb_version()) };
    assert_eq!(v.to_str().unwrap(), env!("CARGO_PKG_VERSION"));
}

fn range_open_next(
    disc: *const SbDisc,
    prev: *mut SbTrackAudio,
    slot: u32,
    first: u32,
    end: u32,
) -> Result<*mut SbTrackAudio, SbStatus> {
    let mut a: *mut SbTrackAudio = ptr::dangling_mut();
    // SAFETY: `disc` is NULL or a live handle; `prev` is NULL or a live handle that the
    // caller does not use again if the call succeeds; `&mut a` is a valid out-pointer.
    match unsafe { sb_range_audio_open_next(disc, prev, slot, first, end, &mut a) } {
        SbStatus::Ok => Ok(a),
        s => {
            assert!(a.is_null(), "*out not cleared on failure");
            Err(s)
        }
    }
}

fn range_open(disc: *const SbDisc, slot: u32, first: u32, end: u32) -> Result<*mut SbTrackAudio, SbStatus> {
    let mut a: *mut SbTrackAudio = ptr::dangling_mut();
    // SAFETY: `disc` is NULL or a live handle; `&mut a` is a valid out-pointer.
    match unsafe { sb_range_audio_open(disc, slot, first, end, &mut a) } {
        SbStatus::Ok => Ok(a),
        s => {
            assert!(a.is_null(), "*out not cleared on failure");
            Err(s)
        }
    }
}

fn range_check(disc: *const SbDisc, slot: u32, first: u32, end: u32) -> SbStatus {
    // SAFETY: `disc` is NULL or a live handle.
    unsafe { sb_range_check(disc, slot, first, end) }
}

/// Reads the rest of a handle: each frame's number, status and bytes.
fn read_frames(a: *mut SbTrackAudio, disc: *const SbDisc) -> Vec<(u32, u32, Vec<u8>)> {
    let mut buf = vec![0u8; audio_channels(a) as usize * 4704];
    let mut out = Vec::new();
    loop {
        let (s, fi) = audio_read(a, disc, &mut buf);
        match s {
            SbStatus::End => return out,
            SbStatus::Ok => out.push((fi.number, fi.status, buf[..fi.size].to_vec())),
            s => panic!("{s:?}: {}", last_error()),
        }
    }
}

#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn whole_area_range_is_tracks_plus_pauses() {
    // Tracks [0, 3), [4, 6), [8, 9) of a 9-frame area; [0, 2), [3, 4) of a 4-frame one.
    let disc = open(&image("dsd-stereo-mch-2064")).unwrap();
    for (slot, playtime, tracks) in
        [(0u32, 9u32, &[(1u32, 0u32, 3u32), (2, 4, 6), (3, 8, 9)][..]), (1, 4, &[(1, 0, 2), (2, 3, 4)])]
    {
        let (s, a) = area(disc, slot);
        assert_eq!((s, a.playtime_frames), (SbStatus::Ok, playtime));
        assert_eq!(range_check(disc, slot, 0, playtime), SbStatus::Ok);
        let h = range_open(disc, slot, 0, playtime).unwrap();
        let whole = read_frames(h, disc);
        audio_free(h);
        assert_eq!(whole.iter().map(|f| f.0).collect::<Vec<_>>(), (0..playtime).collect::<Vec<_>>());
        // The same pieces, through one continued handle: tracks and the ranges between.
        let mut pieces = Vec::new();
        let mut prev: *mut SbTrackAudio = ptr::null_mut();
        let mut at = 0;
        for &(n, start, end) in tracks {
            if start > at {
                prev = range_open_next(disc, prev, slot, at, start).unwrap();
                pieces.extend(read_frames(prev, disc));
            }
            prev = audio_open_next(disc, prev, slot, n).unwrap();
            pieces.extend(read_frames(prev, disc));
            at = end;
        }
        if at < playtime {
            prev = range_open_next(disc, prev, slot, at, playtime).unwrap();
            pieces.extend(read_frames(prev, disc));
        }
        audio_free(prev);
        assert_eq!(pieces, whole, "slot {slot}");
    }
    free(disc);
}

#[test]
fn range_refusals() {
    // The tiny image: one 2-channel area of 10 frames' play time and 1 audio sector
    // (capacity 7), no audio frames.
    let disc = open(&tiny_image()).unwrap();
    for (first, end) in [(0, 0), (5, 2), (0, 11), (10, 11)] {
        assert_eq!(range_check(disc, 0, first, end), SbStatus::FrameRangeInvalid, "[{first}, {end})");
        assert_eq!(range_open(disc, 0, first, end).unwrap_err(), SbStatus::FrameRangeInvalid);
    }
    assert!(last_error().contains("frame range"), "{}", last_error());
    assert_eq!(status_name(SbStatus::FrameRangeInvalid as i32), "frame-range-invalid");
    assert_eq!(range_check(disc, 0, 0, 10), SbStatus::TrackExceedsCapacity);
    assert_eq!(range_check(disc, 1, 0, 1), SbStatus::NoSuchArea);
    assert_eq!(range_check(disc, 9, 0, 1), SbStatus::InvalidArgument);
    assert_eq!(range_check(ptr::null(), 0, 0, 1), SbStatus::InvalidArgument);
    // SAFETY: `disc` is a live handle; a NULL out-pointer is rejected.
    assert_eq!(unsafe { sb_range_audio_open(disc, 0, 0, 1, ptr::null_mut()) }, SbStatus::InvalidArgument);

    let a = range_open(disc, 0, 2, 7).unwrap();
    let frames = read_frames(a, disc);
    assert_eq!(frames.iter().map(|f| f.0).collect::<Vec<_>>(), vec![2, 3, 4, 5, 6]);
    assert!(frames.iter().all(|f| f.1 == SbFrameStatus::Missing as u32 && f.2.iter().all(|&b| b == 0x69)));
    let (mut missing, mut invalid) = (0u64, 0u64);
    // SAFETY: `a` is a live handle; both out-pointers are valid.
    assert_eq!(unsafe { sb_track_audio_counts(a, &mut missing, &mut invalid) }, SbStatus::Ok);
    assert_eq!((missing, invalid), (5, 0));
    // A refused range leaves `prev` untouched; a good one consumes it.
    assert_eq!(range_open_next(disc, a, 0, 0, 8).unwrap_err(), SbStatus::TrackExceedsCapacity);
    assert_eq!(range_open_next(disc, a, 0, 3, 3).unwrap_err(), SbStatus::FrameRangeInvalid);
    let b = range_open_next(disc, a, 0, 0, 7).unwrap();
    assert_eq!(read_frames(b, disc).len(), 7);
    // Tracks continue from ranges too.
    let c = audio_open_next(disc, b, 0, 1).unwrap();
    assert_eq!(read_frames(c, disc).len(), 2);
    audio_free(c);
    free(disc);
}
