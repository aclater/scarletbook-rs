// SPDX-License-Identifier: MIT OR Apache-2.0

//! The C ABI, driven from Rust as a C caller would.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::undocumented_unsafe_blocks)]
#![allow(clippy::multiple_unsafe_ops_per_block)]

use std::ffi::{CStr, CString, c_void};
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
    unsafe { CStr::from_ptr(sb_last_error_message()) }.to_string_lossy().into_owned()
}

fn open(bytes: &[u8]) -> Result<*mut SbDisc, SbStatus> {
    let mut d = ptr::null_mut();
    match unsafe { sb_disc_open_memory(bytes.as_ptr(), bytes.len(), &mut d) } {
        SbStatus::Ok => Ok(d),
        s => Err(s),
    }
}

fn string(f: impl Fn(*mut i8, usize, *mut usize) -> SbStatus) -> Option<String> {
    let mut len = 0usize;
    match f(ptr::null_mut(), 0, &mut len) {
        SbStatus::Absent => return None,
        SbStatus::BufferTooSmall => {}
        s => panic!("unexpected {s:?}"),
    }
    let mut buf = vec![0i8; len + 1];
    assert_eq!(f(buf.as_mut_ptr(), buf.len(), &mut len), SbStatus::Ok);
    let bytes: Vec<u8> = buf[..len].iter().map(|&b| b as u8).collect();
    assert_eq!(buf[len], 0);
    Some(String::from_utf8(bytes).unwrap())
}

/// Decodes a track through the C ABI and hashes each channel.
fn hash_track(disc: *mut SbDisc, slot: u32, number: u32) -> (Vec<String>, u64, u64) {
    let mut a = ptr::null_mut();
    assert_eq!(unsafe { sb_track_audio_open(disc, slot, number, &mut a) }, SbStatus::Ok, "{}", last_error());
    let ch = unsafe { sb_track_audio_channels(a) } as usize;
    let mut hashers: Vec<Sha256> = (0..ch).map(|_| Sha256::new()).collect();
    let mut buf = vec![0u8; ch * 4704];
    let mut info = SbFrameInfo::default();
    loop {
        match unsafe { sb_track_audio_read(a, disc, buf.as_mut_ptr(), buf.len(), &mut info) } {
            SbStatus::End => break,
            SbStatus::Ok => {}
            s => panic!("{s:?}: {}", last_error()),
        }
        assert_eq!(info.size, buf.len());
        for (c, h) in hashers.iter_mut().enumerate() {
            let chan: Vec<u8> = buf.iter().skip(c).step_by(ch).copied().collect();
            h.update(&chan);
        }
    }
    let (mut missing, mut invalid) = (0u64, 0u64);
    assert_eq!(unsafe { sb_track_audio_counts(a, &mut missing, &mut invalid) }, SbStatus::Ok);
    unsafe { sb_track_audio_free(a) };
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
                    assert_ne!(unsafe { sb_track_check(disc, slot, n) }, SbStatus::Ok);
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
        unsafe { sb_disc_free(disc) };
    }
}

#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn toc_accessors() {
    let disc = open(&image("full-text")).unwrap();
    let exp = expected("full-text");
    let toc = &exp["toc"];
    let mut info = SbDiscInfo::default();
    assert_eq!(unsafe { sb_disc_info(disc, &mut info) }, SbStatus::Ok);
    assert_eq!(u64::from(info.sector_size), toc["sector_size"].as_u64().unwrap());
    assert_eq!(info.sectors, toc["sectors"].as_u64().unwrap());
    assert_eq!(info.area_count as usize, toc["areas"].as_array().unwrap().len());
    let title = string(|b, c, l| unsafe { sb_disc_string(disc, 0, b, c, l) });
    assert_eq!(title.as_deref(), toc["text"]["album_title"].as_str());
    let cat = string(|b, c, l| unsafe { sb_disc_string(disc, 16, b, c, l) });
    assert_eq!(cat.as_deref(), toc["album_catalog_number"].as_str());
    let mut area = SbAreaInfo::default();
    assert_eq!(unsafe { sb_disc_area(disc, 0, &mut area) }, SbStatus::Ok);
    let a0 = &toc["areas"][0];
    assert_eq!(u64::from(area.track_count), a0["tracks"].as_array().unwrap().len() as u64);
    let mut track = SbTrackInfo::default();
    assert_eq!(unsafe { sb_track_info(disc, area.slot, 1, &mut track) }, SbStatus::Ok);
    assert_eq!(u64::from(track.duration_frames), a0["tracks"][0]["duration_frames"].as_u64().unwrap());
    let ttitle = string(|b, c, l| unsafe { sb_track_string(disc, area.slot, 1, 0, b, c, l) });
    assert_eq!(ttitle.as_deref(), a0["tracks"][0]["text"]["title"].as_str());
    let isrc = string(|b, c, l| unsafe { sb_track_string(disc, area.slot, 1, 14, b, c, l) });
    assert_eq!(isrc.as_deref(), a0["tracks"][0]["isrc"].as_str());
    unsafe { sb_disc_free(disc) };
}

#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn warnings_and_rejections() {
    let disc = open(&image("track-start-past-playtime")).unwrap();
    let mut info = SbDiscInfo::default();
    assert_eq!(unsafe { sb_disc_info(disc, &mut info) }, SbStatus::Ok);
    let exp = expected("track-start-past-playtime");
    let mut got = Vec::new();
    for i in 0..info.warning_count {
        let mut w = SbWarning { code: ptr::null(), slot: 0, track: 0 };
        assert_eq!(unsafe { sb_disc_warning(disc, i, &mut w) }, SbStatus::Ok);
        got.push(unsafe { CStr::from_ptr(w.code) }.to_str().unwrap().to_owned());
    }
    for w in exp["warnings"].as_array().unwrap() {
        assert!(got.iter().any(|g| g == w["code"].as_str().unwrap()), "{got:?}");
    }
    unsafe { sb_disc_free(disc) };

    assert_eq!(open(&image("master-version-99")).unwrap_err(), SbStatus::MasterTocVersionUnsupported);
    assert!(last_error().contains("version"));
    assert_eq!(open(&[]).unwrap_err(), SbStatus::NotScarletbook);
    assert_eq!(open(&image("master-no-areas")).unwrap_err(), SbStatus::NoAudioAreas);
    let name = unsafe { CStr::from_ptr(sb_status_name(SbStatus::NoAudioAreas as i32)) };
    assert_eq!(name.to_str().unwrap(), "no-audio-areas");
    assert_eq!(unsafe { CStr::from_ptr(sb_status_name(12345)) }.to_str().unwrap(), "unknown");
}

#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn hostile_arguments() {
    let bytes = image("hostile-base");
    let disc = open(&bytes).unwrap();
    let other = open(&bytes).unwrap();
    unsafe {
        assert_eq!(sb_disc_open_memory(ptr::null(), 5, &mut ptr::null_mut()), SbStatus::InvalidArgument);
        assert_eq!(
            sb_disc_open_memory(bytes.as_ptr(), bytes.len(), ptr::null_mut()),
            SbStatus::InvalidArgument
        );
        assert_eq!(sb_disc_open_file(ptr::null(), &mut ptr::null_mut()), SbStatus::InvalidArgument);
        let missing = CString::new("/nonexistent/disc.iso").unwrap();
        assert_eq!(sb_disc_open_file(missing.as_ptr(), &mut ptr::null_mut()), SbStatus::Io);
        assert_eq!(sb_disc_info(ptr::null(), &mut SbDiscInfo::default()), SbStatus::InvalidArgument);
        assert_eq!(sb_disc_info(disc, ptr::null_mut()), SbStatus::InvalidArgument);
        assert_eq!(sb_disc_area(disc, 7, &mut SbAreaInfo::default()), SbStatus::InvalidArgument);
        assert_eq!(sb_track_info(disc, 9, 1, &mut SbTrackInfo::default()), SbStatus::InvalidArgument);
        assert_eq!(sb_track_info(disc, 0, 99, &mut SbTrackInfo::default()), SbStatus::NoSuchTrack);
        assert_eq!(sb_disc_string(disc, 99, ptr::null_mut(), 0, ptr::null_mut()), SbStatus::InvalidArgument);
        let mut a = ptr::null_mut();
        assert_eq!(sb_track_audio_open(disc, 0, 1, &mut a), SbStatus::Ok);
        let mut small = [0u8; 16];
        assert_eq!(
            sb_track_audio_read(a, disc, small.as_mut_ptr(), 16, ptr::null_mut()),
            SbStatus::BufferTooSmall
        );
        let mut buf = vec![0u8; 2 * 4704];
        assert_eq!(
            sb_track_audio_read(a, other, buf.as_mut_ptr(), buf.len(), ptr::null_mut()),
            SbStatus::InvalidArgument
        );
        assert_eq!(sb_track_audio_read(a, disc, buf.as_mut_ptr(), buf.len(), ptr::null_mut()), SbStatus::Ok);
        assert_eq!(
            sb_track_audio_read(ptr::null_mut(), disc, buf.as_mut_ptr(), buf.len(), ptr::null_mut()),
            SbStatus::InvalidArgument
        );
        sb_track_audio_free(a);
        sb_track_audio_free(ptr::null_mut());
        sb_disc_free(ptr::null_mut());
        sb_disc_free(disc);
        sb_disc_free(other);
    }
}

#[test]
fn dst_decode_reports_the_rule() {
    let mut out = vec![0u8; 2 * 4704];
    let mut rule = 0u32;
    let frame = [0x80u8, 0x00];
    let s = unsafe { sb_dst_decode(frame.as_ptr(), frame.len(), 2, out.as_mut_ptr(), out.len(), &mut rule) };
    assert_eq!((s, rule), (SbStatus::InvalidDstFrame, 3));
    let plain = [0x00u8, 1, 2, 3];
    let s = unsafe { sb_dst_decode(plain.as_ptr(), plain.len(), 2, out.as_mut_ptr(), out.len(), &mut rule) };
    assert_eq!(s, SbStatus::Ok);
    assert_eq!(&out[..4], &[1, 2, 3, 0]);
    let s = unsafe { sb_dst_decode(plain.as_ptr(), plain.len(), 7, out.as_mut_ptr(), out.len(), &mut rule) };
    assert_eq!(s, SbStatus::InvalidArgument);
}

unsafe extern "C" fn cb_read(user: *mut c_void, buf: *mut u8, len: usize) -> i64 {
    let c = unsafe { &mut *user.cast::<Cursor<Vec<u8>>>() };
    let out = unsafe { std::slice::from_raw_parts_mut(buf, len) };
    c.read(out).map_or(-1, |n| n as i64)
}

unsafe extern "C" fn cb_seek(user: *mut c_void, offset: i64, whence: i32) -> i64 {
    let c = unsafe { &mut *user.cast::<Cursor<Vec<u8>>>() };
    let pos = match whence {
        0 => SeekFrom::Start(offset as u64),
        1 => SeekFrom::Current(offset),
        _ => SeekFrom::End(offset),
    };
    c.seek(pos).map_or(-1, |p| p as i64)
}

/// A read callback that claims to have read more than asked: must be an I/O error, not UB.
unsafe extern "C" fn cb_read_lies(_: *mut c_void, _: *mut u8, len: usize) -> i64 {
    len as i64 + 100
}

#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn callbacks() {
    let mut cursor = Cursor::new(image("dsd-stereo-mch-2064"));
    let user = (&raw mut cursor).cast::<c_void>();
    let mut disc = ptr::null_mut();
    assert_eq!(
        unsafe { sb_disc_open_callbacks(Some(cb_read), Some(cb_seek), user, &mut disc) },
        SbStatus::Ok
    );
    let mut info = SbDiscInfo::default();
    assert_eq!(unsafe { sb_disc_info(disc, &mut info) }, SbStatus::Ok);
    assert_eq!(info.sector_size, 2064);
    let exp = expected("dsd-stereo-mch-2064");
    let t = &exp["audio"][0]["tracks"][0];
    let (sha, _, _) = hash_track(disc, 0, 1);
    assert_eq!(sha[0], t["sha256"][0].as_str().unwrap());
    unsafe { sb_disc_free(disc) };

    assert_eq!(
        unsafe { sb_disc_open_callbacks(None, Some(cb_seek), user, &mut disc) },
        SbStatus::InvalidArgument
    );
    assert_eq!(
        unsafe { sb_disc_open_callbacks(Some(cb_read_lies), Some(cb_seek), user, &mut disc) },
        SbStatus::Io
    );
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
    img[a + 66] = 10; // play time: 10 frames
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
    assert_eq!(string(|b, c, l| unsafe { sb_disc_string(disc, 16, b, c, l) }).as_deref(), Some("CAT-1"));
    assert_eq!(string(|b, c, l| unsafe { sb_disc_string(disc, 0, b, c, l) }), None);
    let mut exact = [0i8; 5];
    let mut len = 0;
    // Exactly the string length: no room for the NUL.
    assert_eq!(
        unsafe { sb_disc_string(disc, 16, exact.as_mut_ptr(), 5, &mut len) },
        SbStatus::BufferTooSmall
    );
    assert_eq!(len, 5);
    let mut a = ptr::null_mut();
    assert_eq!(unsafe { sb_track_audio_open(disc, 0, 1, &mut a) }, SbStatus::Ok);
    let mut buf = vec![0u8; 2 * 4704];
    let mut fi = SbFrameInfo::default();
    let mut n = 0;
    while unsafe { sb_track_audio_read(a, disc, buf.as_mut_ptr(), buf.len(), &mut fi) } == SbStatus::Ok {
        assert_eq!(fi.status, SbFrameStatus::Missing as u32);
        assert!(buf.iter().all(|&b| b == 0x69));
        n += 1;
    }
    assert_eq!(n, 2);
    unsafe { sb_track_audio_free(a) };
    unsafe { sb_disc_free(disc) };
}
