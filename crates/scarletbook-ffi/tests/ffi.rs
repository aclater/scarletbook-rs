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
    hash_track_on(disc, slot, number, ptr::null())
}

/// `hash_track`, decoding on `pool` (NULL: on the calling thread).
fn hash_track_on(
    disc: *const SbDisc,
    slot: u32,
    number: u32,
    pool: *const SbPool,
) -> (Vec<String>, u64, u64) {
    let a = audio_open(disc, slot, number).unwrap_or_else(|s| panic!("{s:?}: {}", last_error()));
    assert_eq!(set_pool(a, pool), SbStatus::Ok);
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
    (hashers.into_iter().map(|h| hex(&h.finalize())).collect(), missing, invalid)
}

/// Lower-case hex of a digest (sha2 0.11's output has no `LowerHex`).
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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

/// Several discs decoded at once, one per thread, through the C ABI (as a ctypes caller
/// running several conversions would): each thread opens its own disc (from memory or
/// from a file), decodes every track with audio, on the calling thread or on a pool all
/// the threads share, and gets the manifest's hashes and counts. Each thread's error
/// message stays its own while the others fail and succeed around it, and a handle
/// opened on one thread is refused with another thread's disc.
#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn discs_decode_concurrently_on_threads() {
    const THREADS: usize = 8;
    let ids = ["dst-features-2ch-6ch", "dst-invalid-frames-5ch", "hostile-base", "dst-invalid-frames-6ch"];
    let pool = pool_new(2).unwrap();
    let shared = pool.expose_provenance();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(THREADS));
    let threads: Vec<_> = (0..THREADS)
        .map(|i| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let id = ids[i % ids.len()];
                let disc = if i % 2 == 0 {
                    open(&image(id)).unwrap()
                } else {
                    let path =
                        CString::new(corpus().join("images").join(format!("{id}.iso")).to_str().unwrap())
                            .unwrap();
                    let mut d = ptr::null_mut();
                    // SAFETY: `path` is NUL-terminated; `&mut d` is a valid out-pointer.
                    assert_eq!(unsafe { sb_disc_open_file(path.as_ptr(), &mut d) }, SbStatus::Ok);
                    d
                };
                let pool: *const SbPool =
                    if i % 4 < 2 { ptr::null() } else { ptr::with_exposed_provenance(shared) };
                // Every track with audio. A failure is raised only after the barriers below,
                // which the other threads would otherwise wait at forever.
                let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut first = None;
                    for area in expected(id)["audio"].as_array().unwrap() {
                        let slot = if area["area"] == "2ch" { 0 } else { 1 };
                        for t in area["tracks"].as_array().unwrap() {
                            let n = t["number"].as_u64().unwrap() as u32;
                            if t["outcome"] != "ok" {
                                assert_ne!(check(disc, slot, n), SbStatus::Ok);
                                continue;
                            }
                            let (sha, missing, invalid) = hash_track_on(disc, slot, n, pool);
                            let exp: Vec<_> =
                                t["sha256"].as_array().unwrap().iter().map(|h| h.as_str().unwrap()).collect();
                            assert_eq!(sha, exp, "thread {i}: {id} {slot} {n}");
                            assert_eq!(missing, t["missing_frames"].as_u64().unwrap());
                            assert_eq!(invalid, t["invalid_dst_frames"].as_u64().unwrap());
                            first.get_or_insert((slot, n));
                        }
                    }
                    first.unwrap_or_else(|| panic!("{id}: no track decoded"))
                }));
                let slot = area(disc, 0).1.slot;
                // Half the threads fail with a message of their own while the other half
                // fail and succeed; each message survives the other threads' calls.
                let mut messages = Vec::new();
                for phase in 0..2 {
                    if i % 2 == phase {
                        let number = 200 + i as u32;
                        let failed = track(disc, slot, number).0;
                        barrier.wait();
                        barrier.wait();
                        messages.push((failed, last_error(), format!("no track {number} in area")));
                    } else {
                        barrier.wait();
                        let failed = track(disc, slot, 255).0;
                        // A successful call clears the message.
                        let ok = info(disc).0;
                        let message = if ok == SbStatus::Ok { last_error() } else { format!("{ok:?}") };
                        messages.push((failed, message, String::new()));
                        barrier.wait();
                    }
                }
                let (slot, number) = decoded.unwrap_or_else(|e| std::panic::resume_unwind(e));
                for (failed, message, prefix) in messages {
                    assert_eq!(failed, SbStatus::NoSuchTrack);
                    if prefix.is_empty() {
                        assert_eq!(message, "");
                    } else {
                        assert!(message.starts_with(&prefix), "thread {i}: {message:?}");
                    }
                }
                let audio = audio_open(disc, slot, number).unwrap();
                (disc.expose_provenance(), audio.expose_provenance())
            })
        })
        .collect();
    let handles: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    // Every disc has an identity of its own: a track handle is refused with any other disc.
    let (disc, audio): (Vec<*mut SbDisc>, Vec<*mut SbTrackAudio>) = handles
        .iter()
        .map(|&(d, a)| (ptr::with_exposed_provenance_mut(d), ptr::with_exposed_provenance_mut(a)))
        .unzip();
    let mut buf = vec![0u8; 6 * 4704];
    for (i, &a) in audio.iter().enumerate() {
        let other = disc[(i + 1) % THREADS];
        assert_eq!(audio_read(a, other, &mut buf).0, SbStatus::InvalidArgument);
        assert!(last_error().contains("another disc"), "{}", last_error());
        audio_free(a);
    }
    disc.into_iter().for_each(free);
    pool_free(pool);
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

fn layout(disc: *const SbDisc, slot: u32) -> (SbStatus, u32) {
    let mut l = 99;
    // SAFETY: `disc` is NULL or a live handle; `&mut l` is a valid out-pointer.
    (unsafe { sb_area_loudspeaker_layout(disc, slot, &mut l) }, l)
}

fn speaker(disc: *const SbDisc, slot: u32, index: u32) -> (SbStatus, u32) {
    let mut s = 99;
    // SAFETY: `disc` is NULL or a live handle; `&mut s` is a valid out-pointer.
    (unsafe { sb_area_channel_speaker(disc, slot, index, &mut s) }, s)
}

#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn loudspeaker_layouts() {
    let (fl, fr, c, lfe, sl, sr) = (
        SB_SPEAKER_FRONT_LEFT,
        SB_SPEAKER_FRONT_RIGHT,
        SB_SPEAKER_CENTRE,
        SB_SPEAKER_LOW_FREQUENCY_EFFECTS,
        SB_SPEAKER_SURROUND_LEFT,
        SB_SPEAKER_SURROUND_RIGHT,
    );
    // A defined layout of each kind, 1, 3 and 4 channels, and combinations outside the
    // table of specification 3, 3.4.1 (0 and 255 channels, 6 with 0, 2 and 5 with 4).
    for id in [
        "dst-features-2ch-6ch",
        "dst-features-5ch",
        "dst-features-1ch",
        "dst-features-3ch",
        "dst-features-4ch",
        "area-channels-0",
        "area-channels-255",
        "area-channels-6-in-stereo",
        "area-mch-channels-2",
        "area-mch-channels-5",
    ] {
        let disc = open(&image(id)).unwrap();
        let exp = expected(id);
        let (_, i) = info(disc);
        assert_eq!(i.area_count as usize, exp["toc"]["areas"].as_array().unwrap().len(), "{id}");
        for index in 0..i.area_count {
            let (_, a) = area(disc, index);
            let e = &exp["toc"]["areas"][index as usize];
            let (channels, config) = (a.channel_count, a.area_config);
            assert_eq!(u64::from(channels), e["channel_count"].as_u64().unwrap(), "{id}");
            assert_eq!(u64::from(config), e["area_config"].as_u64().unwrap(), "{id}");
            let (want_layout, want): (u32, &[u32]) = match (channels, config & 7) {
                (2, 0) => (SB_LAYOUT_STEREO, &[fl, fr]),
                (5, 3) => (SB_LAYOUT_THREE_TWO, &[fl, fr, c, sl, sr]),
                (6, 4) => (SB_LAYOUT_FIVE_POINT_ONE, &[fl, fr, c, lfe, sl, sr]),
                _ => (SB_LAYOUT_UNSPECIFIED, &[]),
            };
            assert_eq!(layout(disc, a.slot), (SbStatus::Ok, want_layout), "{id} {}", a.slot);
            for ch in 0..channels {
                let w = want.get(ch as usize).copied().unwrap_or(SB_SPEAKER_UNSPECIFIED);
                assert_eq!(speaker(disc, a.slot, ch), (SbStatus::Ok, w), "{id} {} {ch}", a.slot);
            }
            assert_eq!(speaker(disc, a.slot, channels), (SbStatus::InvalidArgument, 99), "{id}");
            assert_eq!(speaker(disc, a.slot, u32::MAX).0, SbStatus::InvalidArgument, "{id}");
        }
        free(disc);
    }
    // Hostile arguments: no area, a bad slot, NULL pointers.
    let disc = open(&image("dst-features-5ch")).unwrap();
    assert_eq!(layout(disc, 0), (SbStatus::NoSuchArea, 99));
    assert_eq!(speaker(disc, 0, 0), (SbStatus::NoSuchArea, 99));
    assert_eq!(layout(disc, 9).0, SbStatus::InvalidArgument);
    assert_eq!(speaker(disc, 9, 0).0, SbStatus::InvalidArgument);
    assert_eq!(layout(ptr::null(), 1).0, SbStatus::InvalidArgument);
    assert_eq!(speaker(ptr::null(), 1, 0).0, SbStatus::InvalidArgument);
    // SAFETY: `disc` is a live handle; NULL out-pointers are rejected.
    assert_eq!(unsafe { sb_area_loudspeaker_layout(disc, 1, ptr::null_mut()) }, SbStatus::InvalidArgument);
    // SAFETY: as above.
    assert_eq!(unsafe { sb_area_channel_speaker(disc, 1, 0, ptr::null_mut()) }, SbStatus::InvalidArgument);
    free(disc);
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
    raw: *mut SbRawFrames,
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
        // SAFETY: `u.audio` is a live handle; a NULL pool is allowed.
        u.probes.push(unsafe { sb_track_audio_set_pool(u.audio, ptr::null()) });
        if !u.raw.is_null() {
            u.probes.push(raw_read(u.raw, u.disc, &mut [0u8; 2 * 4704]).0);
            u.probes.push(cb_read_dst_equivalent(u.raw, u.disc));
            let mut n = 0u64;
            // SAFETY: `u.raw` is a live handle; `&mut n` is a valid out-pointer.
            u.probes.push(unsafe { sb_raw_frames_missing(u.raw, &mut n) });
            // SAFETY: as above.
            u.probes.push(unsafe { sb_raw_frames_malformed_sectors(u.raw, &mut n) });
            u.probes.push(raw_open_next(u.disc, u.raw, 0, 0, 1).map_or_else(|s| s, |_| SbStatus::Ok));
            raw_free(u.raw); // must be a no-op
        }
        free(u.disc); // must be a no-op
        audio_free(u.audio); // must be a no-op
    }
    // SAFETY: the library passes a buffer valid for `len` bytes of writes.
    let out = unsafe { std::slice::from_raw_parts_mut(buf, len) };
    u.cursor.read(out).map_or(-1, |n| n as i64)
}

fn cb_read_dst_equivalent(raw: *mut SbRawFrames, disc: *mut SbDisc) -> SbStatus {
    let mut buf = [0u8; 16];
    let mut fi = SbRawFrameInfo::default();
    // SAFETY: the handles are live; `buf` is valid for its length; `&mut fi` is valid.
    unsafe { sb_raw_frames_read_dst_equivalent(raw, disc, buf.as_mut_ptr(), buf.len(), &mut fi) }
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
        raw: ptr::null_mut(),
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

    // The same from inside a raw frames read: the raw handle and the disc are in use.
    // The track handle is not (only its disc is), so the callback must not be given it: it
    // would really free it.
    let r = raw_open(disc, 1, 0, 4).unwrap();
    user.probes.clear();
    user.disc = disc;
    user.audio = ptr::null_mut();
    user.raw = r;
    let mut big = vec![0u8; SB_MAX_FRAME_BYTES];
    let (s, fi) = raw_read(r, disc, &mut big);
    user.disc = ptr::null_mut();
    user.raw = ptr::null_mut();
    assert_eq!((s, fi.number, fi.kind), (SbStatus::Ok, 0, SbFrameKind::Dsd as u32));
    assert!(user.probes.len() >= 8, "{:?}", user.probes);
    assert!(user.probes.iter().all(|&s| s == SbStatus::InvalidArgument), "{:?}", user.probes);
    // The raw handle survived the attempted free and reads on.
    assert_eq!(raw_read(r, disc, &mut big).0, SbStatus::Ok);
    raw_free(r);
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
    hashers.into_iter().map(|h| hex(&h.finalize())).collect()
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
    assert_eq!(std::mem::size_of::<SbStatus>(), 4);
    assert_eq!(SB_MAX_FRAME_BYTES, 65_536);
}

/// Asserts a struct's size, alignment and the offset of each of its fields, in order.
macro_rules! assert_layout {
    ($t:ty, size $size:expr, align $align:expr, { $($field:ident: $offset:expr),* $(,)? }) => {
        assert_eq!(std::mem::size_of::<$t>(), $size, "size of {}", stringify!($t));
        assert_eq!(std::mem::align_of::<$t>(), $align, "alignment of {}", stringify!($t));
        $(assert_eq!(
            std::mem::offset_of!($t, $field), $offset, "{}.{}", stringify!($t), stringify!($field)
        );)*
    };
}

/// The struct layouts documented in the header (each struct's "Layout" line), on 64-bit
/// targets: every release target. Callers allocate the structs, so a change to any of
/// these numbers is a breaking change (README, "ABI compatibility").
#[test]
#[cfg(target_pointer_width = "64")]
fn struct_layouts_on_64_bit_targets() {
    assert_layout!(SbDiscInfo, size 40, align 8, {
        sector_size: 0, sectors: 8, version_major: 16, version_minor: 17, album_set_size: 18,
        album_sequence_number: 20, hybrid: 22, manufacturer_sector: 23, year: 24, month: 26,
        day: 27, area_count: 28, warning_count: 32,
    });
    assert_layout!(SbAreaInfo, size 52, align 4, {
        slot: 0, kind_channels: 4, toc_lsn: 8, toc_sectors: 12, channel_count: 16,
        area_config: 20, sample_frequency_code: 24, frame_format: 28, max_byte_rate: 32,
        playtime_frames: 36, audio_start_lsn: 40, audio_end_lsn: 44, track_count: 48,
    });
    assert_layout!(SbTrackInfo, size 28, align 4, {
        number: 0, start_lsn: 4, length_sectors: 8, start_frame: 12, duration_frames: 16,
        start_flags: 20, duration_flags: 21, has_genre: 22, genre_category: 23, genre_index: 24,
    });
    assert_layout!(SbWarning, size 24, align 8, { code: 0, slot: 8, track: 12, lsn: 16 });
    assert_layout!(SbFrameInfo, size 24, align 8, { number: 0, status: 4, dst_rule: 8, size: 16 });
    assert_layout!(SbRawFrameInfo, size 16, align 8, { number: 0, kind: 4, size: 8 });
    // The C types of the fields that are not fixed-width integers.
    assert_eq!(std::mem::size_of::<bool>(), 1);
    assert_eq!(std::mem::size_of::<usize>(), 8);
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

fn with_pause(disc: *const SbDisc, slot: u32, number: u32) -> Result<(u32, u32), SbStatus> {
    let (mut a, mut b) = (u32::MAX, u32::MAX);
    // SAFETY: `disc` is NULL or a live handle; both out-pointers are valid.
    match unsafe { sb_track_range_with_pause(disc, slot, number, &mut a, &mut b) } {
        SbStatus::Ok => Ok((a, b)),
        s => Err(s),
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
        // Tracks with pauses: track 1 from 0, each to the next track's start.
        for (i, &(n, start, _)) in tracks.iter().enumerate() {
            let a = if n == 1 { 0 } else { start };
            let b = tracks.get(i + 1).map_or(playtime, |t| t.1);
            assert_eq!(with_pause(disc, slot, n), Ok((a, b)), "slot {slot} track {n}");
            let h = range_open(disc, slot, a, b).unwrap();
            assert_eq!(read_frames(h, disc), whole[a as usize..b as usize]);
            audio_free(h);
        }
        assert_eq!(with_pause(disc, slot, 99), Err(SbStatus::NoSuchTrack));
    }
    free(disc);
}

#[test]
fn range_refusals() {
    // The tiny image: one 2-channel area of 10 frames' play time and 1 audio sector
    // (capacity 7), no audio frames.
    let disc = open(&tiny_image()).unwrap();
    for (first, end) in [(5, 2), (0, 11), (10, 11), (11, 11)] {
        assert_eq!(range_check(disc, 0, first, end), SbStatus::FrameRangeInvalid, "[{first}, {end})");
        assert_eq!(range_open(disc, 0, first, end).unwrap_err(), SbStatus::FrameRangeInvalid);
    }
    assert!(last_error().contains("frame range"), "{}", last_error());
    assert_eq!(range_check(disc, 0, 0, 0), SbStatus::TrackEmpty);
    assert_eq!(range_open(disc, 0, 4, 4).unwrap_err(), SbStatus::TrackEmpty);
    assert_eq!(status_name(SbStatus::FrameRangeInvalid as i32), "frame-range-invalid");
    assert_eq!(range_check(disc, 0, 0, 10), SbStatus::TrackExceedsCapacity);
    assert_eq!(range_check(disc, 1, 0, 1), SbStatus::NoSuchArea);
    assert_eq!(range_check(disc, 9, 0, 1), SbStatus::InvalidArgument);
    assert_eq!(range_check(ptr::null(), 0, 0, 1), SbStatus::InvalidArgument);
    assert_eq!(with_pause(disc, 0, 1), Ok((0, 10)));
    assert_eq!(with_pause(disc, 1, 1), Err(SbStatus::NoSuchArea));
    let mut x = 0;
    // SAFETY: `disc` is a live handle; one out-pointer is valid, the other NULL (rejected).
    let s = unsafe { sb_track_range_with_pause(disc, 0, 1, &mut x, ptr::null_mut()) };
    assert_eq!(s, SbStatus::InvalidArgument);
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
    assert_eq!(range_open_next(disc, a, 0, 3, 3).unwrap_err(), SbStatus::TrackEmpty);
    assert_eq!(range_open_next(disc, a, 0, 3, 2).unwrap_err(), SbStatus::FrameRangeInvalid);
    let b = range_open_next(disc, a, 0, 0, 7).unwrap();
    assert_eq!(read_frames(b, disc).len(), 7);
    // Tracks continue from ranges too.
    let c = audio_open_next(disc, b, 0, 1).unwrap();
    assert_eq!(read_frames(c, disc).len(), 2);
    audio_free(c);
    free(disc);
}

/// What a log callback received, and how to misbehave.
#[derive(Default)]
struct LogUser {
    lines: std::sync::Mutex<Vec<(std::thread::ThreadId, i32, String)>>,
    /// If set, the callback tries to replace the log of this disc and records the status.
    disc: std::sync::atomic::AtomicPtr<SbDisc>,
    reentry: std::sync::Mutex<Vec<SbStatus>>,
    /// If set, the callback tries to read and free this track handle (of `victim_disc`)
    /// and records the status of the read.
    victim: std::sync::atomic::AtomicPtr<SbTrackAudio>,
    victim_disc: std::sync::atomic::AtomicPtr<SbDisc>,
    victim_reads: std::sync::Mutex<Vec<SbStatus>>,
    /// As `victim`, for a raw frames handle.
    raw_victim: std::sync::atomic::AtomicPtr<SbRawFrames>,
}

impl LogUser {
    fn lines(&self) -> Vec<(i32, String)> {
        self.lines.lock().unwrap().iter().map(|(_, l, m)| (*l, m.clone())).collect()
    }

    fn has(&self, level: i32, text: &str) -> bool {
        self.lines().iter().any(|(l, m)| *l == level && m == text)
    }
}

unsafe extern "C" fn cb_log(user: *mut c_void, level: i32, message: *const c_char) {
    // SAFETY: `user` is the `LogUser` given with this callback, alive while the disc is.
    let user = unsafe { &*user.cast::<LogUser>() };
    // SAFETY: the library passes a valid NUL-terminated string for the call.
    let m = unsafe { CStr::from_ptr(message) }.to_str().unwrap().to_owned();
    user.lines.lock().unwrap().push((std::thread::current().id(), level, m));
    let disc = user.disc.swap(ptr::null_mut(), std::sync::atomic::Ordering::Relaxed);
    if !disc.is_null() {
        // SAFETY: `disc` is the live disc whose call invoked this callback, on this thread.
        let s = unsafe { sb_disc_set_log(disc, None, ptr::null_mut(), SB_LOG_OFF) };
        user.reentry.lock().unwrap().push(s);
    }
    let victim = user.victim.swap(ptr::null_mut(), std::sync::atomic::Ordering::Relaxed);
    if !victim.is_null() {
        let disc = user.victim_disc.load(std::sync::atomic::Ordering::Relaxed);
        let mut buf = vec![0u8; 2 * 4704];
        let mut fi = SbFrameInfo::default();
        // SAFETY: `victim` is the live `prev` handle of the call that invoked this callback
        // (on this thread), `disc` its live disc; `buf` and `fi` are valid.
        let s = unsafe { sb_track_audio_read(victim, disc, buf.as_mut_ptr(), buf.len(), &mut fi) };
        user.victim_reads.lock().unwrap().push(s);
        // SAFETY: as above; the library must ignore the free of a handle in use.
        unsafe { sb_track_audio_free(victim) };
    }
    let raw = user.raw_victim.swap(ptr::null_mut(), std::sync::atomic::Ordering::Relaxed);
    if !raw.is_null() {
        let disc = user.victim_disc.load(std::sync::atomic::Ordering::Relaxed);
        let mut buf = vec![0u8; 2 * 4704];
        let mut fi = SbRawFrameInfo::default();
        // SAFETY: `raw` is the live `prev` handle of the call that invoked this callback
        // (on this thread), `disc` its live disc; `buf` and `fi` are valid.
        let s = unsafe { sb_raw_frames_read(raw, disc, buf.as_mut_ptr(), buf.len(), &mut fi) };
        user.victim_reads.lock().unwrap().push(s);
        // SAFETY: as above; the library must ignore the free of a handle in use.
        unsafe { sb_raw_frames_free(raw) };
    }
}

fn open_logged(bytes: &[u8], user: &LogUser, level: i32) -> Result<*mut SbDisc, SbStatus> {
    let mut d: *mut SbDisc = ptr::dangling_mut();
    let u = ptr::from_ref(user).cast_mut().cast::<c_void>();
    // SAFETY: `bytes` is valid for its length; `cb_log` may be called with `u` from any
    // thread while `user` lives, which outlives the disc in every test.
    let s =
        unsafe { sb_disc_open_memory_with_log(bytes.as_ptr(), bytes.len(), Some(cb_log), u, level, &mut d) };
    match s {
        SbStatus::Ok => Ok(d),
        s => {
            assert!(d.is_null());
            Err(s)
        }
    }
}

fn set_log(disc: *const SbDisc, user: Option<&LogUser>, level: i32) -> SbStatus {
    let (cb, u) = match user {
        Some(u) => {
            (Some(cb_log as unsafe extern "C" fn(_, _, _)), ptr::from_ref(u).cast_mut().cast::<c_void>())
        }
        None => (None, ptr::null_mut()),
    };
    // SAFETY: `disc` is a live handle; `cb_log` may be called with `u` while `user` lives.
    unsafe { sb_disc_set_log(disc, cb, u, level) }
}

/// Small enough for Miri: the callback, its levels, replacing it and re-entering.
#[test]
fn log_callback_levels_and_replacement() {
    let user = LogUser::default();
    let disc = open_logged(&tiny_image(), &user, SB_LOG_INFO).unwrap();
    assert!(user.has(SB_LOG_INFO, "master TOC at LSN 510 used, version 1.0"), "{:?}", user.lines());
    assert!(user.lines().iter().all(|(l, _)| (SB_LOG_ERROR..=SB_LOG_INFO).contains(l)));
    assert!(!user.lines().is_empty());

    // Re-entering from the callback, on the calling thread, is refused cleanly.
    user.disc.store(disc, std::sync::atomic::Ordering::Relaxed);
    let a = audio_open(disc, 0, 1).unwrap();
    assert_eq!(*user.reentry.lock().unwrap(), [SbStatus::InvalidArgument]);
    let mut buf = vec![0u8; 2 * 4704];
    assert_eq!(audio_read(a, disc, &mut buf).0, SbStatus::Ok);
    assert!(user.has(SB_LOG_WARN, "area 2ch frame 0: missing, filled with 0x69 (4.6)"));

    // Off: nothing more.
    assert_eq!(set_log(disc, Some(&user), 6), SbStatus::InvalidArgument);
    assert_eq!(set_log(disc, Some(&user), -1), SbStatus::InvalidArgument);
    assert_eq!(set_log(disc, Some(&user), SB_LOG_OFF), SbStatus::Ok);
    let before = user.lines().len();
    assert_eq!(audio_read(a, disc, &mut buf).0, SbStatus::Ok);
    assert_eq!(user.lines().len(), before);

    // Replaced, at the trace level, by another callback's user data.
    let other = LogUser::default();
    assert_eq!(set_log(disc, Some(&other), SB_LOG_TRACE), SbStatus::Ok);
    assert_eq!(audio_read(a, disc, &mut buf).0, SbStatus::End);
    assert_eq!(user.lines().len(), before);
    assert!(
        other.lines().iter().all(|(l, _)| *l == SB_LOG_TRACE || *l == SB_LOG_DEBUG),
        "{:?}",
        other.lines()
    );
    assert_eq!(set_log(disc, None, SB_LOG_TRACE), SbStatus::Ok);
    assert_eq!(set_log(ptr::null(), None, SB_LOG_OFF), SbStatus::InvalidArgument);
    audio_free(a);
    free(disc);

    // A bad level fails the open; a NULL callback or SB_LOG_OFF opens without a log.
    assert_eq!(open_logged(&tiny_image(), &user, 9).unwrap_err(), SbStatus::InvalidArgument);
    let quiet = LogUser::default();
    free(open_logged(&tiny_image(), &quiet, SB_LOG_OFF).unwrap());
    assert!(quiet.lines().is_empty());
    let rejected = LogUser::default();
    assert_eq!(open_logged(&[0u8; 4096], &rejected, SB_LOG_INFO).unwrap_err(), SbStatus::NotScarletbook);
    assert!(rejected.has(SB_LOG_INFO, "no sector format matches: not a Scarletbook image"));
}

#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn log_callback_reports_reads_and_frame_problems() {
    let user = LogUser::default();
    let disc = open_logged(&image("dst-invalid-frames-2ch"), &user, SB_LOG_DEBUG).unwrap();
    assert!(user.has(
        SB_LOG_INFO,
        "sector format Plain detected (signature at byte 1044480): 2048 bytes per sector, data at offset 0, 615 \
         sectors"
    ));
    assert!(user.has(SB_LOG_DEBUG, "read 10 sectors at LSN 510: bytes 1044480..1064960"));
    assert!(user.has(SB_LOG_INFO, "area 2ch: TOC-1 at LSN 540 used (TOC-2 identical)"));
    let a = audio_open(disc, 0, 1).unwrap();
    let mut buf = vec![0u8; 2 * 4704];
    let mut expected = Vec::new();
    loop {
        let (s, fi) = audio_read(a, disc, &mut buf);
        if s == SbStatus::End {
            break;
        }
        assert_eq!(s, SbStatus::Ok);
        if fi.status == SbFrameStatus::Invalid as u32 {
            expected.push(fi.number);
        }
    }
    let logged: Vec<u32> = user
        .lines()
        .iter()
        .filter(|(l, m)| *l == SB_LOG_WARN && m.contains("invalid DST frame"))
        .map(|(_, m)| m.split(' ').nth(3).unwrap().trim_end_matches(':').parse().unwrap())
        .collect();
    assert!(!expected.is_empty());
    assert_eq!(logged, expected);
    assert!(user.has(SB_LOG_DEBUG, "area 2ch: scanning audio sectors LSN 546..609"));
    assert!(user.lines().iter().all(|(l, _)| *l != SB_LOG_TRACE));
    audio_free(a);
    free(disc);
}

/// With the `parallel` feature the callback also runs on the pool's threads, and no longer
/// once the disc is freed, even with decoding jobs still in flight. Which of the pool's
/// threads decodes a group is the scheduler's choice: the caller is one of them and may decode
/// every group itself while it waits (a runner with few cores, #80), so only "on this pool" is
/// checked.
#[cfg(feature = "parallel")]
#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn log_callback_from_the_pool() {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(4).build().unwrap();
    let pool_threads: std::collections::HashSet<_> =
        pool.broadcast(|_| std::thread::current().id()).into_iter().collect();
    let user = LogUser::default();
    let caller = pool.install(|| {
        let disc = open_logged(&image("dst-features-2ch-6ch"), &user, SB_LOG_TRACE).unwrap();
        let a = audio_open(disc, 1, 1).unwrap();
        let mut buf = vec![0u8; 6 * 4704];
        while audio_read(a, disc, &mut buf).0 == SbStatus::Ok {}
        audio_free(a);
        // Free with jobs in flight: open the next track, read one frame, free at once.
        let a = audio_open(disc, 1, 2).unwrap();
        assert_eq!(audio_read(a, disc, &mut buf).0, SbStatus::Ok);
        audio_free(a);
        free(disc);
        std::thread::current().id()
    });
    let after_free = user.lines.lock().unwrap().len();
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert_eq!(user.lines.lock().unwrap().len(), after_free, "callback after sb_disc_free");
    let lines = user.lines.lock().unwrap();
    let groups: Vec<_> =
        lines.iter().filter(|(_, _, m)| m.starts_with("decoding DST frames")).map(|(t, _, _)| t).collect();
    assert!(!groups.is_empty(), "no group decoded");
    assert!(groups.iter().all(|t| pool_threads.contains(t)), "a group decoded off the pool");
    // Frame lines come from the calling thread, in order.
    let frames: Vec<u32> = lines
        .iter()
        .filter(|(_, l, m)| {
            *l == SB_LOG_TRACE && m.starts_with("area multichannel frame ") && !m.contains("(from")
        })
        .map(|(t, _, m)| {
            assert_eq!(*t, caller);
            m.split(' ').nth(3).unwrap().trim_end_matches(':').parse().unwrap()
        })
        .collect();
    assert!(frames.windows(2).all(|w| w[0] < w[1]) && !frames.is_empty(), "{frames:?}");
}

/// A log callback that runs while `sb_track_audio_open_next` or `sb_range_audio_open_next`
/// consumes `prev` cannot reach it: reading it fails and freeing it does nothing (Miri
/// checks that nothing is freed twice or read after being moved).
#[test]
fn log_callback_cannot_reach_a_handle_being_consumed() {
    let user = LogUser::default();
    let disc = open_logged(&tiny_image(), &user, SB_LOG_INFO).unwrap();
    user.victim_disc.store(disc, std::sync::atomic::Ordering::Relaxed);
    let a = audio_open(disc, 0, 1).unwrap();
    let mut buf = vec![0u8; 2 * 4704];
    assert_eq!(audio_read(a, disc, &mut buf).0, SbStatus::Ok);
    // Into a track.
    user.victim.store(a, std::sync::atomic::Ordering::Relaxed);
    let b = audio_open_next(disc, a, 0, 1).unwrap();
    assert_eq!(*user.victim_reads.lock().unwrap(), [SbStatus::InvalidArgument]);
    assert!(user.victim.load(std::sync::atomic::Ordering::Relaxed).is_null(), "the callback ran");
    assert_eq!(read_frames(b, disc).len(), 2);
    // Into a range.
    user.victim.store(b, std::sync::atomic::Ordering::Relaxed);
    let c = range_open_next(disc, b, 0, 0, 7).unwrap();
    assert_eq!(*user.victim_reads.lock().unwrap(), [SbStatus::InvalidArgument; 2]);
    assert_eq!(read_frames(c, disc).len(), 7);
    // A refusal is logged while `prev` is still the caller's: it stays usable.
    user.victim.store(c, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(range_open_next(disc, c, 0, 0, 11).unwrap_err(), SbStatus::FrameRangeInvalid);
    assert_eq!(*user.victim_reads.lock().unwrap(), [SbStatus::InvalidArgument; 3]);
    assert_eq!(audio_read(c, disc, &mut buf).0, SbStatus::End);
    audio_free(c);
    free(disc);
}

fn raw_open_next(
    disc: *const SbDisc,
    prev: *mut SbRawFrames,
    slot: u32,
    first: u32,
    end: u32,
) -> Result<*mut SbRawFrames, SbStatus> {
    let mut r: *mut SbRawFrames = ptr::dangling_mut();
    // SAFETY: `disc` is NULL or a live handle; `prev` is NULL or a live handle that the
    // caller does not use again if the call succeeds; `&mut r` is a valid out-pointer.
    match unsafe { sb_raw_frames_open_next(disc, prev, slot, first, end, &mut r) } {
        SbStatus::Ok => Ok(r),
        s => {
            assert!(r.is_null(), "*out not cleared on failure");
            Err(s)
        }
    }
}

fn raw_open(disc: *const SbDisc, slot: u32, first: u32, end: u32) -> Result<*mut SbRawFrames, SbStatus> {
    let mut r: *mut SbRawFrames = ptr::dangling_mut();
    // SAFETY: `disc` is NULL or a live handle; `&mut r` is a valid out-pointer.
    match unsafe { sb_raw_frames_open(disc, slot, first, end, &mut r) } {
        SbStatus::Ok => Ok(r),
        s => {
            assert!(r.is_null(), "*out not cleared on failure");
            Err(s)
        }
    }
}

fn raw_read(raw: *const SbRawFrames, disc: *const SbDisc, buf: &mut [u8]) -> (SbStatus, SbRawFrameInfo) {
    let mut fi = SbRawFrameInfo::default();
    // SAFETY: the handles are NULL or live; `buf` is valid for its length; `&mut fi` is a
    // valid out-pointer.
    (unsafe { sb_raw_frames_read(raw, disc, buf.as_mut_ptr(), buf.len(), &mut fi) }, fi)
}

fn raw_missing(raw: *const SbRawFrames) -> u64 {
    let mut n = u64::MAX;
    // SAFETY: `raw` is a live handle; `&mut n` is a valid out-pointer.
    assert_eq!(unsafe { sb_raw_frames_missing(raw, &mut n) }, SbStatus::Ok);
    n
}

fn raw_free(raw: *mut SbRawFrames) {
    // SAFETY: `raw` is NULL or a live handle that the caller does not use again.
    unsafe { sb_raw_frames_free(raw) };
}

/// Reads the rest of a raw handle: each position's number, kind and bytes.
fn read_raw(raw: *mut SbRawFrames, disc: *const SbDisc) -> Vec<(u32, u32, Vec<u8>)> {
    let mut buf = vec![0u8; SB_MAX_FRAME_BYTES];
    let mut out = Vec::new();
    loop {
        let (s, fi) = raw_read(raw, disc, &mut buf);
        match s {
            SbStatus::End => return out,
            SbStatus::Ok => out.push((fi.number, fi.kind, buf[..fi.size].to_vec())),
            s => panic!("{s:?}: {}", last_error()),
        }
    }
}

#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn raw_dst_frames_decode_to_the_decoded_read() {
    let mut dst = 0;
    for id in ["dst-features-2ch-6ch", "dst-invalid-frames-5ch", "dst-stereo-5ch-2064", "dsd-stereo-mch-2054"]
    {
        let disc = open(&image(id)).unwrap();
        let (_, i) = info(disc);
        for index in 0..i.area_count {
            let (_, a) = area(disc, index);
            let (slot, playtime) = (a.slot, a.playtime_frames);
            let r = raw_open(disc, slot, 0, playtime).unwrap();
            let ch = sb_raw_channels(r);
            assert_eq!(ch, a.channel_count);
            let raw = read_raw(r, disc);
            let e = raw_open(disc, slot, 0, playtime).unwrap();
            let equivalents = read_equivalents(e, disc);
            raw_free(e);
            let h = range_open(disc, slot, 0, playtime).unwrap();
            let decoded = read_frames(h, disc);
            let (mut missing, mut invalid) = (0u64, 0u64);
            // SAFETY: `h` is a live handle; both out-pointers are valid.
            assert_eq!(unsafe { sb_track_audio_counts(h, &mut missing, &mut invalid) }, SbStatus::Ok);
            assert_eq!(raw_missing(r), missing);
            audio_free(h);
            raw_free(r);
            assert_eq!(raw.len(), decoded.len());
            let mut out = vec![0u8; ch as usize * 4704];
            for (((n, kind, bytes), (dn, status, data)), (en, ekind, equivalent)) in
                raw.iter().zip(&decoded).zip(&equivalents)
            {
                assert_eq!((n, kind), (en, ekind));
                assert_eq!(n, dn);
                // The DST equivalent decodes to the position's audio, whatever its kind.
                let mut rule = 0;
                // SAFETY: `equivalent` and `out` are valid for their lengths and do not
                // overlap; `&mut rule` is a valid out-pointer.
                let s = unsafe {
                    sb_dst_decode(
                        equivalent.as_ptr(),
                        equivalent.len(),
                        ch,
                        out.as_mut_ptr(),
                        out.len(),
                        &mut rule,
                    )
                };
                if s == SbStatus::Ok {
                    assert_eq!(&out, data, "{id} {n}");
                } else {
                    assert_eq!(*status, SbFrameStatus::Invalid as u32, "{id} {n}");
                }
                if *kind != SbFrameKind::Dst as u32 {
                    assert_eq!(equivalent.len(), 1 + ch as usize * 4704);
                    assert_eq!(equivalent[0], 0);
                }
                match *kind {
                    k if k == SbFrameKind::Dsd as u32 => {
                        assert_eq!((status, bytes), (&(SbFrameStatus::Dsd as u32), data));
                    }
                    k if k == SbFrameKind::Missing as u32 => {
                        assert!(bytes.is_empty());
                        assert_eq!(*status, SbFrameStatus::Missing as u32);
                    }
                    k => {
                        assert_eq!(k, SbFrameKind::Dst as u32);
                        let mut rule = 0;
                        // SAFETY: `bytes` and `out` are valid for their lengths and do not
                        // overlap; `&mut rule` is a valid out-pointer.
                        let s = unsafe {
                            sb_dst_decode(
                                bytes.as_ptr(),
                                bytes.len(),
                                ch,
                                out.as_mut_ptr(),
                                out.len(),
                                &mut rule,
                            )
                        };
                        if s == SbStatus::Ok {
                            assert_eq!((status, &out), (&(SbFrameStatus::Decoded as u32), data), "{id} {n}");
                        } else {
                            assert_eq!(s, SbStatus::InvalidDstFrame);
                            assert_eq!(*status, SbFrameStatus::Invalid as u32, "{id} {n}");
                            assert!(data.iter().all(|&b| b == 0x55));
                        }
                        dst += 1;
                    }
                }
            }
        }
        free(disc);
    }
    assert!(dst > 50, "{dst}");
}

/// Reads the rest of a raw handle as DST equivalents.
fn read_equivalents(raw: *mut SbRawFrames, disc: *const SbDisc) -> Vec<(u32, u32, Vec<u8>)> {
    let mut buf = vec![0u8; SB_MAX_FRAME_BYTES];
    let mut out = Vec::new();
    loop {
        let mut fi = SbRawFrameInfo::default();
        // SAFETY: the handles are live; `buf` is valid for its length; `&mut fi` is a valid
        // out-pointer.
        let s = unsafe { sb_raw_frames_read_dst_equivalent(raw, disc, buf.as_mut_ptr(), buf.len(), &mut fi) };
        match s {
            SbStatus::End => return out,
            SbStatus::Ok => out.push((fi.number, fi.kind, buf[..fi.size].to_vec())),
            s => panic!("{s:?}: {}", last_error()),
        }
    }
}

fn sb_raw_channels(r: *const SbRawFrames) -> u32 {
    // SAFETY: `r` is a live handle.
    unsafe { sb_raw_frames_channels(r) }
}

#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn raw_frames_buffers_and_continuation() {
    let id = "dst-features-2ch-6ch";
    let disc = open(&image(id)).unwrap();
    let other = open(&image(id)).unwrap();
    let r = raw_open(disc, 0, 0, 35).unwrap();
    // Too small: the frame is described, not copied, and returned again by the next call.
    let mut small = [0u8; 4];
    let (s, fi) = raw_read(r, disc, &mut small);
    assert_eq!((s, fi.number, fi.kind), (SbStatus::BufferTooSmall, 0, SbFrameKind::Dst as u32));
    assert!(fi.size > 4 && fi.size <= SB_MAX_FRAME_BYTES);
    let mut fi2 = SbRawFrameInfo::default();
    // SAFETY: `r` and `disc` are live handles; a NULL buffer is a size query.
    let s = unsafe { sb_raw_frames_read(r, disc, ptr::null_mut(), 0, &mut fi2) };
    assert_eq!(s, SbStatus::BufferTooSmall);
    assert_eq!((fi2.number, fi2.size), (0, fi.size));
    let mut buf = vec![0u8; fi.size];
    assert_eq!(raw_read(r, disc, &mut buf).0, SbStatus::Ok);
    assert_eq!(raw_read(r, other, &mut buf).0, SbStatus::InvalidArgument);
    assert_eq!(raw_read(ptr::null(), disc, &mut buf).0, SbStatus::InvalidArgument);
    // Frame 1 is an uncoded frame inside the DST area: a DST frame of 1 + 2 x 4704 bytes,
    // returned as stored.
    let mut big = vec![0u8; SB_MAX_FRAME_BYTES];
    let (s, fi) = raw_read(r, disc, &mut big);
    assert_eq!((s, fi.number, fi.kind, fi.size), (SbStatus::Ok, 1, SbFrameKind::Dst as u32, 1 + 2 * 4704));

    // Refusals leave `prev` untouched; a continued handle reads what a fresh one does.
    assert_eq!(raw_open_next(disc, r, 0, 5, 5).unwrap_err(), SbStatus::TrackEmpty);
    assert_eq!(raw_open_next(disc, r, 0, 0, 36).unwrap_err(), SbStatus::FrameRangeInvalid);
    assert_eq!(raw_open_next(other, r, 0, 2, 11).unwrap_err(), SbStatus::InvalidArgument);
    assert_eq!(raw_open_next(disc, r, 7, 2, 11).unwrap_err(), SbStatus::InvalidArgument);
    let fresh = raw_open(disc, 0, 2, 11).unwrap();
    let expected = read_raw(fresh, disc);
    raw_free(fresh);
    let c = raw_open_next(disc, r, 0, 2, 11).unwrap();
    assert_eq!(read_raw(c, disc), expected);
    let d = raw_open_next(disc, c, 0, 11, 22).unwrap();
    assert_eq!(read_raw(d, disc).first().map(|f| f.0), Some(11));
    raw_free(d);
    raw_free(ptr::null_mut());
    free(disc);
    free(other);
}

#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn raw_frames_report_malformed_sectors_as_decoded_reads() {
    let disc = open(&image("audio-random")).unwrap();
    let (_, a) = area(disc, 0);
    let r = raw_open(disc, a.slot, 0, a.playtime_frames).unwrap();
    let h = range_open(disc, a.slot, 0, a.playtime_frames).unwrap();
    let raw = read_raw(r, disc);
    let decoded = read_frames(h, disc);
    assert_eq!(raw.iter().map(|f| f.0).collect::<Vec<_>>(), decoded.iter().map(|f| f.0).collect::<Vec<_>>());
    let (mut rc, mut dc) = (0u64, 0u64);
    // SAFETY: the handles are live; the out-pointers are valid.
    assert_eq!(unsafe { sb_raw_frames_malformed_sectors(r, &mut rc) }, SbStatus::Ok);
    // SAFETY: as above.
    assert_eq!(unsafe { sb_track_audio_malformed_sectors(h, &mut dc) }, SbStatus::Ok);
    assert!(rc > 0);
    assert_eq!(rc, dc);
    let mut w = SbWarning { code: ptr::null(), slot: 0, track: 0, lsn: 0 };
    let mut v = w;
    // SAFETY: as above.
    assert_eq!(unsafe { sb_raw_frames_warning(r, 0, &mut w) }, SbStatus::Ok);
    // SAFETY: as above.
    assert_eq!(unsafe { sb_track_audio_warning(h, 0, &mut v) }, SbStatus::Ok);
    assert_eq!((w.code, w.slot, w.lsn), (v.code, v.slot, v.lsn));
    // SAFETY: as above; the index is past the end.
    assert_eq!(unsafe { sb_raw_frames_warning(r, u32::MAX, &mut w) }, SbStatus::InvalidArgument);
    raw_free(r);
    audio_free(h);
    free(disc);
}

#[test]
fn raw_frames_of_the_tiny_image() {
    let disc = open(&tiny_image()).unwrap();
    assert_eq!(raw_open(disc, 0, 0, 10).unwrap_err(), SbStatus::TrackExceedsCapacity);
    assert_eq!(raw_open(disc, 0, 3, 3).unwrap_err(), SbStatus::TrackEmpty);
    assert_eq!(raw_open(disc, 0, 3, 2).unwrap_err(), SbStatus::FrameRangeInvalid);
    assert_eq!(raw_open(disc, 1, 0, 1).unwrap_err(), SbStatus::NoSuchArea);
    // SAFETY: `disc` is a live handle; a NULL out-pointer is rejected.
    assert_eq!(unsafe { sb_raw_frames_open(disc, 0, 0, 1, ptr::null_mut()) }, SbStatus::InvalidArgument);
    let r = raw_open(disc, 0, 1, 4).unwrap();
    assert_eq!(sb_raw_channels(r), 2);
    // Missing positions have no bytes: a NULL buffer is enough.
    for n in 1..4 {
        let mut fi = SbRawFrameInfo::default();
        // SAFETY: `r` and `disc` are live handles; `&mut fi` is a valid out-pointer.
        assert_eq!(unsafe { sb_raw_frames_read(r, disc, ptr::null_mut(), 0, &mut fi) }, SbStatus::Ok);
        assert_eq!((fi.number, fi.kind, fi.size), (n, SbFrameKind::Missing as u32, 0));
        assert_eq!(raw_missing(r), u64::from(n));
    }
    assert_eq!(raw_read(r, disc, &mut [0u8; 1]).0, SbStatus::End);
    // The DST equivalent of a missing position: 0x00, then 2 x 4704 bytes of 0x69. A
    // buffer one byte short keeps the position for the next call.
    let e = raw_open(disc, 0, 0, 1).unwrap();
    let mut buf = vec![0u8; 2 * 4704];
    let mut fi = SbRawFrameInfo::default();
    // SAFETY: `e` and `disc` are live handles; `buf` is valid for its length; `&mut fi` is
    // a valid out-pointer.
    let s = unsafe { sb_raw_frames_read_dst_equivalent(e, disc, buf.as_mut_ptr(), buf.len(), &mut fi) };
    assert_eq!((s, fi.size), (SbStatus::BufferTooSmall, 1 + 2 * 4704));
    assert_eq!(raw_missing(e), 0);
    let all = read_equivalents(e, disc);
    assert_eq!(all.len(), 1);
    assert_eq!((all[0].0, all[0].1), (0, SbFrameKind::Missing as u32));
    assert_eq!(all[0].2[0], 0);
    assert!(all[0].2[1..].iter().all(|&b| b == 0x69));
    assert_eq!(raw_missing(e), 1);
    raw_free(e);
    let mut count = 9u64;
    // SAFETY: `r` is a live handle; `&mut count` is a valid out-pointer.
    assert_eq!(unsafe { sb_raw_frames_malformed_sectors(r, &mut count) }, SbStatus::Ok);
    assert_eq!(count, 0);
    // The disc may be freed first.
    free(disc);
    raw_free(r);
}

fn list_open(disc: *const SbDisc, slot: u32) -> Result<*mut SbRawFrames, SbStatus> {
    let mut r: *mut SbRawFrames = ptr::dangling_mut();
    // SAFETY: `disc` is NULL or a live handle; `&mut r` is a valid out-pointer.
    match unsafe { sb_stored_frame_list_open(disc, slot, &mut r) } {
        SbStatus::Ok => Ok(r),
        s => {
            assert!(r.is_null(), "*out not cleared on failure");
            Err(s)
        }
    }
}

#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn stored_frame_list() {
    let id = "dst-features-2ch-6ch";
    let disc = open(&image(id)).unwrap();
    // The list holds every accepted frame: here every position of the play range.
    let l = list_open(disc, 0).unwrap();
    assert_eq!(sb_raw_channels(l), 2);
    let list = read_raw(l, disc);
    let r = raw_open(disc, 0, 0, 35).unwrap();
    let range = read_raw(r, disc);
    raw_free(r);
    let stored: Vec<_> = range.into_iter().filter(|f| f.1 != SbFrameKind::Missing as u32).collect();
    assert_eq!(list, stored);
    assert_eq!(raw_missing(l), 0);
    // No DST equivalents for the list (4.6.1 item 4).
    let mut buf = vec![0u8; SB_MAX_FRAME_BYTES];
    let mut fi = SbRawFrameInfo::default();
    // SAFETY: `l` and `disc` are live handles; `buf` is valid for its length; `&mut fi` is
    // a valid out-pointer.
    let s = unsafe { sb_raw_frames_read_dst_equivalent(l, disc, buf.as_mut_ptr(), buf.len(), &mut fi) };
    assert_eq!(s, SbStatus::InvalidArgument);
    // A list handle continues into a range, afresh.
    let c = raw_open_next(disc, l, 0, 3, 5).unwrap();
    assert_eq!(read_raw(c, disc).iter().map(|f| f.0).collect::<Vec<_>>(), vec![3, 4]);
    raw_free(c);
    assert_eq!(list_open(disc, 7).unwrap_err(), SbStatus::InvalidArgument);
    free(disc);

    let disc = open(&image("area-channels-0")).unwrap();
    assert_eq!(list_open(disc, 0).unwrap_err(), SbStatus::ChannelCountUnsupported);
    free(disc);
    let disc = open(&tiny_image()).unwrap();
    assert_eq!(list_open(disc, 1).unwrap_err(), SbStatus::NoSuchArea);
    let l = list_open(disc, 0).unwrap();
    assert!(read_raw(l, disc).is_empty());
    raw_free(l);
    free(disc);
}

/// As `log_callback_cannot_reach_a_handle_being_consumed`, for `sb_raw_frames_open_next`:
/// a log callback that runs while it consumes `prev` (a range's or a stored frame list's
/// handle) cannot read or free it.
#[test]
fn log_callback_cannot_reach_a_raw_handle_being_consumed() {
    let user = LogUser::default();
    let disc = open_logged(&tiny_image(), &user, SB_LOG_INFO).unwrap();
    user.victim_disc.store(disc, std::sync::atomic::Ordering::Relaxed);
    let a = raw_open(disc, 0, 0, 3).unwrap();
    let mut buf = vec![0u8; SB_MAX_FRAME_BYTES];
    assert_eq!(raw_read(a, disc, &mut buf).0, SbStatus::Ok);
    // A range into a range.
    user.raw_victim.store(a, std::sync::atomic::Ordering::Relaxed);
    let b = raw_open_next(disc, a, 0, 1, 5).unwrap();
    assert_eq!(*user.victim_reads.lock().unwrap(), [SbStatus::InvalidArgument]);
    assert!(user.raw_victim.load(std::sync::atomic::Ordering::Relaxed).is_null(), "the callback ran");
    assert_eq!(read_raw(b, disc).len(), 4);
    // A refusal is logged while `prev` is still the caller's: it stays usable.
    user.raw_victim.store(b, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(raw_open_next(disc, b, 0, 0, 11).unwrap_err(), SbStatus::FrameRangeInvalid);
    assert_eq!(*user.victim_reads.lock().unwrap(), [SbStatus::InvalidArgument; 2]);
    assert_eq!(raw_read(b, disc, &mut buf).0, SbStatus::End);
    // A stored frame list into a range.
    let l = list_open(disc, 0).unwrap();
    user.raw_victim.store(l, std::sync::atomic::Ordering::Relaxed);
    let c = raw_open_next(disc, l, 0, 0, 2).unwrap();
    assert_eq!(*user.victim_reads.lock().unwrap(), [SbStatus::InvalidArgument; 3]);
    assert_eq!(read_raw(c, disc).len(), 2);
    raw_free(b);
    raw_free(c);
    free(disc);
}

fn pool_new(threads: u32) -> Result<*mut SbPool, SbStatus> {
    let mut p: *mut SbPool = ptr::dangling_mut();
    // SAFETY: `&mut p` is a valid out-pointer.
    match unsafe { sb_pool_new(threads, &mut p) } {
        SbStatus::Ok => Ok(p),
        s => {
            assert!(p.is_null(), "*out not cleared on failure");
            Err(s)
        }
    }
}

fn pool_threads(pool: *const SbPool) -> u32 {
    // SAFETY: `pool` is NULL or a live handle.
    unsafe { sb_pool_threads(pool) }
}

fn pool_free(pool: *mut SbPool) {
    // SAFETY: `pool` is NULL or a live handle that the caller does not use again.
    unsafe { sb_pool_free(pool) };
}

fn set_pool(audio: *const SbTrackAudio, pool: *const SbPool) -> SbStatus {
    // SAFETY: both are NULL or live handles.
    unsafe { sb_track_audio_set_pool(audio, pool) }
}

/// Every track of area `index` through one continued handle (given `pool` once, at the
/// first track), then the whole area `[0, P)` through a fresh one: each frame's number,
/// status and bytes, hashed, with the counts.
fn decode_area_with(disc: *const SbDisc, index: u32, pool: *const SbPool) -> (Vec<String>, Vec<String>) {
    let (s, info) = area(disc, index);
    assert_eq!(s, SbStatus::Ok);
    let slot = info.slot;
    let digest = |frames: Vec<(u32, u32, Vec<u8>)>| {
        let mut h = Sha256::new();
        for (number, status, bytes) in &frames {
            h.update(number.to_le_bytes());
            h.update(status.to_le_bytes());
            h.update(bytes);
        }
        format!("{} frames {}", frames.len(), hex(&h.finalize()))
    };
    let mut tracks = Vec::new();
    let mut prev: *mut SbTrackAudio = ptr::null_mut();
    for n in 1..=info.track_count {
        if check(disc, slot, n) != SbStatus::Ok {
            tracks.push(format!("track {n}: {:?}", check(disc, slot, n)));
            continue;
        }
        let first = prev.is_null();
        prev = audio_open_next(disc, prev, slot, n).unwrap();
        if first {
            assert_eq!(set_pool(prev, pool), SbStatus::Ok);
        }
        let frames = read_frames(prev, disc);
        let (mut missing, mut invalid) = (0u64, 0u64);
        // SAFETY: `prev` is a live handle; both out-pointers are valid.
        assert_eq!(unsafe { sb_track_audio_counts(prev, &mut missing, &mut invalid) }, SbStatus::Ok);
        tracks.push(format!("track {n}: {} missing {missing} invalid {invalid}", digest(frames)));
    }
    audio_free(prev);
    let mut whole = Vec::new();
    if range_check(disc, slot, 0, info.playtime_frames) == SbStatus::Ok {
        let h = range_open(disc, slot, 0, info.playtime_frames).unwrap();
        assert_eq!(set_pool(h, pool), SbStatus::Ok);
        whole.push(digest(read_frames(h, disc)));
        audio_free(h);
    }
    (tracks, whole)
}

/// Corpus areas decoded on pools of 1, 2, 4 and one thread per CPU give exactly the
/// calling thread's frames, statuses and counts, for tracks (continued from handle to
/// handle, which keeps the pool) and whole-area ranges; above one thread, DST frames are
/// decoded on the pool's threads while frame lines stay on the caller.
#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn pool_decoding_equals_the_calling_thread() {
    for id in ["dst-features-2ch-6ch", "dst-invalid-frames-5ch", "dst-invalid-frames-2ch", "hostile-base"] {
        let bytes = image(id);
        let disc = open(&bytes).unwrap();
        let (s, d) = info(disc);
        assert_eq!(s, SbStatus::Ok);
        let serial: Vec<_> = (0..d.area_count).map(|i| decode_area_with(disc, i, ptr::null())).collect();
        free(disc);
        for threads in [1, 2, 4, 0] {
            let pool = pool_new(threads).unwrap();
            let n = pool_threads(pool);
            assert!(n >= 1 && (threads == 0 || n == threads), "{n}");
            let user = LogUser::default();
            let disc = open_logged(&bytes, &user, SB_LOG_TRACE).unwrap();
            let pooled: Vec<_> = (0..d.area_count).map(|i| decode_area_with(disc, i, pool)).collect();
            assert_eq!(pooled, serial, "{id} on {threads} threads");
            free(disc);
            pool_free(pool);
            let caller = std::thread::current().id();
            let lines = user.lines.lock().unwrap();
            let elsewhere =
                lines.iter().any(|(t, _, m)| m.starts_with("decoding DST frames") && *t != caller);
            let dst = lines.iter().any(|(_, _, m)| m.starts_with("decoding DST frames"));
            assert_eq!(elsewhere, dst && n > 1, "{id} on {threads} threads");
            assert!(
                lines
                    .iter()
                    .filter(|(_, _, m)| !m.starts_with("decoding DST frames"))
                    .all(|(t, _, _)| *t == caller)
            );
        }
    }
}

/// Two handles read in turn, frame by frame, on one pool and on two pools, and handles of
/// two discs read on two threads at once on one pool: the frames of reading each alone on
/// the calling thread.
#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn handles_share_pools() {
    let id = "dst-features-2ch-6ch";
    let bytes = image(id);
    let disc = open(&bytes).unwrap();
    let whole = |slot| {
        let (_, a) = area(disc, slot);
        let h = range_open(disc, slot, 0, a.playtime_frames).unwrap();
        let frames = read_frames(h, disc);
        audio_free(h);
        frames
    };
    let expected = [whole(0), whole(1)];
    let (p, q) = (pool_new(2).unwrap(), pool_new(3).unwrap());
    for (first, second) in [(p, p), (p, q)] {
        let handles: Vec<_> = [(0, first), (1, second)]
            .into_iter()
            .map(|(slot, pool)| {
                let (_, a) = area(disc, slot);
                let h = range_open(disc, slot, 0, a.playtime_frames).unwrap();
                assert_eq!(set_pool(h, pool), SbStatus::Ok);
                h
            })
            .collect();
        let mut got = [Vec::new(), Vec::new()];
        let mut buf = vec![0u8; 6 * 4704];
        let mut done = [false, false];
        while !done.iter().all(|&d| d) {
            for (i, &h) in handles.iter().enumerate() {
                if done[i] {
                    continue;
                }
                match audio_read(h, disc, &mut buf) {
                    (SbStatus::Ok, fi) => got[i].push((fi.number, fi.status, buf[..fi.size].to_vec())),
                    (SbStatus::End, _) => done[i] = true,
                    (s, _) => panic!("{s:?}: {}", last_error()),
                }
            }
        }
        handles.into_iter().for_each(audio_free);
        assert_eq!(got, expected);
    }
    free(disc);
    pool_free(q);
    // Two threads, each with its own disc, on one pool at once.
    let shared = p.expose_provenance();
    let threads: Vec<_> = (0..2u32)
        .map(|slot| {
            let bytes = bytes.clone();
            std::thread::spawn(move || {
                let pool: *const SbPool = ptr::with_exposed_provenance(shared);
                let disc = open(&bytes).unwrap();
                let (_, a) = area(disc, slot);
                let h = range_open(disc, slot, 0, a.playtime_frames).unwrap();
                assert_eq!(set_pool(h, pool), SbStatus::Ok);
                let frames = read_frames(h, disc);
                audio_free(h);
                free(disc);
                frames
            })
        })
        .collect();
    let got: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(got, expected);
    pool_free(p);
}

/// A pool may be freed before or after the handles given it, and changed or removed in the
/// middle of a read, with decoding jobs in flight, without changing the frames.
#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn pools_are_freed_in_any_order() {
    let disc = open(&image("dst-features-2ch-6ch")).unwrap();
    let (_, a) = area(disc, 1);
    let end = a.playtime_frames;
    let h = range_open(disc, 1, 0, end).unwrap();
    let expected = read_frames(h, disc);
    audio_free(h);
    let mut buf = vec![0u8; 6 * 4704];
    let mut first = |h| {
        let (s, fi) = audio_read(h, disc, &mut buf);
        assert_eq!(s, SbStatus::Ok);
        (fi.number, fi.status, buf[..fi.size].to_vec())
    };
    // The pool first: the handle keeps it running.
    let pool = pool_new(4).unwrap();
    let h = range_open(disc, 1, 0, end).unwrap();
    assert_eq!(set_pool(h, pool), SbStatus::Ok);
    let mut got = vec![first(h)];
    pool_free(pool);
    got.extend(read_frames(h, disc));
    audio_free(h);
    assert_eq!(got, expected);
    // The handle first, with jobs in flight.
    let pool = pool_new(4).unwrap();
    let h = range_open(disc, 1, 0, end).unwrap();
    assert_eq!(set_pool(h, pool), SbStatus::Ok);
    assert_eq!(first(h), expected[0]);
    audio_free(h);
    pool_free(pool);
    // Moved to another pool, then to none, between frames; the first pool freed while
    // its jobs may still run.
    let (p, q) = (pool_new(2).unwrap(), pool_new(4).unwrap());
    let h = range_open(disc, 1, 0, end).unwrap();
    assert_eq!(set_pool(h, p), SbStatus::Ok);
    let mut got = vec![first(h)];
    assert_eq!(set_pool(h, q), SbStatus::Ok);
    pool_free(p);
    got.push(first(h));
    assert_eq!(set_pool(h, ptr::null()), SbStatus::Ok);
    got.extend(read_frames(h, disc));
    audio_free(h);
    pool_free(q);
    assert_eq!(got, expected);
    free(disc);
}

/// The argument checks, without starting a thread (for Miri).
#[test]
fn pool_arguments() {
    assert_eq!(pool_new(SB_MAX_POOL_THREADS + 1).unwrap_err(), SbStatus::InvalidArgument);
    // SAFETY: a NULL out-pointer is allowed (and refused).
    assert_eq!(unsafe { sb_pool_new(1, ptr::null_mut()) }, SbStatus::InvalidArgument);
    assert_eq!(status_name(SbStatus::ThreadsUnavailable as i32), "threads-unavailable");
    assert_eq!(status_name(SbStatus::OutOfMemory as i32), "out-of-memory");
    assert_eq!(pool_threads(ptr::null()), 0);
    pool_free(ptr::null_mut());
    assert_eq!(set_pool(ptr::null(), ptr::null()), SbStatus::InvalidArgument);
    let disc = open(&tiny_image()).unwrap();
    let a = audio_open(disc, 0, 1).unwrap();
    assert_eq!(set_pool(a, ptr::null()), SbStatus::Ok);
    assert_eq!(read_frames(a, disc).len(), 2);
    audio_free(a);
    free(disc);
}

/// Pools started and freed in either order around a handle of a small image. CI runs it
/// under Miri with Tree Borrows (the `miri` job), where it passes.
#[test]
#[cfg_attr(miri, ignore = "rayon's crossbeam-epoch fails Stacked Borrows; run with Tree Borrows")]
fn pool_handles_of_a_small_image() {
    let disc = open(&tiny_image()).unwrap();
    for pool_first in [true, false] {
        let pool = pool_new(2).unwrap();
        assert_eq!(pool_threads(pool), 2);
        let a = audio_open(disc, 0, 1).unwrap();
        assert_eq!(set_pool(ptr::null(), pool), SbStatus::InvalidArgument);
        assert_eq!(set_pool(a, pool), SbStatus::Ok);
        if pool_first {
            pool_free(pool);
        }
        let frames = read_frames(a, disc);
        assert_eq!(frames.iter().map(|f| (f.0, f.1)).collect::<Vec<_>>(), [(0, 2), (1, 2)]);
        assert_eq!(set_pool(a, ptr::null()), SbStatus::Ok);
        audio_free(a);
        if !pool_first {
            pool_free(pool);
        }
    }
    let pool = pool_new(0).unwrap();
    assert!((1..=SB_MAX_POOL_THREADS).contains(&pool_threads(pool)));
    pool_free(pool);
    free(disc);
}

fn clone(disc: *const SbDisc) -> Result<*mut SbDisc, SbStatus> {
    let mut d: *mut SbDisc = ptr::dangling_mut();
    // SAFETY: `disc` is NULL or a live handle not used by another thread during the call;
    // `&mut d` is a valid out-pointer.
    match unsafe { sb_disc_clone(disc, &mut d) } {
        SbStatus::Ok => Ok(d),
        s => {
            assert!(d.is_null(), "*out not cleared on failure");
            Err(s)
        }
    }
}

fn clone_logged(disc: *const SbDisc, user: &LogUser, level: i32) -> Result<*mut SbDisc, SbStatus> {
    let mut d: *mut SbDisc = ptr::dangling_mut();
    let u = ptr::from_ref(user).cast_mut().cast::<c_void>();
    // SAFETY: `disc` is a live handle; `cb_log` may be called with `u` from any thread
    // while `user` lives, which outlives the clone; `&mut d` is a valid out-pointer.
    match unsafe { sb_disc_clone_with_log(disc, Some(cb_log), u, level, &mut d) } {
        SbStatus::Ok => Ok(d),
        s => {
            assert!(d.is_null(), "*out not cleared on failure");
            Err(s)
        }
    }
}

/// One image decoded on several threads at once, each through a clone of one disc
/// (opened from memory or from a file), hostile images included: every track gives the
/// manifest's hashes and counts.
#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn clones_decode_one_image_on_threads() {
    const THREADS: usize = 6;
    for id in ["dst-features-2ch-6ch", "dst-invalid-frames-5ch", "hostile-base", "dst-invalid-frames-6ch"] {
        for from_file in [false, true] {
            let disc = if from_file {
                let path = CString::new(corpus().join("images").join(format!("{id}.iso")).to_str().unwrap())
                    .unwrap();
                let mut d = ptr::null_mut();
                // SAFETY: `path` is NUL-terminated; `&mut d` is a valid out-pointer.
                assert_eq!(unsafe { sb_disc_open_file(path.as_ptr(), &mut d) }, SbStatus::Ok);
                d
            } else {
                open(&image(id)).unwrap()
            };
            let mut jobs = Vec::new();
            for area in expected(id)["audio"].as_array().unwrap() {
                let slot = if area["area"] == "2ch" { 0 } else { 1 };
                for t in area["tracks"].as_array().unwrap() {
                    jobs.push((slot, t.clone()));
                }
            }
            // Clones are made on this thread, then each used by one thread only.
            let clones: Vec<usize> = (0..THREADS).map(|_| clone(disc).unwrap().expose_provenance()).collect();
            std::thread::scope(|s| {
                for (i, &c) in clones.iter().enumerate() {
                    let jobs = &jobs;
                    s.spawn(move || {
                        let c: *const SbDisc = ptr::with_exposed_provenance(c);
                        for (slot, t) in jobs.iter().skip(i).step_by(THREADS) {
                            let n = t["number"].as_u64().unwrap() as u32;
                            if t["outcome"] != "ok" {
                                assert_ne!(check(c, *slot, n), SbStatus::Ok);
                                continue;
                            }
                            let (sha, missing, invalid) = hash_track(c, *slot, n);
                            let exp: Vec<_> =
                                t["sha256"].as_array().unwrap().iter().map(|h| h.as_str().unwrap()).collect();
                            assert_eq!(sha, exp, "{id} clone {i}: {slot} {n}");
                            assert_eq!(missing, t["missing_frames"].as_u64().unwrap());
                            assert_eq!(invalid, t["invalid_dst_frames"].as_u64().unwrap());
                        }
                    });
                }
            });
            // The original may go first.
            free(disc);
            clones.into_iter().map(ptr::with_exposed_provenance_mut).for_each(free);
        }
    }
}

/// A track continued and read through different clones gives what one disc gives.
#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn handles_continue_across_clones() {
    let id = "dst-features-2ch-6ch";
    let disc = open(&image(id)).unwrap();
    let clones = [clone(disc).unwrap(), clone(disc).unwrap()];
    let exp = expected(id);
    for (slot, area) in exp["audio"].as_array().unwrap().iter().enumerate() {
        let slot = slot as u32;
        let mut prev: *mut SbTrackAudio = ptr::null_mut();
        for (k, t) in area["tracks"].as_array().unwrap().iter().enumerate() {
            let n = t["number"].as_u64().unwrap() as u32;
            if check(disc, slot, n) != SbStatus::Ok {
                continue;
            }
            // Opened (or continued) through one disc, read through the next.
            let a = audio_open_next(clones[k % 2], prev, slot, n).unwrap();
            let read_through = if k % 3 == 0 { disc } else { clones[(k + 1) % 2] };
            let exp_sha: Vec<String> =
                t["sha256"].as_array().unwrap().iter().map(|h| h.as_str().unwrap().to_owned()).collect();
            assert_eq!(hash_handle(a, read_through), exp_sha, "{id} {slot} {n}");
            prev = a;
        }
        audio_free(prev);
    }
    free(disc);
    clones.into_iter().for_each(free);
}

/// Small enough for Miri: clones share the disc's number, others do not; a clone outlives
/// its original; what cannot be cloned is refused.
#[test]
fn clones_share_handles_and_refusals() {
    let bytes = tiny_image();
    let disc = open(&bytes).unwrap();
    let other = open(&bytes).unwrap();
    let c = clone(disc).unwrap();
    let cc = clone(c).unwrap();
    let a = audio_open(disc, 0, 1).unwrap();
    let mut buf = vec![0u8; 2 * 4704];
    assert_eq!(audio_read(a, c, &mut buf).0, SbStatus::Ok);
    let raw = raw_open(c, 0, 0, 2).unwrap();
    assert_eq!(raw_read(raw, disc, &mut buf).0, SbStatus::Ok);
    assert_eq!(raw_read(raw, other, &mut buf).0, SbStatus::InvalidArgument);
    assert_eq!(raw_read(raw, cc, &mut buf).0, SbStatus::Ok);
    raw_free(raw);
    // An image opened on its own is another disc, even with the same bytes.
    assert_eq!(audio_read(a, other, &mut buf).0, SbStatus::InvalidArgument);
    assert!(last_error().contains("another disc"), "{}", last_error());
    free(disc);
    assert_eq!(audio_read(a, cc, &mut buf).0, SbStatus::Ok);
    assert_eq!(audio_read(a, c, &mut buf).0, SbStatus::End);
    audio_free(a);
    // A disc's info is its clone's.
    let (s, i) = info(c);
    assert_eq!((s, i.area_count), (SbStatus::Ok, info(other).1.area_count));
    // Refusals.
    assert_eq!(clone(ptr::null()).unwrap_err(), SbStatus::InvalidArgument);
    // SAFETY: `c` is a live handle; a NULL out-pointer is refused.
    assert_eq!(unsafe { sb_disc_clone(c, ptr::null_mut()) }, SbStatus::InvalidArgument);
    let mut user = User {
        cursor: Cursor::new(bytes.clone()),
        disc: ptr::null_mut(),
        audio: ptr::null_mut(),
        raw: ptr::null_mut(),
        probes: Vec::new(),
    };
    let cb = open_callbacks(Some(cb_read), &mut user).unwrap();
    assert_eq!(clone(cb).unwrap_err(), SbStatus::InvalidArgument);
    assert!(last_error().contains("read and seek callbacks"), "{}", last_error());
    for d in [cb, c, cc, other] {
        free(d);
    }
}

/// A clone reports to its own callback only, from its check of the image on, and keeps it
/// after the original's is replaced or freed.
#[test]
fn a_clone_logs_to_its_own_callback() {
    let bytes = tiny_image();
    let (first, second) = (LogUser::default(), LogUser::default());
    let disc = open_logged(&bytes, &first, SB_LOG_DEBUG).unwrap();
    let before = first.lines().len();
    let silent = clone(disc).unwrap();
    let logged = clone_logged(disc, &second, SB_LOG_DEBUG).unwrap();
    assert!(second.lines().iter().any(|(_, m)| m.contains("shared reader checked")), "{:?}", second.lines());
    for d in [silent, logged] {
        let a = audio_open(d, 0, 1).unwrap();
        hash_handle(a, d);
        audio_free(a);
    }
    assert_eq!(first.lines().len(), before, "the original's callback got the clones' lines");
    assert_eq!(set_log(disc, None, SB_LOG_OFF), SbStatus::Ok);
    free(disc);
    let n = second.lines().len();
    let a = audio_open(logged, 0, 1).unwrap();
    hash_handle(a, logged);
    audio_free(a);
    assert!(second.lines().len() > n);
    // A level outside the range is refused, with no clone.
    assert_eq!(clone_logged(logged, &second, 9).unwrap_err(), SbStatus::InvalidArgument);
    free(silent);
    free(logged);
}

/// An image served by a positional read callback, failing (with -1, or as a short read
/// with `short`) at the offsets in `fault`.
struct AtUser {
    data: Vec<u8>,
    fault: std::ops::Range<u64>,
    short: bool,
    /// Calls running now, and the most seen at once.
    running: std::sync::atomic::AtomicUsize,
    most: std::sync::atomic::AtomicUsize,
}

impl AtUser {
    fn new(data: Vec<u8>) -> Self {
        AtUser { data, fault: 0..0, short: false, running: 0.into(), most: 0.into() }
    }

    /// What a read of `len` bytes at `offset` gives: bytes, or -1 / 0 in the fault window
    /// (a read reaching into it stops at its start).
    fn read(&self, offset: u64, out: &mut [u8]) -> i64 {
        if self.fault.contains(&offset) {
            return if self.short { 0 } else { -1 };
        }
        let start = (offset as usize).min(self.data.len());
        let mut end = (start + out.len()).min(self.data.len());
        if offset < self.fault.start {
            end = end.min(self.fault.start as usize);
        }
        out[..end - start].copy_from_slice(&self.data[start..end]);
        (end - start) as i64
    }
}

unsafe extern "C" fn cb_read_at(user: *mut c_void, offset: u64, buf: *mut u8, len: usize) -> i64 {
    use std::sync::atomic::Ordering::SeqCst;
    // SAFETY: `user` is an `AtUser` that outlives the disc and its clones; it is only read.
    let u = unsafe { &*user.cast::<AtUser>() };
    let now = u.running.fetch_add(1, SeqCst) + 1;
    u.most.fetch_max(now, SeqCst);
    // SAFETY: the library passes a buffer valid for `len` bytes of writes.
    let out = unsafe { std::slice::from_raw_parts_mut(buf, len) };
    let n = u.read(offset, out);
    u.running.fetch_sub(1, SeqCst);
    n
}

unsafe extern "C" fn cb_read_at_lies(_: *mut c_void, _: u64, _: *mut u8, len: usize) -> i64 {
    len as i64 + 1
}

/// The same image and faults through read and seek callbacks.
struct SeekUser<'a>(&'a AtUser, u64);

unsafe extern "C" fn cb_fault_read(user: *mut c_void, buf: *mut u8, len: usize) -> i64 {
    // SAFETY: `user` is a `SeekUser` used by one disc on one thread at a time.
    let u = unsafe { &mut *user.cast::<SeekUser<'_>>() };
    // SAFETY: the library passes a buffer valid for `len` bytes of writes.
    let n = u.0.read(u.1, unsafe { std::slice::from_raw_parts_mut(buf, len) });
    if n > 0 {
        u.1 += n as u64;
    }
    n
}

unsafe extern "C" fn cb_fault_seek(user: *mut c_void, offset: i64, whence: i32) -> i64 {
    // SAFETY: as in `cb_fault_read`.
    let u = unsafe { &mut *user.cast::<SeekUser<'_>>() };
    let base = match whence {
        0 => 0,
        1 => u.1 as i64,
        _ => u.0.data.len() as i64,
    };
    match base.checked_add(offset).filter(|&p| p >= 0) {
        Some(p) => {
            u.1 = p as u64;
            p
        }
        None => -1,
    }
}

fn open_at(user: &AtUser, size: u64) -> Result<*mut SbDisc, SbStatus> {
    let mut d: *mut SbDisc = ptr::dangling_mut();
    let u = ptr::from_ref(user).cast_mut().cast::<c_void>();
    // SAFETY: `cb_read_at` may be called with `u` from any thread, concurrently, while
    // `user` lives, which outlives the disc and its clones; `&mut d` is a valid out-pointer.
    match unsafe { sb_disc_open_callbacks_at(Some(cb_read_at), size, u, &mut d) } {
        SbStatus::Ok => Ok(d),
        s => {
            assert!(d.is_null(), "*out not cleared on failure");
            Err(s)
        }
    }
}

fn open_seek(user: &mut SeekUser<'_>) -> Result<*mut SbDisc, SbStatus> {
    let mut d: *mut SbDisc = ptr::dangling_mut();
    let u = ptr::from_mut(user).cast::<c_void>();
    // SAFETY: the callbacks follow the contract; `user` outlives the disc, used on this
    // thread only; `&mut d` is a valid out-pointer.
    match unsafe { sb_disc_open_callbacks(Some(cb_fault_read), Some(cb_fault_seek), u, &mut d) } {
        SbStatus::Ok => Ok(d),
        s => {
            assert!(d.is_null());
            Err(s)
        }
    }
}

fn clone_seek(disc: *const SbDisc, user: &mut SeekUser<'_>) -> Result<*mut SbDisc, SbStatus> {
    let mut d: *mut SbDisc = ptr::dangling_mut();
    let u = ptr::from_mut(user).cast::<c_void>();
    // SAFETY: as in `open_seek`; `disc` is a live handle.
    match unsafe { sb_disc_clone_callbacks(disc, Some(cb_fault_read), Some(cb_fault_seek), u, &mut d) } {
        SbStatus::Ok => Ok(d),
        s => {
            assert!(d.is_null(), "*out not cleared on failure");
            Err(s)
        }
    }
}

/// Every read of a track: each frame's number, status and bytes, and the final status.
fn read_all(disc: *const SbDisc, slot: u32, n: u32) -> (Vec<(u32, u32, Vec<u8>)>, SbStatus) {
    let a = audio_open(disc, slot, n).unwrap();
    let mut buf = vec![0u8; audio_channels(a) as usize * 4704];
    let mut frames = Vec::new();
    let end = loop {
        match audio_read(a, disc, &mut buf) {
            (SbStatus::Ok, fi) => frames.push((fi.number, fi.status, buf.clone())),
            (s, _) => break s,
        }
    };
    audio_free(a);
    (frames, end)
}

/// One image decoded on 6 threads through clones of a disc served by a positional read
/// callback: the manifest's hashes, with calls at once (usually).
#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn read_at_callbacks_serve_clones_on_threads() {
    const THREADS: usize = 6;
    for id in ["dst-features-2ch-6ch", "hostile-base"] {
        let user = AtUser::new(image(id));
        let disc = open_at(&user, user.data.len() as u64).unwrap();
        let mut jobs = Vec::new();
        for area in expected(id)["audio"].as_array().unwrap() {
            let slot = if area["area"] == "2ch" { 0 } else { 1 };
            for t in area["tracks"].as_array().unwrap().iter().filter(|t| t["outcome"] == "ok") {
                jobs.push((slot, t.clone()));
            }
        }
        let clones: Vec<usize> = (0..THREADS).map(|_| clone(disc).unwrap().expose_provenance()).collect();
        std::thread::scope(|s| {
            for (i, &c) in clones.iter().enumerate() {
                let jobs = &jobs;
                s.spawn(move || {
                    let c: *const SbDisc = ptr::with_exposed_provenance(c);
                    for (slot, t) in jobs.iter().skip(i % jobs.len()).step_by(THREADS) {
                        let n = t["number"].as_u64().unwrap() as u32;
                        let exp: Vec<_> =
                            t["sha256"].as_array().unwrap().iter().map(|h| h.as_str().unwrap()).collect();
                        assert_eq!(hash_track(c, *slot, n).0, exp, "{id} clone {i}: {slot} {n}");
                    }
                });
            }
        });
        eprintln!("{id}: at most {} read calls at once", user.most.load(std::sync::atomic::Ordering::SeqCst));
        free(disc);
        clones.into_iter().map(ptr::with_exposed_provenance_mut).for_each(free);
    }
}

/// Failing and short reads give through a positional callback, its clones and a clone with
/// read and seek callbacks exactly what they give through read and seek callbacks.
#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn read_at_callback_failures_match_read_and_seek() {
    let id = "dst-features-2ch-6ch";
    let bytes = image(id);
    let probe = open(&bytes).unwrap();
    let (_, a) = area(probe, 0);
    free(probe);
    let mid = u64::from(a.audio_start_lsn + a.audio_end_lsn) / 2;
    for short in [false, true] {
        let mut user = AtUser::new(bytes.clone());
        user.fault = mid * 2048 + 100..mid * 2048 + 4196;
        user.short = short;
        let mut seek_user = SeekUser(&user, 0);
        let seek = open_seek(&mut seek_user).unwrap();
        let at = open_at(&user, bytes.len() as u64).unwrap();
        let at_clone = clone(at).unwrap();
        let mut seek_user2 = SeekUser(&user, 0);
        let seek_clone = clone_seek(at, &mut seek_user2).unwrap();
        let mut failed = 0;
        for slot in [0u32, 1] {
            for n in 1..=info_tracks(seek, slot) {
                if check(seek, slot, n) != SbStatus::Ok {
                    continue;
                }
                let want = read_all(seek, slot, n);
                failed += usize::from(want.1 != SbStatus::End);
                for d in [at, at_clone, seek_clone] {
                    assert_eq!(read_all(d, slot, n), want, "short {short}: {slot} {n}");
                }
            }
        }
        assert!(failed > 0, "the fault must fail a read");
        for d in [seek_clone, at_clone, at, seek] {
            free(d);
        }
    }
}

fn info_tracks(disc: *const SbDisc, slot: u32) -> u32 {
    (0..info(disc).1.area_count)
        .map(|i| area(disc, i).1)
        .find(|a| a.slot == slot)
        .map_or(0, |a| a.track_count)
}

/// Small enough for Miri: the positional opener and clones through callbacks, and what
/// they refuse.
#[test]
fn read_at_callbacks_and_callback_clones() {
    let bytes = tiny_image();
    let user = AtUser::new(bytes.clone());
    let disc = open_at(&user, bytes.len() as u64).unwrap();
    let c = clone(disc).unwrap();
    let a = audio_open(c, 0, 1).unwrap();
    assert_eq!(hash_handle(a, disc).len(), 2);
    audio_free(a);
    // A NULL callback; a callback that claims more bytes than asked; a size past the data.
    let mut d = ptr::dangling_mut();
    // SAFETY: a NULL callback is refused; `&mut d` is a valid out-pointer.
    let s = unsafe { sb_disc_open_callbacks_at(None, 1, ptr::null_mut(), &mut d) };
    assert_eq!(s, SbStatus::InvalidArgument);
    assert!(d.is_null());
    // SAFETY: `cb_read_at_lies` writes nothing; `&mut d` is a valid out-pointer.
    let s = unsafe { sb_disc_open_callbacks_at(Some(cb_read_at_lies), 1 << 20, ptr::null_mut(), &mut d) };
    assert_eq!(s, SbStatus::Io);
    assert!(d.is_null());
    // Clones through read and seek callbacks: of the same image, or refused.
    let mut same = SeekUser(&user, 0);
    let sc = clone_seek(disc, &mut same).unwrap();
    let a = audio_open(disc, 0, 1).unwrap();
    assert_eq!(hash_handle(a, sc).len(), 2);
    audio_free(a);
    // A single-position clone cannot be cloned again without callbacks.
    assert_eq!(clone(sc).unwrap_err(), SbStatus::InvalidArgument);
    let mut longer = bytes.clone();
    longer.extend_from_slice(&[0u8; 2048]);
    let mut changed = bytes.clone();
    changed[540 * 2048 + 2 * 2048 + 2000] ^= 1; // in the area TOC copy used
    for other in [longer, changed] {
        let other_user = AtUser::new(other);
        let mut u = SeekUser(&other_user, 0);
        assert_eq!(clone_seek(disc, &mut u).unwrap_err(), SbStatus::InvalidArgument);
        assert!(last_error().contains("another image"), "{}", last_error());
    }
    let mut d = ptr::dangling_mut();
    // SAFETY: a NULL callback is refused before anything is called.
    let s = unsafe { sb_disc_clone_callbacks(disc, None, Some(cb_fault_seek), ptr::null_mut(), &mut d) };
    assert_eq!(s, SbStatus::InvalidArgument);
    assert!(d.is_null());
    for d in [sc, c, disc] {
        free(d);
    }
}

/// The seek index `disc` keeps for slot 0: its frontier and whether it is complete, or
/// `None` when none is kept.
fn kept_index(disc: *const SbDisc) -> Option<(u64, bool)> {
    let mut index = ptr::null_mut();
    // SAFETY: a live handle and a valid out-pointer.
    match unsafe { sb_disc_seek_index_get(disc, 0, &mut index) } {
        SbStatus::Absent => None,
        SbStatus::Ok => {
            let (mut lsn, mut complete) = (0u64, false);
            // SAFETY: a live index handle and valid out-pointers.
            assert_eq!(unsafe { sb_seek_index_frontier(index, &mut lsn, &mut complete) }, SbStatus::Ok);
            // SAFETY: a live index handle, freed once.
            unsafe { sb_seek_index_free(index) };
            Some((lsn, complete))
        }
        s => panic!("{s:?}: {}", last_error()),
    }
}

/// A disc opened with a positional read callback and its clones (through `sb_disc_clone`,
/// and through read and seek callbacks) keep one seek index: enabled through one, built
/// through another on another thread, seen and disabled through the third; reads through
/// any of them equal reads without an index.
#[test]
#[cfg_attr(miri, ignore = "reads a whole corpus image")]
fn read_at_discs_and_callback_clones_share_the_seek_index() {
    let id = "dst-features-2ch-6ch";
    let user = AtUser::new(image(id));
    let plain = open(&user.data).unwrap();
    let disc = open_at(&user, user.data.len() as u64).unwrap();
    let at_clone = clone(disc).unwrap();
    let mut seek_user = SeekUser(&user, 0);
    let cb_clone = clone_seek(disc, &mut seek_user).unwrap();
    // Enabled through the callbacks clone: every disc of the family sees it.
    // SAFETY: a live handle.
    let s = unsafe { sb_disc_seek_index_enable(cb_clone, 0, 4, SB_SEEK_DEFAULT_INLINE_BUDGET) };
    assert_eq!(s, SbStatus::Ok, "{}", last_error());
    let empty = kept_index(disc).expect("kept for the original");
    assert!(!empty.1);
    assert_eq!(kept_index(at_clone), Some(empty));
    // Built by a whole-area read through the positional clone on another thread.
    let (_, a) = area(disc, 0);
    let p = a.playtime_frames;
    let c = at_clone.expose_provenance();
    let whole = std::thread::spawn(move || {
        let c: *mut SbDisc = ptr::with_exposed_provenance_mut(c);
        let h = range_open(c, 0, 0, p).unwrap();
        let got = read_frames(h, c);
        audio_free(h);
        got
    })
    .join()
    .unwrap();
    let h = range_open(plain, 0, 0, p).unwrap();
    assert_eq!(whole, read_frames(h, plain));
    audio_free(h);
    // The original and the callbacks clone see the complete index, and seeks from it read
    // as without one.
    for d in [disc, cb_clone] {
        assert!(kept_index(d).expect("kept").1, "complete");
    }
    for (d, first) in [(disc, p / 2), (cb_clone, p - 1), (at_clone, p / 3)] {
        let a = range_open(d, 0, first, p).unwrap();
        let b = range_open(plain, 0, first, p).unwrap();
        assert_eq!(read_frames(a, d), read_frames(b, plain), "first {first}");
        audio_free(a);
        audio_free(b);
    }
    // Disabled through the original: gone for every clone.
    // SAFETY: a live handle.
    assert_eq!(unsafe { sb_disc_seek_index_disable(disc, 0) }, SbStatus::Ok);
    assert_eq!((kept_index(at_clone), kept_index(cb_clone)), (None, None));
    for d in [cb_clone, at_clone, disc, plain] {
        free(d);
    }
}
