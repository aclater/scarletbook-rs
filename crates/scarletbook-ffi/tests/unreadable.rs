// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unreadable sectors through the C ABI (#435): marked with `sb_disc_unreadable_add` or
//! reported by a read callback (`SB_READ_UNREADABLE`), their frames are lost in decoded
//! reads, raw reads and the stored frame list, counted apart from missing ones, and
//! `sb_disc_read_sectors` refuses them.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::ffi::{CStr, c_void};
use std::ptr;

use scarletbook_ffi::*;

/// The core crate's builder of small synthetic images.
#[path = "../../scarletbook/tests/common/mod.rs"]
mod common;

use common::{AUDIO_LSN, PFrame, S, Spec, TrackSpec, pack};

/// Calls into the C ABI. Every call in this file passes live handles, pointers valid for
/// the lengths given and out-pointers valid for writes.
macro_rules! c {
    ($e:expr) => {{
        // SAFETY: as the macro's documentation says, for each use.
        unsafe { $e }
    }};
}

const FRAMES: u32 = 6;
/// The unreadable audio sector (index from the area's first).
const BAD: usize = 2;

/// A 2-channel area of uncoded DST frames of 3000 bytes (one to three sectors each), and
/// the frames whose packets lie in audio sector `BAD`.
fn image() -> (Vec<u8>, BTreeSet<u32>) {
    let frames: Vec<PFrame> = (0..FRAMES)
        .map(|number| {
            let payload: Vec<u8> = (0..3000).map(|i| (i as u32 * 3 + number) as u8).collect();
            PFrame { number, dst: true, data: [&[0x40], &payload[..]].concat() }
        })
        .collect();
    let audio = pack(&frames);
    let touched = touched(&audio);
    let tracks = vec![TrackSpec { start: 0, duration: FRAMES }];
    (Spec { channels: 2, playtime: FRAMES, tracks, audio }.build(), touched)
}

/// The frames with a packet in audio sector `BAD`, from the sector headers (4.3).
fn touched(audio: &[[u8; S]]) -> BTreeSet<u32> {
    let mut out = BTreeSet::new();
    let mut cur = None;
    for (i, s) in audio.iter().enumerate() {
        let (f, k) = (usize::from((s[0] >> 2) & 7), usize::from(s[0] >> 5));
        let mut x = 0;
        for p in 0..k {
            let w = u16::from_be_bytes([s[1 + 2 * p], s[2 + 2 * p]]);
            if w & 0x8000 != 0 && x < f {
                let r = 1 + 2 * k + x * 4;
                x += 1;
                cur = Some((u32::from(s[r]) * 60 + u32::from(s[r + 1])) * 75 + u32::from(s[r + 2]));
            }
            if let (Some(n), true) = (cur, i == BAD) {
                out.insert(n);
            }
        }
    }
    out
}

fn open(bytes: &[u8]) -> *mut SbDisc {
    let mut d = ptr::null_mut();
    assert_eq!(c!(sb_disc_open_memory(bytes.as_ptr(), bytes.len(), &mut d)), SbStatus::Ok);
    d
}

/// Every position of the area's track through `sb_raw_frames_read`, with the missing and
/// lost counts.
fn raw(disc: *const SbDisc) -> (Vec<(u32, u32, u32)>, u64, u64) {
    let mut r = ptr::null_mut();
    assert_eq!(c!(sb_raw_frames_open(disc, 0, 0, FRAMES, &mut r)), SbStatus::Ok);
    let out = read_raw(r, disc);
    let (mut missing, mut lost) = (u64::MAX, u64::MAX);
    assert_eq!(c!(sb_raw_frames_missing(r, &mut missing)), SbStatus::Ok);
    assert_eq!(c!(sb_raw_frames_lost(r, &mut lost)), SbStatus::Ok);
    c!(sb_raw_frames_free(r));
    (out, missing, lost)
}

fn read_raw(r: *const SbRawFrames, disc: *const SbDisc) -> Vec<(u32, u32, u32)> {
    let mut buf = vec![0u8; SB_MAX_FRAME_BYTES];
    let mut out = Vec::new();
    loop {
        let mut info = SbRawFrameInfo::default();
        match c!(sb_raw_frames_read(r, disc, buf.as_mut_ptr(), buf.len(), &mut info)) {
            SbStatus::Ok => out.push((info.number, info.kind, info.size)),
            SbStatus::End => return out,
            s => panic!("{s:?}"),
        }
    }
}

fn check_raw(what: &str, got: &[(u32, u32, u32)], touched: &BTreeSet<u32>) {
    let numbers: Vec<u32> = got.iter().map(|f| f.0).collect();
    assert_eq!(numbers, (0..FRAMES).collect::<Vec<_>>(), "{what}");
    for &(n, kind, size) in got {
        if touched.contains(&n) {
            assert_eq!((kind, size), (SbFrameKind::Lost as u32, 0), "{what}: position {n}");
        } else {
            assert_eq!((kind, size), (SbFrameKind::Dst as u32, 3001), "{what}: position {n}");
        }
    }
}

/// The flags byte of the bytes `sb_seek_index_save` gives for `index`.
fn saved_flags(index: *const SbSeekIndex) -> u8 {
    let mut len = 0;
    assert_eq!(c!(sb_seek_index_save(index, ptr::null_mut(), 0, &mut len)), SbStatus::BufferTooSmall);
    let mut buf = vec![0u8; len];
    assert_eq!(c!(sb_seek_index_save(index, buf.as_mut_ptr(), buf.len(), &mut len)), SbStatus::Ok);
    buf[14]
}

/// A larger area for building seek state past a mark: 40 frames of 3000 bytes.
fn larger_image() -> Vec<u8> {
    let frames: Vec<PFrame> = (0..40u32)
        .map(|number| {
            let payload: Vec<u8> = (0..3000).map(|i| (i as u32 * 7 + number) as u8).collect();
            PFrame { number, dst: true, data: [&[0x40], &payload[..]].concat() }
        })
        .collect();
    Spec {
        channels: 2,
        playtime: 40,
        tracks: vec![TrackSpec { start: 0, duration: 40 }],
        audio: pack(&frames),
    }
    .build()
}

/// Positions `[first, 40)` through `sb_raw_frames_read`.
fn raw_from(disc: *const SbDisc, first: u32) -> Vec<(u32, u32, u32)> {
    let mut r = ptr::null_mut();
    assert_eq!(c!(sb_raw_frames_open(disc, 0, first, 40, &mut r)), SbStatus::Ok);
    let out = read_raw(r, disc);
    c!(sb_raw_frames_free(r));
    out
}

/// Building the seek state past marked sectors ahead of the reads (#443):
/// `sb_disc_seek_index_extend_past_marks` and `sb_seek_index_extend_past_marks` end, in
/// small steps, with the state saved with the index (flags bit 2), and reads past the mark
/// equal those of a disc without an index. Without marks there is nothing to do; NULL
/// handles and a disc without an index are refused.
#[test]
fn seek_state_past_marks_builds_ahead() {
    let img = larger_image();
    let mark = (AUDIO_LSN + 8) as u64;
    let plain = open(&img);
    assert_eq!(c!(sb_disc_unreadable_add(plain, mark, 1)), SbStatus::Ok);
    let want = raw_from(plain, 30);
    // The disc's own index.
    let disc = open(&img);
    let mut complete = 9u8;
    assert_eq!(
        c!(sb_disc_seek_index_extend_past_marks(disc, 0, 8, &mut complete)),
        SbStatus::InvalidArgument
    );
    assert_eq!(c!(sb_disc_seek_index_enable(disc, 0, 2, SB_SEEK_DEFAULT_INLINE_BUDGET)), SbStatus::Ok);
    assert_eq!(c!(sb_disc_seek_index_extend_past_marks(disc, 0, 8, &mut complete)), SbStatus::Ok);
    assert_eq!(complete, 1, "nothing marked: nothing to do");
    assert_eq!(c!(sb_disc_unreadable_add(disc, mark, 1)), SbStatus::Ok);
    let mut calls = 0;
    loop {
        assert_eq!(c!(sb_disc_seek_index_extend_past_marks(disc, 0, 8, &mut complete)), SbStatus::Ok);
        if complete == 1 {
            break;
        }
        calls += 1;
        assert!(calls < 200, "does not end");
    }
    let mut index = ptr::null_mut();
    assert_eq!(c!(sb_disc_seek_index_get(disc, 0, &mut index)), SbStatus::Ok);
    assert_eq!(saved_flags(index) & 4, 4, "the state past the mark is kept");
    c!(sb_seek_index_free(index));
    assert_eq!(raw_from(disc, 30), want);
    // A caller's index, through a disc of its own; NULL `complete` is allowed.
    let other = open(&img);
    assert_eq!(c!(sb_disc_unreadable_add(other, mark, 1)), SbStatus::Ok);
    let mut index = ptr::null_mut();
    assert_eq!(c!(sb_seek_index_new(other, 0, 2, SB_SEEK_DEFAULT_INLINE_BUDGET, &mut index)), SbStatus::Ok);
    assert_eq!(c!(sb_seek_index_extend_past_marks(other, index, 1 << 20, ptr::null_mut())), SbStatus::Ok);
    assert_eq!(c!(sb_seek_index_extend_past_marks(other, index, 8, &mut complete)), SbStatus::Ok);
    assert_eq!(complete, 1);
    assert_eq!(saved_flags(index) & 4, 4);
    assert_eq!(
        c!(sb_seek_index_extend_past_marks(ptr::null(), index, 8, &mut complete)),
        SbStatus::InvalidArgument
    );
    assert_eq!(
        c!(sb_seek_index_extend_past_marks(other, ptr::null_mut(), 8, &mut complete)),
        SbStatus::InvalidArgument
    );
    c!(sb_seek_index_free(index));
    for d in [plain, disc, other] {
        c!(sb_disc_free(d));
    }
}

#[test]
fn marked_sectors_are_lost_in_every_read() {
    let (img, touched) = image();
    assert!(!touched.is_empty());
    let disc = open(&img);
    let lsn = (AUDIO_LSN + BAD) as u64;
    assert_eq!(c!(sb_disc_unreadable_add(disc, lsn, 1)), SbStatus::Ok);
    let (got, missing, lost) = raw(disc);
    check_raw("raw", &got, &touched);
    assert_eq!((missing, lost), (0, touched.len() as u64));
    // Decoded.
    let mut a = ptr::null_mut();
    assert_eq!(c!(sb_track_audio_open(disc, 0, 1, &mut a)), SbStatus::Ok);
    let mut buf = vec![0u8; 2 * 4704];
    for n in 0..FRAMES {
        let mut info = SbFrameInfo::default();
        assert_eq!(c!(sb_track_audio_read(a, disc, buf.as_mut_ptr(), buf.len(), &mut info)), SbStatus::Ok);
        let want = if touched.contains(&n) { SbFrameStatus::Lost } else { SbFrameStatus::Decoded };
        assert_eq!((info.number, info.status), (n, want as u32));
        if touched.contains(&n) {
            assert!(buf.iter().all(|&b| b == 0x69));
        }
    }
    let (mut missing, mut invalid, mut lost) = (9, 9, 9);
    assert_eq!(c!(sb_track_audio_counts(a, &mut missing, &mut invalid)), SbStatus::Ok);
    assert_eq!(c!(sb_track_audio_lost(a, &mut lost)), SbStatus::Ok);
    assert_eq!((missing, invalid, lost), (0, 0, touched.len() as u64));
    c!(sb_track_audio_free(a));
    // The stored frame list: one entry per position, lost ones included.
    let mut r = ptr::null_mut();
    assert_eq!(c!(sb_stored_frame_list_open(disc, 0, &mut r)), SbStatus::Ok);
    check_raw("list", &read_raw(r, disc), &touched);
    let mut lost = 0;
    assert_eq!(c!(sb_raw_frames_lost(r, &mut lost)), SbStatus::Ok);
    assert_eq!(lost, touched.len() as u64);
    c!(sb_raw_frames_free(r));
    // Raw sector reads refuse it; the set reads back; removed, everything decodes.
    let mut sectors = vec![0u8; 2 * S];
    let status = c!(sb_disc_read_sectors(disc, lsn - 1, 2, sectors.as_mut_ptr(), sectors.len()));
    assert_eq!(status, SbStatus::SectorUnreadable);
    let name = c!(CStr::from_ptr(sb_status_name(SbStatus::SectorUnreadable as i32)));
    assert_eq!(name.to_str().unwrap(), "sector-unreadable");
    let (mut first, mut count) = (0, 0);
    assert_eq!(c!(sb_disc_unreadable_range(disc, 0, &mut first, &mut count)), SbStatus::Ok);
    assert_eq!((first, count), (lsn, 1));
    assert_eq!(c!(sb_disc_unreadable_range(disc, 1, &mut first, &mut count)), SbStatus::Absent);
    assert_eq!(c!(sb_disc_unreadable_remove(disc, 0, u64::MAX)), SbStatus::Ok);
    check_raw("unmarked", &raw(disc).0, &BTreeSet::new());
    assert_eq!(c!(sb_disc_unreadable_add(ptr::null(), 0, 1)), SbStatus::InvalidArgument);
    c!(sb_disc_free(disc));
}

/// A positional read callback over an image, reporting audio sector `BAD` unreadable.
struct Source {
    bytes: Vec<u8>,
}

unsafe extern "C" fn read_at(user: *mut c_void, offset: u64, buf: *mut u8, len: usize) -> i64 {
    // SAFETY: `user` is the `Source` the test passed, alive while the disc is.
    let src = unsafe { &*user.cast::<Source>() };
    let bad = ((AUDIO_LSN + BAD) * S) as u64;
    if offset < bad + S as u64 && offset + len as u64 > bad {
        return SB_READ_UNREADABLE;
    }
    let start = (offset as usize).min(src.bytes.len());
    let n = len.min(src.bytes.len() - start);
    // SAFETY: `buf` is valid for `len >= n` bytes of writes, by the callback's contract.
    unsafe { ptr::copy_nonoverlapping(src.bytes[start..].as_ptr(), buf, n) };
    n as i64
}

#[test]
fn callbacks_report_unreadable_sectors() {
    let (img, touched) = image();
    let src = Box::new(Source { bytes: img });
    let size = src.bytes.len() as u64;
    let user = ptr::from_ref(&*src).cast_mut().cast::<c_void>();
    let mut disc = ptr::null_mut();
    assert_eq!(c!(sb_disc_open_callbacks_at(Some(read_at), size, user, &mut disc)), SbStatus::Ok);
    let (got, missing, lost) = raw(disc);
    check_raw("callback", &got, &touched);
    assert_eq!((missing, lost), (0, touched.len() as u64));
    let (mut first, mut count) = (0, 0);
    assert_eq!(c!(sb_disc_unreadable_range(disc, 0, &mut first, &mut count)), SbStatus::Ok);
    assert_eq!((first, count), ((AUDIO_LSN + BAD) as u64, 1), "the sector joined the set");
    let mut clone = ptr::null_mut();
    assert_eq!(c!(sb_disc_clone(disc, &mut clone)), SbStatus::Ok);
    assert_eq!(c!(sb_disc_unreadable_range(clone, 0, &mut first, &mut count)), SbStatus::Ok, "shared");
    c!(sb_disc_free(clone));
    c!(sb_disc_free(disc));
    drop(src);
}

/// `sb_track_audio_read_with_stored` reports a lost position as lost in both of its
/// descriptions, and counts it.
#[test]
fn reads_with_stored_report_lost_positions() {
    let (img, touched) = image();
    let disc = open(&img);
    assert_eq!(c!(sb_disc_unreadable_add(disc, (AUDIO_LSN + BAD) as u64, 1)), SbStatus::Ok);
    for equivalent in [0u8, 1] {
        let mut a = ptr::null_mut();
        assert_eq!(c!(sb_track_audio_open(disc, 0, 1, &mut a)), SbStatus::Ok);
        assert_eq!(c!(sb_track_audio_keep_stored(a)), SbStatus::Ok);
        let mut buf = vec![0u8; 2 * 4704];
        let mut raw = vec![0u8; SB_MAX_FRAME_BYTES];
        for n in 0..FRAMES {
            let (mut info, mut raw_info) = (SbFrameInfo::default(), SbRawFrameInfo::default());
            let status = c!(sb_track_audio_read_with_stored(
                a,
                disc,
                buf.as_mut_ptr(),
                buf.len(),
                &mut info,
                raw.as_mut_ptr(),
                raw.len(),
                &mut raw_info,
                equivalent
            ));
            assert_eq!(status, SbStatus::Ok);
            assert_eq!((info.number, raw_info.number), (n, n));
            if touched.contains(&n) {
                assert_eq!(info.status, SbFrameStatus::Lost as u32, "{n}");
                assert_eq!(raw_info.kind, SbFrameKind::Lost as u32, "{n}, equivalent {equivalent}");
                // No bytes, or a missing position's DST equivalent.
                assert_eq!(raw_info.size, if equivalent == 1 { 1 + 2 * 4704 } else { 0 });
            } else {
                assert_eq!(raw_info.kind, SbFrameKind::Dst as u32, "{n}");
            }
        }
        let (mut missing, mut invalid, mut lost) = (9, 9, 9);
        assert_eq!(c!(sb_track_audio_counts(a, &mut missing, &mut invalid)), SbStatus::Ok);
        assert_eq!(c!(sb_track_audio_lost(a, &mut lost)), SbStatus::Ok);
        assert_eq!((missing, invalid, lost), (0, 0, touched.len() as u64));
        c!(sb_track_audio_free(a));
    }
    c!(sb_disc_free(disc));
}

/// The bytes `sb_seek_index_save` gives for `index`.
fn save(index: *const SbSeekIndex) -> Vec<u8> {
    let mut len = 0;
    assert_eq!(c!(sb_seek_index_save(index, ptr::null_mut(), 0, &mut len)), SbStatus::BufferTooSmall);
    let mut buf = vec![0u8; len];
    assert_eq!(c!(sb_seek_index_save(index, buf.as_mut_ptr(), buf.len(), &mut len)), SbStatus::Ok);
    buf
}

/// `sb_seek_index_without_damage` (#443): a copy without the state past marked sectors,
/// which saves without flags bit 2 (the form of indexes before #441) and reads the same;
/// the index it was made from is unchanged. NULL arguments are refused.
#[test]
fn indexes_without_damage_save_in_the_earlier_form() {
    let (img, touched) = image();
    let disc = open(&img);
    assert_eq!(c!(sb_disc_seek_index_enable(disc, 0, 1, SB_SEEK_DEFAULT_INLINE_BUDGET)), SbStatus::Ok);
    assert_eq!(c!(sb_disc_unreadable_add(disc, (AUDIO_LSN + BAD) as u64, 1)), SbStatus::Ok);
    let (got, _, lost) = raw(disc);
    check_raw("indexed", &got, &touched);
    assert_eq!(lost, touched.len() as u64);
    let mut index = ptr::null_mut();
    assert_eq!(c!(sb_disc_seek_index_get(disc, 0, &mut index)), SbStatus::Ok);
    let bytes = save(index);
    assert_eq!(bytes[14] & 4, 4, "the state past the mark is saved");
    let mut plain = ptr::null_mut();
    assert_eq!(c!(sb_seek_index_without_damage(index, &mut plain)), SbStatus::Ok);
    assert!(!plain.is_null());
    let plain_bytes = save(plain);
    assert_eq!(plain_bytes[14] & 4, 0, "saved without it");
    assert_eq!(save(index), bytes, "the index it was made from is unchanged");
    // Loaded and kept by a new disc with the same mark, it reads the same.
    let again = open(&img);
    let mut loaded = ptr::null_mut();
    let budget = SB_SEEK_DEFAULT_INLINE_BUDGET;
    assert_eq!(
        c!(sb_seek_index_load(again, 0, plain_bytes.as_ptr(), plain_bytes.len(), budget, &mut loaded)),
        SbStatus::Ok
    );
    assert_eq!(c!(sb_disc_seek_index_set(again, 0, loaded)), SbStatus::Ok);
    assert_eq!(c!(sb_disc_unreadable_add(again, (AUDIO_LSN + BAD) as u64, 1)), SbStatus::Ok);
    check_raw("from the copy", &raw(again).0, &touched);
    // NULL arguments.
    let mut out = plain;
    assert_eq!(c!(sb_seek_index_without_damage(ptr::null(), &mut out)), SbStatus::InvalidArgument);
    assert!(out.is_null());
    assert_eq!(c!(sb_seek_index_without_damage(index, ptr::null_mut())), SbStatus::InvalidArgument);
    for i in [index, plain, loaded] {
        c!(sb_seek_index_free(i));
    }
    c!(sb_disc_free(disc));
    c!(sb_disc_free(again));
}
