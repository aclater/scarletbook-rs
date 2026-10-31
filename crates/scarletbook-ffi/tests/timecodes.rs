// SPDX-License-Identifier: MIT OR Apache-2.0

//! TOC timecodes as stored (issue #127) through the C ABI and the Rust API, against the
//! stored bytes: the area play time (area TOC offset 64) and each track's start and
//! duration in `SACDTRL2` (specification 3, 3.7.2), before any clamping.
//!
//! The stored bytes never come from the library: the test takes the area TOC copy the
//! parse used from the image file itself, with the data offsets of specification 1, 1.2,
//! and walks its sub-structures (3.7.0) to find `SACDTRL2`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::Cursor;
use std::path::PathBuf;
use std::ptr;

use scarletbook::{Disc, Error, Slot};
use scarletbook_ffi::*;

const S: usize = 2048;

fn corpus_images() -> Vec<(String, Vec<u8>)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/images");
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "iso"))
        .map(|p| (p.file_stem().unwrap().to_string_lossy().into_owned(), std::fs::read(p).unwrap()))
        .collect();
    v.sort();
    v
}

/// The user data of `count` sectors from `lsn` of an image of `p`-byte stored sectors.
fn sectors(image: &[u8], p: usize, lsn: usize, count: usize) -> Vec<u8> {
    let d = match p {
        2048 => 0,
        2054 => 6,
        2064 => 12,
        _ => panic!("stored sector size {p}"),
    };
    (lsn..lsn + count).flat_map(|l| image[l * p + d..][..S].to_vec()).collect()
}

/// The position of the last `SACDTRL2` the walk of specification 3, 3.7.0 records in `z`.
fn trl2(z: &[u8]) -> usize {
    let (mut p, mut found) = (S, None);
    while p + 8 <= z.len() {
        p += match &z[p..p + 8] {
            b"SACDTTxt" | b"SACDTRL1" => S,
            b"SACDTRL2" => {
                found = Some(p);
                S
            }
            b"SACD_IGL" => 2 * S,
            b"SACD_ACC" => 32 * S,
            _ => break,
        };
    }
    found.expect("an area that survives has a SACDTRL2")
}

fn tc(b: &[u8]) -> SbTimecode {
    SbTimecode { minutes: b[0], seconds: b[1], frames: b[2] }
}

fn open(bytes: &[u8]) -> Option<*mut SbDisc> {
    let mut d = ptr::null_mut();
    // SAFETY: `bytes` is valid for its length; `&mut d` is a valid out-pointer.
    (unsafe { sb_disc_open_memory(bytes.as_ptr(), bytes.len(), &mut d) } == SbStatus::Ok).then_some(d)
}

fn playtime(disc: *const SbDisc, slot: u32) -> Result<SbTimecode, SbStatus> {
    let mut t = SbTimecode { minutes: 0xEE, seconds: 0xEE, frames: 0xEE };
    // SAFETY: `disc` is NULL or a live handle; `&mut t` is a valid out-pointer.
    match unsafe { sb_area_playtime_stored(disc, slot, &mut t) } {
        SbStatus::Ok => Ok(t),
        s => Err(s),
    }
}

fn track(disc: *const SbDisc, slot: u32, n: u32) -> Result<(SbTimecode, SbTimecode), SbStatus> {
    let (mut a, mut b) = (SbTimecode::default(), SbTimecode::default());
    // SAFETY: `disc` is NULL or a live handle; the outputs are valid.
    match unsafe { sb_track_timecodes_stored(disc, slot, n, &mut a, &mut b) } {
        SbStatus::Ok => Ok((a, b)),
        s => Err(s),
    }
}

fn frames(t: SbTimecode) -> u32 {
    (u32::from(t.minutes) * 60 + u32::from(t.seconds)) * 75 + u32::from(t.frames)
}

/// On every corpus image that opens: the stored timecodes equal the bytes of the file, the
/// Rust API's, and the frame counts are those of the TOC as before (clamped only when the
/// track passes the play time).
#[test]
#[cfg_attr(miri, ignore = "reads every corpus image")]
fn stored_timecodes_of_every_corpus_image() {
    let (mut images, mut tracks, mut clamped, mut out_of_range) = (0, 0, 0, 0);
    for (id, bytes) in corpus_images() {
        let Some(disc) = open(&bytes) else { continue };
        let rust = Disc::open(Cursor::new(&bytes[..])).unwrap();
        let toc = rust.toc();
        let p = toc.sector_format.stored_size() as usize;
        for a in &toc.areas {
            let slot = match a.slot {
                Slot::TwoChannel => 0,
                Slot::Multichannel => 1,
            };
            let z = sectors(&bytes, p, a.toc_lsn as usize, usize::from(a.toc_sectors));
            let pt = playtime(disc, slot).unwrap();
            assert_eq!(pt, tc(&z[64..]), "{id}: area {slot} play time");
            assert_eq!(frames(pt), a.playtime_frames(), "{id}");
            if pt.seconds > 59 || pt.frames > 74 {
                out_of_range += 1;
            }
            let t2 = trl2(&z);
            for t in &a.tracks {
                let n = u32::from(t.number);
                let i = usize::from(t.number) - 1;
                let (start, duration) = track(disc, slot, n).unwrap();
                assert_eq!(start, tc(&z[t2 + 8 + 4 * i..]), "{id}: area {slot} track {n} start");
                assert_eq!(duration, tc(&z[t2 + 1028 + 4 * i..]), "{id}: area {slot} track {n} duration");
                let (rs, rd) = rust.track_timecodes_stored(a.slot, n).unwrap();
                assert_eq!((SbTimecode::from(rs), SbTimecode::from(rd)), (start, duration));
                // The frame counts are the stored ones, clamped to the play time (3.9.2).
                let (s, d) = (frames(start), frames(duration));
                if s + d > a.playtime_frames() {
                    clamped += 1;
                    let s2 = s.min(a.playtime_frames());
                    assert_eq!((t.start_frame, t.duration_frames), (s2, d.min(a.playtime_frames() - s2)));
                } else {
                    assert_eq!((t.start_frame, t.duration_frames), (s, d), "{id}: track {n}");
                }
                tracks += 1;
            }
            let past = a.tracks.len() as u32 + 1;
            assert_eq!(track(disc, slot, past).unwrap_err(), SbStatus::NoSuchTrack, "{id}");
            assert!(matches!(rust.track_timecodes_stored(a.slot, past), Err(Error::NoSuchTrack { .. })));
            assert_eq!(track(disc, slot, 0).unwrap_err(), SbStatus::NoSuchTrack, "{id}");
        }
        for (slot, n) in [(Slot::TwoChannel, 0), (Slot::Multichannel, 1)] {
            if toc.area(slot).is_none() {
                assert_eq!(playtime(disc, n).unwrap_err(), SbStatus::NoSuchArea, "{id}");
                assert_eq!(track(disc, n, 1).unwrap_err(), SbStatus::NoSuchArea, "{id}");
                assert!(matches!(rust.track_timecodes_stored(slot, 1), Err(Error::NoSuchArea(_))));
            }
        }
        // SAFETY: `disc` is live and not used again.
        unsafe { sb_disc_free(disc) };
        images += 1;
    }
    println!("{images} images, {tracks} tracks, {clamped} clamped, {out_of_range} play times out of range");
    assert!(images > 100 && tracks > images, "{images} images, {tracks} tracks");
    assert!(clamped > 0 && out_of_range > 0, "the corpus has clamped tracks and out-of-range play times");
}

/// The struct's layout, and the argument checks (no image: also under Miri).
#[test]
fn layout_and_arguments() {
    assert_eq!(std::mem::size_of::<SbTimecode>(), 3);
    assert_eq!(std::mem::align_of::<SbTimecode>(), 1);
    assert_eq!(std::mem::offset_of!(SbTimecode, minutes), 0);
    assert_eq!(std::mem::offset_of!(SbTimecode, seconds), 1);
    assert_eq!(std::mem::offset_of!(SbTimecode, frames), 2);
    assert_eq!(playtime(ptr::null(), 0).unwrap_err(), SbStatus::InvalidArgument);
    assert_eq!(track(ptr::null(), 0, 1).unwrap_err(), SbStatus::InvalidArgument);
    // SAFETY: NULL handles and outputs are refused.
    let s = unsafe { sb_area_playtime_stored(ptr::null(), 0, ptr::null_mut()) };
    assert_eq!(s, SbStatus::InvalidArgument);
}
