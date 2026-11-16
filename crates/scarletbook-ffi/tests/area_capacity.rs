// SPDX-License-Identifier: MIT OR Apache-2.0

//! `sb_area_capacity` (#340) against the Rust API's `Disc::area_capacity`: the same figures
//! for every corpus image, the hostile ones among them, and for tiny synthetic images with
//! hostile TOCs, driven from Rust as a C caller would.
//!
//! Each unsafe call sits in a small wrapper that states why its arguments meet the
//! function's contract; the tests themselves are safe code.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::Cursor;
use std::path::PathBuf;
use std::ptr;

use scarletbook::{AreaCapacity, Disc, Error, Slot};
use scarletbook_ffi::*;

const S: usize = 2048;

fn open(bytes: &[u8]) -> Result<*mut SbDisc, SbStatus> {
    let mut d: *mut SbDisc = ptr::dangling_mut();
    // SAFETY: `bytes` is valid for its length; `&mut d` is a valid out-pointer.
    match unsafe { sb_disc_open_memory(bytes.as_ptr(), bytes.len(), &mut d) } {
        SbStatus::Ok => Ok(d),
        s => Err(s),
    }
}

fn free(disc: *mut SbDisc) {
    // SAFETY: `disc` is NULL or a live handle that the caller does not use again.
    unsafe { sb_disc_free(disc) };
}

fn capacity(disc: *const SbDisc, slot: u32) -> Result<SbAreaCapacity, SbStatus> {
    // Reserved bytes and flags that the call must overwrite.
    let mut c = SbAreaCapacity { has_plain_frame_bound: 0xEE, reserved: [0xEE; 7], ..Default::default() };
    // SAFETY: `disc` is NULL or a live handle; `&mut c` is a valid out-pointer.
    match unsafe { sb_area_capacity(disc, slot, &mut c) } {
        SbStatus::Ok => Ok(c),
        s => Err(s),
    }
}

/// The C figures of the area from `slot` equal the Rust API's, or both fail alike.
fn check(disc: *const SbDisc, rust: &Disc<Cursor<&[u8]>>, slot: Slot, what: &str) -> Option<AreaCapacity> {
    let n = match slot {
        Slot::TwoChannel => 0,
        Slot::Multichannel => 1,
    };
    let want = match rust.area_capacity(slot) {
        Ok(c) => c,
        Err(Error::NoSuchArea(_)) => {
            assert_eq!(capacity(disc, n).unwrap_err(), SbStatus::NoSuchArea, "{what}: {slot}");
            return None;
        }
        Err(e) => panic!("{what}: {slot}: {e}"),
    };
    let c = capacity(disc, n).unwrap();
    assert_eq!(
        (c.audio_sectors, c.capacity, c.playtime_frames, c.max_track_end),
        (want.audio_sectors, want.capacity, want.playtime_frames, want.max_track_end),
        "{what}: {slot}"
    );
    let bound = (c.has_plain_frame_bound == 1).then_some(c.plain_frame_bound);
    assert_eq!(bound, want.plain_frame_bound, "{what}: {slot}");
    assert!(c.has_plain_frame_bound <= 1 && (bound.is_some() || c.plain_frame_bound == 0), "{what}");
    // The same value, infinity included; never NaN.
    assert_eq!(c.frames_per_sector.to_bits(), want.frames_per_sector().to_bits(), "{what}: {slot}");
    assert!(!c.frames_per_sector.is_nan(), "{what}: {slot}");
    assert_eq!(c.reserved, [0; 7], "{what}: {slot}");
    Some(want)
}

/// Every corpus image that opens gives the same figures through C as through Rust; the
/// hostile images #340 names give the figures measured there.
#[test]
#[cfg_attr(miri, ignore = "reads every corpus image; c_equals_rust_on_tiny_images covers the calls")]
fn c_equals_rust_on_the_corpus() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/images");
    let mut paths: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    paths.sort();
    let (mut images, mut areas) = (0, 0);
    let mut hostile = Vec::new();
    for p in paths.iter().filter(|p| p.extension().is_some_and(|x| x == "iso")) {
        let id = p.file_stem().unwrap().to_string_lossy().into_owned();
        let bytes = std::fs::read(p).unwrap();
        let Ok(disc) = open(&bytes) else { continue };
        let rust = Disc::open(Cursor::new(&bytes[..])).unwrap();
        for slot in [Slot::TwoChannel, Slot::Multichannel] {
            let Some(c) = check(disc, &rust, slot, &id) else { continue };
            areas += 1;
            if slot == Slot::TwoChannel {
                hostile.push((id.clone(), c));
            }
        }
        images += 1;
        free(disc);
    }
    assert!(images > 100 && areas > images, "{images} images, {areas} areas");
    let two = |id: &str| hostile.iter().find(|(i, _)| i == id).unwrap().1;
    // P / n = 1 166 880 / 56, about 20 837 (#340).
    let c = two("area-playtime-max");
    assert_eq!((c.audio_sectors, c.playtime_frames, c.max_track_end), (56, 1_166_880, 12));
    assert_eq!(c.frames_per_sector().floor(), 20_837.0);
    // 12 / 7, about 1.71 (#340); the 7 sectors left hold at most 1 plain frame.
    let c = two("truncated-in-audio");
    assert_eq!((c.audio_sectors, c.playtime_frames, c.plain_frame_bound), (7, 12, Some(1)));
    assert_eq!((c.frames_per_sector() * 100.0).round(), 171.0);
    // The TOC of these is hostile-base's: their sectors, not their TOC, are hostile.
    for id in ["audio-zero", "audio-random"] {
        assert_eq!(two(id), two("hostile-base"), "{id}");
    }
    // The area's last audio LSN lies past the end of the image: only the sectors that
    // exist count.
    let c = two("area-audio-past-end");
    assert_eq!((c.audio_sectors, c.capacity, c.plain_frame_bound), (157, 1099, Some(34)));
    // Every audio sector past the end: infinitely many frames per sector.
    let c = two("area-audio-start-past-end");
    assert_eq!((c.audio_sectors, c.capacity, c.plain_frame_bound), (0, 0, Some(0)));
    assert_eq!(c.frames_per_sector(), f64::INFINITY);
    for id in ["area-channels-0", "area-channels-255"] {
        assert_eq!(two(id).plain_frame_bound, None, "{id}");
    }
}

/// A tiny image: a 2-channel area of `channels` channels whose audio sectors are
/// `[start, end]` as stored, play time `playtime` frames and one track of `duration`
/// frames, in an image of `sectors` sectors (at least 543).
fn tiny_image(
    sectors: usize,
    channels: u8,
    (start, end): (u32, u32),
    playtime: u32,
    duration: u32,
) -> Vec<u8> {
    let tc = |n: u32| [(n / 4500) as u8, (n / 75 % 60) as u8, (n % 75) as u8];
    let mut img = vec![0u8; sectors * S];
    let m = 510 * S;
    img[m..m + 8].copy_from_slice(b"SACDMTOC");
    img[m + 8] = 1;
    img[m + 64..m + 68].copy_from_slice(&540u32.to_be_bytes());
    img[m + 84..m + 86].copy_from_slice(&3u16.to_be_bytes());
    let a = 540 * S;
    img[a..a + 8].copy_from_slice(b"TWOCHTOC");
    img[a + 8..a + 10].copy_from_slice(&[1, 20]);
    img[a + 32] = channels;
    img[a + 64..a + 67].copy_from_slice(&tc(playtime));
    img[a + 69] = 1;
    img[a + 72..a + 76].copy_from_slice(&start.to_be_bytes());
    img[a + 76..a + 80].copy_from_slice(&end.to_be_bytes());
    img[a + S..a + S + 8].copy_from_slice(b"SACDTRL1");
    let t2 = a + 2 * S;
    img[t2..t2 + 8].copy_from_slice(b"SACDTRL2");
    img[t2 + 1028..t2 + 1031].copy_from_slice(&tc(duration));
    img
}

/// Tiny images with hostile TOCs, small enough for Miri: the C figures equal the Rust
/// API's and their derivation.
#[test]
fn c_equals_rust_on_tiny_images() {
    // 10 audio sectors, 2 channels: capacity 70, at most floor(10 x 2045 / 9408) = 2 plain
    // frames; the TOC claims 75 frames, 7.5 per sector.
    /// Image sectors, channels, audio LSNs, play time, track duration; then the expected
    /// `n`, capacity and play time, plain bound and frames per sector.
    type Case = (usize, u8, (u32, u32), u32, u32, [u64; 3], Option<u64>, f64);
    let cases: [Case; 5] = [
        (553, 2, (543, 552), 75, 30, [10, 70, 75], Some(2), 7.5),
        // The image ends 4 sectors into the area.
        (547, 6, (543, 552), 3, 3, [4, 28, 3], Some(0), 0.75),
        // Every audio sector past the end of the image: n = 0, infinitely many per sector.
        (543, 2, (600, 700), 0, 0, [0, 0, 0], Some(0), f64::INFINITY),
        // No audio for these channel counts: no plain bound.
        (553, 0, (543, 552), 5, 5, [10, 70, 5], None, 0.5),
        (553, 255, (543, 552), 5, 5, [10, 70, 5], None, 0.5),
    ];
    for (i, (sectors, channels, range, playtime, duration, [n, q, p], bound, per)) in
        cases.into_iter().enumerate()
    {
        let img = tiny_image(sectors, channels, range, playtime, duration);
        let disc = open(&img).unwrap_or_else(|e| panic!("case {i}: {e:?}"));
        let rust = Disc::open(Cursor::new(&img[..])).unwrap();
        let c = check(disc, &rust, Slot::TwoChannel, &format!("case {i}")).unwrap();
        assert_eq!(
            (c.audio_sectors, c.capacity, u64::from(c.playtime_frames), c.plain_frame_bound),
            (n, q, p, bound),
            "case {i}"
        );
        assert_eq!(c.max_track_end, duration, "case {i}");
        assert_eq!(c.frames_per_sector(), per, "case {i}");
        // No multichannel area; bad arguments.
        assert_eq!(capacity(disc, 1).unwrap_err(), SbStatus::NoSuchArea);
        assert_eq!(capacity(disc, 2).unwrap_err(), SbStatus::InvalidArgument);
        // SAFETY: `disc` is a live handle; a NULL output pointer is allowed and refused.
        assert_eq!(unsafe { sb_area_capacity(disc, 0, ptr::null_mut()) }, SbStatus::InvalidArgument);
        free(disc);
    }
    assert_eq!(capacity(ptr::null(), 0).unwrap_err(), SbStatus::InvalidArgument);
}
