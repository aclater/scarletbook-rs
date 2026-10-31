// SPDX-License-Identifier: MIT OR Apache-2.0

//! `sb_disc_toc`, the whole TOC in one buffer (#132): its layout is pinned (golden
//! offsets and sizes, the numbers the header and README document), and on every corpus
//! image each field read from the buffer equals what the per-field call returns.
//!
//! The buffer is parsed here with the documented offsets, not through the Rust structs, so
//! the test checks the documentation as well as the code.
//!
//! Each unsafe call sits in a small wrapper that states why its arguments meet the
//! function's contract; the tests themselves are safe code.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::ffi::{CStr, c_char};
use std::mem::{offset_of, size_of};
use std::path::PathBuf;
use std::ptr;

use scarletbook::text::MAX_STRING_BYTES;
use scarletbook_ffi::*;

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

/// `sb_disc_toc` into `buf` (NULL when `None`) with capacity `cap`.
fn toc_call(
    disc: *const SbDisc,
    flags: u32,
    buf: Option<&mut [u8]>,
    cap: usize,
    len: *mut usize,
) -> SbStatus {
    let p = buf.map_or(ptr::null_mut(), |b| {
        assert!(b.len() >= cap);
        b.as_mut_ptr()
    });
    // SAFETY: `disc` is NULL or a live handle; `p` is NULL or valid for `cap` bytes; `len`
    // is NULL or valid for a write.
    unsafe { sb_disc_toc(disc, flags, p, cap, len) }
}

/// The layout, by the recommended pattern: a 64 KiB buffer, then `*len` bytes if that is too
/// small.
fn toc(disc: *const SbDisc) -> Vec<u8> {
    let mut buf = vec![0xAAu8; 64 * 1024];
    let mut len = 0usize;
    let cap = buf.len();
    match toc_call(disc, 0, Some(&mut buf), cap, &mut len) {
        SbStatus::Ok => {}
        SbStatus::BufferTooSmall => {
            buf = vec![0xAAu8; len];
            assert_eq!(toc_call(disc, 0, Some(&mut buf), len, &mut len), SbStatus::Ok);
        }
        s => panic!("{s:?}"),
    }
    buf.truncate(len);
    buf
}

fn u8_at(b: &[u8], at: usize) -> u8 {
    b[at]
}
fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(b[at..at + 2].try_into().unwrap())
}
fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

/// The bytes of the `SbTocStr` at `at`, checked against the string area; `None` if absent.
fn str_at(b: &[u8], at: usize) -> Option<Vec<u8>> {
    let (offset, length) = (u32_at(b, at), u32_at(b, at + 4));
    if offset == SB_TOC_ABSENT {
        assert_eq!(length, 0);
        return None;
    }
    let (strings, size) = (u32_at(b, 44) as usize, u32_at(b, 48) as usize);
    let (o, l) = (offset as usize, length as usize);
    assert!(o >= strings && o + l < strings + size, "string inside the string area");
    assert_eq!(b[o + l], 0, "NUL after the string");
    Some(b[o..o + l].to_vec())
}

/// A string through the per-field call `f(buf, cap, len)`, `None` if absent.
fn per_field(f: impl Fn(*mut c_char, usize, *mut usize) -> SbStatus) -> Option<Vec<u8>> {
    let mut len = 0usize;
    match f(ptr::null_mut(), 0, &mut len) {
        SbStatus::Absent => return None,
        SbStatus::BufferTooSmall => {}
        s => panic!("{s:?}"),
    }
    let mut buf = vec![0 as c_char; len + 1];
    assert_eq!(f(buf.as_mut_ptr(), len + 1, &mut len), SbStatus::Ok);
    Some(buf[..len].iter().map(|&c| c as u8).collect())
}

#[test]
fn layout_is_pinned() {
    // Sizes of format 1.0.
    assert_eq!(
        [
            size_of::<SbTocHeader>(),
            size_of::<SbTocDisc>(),
            size_of::<SbTocArea>(),
            size_of::<SbTocTrack>(),
            size_of::<SbTocWarning>(),
            size_of::<SbTocStr>()
        ],
        [56, 256, 152, 168, 24, 8]
    );
    assert_eq!((SB_TOC_MAJOR, SB_TOC_MINOR, SB_TOC_ABSENT), (1, 0, 0xFFFF_FFFF));
    // Every offset the documentation states.
    assert_eq!(
        [
            offset_of!(SbTocHeader, major),
            offset_of!(SbTocHeader, minor),
            offset_of!(SbTocHeader, size),
            offset_of!(SbTocHeader, disc_offset),
            offset_of!(SbTocHeader, disc_record_size),
            offset_of!(SbTocHeader, area_record_size),
            offset_of!(SbTocHeader, track_record_size),
            offset_of!(SbTocHeader, warning_record_size),
            offset_of!(SbTocHeader, areas_offset),
            offset_of!(SbTocHeader, area_count),
            offset_of!(SbTocHeader, warnings_offset),
            offset_of!(SbTocHeader, warning_count),
            offset_of!(SbTocHeader, strings_offset),
            offset_of!(SbTocHeader, strings_size),
            offset_of!(SbTocHeader, reserved),
        ],
        [8, 10, 12, 16, 20, 22, 24, 26, 28, 32, 36, 40, 44, 48, 52]
    );
    assert_eq!(
        [
            offset_of!(SbTocDisc, album_genres),
            offset_of!(SbTocDisc, disc_genres),
            offset_of!(SbTocDisc, text_channel_count),
            offset_of!(SbTocDisc, text_charset),
            offset_of!(SbTocDisc, disc_flags),
            offset_of!(SbTocDisc, text_channels),
            offset_of!(SbTocDisc, reserved2),
            offset_of!(SbTocDisc, strings),
            offset_of!(SbTocDisc, catalog_numbers_stored),
            offset_of!(SbTocDisc, manufacturer),
        ],
        [32, 40, 48, 49, 50, 52, 84, 88, 232, 248]
    );
    assert_eq!(
        [
            offset_of!(SbTocArea, details),
            offset_of!(SbTocArea, loudspeaker_layout),
            offset_of!(SbTocArea, channel_speakers),
            offset_of!(SbTocArea, playtime_stored),
            offset_of!(SbTocArea, tracks_offset),
            offset_of!(SbTocArea, strings),
            offset_of!(SbTocArea, text_channels),
        ],
        [52, 60, 64, 70, 76, 80, 112]
    );
    assert_eq!(
        [
            offset_of!(SbTocTrack, start_stored),
            offset_of!(SbTocTrack, duration_stored),
            offset_of!(SbTocTrack, strings),
            offset_of!(SbTocTrack, reserved2),
        ],
        [28, 31, 36, 164]
    );
    assert_eq!(
        [offset_of!(SbTocWarning, slot), offset_of!(SbTocWarning, track), offset_of!(SbTocWarning, code)],
        [8, 12, 16]
    );
    // The embedded structs of the per-field calls keep their layouts.
    assert_eq!(
        [
            size_of::<SbDiscInfo>(),
            size_of::<SbAreaInfo>(),
            size_of::<SbTrackInfo>(),
            size_of::<SbAreaDetails>()
        ],
        [32, 52, 28, 8]
    );

    // A header as written.
    let bytes = tiny_image();
    let disc = open(&bytes).unwrap();
    let b = toc(disc);
    free(disc);
    assert_eq!(&b[..8], b"SBTOC\0\0\0");
    assert_eq!((u16_at(&b, 8), u16_at(&b, 10)), (1, 0));
    assert_eq!(u32_at(&b, 12) as usize, b.len());
    assert_eq!(u32_at(&b, 16), 56);
    assert_eq!([u16_at(&b, 20), u16_at(&b, 22), u16_at(&b, 24), u16_at(&b, 26)], [256, 152, 168, 24]);
    assert_eq!(u32_at(&b, 28), 56 + 256);
    assert_eq!(u32_at(&b, 52), 0);
    assert_eq!(u32_at(&b, 44) + u32_at(&b, 48), u32_at(&b, 12), "the strings end the layout");
}

#[test]
fn a_tiny_image_field_by_field() {
    let bytes = tiny_image();
    let disc = open(&bytes).unwrap();
    check_disc("tiny", disc);
    free(disc);
}

#[test]
#[cfg_attr(miri, ignore = "reads every corpus image")]
fn every_field_equals_the_per_field_call() {
    let mut opened = 0;
    for (name, bytes) in corpus_images() {
        let Ok(disc) = open(&bytes) else { continue };
        opened += 1;
        check_disc(&name, disc);
        free(disc);
    }
    assert!(opened > 100, "{opened} corpus images opened");
}

/// Calls into the library from `check_disc`.
macro_rules! ffi {
    ($call:expr) => {{
        // SAFETY: in `check_disc`, `disc` is a live handle, out-pointers point to locals,
        // and string calls get a buffer valid for the capacity passed (see `per_field`).
        unsafe { $call }
    }};
}

fn check_disc(name: &str, disc: *mut SbDisc) {
    let b = toc(disc);
    let mut info = SbDiscInfo::default();
    assert_eq!(ffi!(sb_disc_info(disc, &mut info)), SbStatus::Ok);
    let d = u32_at(&b, 16) as usize;
    assert_eq!(u64_at(&b, d), info.sectors, "{name}");
    assert_eq!(u32_at(&b, d + 8), info.sector_size, "{name}");
    assert_eq!([u8_at(&b, d + 12), u8_at(&b, d + 13)], [info.version_major, info.version_minor], "{name}");
    assert_eq!([u16_at(&b, d + 14), u16_at(&b, d + 16)], [info.album_set_size, info.album_sequence_number]);
    assert_eq!([u8_at(&b, d + 18), u8_at(&b, d + 19)], [info.hybrid, info.manufacturer_sector], "{name}");
    assert_eq!((u16_at(&b, d + 20), u8_at(&b, d + 22), u8_at(&b, d + 23)), (info.year, info.month, info.day));
    assert_eq!([u32_at(&b, d + 24), u32_at(&b, d + 28)], [info.area_count, info.warning_count], "{name}");
    assert_eq!(u32_at(&b, 32), info.area_count);
    assert_eq!(u32_at(&b, 40), info.warning_count);
    for (list, at) in [(SB_GENRE_LIST_ALBUM, 32), (SB_GENRE_LIST_DISC, 40)] {
        for i in 0..4 {
            let mut g = SbGenre::default();
            assert_eq!(ffi!(sb_disc_genre(disc, list, i, &mut g)), SbStatus::Ok);
            let o = d + at + 2 * i as usize;
            assert_eq!([u8_at(&b, o), u8_at(&b, o + 1)], [g.category, g.index], "{name} genre {list} {i}");
        }
    }
    let mut channels = 0u32;
    assert_eq!(ffi!(sb_disc_text_channel_count(disc, &mut channels)), SbStatus::Ok);
    assert_eq!(u32::from(u8_at(&b, d + 48)), channels, "{name}");
    for i in 0..8u32 {
        let o = d + 52 + 4 * i as usize;
        let mut c = SbTextChannel::default();
        match ffi!(sb_disc_text_channel(disc, i, &mut c)) {
            SbStatus::Ok => assert_eq!(b[o..o + 4], [c.language[0], c.language[1], c.charset, 0], "{name}"),
            _ => assert_eq!(b[o..o + 4], [0; 4], "{name} channel {i}"),
        }
    }
    for field in 0..18u32 {
        let want = per_field(|p, c, l| ffi!(sb_disc_string(disc, field, p, c, l)));
        assert_eq!(str_at(&b, d + 88 + 8 * field as usize), want, "{name} disc string {field}");
    }
    for (k, field) in
        [SbDiscString::AlbumCatalogNumber, SbDiscString::DiscCatalogNumber].into_iter().enumerate()
    {
        let want = per_field(|p, c, l| ffi!(sb_disc_catalog_number_stored(disc, field as u32, p, c, l)));
        assert_eq!(str_at(&b, d + 232 + 8 * k), want, "{name} catalogue {k}");
    }
    let mut len = 0usize;
    let manufacturer = match ffi!(sb_disc_manufacturer(disc, ptr::null_mut(), 0, &mut len)) {
        SbStatus::Absent => None,
        SbStatus::BufferTooSmall => {
            let mut m = vec![0u8; len];
            assert_eq!(ffi!(sb_disc_manufacturer(disc, m.as_mut_ptr(), len, &mut len)), SbStatus::Ok);
            Some(m)
        }
        s => panic!("{s:?}"),
    };
    assert_eq!(str_at(&b, d + 248), manufacturer, "{name}");

    let mut track_records = u32_at(&b, 28) as usize + 152 * info.area_count as usize;
    for index in 0..info.area_count {
        let a = u32_at(&b, 28) as usize + 152 * index as usize;
        let mut area = SbAreaInfo::default();
        assert_eq!(ffi!(sb_disc_area(disc, index, &mut area)), SbStatus::Ok);
        let fields = [
            area.slot,
            area.kind_channels,
            area.toc_lsn,
            area.toc_sectors,
            area.channel_count,
            area.area_config,
            area.sample_frequency_code,
            area.frame_format,
            area.max_byte_rate,
            area.playtime_frames,
            area.audio_start_lsn,
            area.audio_end_lsn,
            area.track_count,
        ];
        for (k, v) in fields.into_iter().enumerate() {
            assert_eq!(u32_at(&b, a + 4 * k), v, "{name} area {index} field {k}");
        }
        let slot = area.slot;
        let mut x = SbAreaDetails::default();
        assert_eq!(ffi!(sb_area_details(disc, slot, &mut x)), SbStatus::Ok);
        let want = [
            x.version_major,
            x.version_minor,
            x.max_available_channels,
            x.mute_flags,
            x.track_attribute,
            x.track_offset,
            x.text_channel_count,
            x.text_charset,
        ];
        assert_eq!(b[a + 52..a + 60], want, "{name}");
        let mut layout = 0u32;
        assert_eq!(ffi!(sb_area_loudspeaker_layout(disc, slot, &mut layout)), SbStatus::Ok);
        assert_eq!(u32_at(&b, a + 60), layout, "{name}");
        for c in 0..6u32 {
            let mut speaker = 0u32;
            let want = match ffi!(sb_area_channel_speaker(disc, slot, c, &mut speaker)) {
                SbStatus::Ok => speaker as u8,
                _ => 0,
            };
            assert_eq!(u8_at(&b, a + 64 + c as usize), want, "{name} speaker {c}");
        }
        let mut playtime = SbTimecode::default();
        assert_eq!(ffi!(sb_area_playtime_stored(disc, slot, &mut playtime)), SbStatus::Ok);
        assert_eq!(b[a + 70..a + 73], [playtime.minutes, playtime.seconds, playtime.frames], "{name}");
        assert_eq!(b[a + 73..a + 76], [0; 3]);
        for field in 0..4u32 {
            let want = per_field(|p, c, l| ffi!(sb_area_string(disc, slot, field, p, c, l)));
            assert_eq!(str_at(&b, a + 80 + 8 * field as usize), want, "{name} area string {field}");
        }
        for i in 0..10u32 {
            let o = a + 112 + 4 * i as usize;
            let mut c = SbTextChannel::default();
            match ffi!(sb_area_text_channel(disc, slot, i, &mut c)) {
                SbStatus::Ok => {
                    assert_eq!(b[o..o + 4], [c.language[0], c.language[1], c.charset, 0], "{name}")
                }
                _ => assert_eq!(b[o..o + 4], [0; 4], "{name} area channel {i}"),
            }
        }

        let tracks = u32_at(&b, a + 76) as usize;
        assert_eq!(tracks, track_records, "{name}: track records follow each other");
        for number in 1..=area.track_count {
            let t = tracks + 168 * (number - 1) as usize;
            let mut ti = SbTrackInfo::default();
            assert_eq!(ffi!(sb_track_info(disc, slot, number, &mut ti)), SbStatus::Ok);
            for (k, v) in [ti.number, ti.start_lsn, ti.length_sectors, ti.start_frame, ti.duration_frames]
                .into_iter()
                .enumerate()
            {
                assert_eq!(u32_at(&b, t + 4 * k), v, "{name} track {number} field {k}");
            }
            let want =
                [ti.start_flags, ti.duration_flags, ti.has_genre, ti.genre_category, ti.genre_index, 0, 0, 0];
            assert_eq!(b[t + 20..t + 28], want, "{name} track {number}");
            let (mut s, mut l) = (SbTimecode::default(), SbTimecode::default());
            assert_eq!(ffi!(sb_track_timecodes_stored(disc, slot, number, &mut s, &mut l)), SbStatus::Ok);
            assert_eq!(
                b[t + 28..t + 36],
                [s.minutes, s.seconds, s.frames, l.minutes, l.seconds, l.frames, 0, 0]
            );
            for field in 0..16u32 {
                let want = (field < 15)
                    .then(|| per_field(|p, c, l| ffi!(sb_track_string(disc, slot, number, field, p, c, l))))
                    .flatten();
                assert_eq!(
                    str_at(&b, t + 36 + 8 * field as usize),
                    want,
                    "{name} track {number} string {field}"
                );
            }
            assert_eq!(u32_at(&b, t + 164), 0);
        }
        track_records += 168 * area.track_count as usize;
    }

    let warnings = u32_at(&b, 36) as usize;
    assert_eq!(warnings, track_records, "{name}: warnings follow the tracks");
    for i in 0..info.warning_count {
        let o = warnings + 24 * i as usize;
        let mut w = SbWarning { lsn: 0, slot: 0, track: 0, code: ptr::null() };
        assert_eq!(ffi!(sb_disc_warning(disc, i, &mut w)), SbStatus::Ok);
        assert_eq!(
            (u64_at(&b, o) as i64, u32_at(&b, o + 8) as i32, u32_at(&b, o + 12)),
            (w.lsn, w.slot, w.track)
        );
        // SAFETY: `code` is a static NUL-terminated string.
        let code = unsafe { CStr::from_ptr(w.code) }.to_bytes().to_vec();
        assert_eq!(str_at(&b, o + 16), Some(code), "{name} warning {i}");
    }
    assert_eq!(u32_at(&b, 44) as usize, warnings + 24 * info.warning_count as usize, "{name}");
}

#[test]
fn size_query_and_refusals() {
    let bytes = tiny_image();
    let disc = open(&bytes).unwrap();
    let mut len = usize::MAX;
    assert_eq!(toc_call(disc, 0, None, 0, &mut len), SbStatus::BufferTooSmall);
    let size = len;
    let mut buf = vec![0x5Au8; size + 8];
    // One byte short: nothing written.
    assert_eq!(toc_call(disc, 0, Some(&mut buf), size - 1, &mut len), SbStatus::BufferTooSmall);
    assert_eq!(len, size);
    assert!(buf.iter().all(|&x| x == 0x5A));
    // Exactly the size: written, and not a byte past it.
    assert_eq!(toc_call(disc, 0, Some(&mut buf), size, &mut len), SbStatus::Ok);
    assert_eq!(len, size);
    assert!(buf[size..].iter().all(|&x| x == 0x5A));
    assert_eq!(&buf[..size], toc(disc).as_slice(), "the same on every call");
    // Refusals.
    assert_eq!(toc_call(disc, 1, Some(&mut buf), size, &mut len), SbStatus::InvalidArgument);
    assert_eq!(toc_call(disc, 0x8000_0000, None, 0, &mut len), SbStatus::InvalidArgument);
    assert_eq!(toc_call(disc, 0, Some(&mut buf), size, ptr::null_mut()), SbStatus::InvalidArgument);
    assert_eq!(toc_call(ptr::null(), 0, None, 0, &mut len), SbStatus::InvalidArgument);
    // A clone gives the same layout.
    let mut clone: *mut SbDisc = ptr::null_mut();
    // SAFETY: `disc` is a live handle; `&mut clone` is a valid out-pointer.
    assert_eq!(unsafe { sb_disc_clone(disc, &mut clone) }, SbStatus::Ok);
    assert_eq!(toc(clone), toc(disc));
    free(clone);
    free(disc);
}

/// An upper bound of the layout's size: 2 areas of 255 tracks, every string as long as
/// decoding can make it (a text string is at most `MAX_STRING_BYTES` stored bytes, and no
/// character set gives more than 4 UTF-8 bytes per stored byte), and more warnings than
/// the parser can raise (at most 3 per track, a few per area and for the disc).
const MAX_LAYOUT: u64 = {
    let string = 4 * MAX_STRING_BYTES as u64 + 1;
    let warnings = 8 + 2 * (16 + 255 * 4);
    let records = 56 + 256 + 2 * 152 + 2 * 255 * 168 + warnings * 24;
    let strings = (18 + 2 + 2 * 4 + 2 * 255 * 15) * string + (2040 + 1) + warnings * 32;
    records + strings
};

#[test]
#[cfg_attr(miri, ignore = "reads every corpus image")]
fn the_size_is_bounded_and_fits_the_offsets() {
    const { assert!(MAX_LAYOUT < u32::MAX as u64) };
    for (name, bytes) in corpus_images() {
        let Ok(disc) = open(&bytes) else { continue };
        let mut len = 0usize;
        assert_eq!(toc_call(disc, 0, None, 0, &mut len), SbStatus::BufferTooSmall);
        assert!((len as u64) <= MAX_LAYOUT, "{name}: {len}");
        free(disc);
    }
}

/// The tiny image of `toc_fields.rs`, with genres, two text channels, padded catalogue
/// numbers and area fields set: small enough for Miri.
fn tiny_image() -> Vec<u8> {
    let s = 2048;
    let mut img = vec![0u8; 547 * s];
    let m = 510 * s;
    img[m..m + 8].copy_from_slice(b"SACDMTOC");
    img[m + 8] = 1;
    img[m + 24..m + 40].copy_from_slice(b" CAT-1\xa0\0junkjunk");
    img[m + 40..m + 48].copy_from_slice(&[1, 0, 0, 14, 2, 0, 0, 7]);
    img[m + 64..m + 68].copy_from_slice(&540u32.to_be_bytes());
    img[m + 84..m + 86].copy_from_slice(&6u16.to_be_bytes());
    img[m + 88..m + 104].copy_from_slice(b"DISC-2          ");
    img[m + 104..m + 108].copy_from_slice(&[1, 0, 0, 5]);
    img[m + 128] = 9;
    img[m + 136..m + 144].copy_from_slice(&[b'e', b'n', 2, 0, 0, b'e', 0x0B, 0]);
    let man = m + 9 * s;
    img[man..man + 8].copy_from_slice(b"SACD_Man");
    for (i, b) in img[man + 8..man + s].iter_mut().enumerate() {
        *b = i as u8;
    }
    let a = 540 * s;
    img[a..a + 8].copy_from_slice(b"TWOCHTOC");
    img[a + 8..a + 10].copy_from_slice(&[1, 20]);
    img[a + 32] = 2;
    img[a + 34..a + 36].copy_from_slice(&[6, 0x81]);
    img[a + 48] = 0xA3;
    img[a + 66] = 10;
    img[a + 68] = 4;
    img[a + 69] = 1;
    img[a + 72..a + 76].copy_from_slice(&546u32.to_be_bytes());
    img[a + 76..a + 80].copy_from_slice(&546u32.to_be_bytes());
    img[a + 80] = 1;
    img[a + 88..a + 92].copy_from_slice(&[b'j', b'a', 3, 0]);
    img[a + s..a + s + 8].copy_from_slice(b"SACDTRL1");
    let t2 = a + 2 * s;
    img[t2..t2 + 8].copy_from_slice(b"SACDTRL2");
    img[t2 + 1028 + 2] = 2;
    img
}
