// SPDX-License-Identifier: MIT OR Apache-2.0

//! Stored and decoded frames of one range in one pass through the C ABI (issue #107):
//! `sb_track_audio_read_with_stored` gives, per position, what `sb_track_audio_read` and
//! `sb_raw_frames_read` (or `_read_dst_equivalent`) give on separate handles, reading each
//! audio sector once.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};

use scarletbook_ffi::*;

fn images() -> Vec<(String, Vec<u8>)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/images");
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let p = e.unwrap().path();
            (p.file_name().unwrap().to_string_lossy().into_owned(), std::fs::read(&p).unwrap())
        })
        .collect();
    v.sort();
    assert!(v.len() > 50);
    v
}

fn open(bytes: &[u8]) -> Option<*mut SbDisc> {
    let mut d = ptr::null_mut();
    // SAFETY: `bytes` is valid for its length; `&mut d` is a valid out-pointer.
    (unsafe { sb_disc_open_memory(bytes.as_ptr(), bytes.len(), &mut d) } == SbStatus::Ok).then_some(d)
}

/// A positional source counting the bytes read.
struct Counting {
    bytes: Vec<u8>,
    read: AtomicU64,
}

unsafe extern "C" fn counting_read_at(user: *mut c_void, offset: u64, buf: *mut u8, len: usize) -> i64 {
    // SAFETY: `user` is the `Counting` given to sb_disc_open_callbacks_at, alive for the
    // disc's life; only shared references are made (its counter is atomic).
    let s = unsafe { &*user.cast::<Counting>() };
    let start = (offset as usize).min(s.bytes.len());
    let n = len.min(s.bytes.len() - start);
    // SAFETY: the library passes a buffer valid for `len >= n` bytes of writes, which does
    // not overlap the source (owned by `s`).
    let out = unsafe { std::slice::from_raw_parts_mut(buf, n) };
    out.copy_from_slice(&s.bytes[start..start + n]);
    s.read.fetch_add(n as u64, Ordering::SeqCst);
    n as i64
}

fn ranges(disc: *const SbDisc) -> Vec<(u32, u32, u32)> {
    let mut out = Vec::new();
    let mut info = SbDiscInfo::default();
    // SAFETY: `disc` is live; `&mut info` is a valid out-pointer.
    assert_eq!(unsafe { sb_disc_info(disc, &mut info) }, SbStatus::Ok);
    for index in 0..info.area_count {
        let mut area = SbAreaInfo::default();
        // SAFETY: `disc` is live; `&mut area` is a valid out-pointer.
        assert_eq!(unsafe { sb_disc_area(disc, index, &mut area) }, SbStatus::Ok);
        let slot = area.slot;
        let mut v = vec![(0, area.playtime_frames)];
        for n in 1..=area.track_count {
            let mut t = SbTrackInfo::default();
            // SAFETY: `disc` is live; `&mut t` is a valid out-pointer.
            if unsafe { sb_track_info(disc, slot, n, &mut t) } == SbStatus::Ok {
                v.push((t.start_frame, t.start_frame.saturating_add(t.duration_frames)));
            }
        }
        for (f, e) in v {
            // SAFETY: `disc` is live.
            if unsafe { sb_range_check(disc, slot, f, e) } == SbStatus::Ok {
                out.push((slot, f, e));
            }
        }
    }
    out
}

fn audio_open(disc: *const SbDisc, (slot, f, e): (u32, u32, u32), pool: *const SbPool) -> *mut SbTrackAudio {
    let mut a = ptr::null_mut();
    // SAFETY: `disc` is live; `&mut a` is a valid out-pointer.
    assert_eq!(unsafe { sb_range_audio_open(disc, slot, f, e, &mut a) }, SbStatus::Ok);
    // SAFETY: `a` is live; `pool` NULL or live.
    assert_eq!(unsafe { sb_track_audio_set_pool(a, pool) }, SbStatus::Ok);
    a
}

/// A decoded position: status, number, frame status, rule, bytes.
type Decoded = (SbStatus, u32, u32, u32, Vec<u8>);
/// A stored position: number, kind, bytes.
type Raw = (u32, u32, Vec<u8>);

fn decoded_read(a: *const SbTrackAudio, disc: *const SbDisc) -> Decoded {
    let mut buf = vec![0u8; 6 * 4704];
    let mut fi = SbFrameInfo::default();
    // SAFETY: live handles; `buf` valid for its length; `&mut fi` valid.
    let s = unsafe { sb_track_audio_read(a, disc, buf.as_mut_ptr(), buf.len(), &mut fi) };
    buf.truncate(fi.size as usize);
    (s, fi.number, fi.status, fi.dst_rule, buf)
}

fn raw_read(r: *const SbRawFrames, disc: *const SbDisc, equivalent: bool) -> (SbStatus, Raw) {
    let mut buf = vec![0u8; SB_MAX_FRAME_BYTES];
    let mut fi = SbRawFrameInfo::default();
    let (ptr, len) = (buf.as_mut_ptr(), buf.len());
    let s = if equivalent {
        // SAFETY: live handles; `buf` valid for its length; `&mut fi` valid.
        unsafe { sb_raw_frames_read_dst_equivalent(r, disc, ptr, len, &mut fi) }
    } else {
        // SAFETY: as above.
        unsafe { sb_raw_frames_read(r, disc, ptr, len, &mut fi) }
    };
    buf.truncate(fi.size as usize);
    (s, (fi.number, fi.kind, buf))
}

/// One read of both; with `raw_cap` small, may return BUFFER_TOO_SMALL.
fn both_read(
    a: *const SbTrackAudio,
    disc: *const SbDisc,
    equivalent: bool,
    raw_cap: usize,
) -> (Decoded, Raw) {
    let mut buf = vec![0u8; 6 * 4704];
    let mut fi = SbFrameInfo::default();
    let mut raw = vec![0u8; raw_cap];
    let mut ri = SbRawFrameInfo::default();
    // SAFETY: live handles; both buffers valid for their lengths and apart; infos valid.
    let s = unsafe {
        sb_track_audio_read_with_stored(
            a,
            disc,
            buf.as_mut_ptr(),
            buf.len(),
            &mut fi,
            raw.as_mut_ptr(),
            raw.len(),
            &mut ri,
            u8::from(equivalent),
        )
    };
    buf.truncate(fi.size as usize);
    raw.truncate((ri.size as usize).min(raw_cap));
    ((s, fi.number, fi.status, fi.dst_rule, buf), (ri.number, ri.kind, raw))
}

fn counts(a: *const SbTrackAudio) -> (u64, u64, u64) {
    let (mut m, mut i, mut ms) = (0, 0, 0);
    // SAFETY: `a` is live; the outputs are valid.
    assert_eq!(unsafe { sb_track_audio_counts(a, &mut m, &mut i) }, SbStatus::Ok);
    // SAFETY: as above.
    assert_eq!(unsafe { sb_track_audio_malformed_sectors(a, &mut ms) }, SbStatus::Ok);
    (m, i, ms)
}

fn keep(a: *const SbTrackAudio) -> SbStatus {
    // SAFETY: `a` is live.
    unsafe { sb_track_audio_keep_stored(a) }
}

fn audio_free(a: *mut SbTrackAudio) {
    // SAFETY: `a` is live and not used again.
    unsafe { sb_track_audio_free(a) };
}

/// On every corpus image, every track and whole area of every area, on the calling thread
/// and on a pool: per position, the decoded frame and the stored content (and DST
/// equivalent) of the separate handles; the same counts; then END on both.
#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn equals_the_separate_handles_on_the_corpus() {
    let mut pool = ptr::null_mut();
    // SAFETY: `&mut pool` is a valid out-pointer.
    assert_eq!(unsafe { sb_pool_new(4, &mut pool) }, SbStatus::Ok);
    let mut checked = 0;
    for (name, bytes) in images() {
        let Some(disc) = open(&bytes) else { continue };
        for range in ranges(disc) {
            for (p, equivalent) in [(ptr::null(), false), (pool.cast_const(), true)] {
                let a = audio_open(disc, range, p);
                let mut r = ptr::null_mut();
                // SAFETY: `disc` is live; `&mut r` is a valid out-pointer.
                let s = unsafe { sb_raw_frames_open(disc, range.0, range.1, range.2, &mut r) };
                assert_eq!(s, SbStatus::Ok);
                let b = audio_open(disc, range, p);
                assert_eq!(keep(b), SbStatus::Ok);
                loop {
                    let d = decoded_read(a, disc);
                    let (rs, raw) = raw_read(r, disc, equivalent);
                    let (bd, braw) = both_read(b, disc, equivalent, SB_MAX_FRAME_BYTES);
                    assert_eq!(bd, d, "{name} {range:?}");
                    if d.0 != SbStatus::Ok {
                        assert_eq!(rs, d.0, "{name} {range:?} raw");
                        break;
                    }
                    assert_eq!(rs, SbStatus::Ok);
                    assert_eq!(braw, raw, "{name} {range:?} frame {}", d.1);
                }
                assert_eq!(counts(b), counts(a), "{name} {range:?}");
                let (mut m, mut ms) = (0, 0);
                // SAFETY: `r` is live; the outputs are valid.
                assert_eq!(unsafe { sb_raw_frames_missing(r, &mut m) }, SbStatus::Ok);
                // SAFETY: as above.
                assert_eq!(unsafe { sb_raw_frames_malformed_sectors(r, &mut ms) }, SbStatus::Ok);
                assert_eq!((m, ms), (counts(b).0, counts(b).2));
                audio_free(a);
                audio_free(b);
                // SAFETY: `r` is live and not used again.
                unsafe { sb_raw_frames_free(r) };
                checked += 1;
            }
        }
        // SAFETY: `disc` is live and not used again.
        unsafe { sb_disc_free(disc) };
    }
    assert!(checked > 100, "{checked}");
    // SAFETY: `pool` is live and not used again.
    unsafe { sb_pool_free(pool) };
}

/// Each audio sector is read once (counted by the read callback): the one-pass handle
/// reads exactly the bytes of a decoded-only handle. A raw buffer too small holds the
/// position for the next call; the counts do not include it meanwhile.
#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn reads_each_sector_once_and_holds_for_a_small_buffer() {
    for id in ["dst-features-2ch-6ch", "dsd-stereo-mch-2064", "dst-invalid-frames-6ch"] {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/images").join(format!("{id}.iso"));
        let src = Counting { bytes: std::fs::read(path).unwrap(), read: AtomicU64::new(0) };
        let mut disc = ptr::null_mut();
        let user = ptr::from_ref(&src).cast_mut().cast::<c_void>();
        // SAFETY: the callback copies at most `len` bytes and does not unwind; `src`
        // outlives the disc; `&mut disc` is a valid out-pointer.
        let s = unsafe {
            sb_disc_open_callbacks_at(Some(counting_read_at), src.bytes.len() as u64, user, &mut disc)
        };
        assert_eq!(s, SbStatus::Ok);
        for range in ranges(disc) {
            let a = audio_open(disc, range, ptr::null());
            let r0 = src.read.load(Ordering::SeqCst);
            let mut want = Vec::new();
            loop {
                let d = decoded_read(a, disc);
                if d.0 != SbStatus::Ok {
                    break;
                }
                want.push(d);
            }
            let decoded_bytes = src.read.load(Ordering::SeqCst) - r0;
            audio_free(a);
            let b = audio_open(disc, range, ptr::null());
            assert_eq!(keep(b), SbStatus::Ok);
            let r0 = src.read.load(Ordering::SeqCst);
            let mut got = Vec::new();
            let mut held = 0;
            loop {
                // A buffer too small for every non-empty stored frame first.
                let (d, raw) = both_read(b, disc, false, 1);
                if d.0 == SbStatus::BufferTooSmall {
                    held += 1;
                    assert!(raw.2.iter().all(|&x| x == 0), "nothing written");
                    let before = counts(b);
                    let (d2, raw2) = both_read(b, disc, false, SB_MAX_FRAME_BYTES);
                    assert_eq!((d2.1, raw2.0), (d.1, d.1), "{id}: the same position again");
                    assert_eq!(d2.0, SbStatus::Ok);
                    let after = counts(b);
                    assert!(after.0 + after.1 >= before.0 + before.1);
                    got.push(d2);
                    continue;
                }
                if d.0 != SbStatus::Ok {
                    assert_eq!(d.0, SbStatus::End);
                    break;
                }
                got.push(d);
            }
            assert_eq!(src.read.load(Ordering::SeqCst) - r0, decoded_bytes, "{id} {range:?}: one read");
            assert!(got == want, "{id} {range:?}");
            assert!(held > 0, "{id} {range:?}");
            audio_free(b);
        }
        // SAFETY: `disc` is live and not used again.
        unsafe { sb_disc_free(disc) };
    }
}

/// Refusals, on a corpus-free path where possible (Miri).
#[test]
fn refusals() {
    // SAFETY: NULL handles are refused.
    assert_eq!(unsafe { sb_track_audio_keep_stored(ptr::null()) }, SbStatus::InvalidArgument);
    let mut buf = [0u8; 16];
    // SAFETY: NULL handles are refused before any buffer is used.
    let s = unsafe {
        sb_track_audio_read_with_stored(
            ptr::null(),
            ptr::null(),
            buf.as_mut_ptr(),
            buf.len(),
            ptr::null_mut(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            0,
        )
    };
    assert_eq!(s, SbStatus::InvalidArgument);
}

/// Asked for after the first read, refused; without it, the combined read is refused; a
/// held position read by sb_track_audio_read is the same frame.
#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn when_it_can_be_asked_for() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/images/dst-features-2ch-6ch.iso");
    let disc = open(&std::fs::read(path).unwrap()).unwrap();
    let range = ranges(disc)[0];
    let a = audio_open(disc, range, ptr::null());
    assert_eq!(both_read(a, disc, false, SB_MAX_FRAME_BYTES).0.0, SbStatus::InvalidArgument);
    assert_eq!(decoded_read(a, disc).0, SbStatus::Ok);
    assert_eq!(keep(a), SbStatus::InvalidArgument);
    audio_free(a);
    let b = audio_open(disc, range, ptr::null());
    assert_eq!(keep(b), SbStatus::Ok);
    assert_eq!(keep(b), SbStatus::Ok, "asking again changes nothing");
    let (d, _) = both_read(b, disc, true, 0);
    assert_eq!(d.0, SbStatus::BufferTooSmall, "a DST equivalent is never empty");
    let d2 = decoded_read(b, disc);
    assert_eq!((d2.0, d2.1), (SbStatus::Ok, d.1));
    let mut buf = [0u8; 6 * 4704];
    let mut raw = [0u8; 16];
    // SAFETY: live handles; buffers valid for their lengths and apart.
    let s = unsafe {
        sb_track_audio_read_with_stored(
            b,
            disc,
            buf.as_mut_ptr(),
            buf.len(),
            ptr::null_mut(),
            raw.as_mut_ptr(),
            raw.len(),
            ptr::null_mut(),
            2,
        )
    };
    assert_eq!(s, SbStatus::InvalidArgument);
    audio_free(b);
    // SAFETY: `disc` is live and not used again.
    unsafe { sb_disc_free(disc) };
}

/// A handle continued while a position is held: a range starting at that position scans
/// afresh and delivers it, with the decoded and stored bytes and the counts of a new
/// handle of the range.
#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn a_held_position_is_delivered_after_open_next() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/images/dst-features-2ch-6ch.iso");
    let disc = open(&std::fs::read(path).unwrap()).unwrap();
    for (slot, first, end) in ranges(disc).into_iter().filter(|r| r.2 - r.1 > 20) {
        let b = audio_open(disc, (slot, first, end), ptr::null());
        assert_eq!(keep(b), SbStatus::Ok);
        for _ in 0..5 {
            assert_eq!(both_read(b, disc, true, SB_MAX_FRAME_BYTES).0.0, SbStatus::Ok);
        }
        let (d, _) = both_read(b, disc, true, 1);
        assert_eq!(d.0, SbStatus::BufferTooSmall);
        let n = d.1;
        let mut c = ptr::null_mut();
        // SAFETY: `disc` and `b` are live; `b` is not used again; `&mut c` is valid.
        assert_eq!(unsafe { sb_range_audio_open_next(disc, b, slot, n, end, &mut c) }, SbStatus::Ok);
        let fresh = audio_open(disc, (slot, n, end), ptr::null());
        assert_eq!(keep(fresh), SbStatus::Ok);
        let mut k = 0;
        loop {
            let got = both_read(c, disc, true, SB_MAX_FRAME_BYTES);
            let want = both_read(fresh, disc, true, SB_MAX_FRAME_BYTES);
            assert_eq!(got, want, "slot {slot} position {k}");
            if k == 0 {
                assert_eq!((got.0.0, got.0.1), (SbStatus::Ok, n), "the held position, delivered");
            }
            if got.0.0 != SbStatus::Ok {
                break;
            }
            k += 1;
        }
        assert_eq!(counts(c), counts(fresh));
        audio_free(c);
        audio_free(fresh);
    }
    // SAFETY: `disc` is live and not used again.
    unsafe { sb_disc_free(disc) };
}
