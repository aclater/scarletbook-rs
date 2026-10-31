// SPDX-License-Identifier: MIT OR Apache-2.0

//! The TOC fields of issue #104 through the C ABI (genres, text channels, catalogue
//! numbers as stored, the manufacturer sector, more area fields), driven from Rust as a C
//! caller would, against the Rust API and against the stored bytes.
//!
//! The stored bytes never come from the library: the test takes the user data of the TOC
//! sectors from the image file itself, with the data offsets of specification 1, 1.2.
//!
//! Each unsafe call sits in a small wrapper that states why its arguments meet the
//! function's contract; the tests themselves are safe code.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::ffi::c_char;
use std::io::Cursor;
use std::path::PathBuf;
use std::ptr;

use scarletbook::{Area, Disc, MasterToc, Slot};
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

/// The user data of sector `lsn` of an image of `p`-byte stored sectors.
fn sector(image: &[u8], p: usize, lsn: usize) -> &[u8] {
    let d = match p {
        2048 => 0,
        2054 => 6,
        2064 => 12,
        _ => panic!("stored sector size {p}"),
    };
    &image[lsn * p + d..][..S]
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

fn sector_size(disc: *const SbDisc) -> usize {
    let mut i = SbDiscInfo::default();
    // SAFETY: `disc` is a live handle; `&mut i` is a valid out-pointer.
    assert_eq!(unsafe { sb_disc_info(disc, &mut i) }, SbStatus::Ok);
    i.sector_size as usize
}

fn genre(disc: *const SbDisc, list: u32, index: u32) -> Result<SbGenre, SbStatus> {
    let mut g = SbGenre::default();
    // SAFETY: `disc` is NULL or a live handle; `&mut g` is a valid out-pointer.
    match unsafe { sb_disc_genre(disc, list, index, &mut g) } {
        SbStatus::Ok => Ok(g),
        s => Err(s),
    }
}

fn text_channel_count(disc: *const SbDisc) -> u32 {
    let mut n = u32::MAX;
    // SAFETY: `disc` is a live handle; `&mut n` is a valid out-pointer.
    assert_eq!(unsafe { sb_disc_text_channel_count(disc, &mut n) }, SbStatus::Ok);
    n
}

fn disc_channel(disc: *const SbDisc, index: u32) -> Result<SbTextChannel, SbStatus> {
    let mut c = SbTextChannel { reserved: 0xEE, ..Default::default() };
    // SAFETY: `disc` is a live handle; `&mut c` is a valid out-pointer.
    match unsafe { sb_disc_text_channel(disc, index, &mut c) } {
        SbStatus::Ok => Ok(c),
        s => Err(s),
    }
}

fn area_channel(disc: *const SbDisc, slot: u32, index: u32) -> Result<SbTextChannel, SbStatus> {
    let mut c = SbTextChannel { reserved: 0xEE, ..Default::default() };
    // SAFETY: `disc` is a live handle; `&mut c` is a valid out-pointer.
    match unsafe { sb_area_text_channel(disc, slot, index, &mut c) } {
        SbStatus::Ok => Ok(c),
        s => Err(s),
    }
}

fn area_details(disc: *const SbDisc, slot: u32) -> Result<SbAreaDetails, SbStatus> {
    let mut a = SbAreaDetails::default();
    // SAFETY: `disc` is a live handle; `&mut a` is a valid out-pointer.
    match unsafe { sb_area_details(disc, slot, &mut a) } {
        SbStatus::Ok => Ok(a),
        s => Err(s),
    }
}

/// Copies bytes out with a call `f(buf, cap, len)` following the string convention: first
/// sizing, then copying with room for the NUL.
fn copied(f: impl Fn(*mut c_char, usize, *mut usize) -> SbStatus) -> Vec<u8> {
    let mut len = usize::MAX;
    assert_eq!(f(ptr::null_mut(), 0, &mut len), SbStatus::BufferTooSmall);
    let mut buf = vec![0x55 as c_char; len + 2];
    assert_eq!(f(buf.as_mut_ptr(), len, &mut len), SbStatus::BufferTooSmall, "no room for the NUL");
    assert!(buf.iter().all(|&b| b == 0x55));
    assert_eq!(f(buf.as_mut_ptr(), len + 1, &mut len), SbStatus::Ok);
    assert_eq!((buf[len], buf[len + 1]), (0, 0x55));
    buf[..len].iter().map(|&b| b as u8).collect()
}

fn catalog_stored(disc: *const SbDisc, field: SbDiscString) -> Vec<u8> {
    // SAFETY: `disc` is a live handle; `copied` passes a buffer valid for `c` bytes (or
    // NULL) and a valid length pointer.
    copied(|b, c, l| unsafe { sb_disc_catalog_number_stored(disc, field as u32, b, c, l) })
}

fn disc_string(disc: *const SbDisc, field: SbDiscString) -> Vec<u8> {
    // SAFETY: as in `catalog_stored`.
    copied(|b, c, l| unsafe { sb_disc_string(disc, field as u32, b, c, l) })
}

/// The manufacturer data, or the status and length of a call that has none.
fn manufacturer(disc: *const SbDisc) -> Result<Vec<u8>, (SbStatus, usize)> {
    let mut len = usize::MAX;
    // SAFETY: `disc` is a live handle; a NULL buffer is allowed; `&mut len` is valid.
    let s = unsafe { sb_disc_manufacturer(disc, ptr::null_mut(), 0, &mut len) };
    if s != SbStatus::BufferTooSmall {
        return Err((s, len));
    }
    let mut buf = vec![0x55u8; len + 1];
    // SAFETY: `buf` is valid for `len - 1` bytes of writes.
    let s = unsafe { sb_disc_manufacturer(disc, buf.as_mut_ptr(), len - 1, &mut len) };
    assert_eq!(s, SbStatus::BufferTooSmall);
    assert!(buf.iter().all(|&b| b == 0x55));
    // SAFETY: `buf` is valid for `len` bytes of writes.
    assert_eq!(unsafe { sb_disc_manufacturer(disc, buf.as_mut_ptr(), len, &mut len) }, SbStatus::Ok);
    assert_eq!(buf[len], 0x55, "written past the data");
    buf.truncate(len);
    Ok(buf)
}

fn stored_channel(b: &[u8]) -> ([u8; 2], u8) {
    ([b[0], b[1]], b[2])
}

/// The master TOC's fields of #104 against the Rust API and the stored sector 0 (`s0`) and
/// manufacturer sector (`s9`).
fn check_master(disc: *const SbDisc, m: &MasterToc, s0: &[u8], s9: &[u8], what: &str) {
    for (list, rust, at) in
        [(SB_GENRE_LIST_ALBUM, &m.album_genres, 40), (SB_GENRE_LIST_DISC, &m.disc_genres, 104)]
    {
        for i in 0..4 {
            let g = genre(disc, list, i as u32).unwrap();
            assert_eq!((g.category, g.index), (rust[i].category, rust[i].index), "{what}: genre {list} {i}");
            assert_eq!((g.category, g.index), (s0[at + 4 * i], s0[at + 4 * i + 3]), "{what}: stored genre");
        }
        assert_eq!(genre(disc, list, 4).unwrap_err(), SbStatus::InvalidArgument);
    }
    let n = text_channel_count(disc);
    assert_eq!(n, u32::from(m.text_channel_count), "{what}");
    assert_eq!(n, u32::from(s0[128]), "{what}");
    let listed = n.min(8);
    assert_eq!(listed as usize, m.locales.len(), "{what}");
    for i in 0..listed {
        let c = disc_channel(disc, i).unwrap();
        let l = &m.locales[i as usize];
        assert_eq!(
            (c.language, c.charset, c.reserved),
            (l.language_code, l.charset, 0),
            "{what}: channel {i}"
        );
        assert_eq!((c.language, c.charset), stored_channel(&s0[136 + 4 * i as usize..]), "{what}: stored");
        let trimmed: String = c.language.iter().map(|&b| char::from(b)).collect();
        assert_eq!(trimmed.trim_end_matches(['\0', ' ']), l.language, "{what}: channel {i} trimmed");
    }
    assert_eq!(disc_channel(disc, listed).unwrap_err(), SbStatus::InvalidArgument, "{what}");
    for (field, rust, trimmed, at) in [
        (SbDiscString::AlbumCatalogNumber, &m.album_catalog_number_stored, &m.album_catalog_number, 24),
        (SbDiscString::DiscCatalogNumber, &m.disc_catalog_number_stored, &m.disc_catalog_number, 88),
    ] {
        let stored = catalog_stored(disc, field);
        let raw = &s0[at..at + 16];
        let want = &raw[..raw.iter().position(|&b| b == 0).unwrap_or(16)];
        assert_eq!(stored, want, "{what}: {field:?} as stored");
        assert_eq!(&stored, rust, "{what}: {field:?}");
        assert_eq!(disc_string(disc, field), trimmed.as_bytes(), "{what}: {field:?} trimmed");
    }
    match manufacturer(disc) {
        Ok(bytes) => {
            assert_eq!(&s9[..8], b"SACD_Man", "{what}");
            assert_eq!(bytes.len(), SB_MANUFACTURER_BYTES as usize);
            assert_eq!(bytes, s9[8..], "{what}: manufacturer data");
            assert_eq!(Some(&bytes), m.manufacturer.as_ref(), "{what}");
        }
        Err(e) => {
            assert_eq!(e, (SbStatus::Absent, 0), "{what}");
            assert_ne!(&s9[..8], b"SACD_Man", "{what}");
            assert!(m.manufacturer.is_none());
        }
    }
}

/// An area's fields of #104 against the Rust API and its stored TOC sector 0 (`z`).
fn check_area(disc: *const SbDisc, a: &Area, z: &[u8], what: &str) {
    let slot = match a.slot {
        Slot::TwoChannel => 0,
        Slot::Multichannel => 1,
    };
    let d = area_details(disc, slot).unwrap();
    let got = [
        d.version_major,
        d.version_minor,
        d.max_available_channels,
        d.mute_flags,
        d.track_attribute,
        d.track_offset,
        d.text_channel_count,
        d.text_charset,
    ];
    let rust = [
        a.version.major,
        a.version.minor,
        a.max_available_channels,
        a.area_mute_flags,
        a.track_attribute,
        a.track_offset,
        a.text_channel_count,
        a.text_charset,
    ];
    assert_eq!(got, rust, "{what}: area {slot}");
    assert_eq!(
        got,
        [z[8], z[9], z[34], z[35], z[48] & 0x0F, z[68], z[80], z[90]],
        "{what}: area {slot} stored"
    );
    let listed = u32::from(d.text_channel_count.min(10));
    assert_eq!(listed as usize, a.locales.len());
    for i in 0..listed {
        let c = area_channel(disc, slot, i).unwrap();
        let l = &a.locales[i as usize];
        assert_eq!(
            (c.language, c.charset, c.reserved),
            (l.language_code, l.charset, 0),
            "{what}: {slot} {i}"
        );
        assert_eq!((c.language, c.charset), stored_channel(&z[88 + 4 * i as usize..]), "{what}: {slot} {i}");
    }
    assert_eq!(area_channel(disc, slot, listed).unwrap_err(), SbStatus::InvalidArgument);
}

/// Every #104 field of every corpus image that opens equals the Rust API's and the stored
/// bytes.
#[test]
#[cfg_attr(miri, ignore = "reads every corpus image")]
fn toc_fields_of_every_corpus_image() {
    let (mut images, mut areas, mut channels) = (0, 0, 0);
    for (id, bytes) in corpus_images() {
        let Ok(disc) = open(&bytes) else { continue };
        let rust = Disc::open(Cursor::new(&bytes[..])).unwrap();
        let toc = rust.toc();
        let p = sector_size(disc);
        check_master(disc, &toc.master, sector(&bytes, p, 510), sector(&bytes, p, 519), &id);
        for a in &toc.areas {
            check_area(disc, a, sector(&bytes, p, a.toc_lsn as usize), &id);
            channels += a.locales.len();
        }
        for (slot, n) in [(Slot::TwoChannel, 0), (Slot::Multichannel, 1)] {
            if toc.area(slot).is_none() {
                assert_eq!(area_details(disc, n).unwrap_err(), SbStatus::NoSuchArea, "{id}");
            }
        }
        images += 1;
        areas += toc.areas.len();
        channels += toc.master.locales.len();
        free(disc);
    }
    assert!(
        images > 100 && areas > images && channels > 0,
        "{images} images, {areas} areas, {channels} channels"
    );
}

/// The tiny image of the `ffi` tests, with genres, two text channels, padded catalogue
/// numbers and area fields set: small enough for Miri.
fn tiny_image() -> Vec<u8> {
    let s = S;
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

#[test]
fn toc_fields_of_a_tiny_image() {
    let img = tiny_image();
    let disc = open(&img).unwrap();
    let rust = Disc::open(Cursor::new(&img[..])).unwrap();
    check_master(disc, &rust.toc().master, &img[510 * S..511 * S], &img[519 * S..520 * S], "tiny");
    check_area(disc, &rust.toc().areas[0], &img[540 * S..541 * S], "tiny");
    // The values themselves.
    let g = genre(disc, SB_GENRE_LIST_ALBUM, 1).unwrap();
    assert_eq!((g.category, g.index), (2, 7));
    assert_eq!(text_channel_count(disc), 9);
    let c = disc_channel(disc, 1).unwrap();
    assert_eq!((c.language, c.charset), ([0, b'e'], 0x0B));
    assert_eq!(catalog_stored(disc, SbDiscString::AlbumCatalogNumber), b" CAT-1\xa0");
    assert_eq!(disc_string(disc, SbDiscString::AlbumCatalogNumber), b"CAT-1");
    assert_eq!(catalog_stored(disc, SbDiscString::DiscCatalogNumber), b"DISC-2          ");
    let d = area_details(disc, 0).unwrap();
    assert_eq!(
        [d.version_major, d.version_minor, d.max_available_channels, d.mute_flags, d.track_attribute],
        [1, 20, 6, 0x81, 3]
    );
    assert_eq!((d.track_offset, d.text_channel_count, d.text_charset), (4, 1, 3));
    assert_eq!(area_channel(disc, 0, 0).unwrap().language, *b"ja");
    assert_eq!(manufacturer(disc).unwrap()[..3], [0, 1, 2]);

    // Refusals.
    assert_eq!(genre(disc, 2, 0).unwrap_err(), SbStatus::InvalidArgument);
    assert_eq!(genre(ptr::null(), 0, 0).unwrap_err(), SbStatus::InvalidArgument);
    assert_eq!(area_details(disc, 1).unwrap_err(), SbStatus::NoSuchArea);
    assert_eq!(area_details(disc, 2).unwrap_err(), SbStatus::InvalidArgument);
    assert_eq!(area_channel(disc, 1, 0).unwrap_err(), SbStatus::NoSuchArea);
    let mut len = 0usize;
    // SAFETY: `disc` is a live handle; a NULL buffer is allowed; `&mut len` is valid.
    let s = unsafe { sb_disc_catalog_number_stored(disc, 0, ptr::null_mut(), 0, &mut len) };
    assert_eq!(s, SbStatus::InvalidArgument);
    // SAFETY: as above; a NULL length is refused.
    let s = unsafe { sb_disc_manufacturer(disc, ptr::null_mut(), 0, ptr::null_mut()) };
    assert_eq!(s, SbStatus::InvalidArgument);
    // SAFETY: `disc` is a live handle; NULL out-pointers are refused.
    let s = unsafe { sb_disc_text_channel_count(disc, ptr::null_mut()) };
    assert_eq!(s, SbStatus::InvalidArgument);
    // SAFETY: as above.
    assert_eq!(unsafe { sb_area_details(disc, 0, ptr::null_mut()) }, SbStatus::InvalidArgument);
    free(disc);

    // Without a manufacturer sector: absent, length 0.
    let mut img = img;
    img[519 * S] = b'X';
    let disc = open(&img).unwrap();
    assert_eq!(manufacturer(disc).unwrap_err(), (SbStatus::Absent, 0));
    free(disc);
}
