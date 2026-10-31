// SPDX-License-Identifier: MIT OR Apache-2.0

//! Regression tests for TOC rules that no corpus case exercises (the corpus README's
//! "known coverage limits"), each on a patched synthetic disc.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::io::Cursor;

use common::{AREA_TOC_LSN, S, Spec};
use scarletbook::{Disc, Error, Slot, WarningCode};

const A: usize = AREA_TOC_LSN * S;

fn put(img: &mut [u8], at: usize, bytes: &[u8]) {
    img[at..at + bytes.len()].copy_from_slice(bytes);
}

/// Writes master TOC bytes at `offset` in all three copies (LSN 510, 520, 530), so that
/// the copy comparison stays silent.
fn patch_master(img: &mut [u8], offset: usize, bytes: &[u8]) {
    for lsn in [510, 520, 530] {
        put(img, lsn * S + offset, bytes);
    }
}

fn set_area_toc_sectors(img: &mut [u8], n: u16) {
    patch_master(img, 84, &n.to_be_bytes());
}

/// The simple disc with enough audio sectors that a 9-sector area TOC at LSN 540 fits in
/// the image.
fn roomy() -> Vec<u8> {
    Spec { audio: vec![[0u8; S]; 4], ..Spec::simple() }.build()
}

fn open(img: Vec<u8>) -> Disc<Cursor<Vec<u8>>> {
    Disc::open(Cursor::new(img)).unwrap()
}

fn has(disc: &Disc<Cursor<Vec<u8>>>, code: WarningCode) -> bool {
    disc.warnings().iter().any(|w| w.code == code)
}

/// Rebuilds the area TOC's sub-structure chain from sector 1 on: each entry is a
/// signature, or `None` for a sector of zeros. The track list contents are taken from
/// the builder's own `SACDTRL1` (sector 4) and `SACDTRL2` (sector 5).
fn chain(img: &mut [u8], sectors: &[&[u8; 8]]) {
    let trl1: Vec<u8> = img[A + 4 * S..A + 5 * S].to_vec();
    let trl2: Vec<u8> = img[A + 5 * S..A + 6 * S].to_vec();
    let ttxt: Vec<u8> = img[A + S..A + 2 * S].to_vec();
    let igl: Vec<u8> = img[A + 2 * S..A + 4 * S].to_vec();
    let mut p = A + S;
    for &sig in sectors {
        match sig {
            b"SACDTRL1" => put(img, p, &trl1),
            b"SACDTRL2" => put(img, p, &trl2),
            b"SACDTTxt" => put(img, p, &ttxt),
            b"SACD_IGL" => {
                let room = (img.len() - p).min(2 * S);
                put(img, p, &igl[..room]);
            }
            other => {
                img[p..p + S].fill(0);
                put(img, p, other);
            }
        }
        p += S;
    }
}

#[test]
fn igl_past_the_end_skips_the_area() {
    let mut img = Spec::simple().build();
    // 1 TTxt, 2 TRL1, 3 TRL2, 4 TTxt (ignored), 5 SACD_IGL: 5 x 2048 + 4096 > 6 x 2048.
    chain(&mut img, &[b"SACDTTxt", b"SACDTRL1", b"SACDTRL2", b"SACDTTxt", b"SACD_IGL"]);
    assert!(matches!(Disc::open(Cursor::new(img.clone())), Err(Error::NoAudioAreas)));
    // With no tracks the extent is not checked (specification 3, 3.7.3 item 2).
    put(&mut img, A + 69, &[0]);
    let disc = open(img);
    assert_eq!(disc.toc().areas[0].tracks.len(), 0);
    assert!(!has(&disc, WarningCode::AreaTocMalformed));
}

#[test]
fn manufacturer_missing() {
    let mut img = Spec::simple().build();
    patch_master(&mut img, 9 * S, b"XXXXXXXX");
    let disc = open(img);
    assert!(has(&disc, WarningCode::ManufacturerMissing));
    assert!(disc.toc().master.manufacturer.is_none());
}

#[test]
fn oversized_area_toc_skips_only_its_area() {
    let mut img = Spec::simple().build();
    patch_master(&mut img, 72, &600u32.to_be_bytes());
    patch_master(&mut img, 86, &129u16.to_be_bytes());
    let disc = open(img);
    assert_eq!(disc.toc().areas.len(), 1);
    assert!(
        disc.warnings()
            .iter()
            .any(|w| w.code == WarningCode::AreaTocTooLarge && w.slot == Some(Slot::Multichannel))
    );
}

#[test]
fn text_strings_are_cut_at_4096_bytes() {
    let mut img = roomy();
    // Grow the area TOC to 9 sectors and put 5000 bytes of text in sectors 6 to 8 (the
    // walk stops at sector 6, whose bytes are not a signature).
    set_area_toc_sectors(&mut img, 9);
    put(&mut img, A + 6 * S, &[b'A'; 5000]);
    put(&mut img, A + 144, &((6 * S) as u16).to_be_bytes());
    let disc = open(img);
    let text = disc.toc().areas[0].text.description.as_deref().unwrap();
    assert_eq!(text.len(), 4096);
    assert!(text.bytes().all(|b| b == b'A'));
}

#[test]
fn area_text_offsets_beyond_the_first_sector() {
    let mut img = Spec::simple().build();
    // Inside the area TOC but past sector 0: valid (specification 3, 3.6).
    put(&mut img, A + S + 1500, b"Far text\0");
    put(&mut img, A + 144, &((S + 1500) as u16).to_be_bytes());
    // At E = 6 x 2048 or beyond: absent, silently.
    put(&mut img, A + 146, &((6 * S) as u16).to_be_bytes());
    let disc = open(img);
    let text = &disc.toc().areas[0].text;
    assert_eq!(text.description.as_deref(), Some("Far text"));
    assert_eq!(text.copyright, None);
}

#[test]
fn area_locales_are_capped_at_10() {
    let mut img = Spec::simple().build();
    put(&mut img, A + 80, &[12]);
    let disc = open(img);
    assert_eq!(disc.toc().areas[0].locales.len(), 10);
}

#[test]
fn last_igl_and_first_ttxt_win() {
    let mut img = roomy();
    set_area_toc_sectors(&mut img, 9);
    // 1 TTxt, 2-3 IGL, 4 TRL1, 5 TRL2 (from the builder), then 6 TTxt, 7-8 IGL.
    let ttxt2 = A + 6 * S;
    put(&mut img, ttxt2, b"SACDTTxt");
    put(&mut img, ttxt2 + 8, &600u16.to_be_bytes());
    put(&mut img, ttxt2 + 600, &[1, 0, 0, 0, 1, 0x20]);
    put(&mut img, ttxt2 + 606, b"Second\0");
    let igl2 = A + 7 * S;
    img[igl2..igl2 + 2 * S].fill(0);
    put(&mut img, igl2, b"SACD_IGL");
    put(&mut img, igl2 + 8, b"ZZLAST000001");
    put(&mut img, igl2 + 3072, &[1, 0, 0, 9]);
    let disc = open(img);
    let t = &disc.toc().areas[0].tracks[0];
    assert_eq!(t.isrc.as_deref(), Some("ZZLAST000001"));
    assert_eq!(t.genre.map(|g| g.index), Some(9));
    assert_eq!(t.text.title.as_deref(), Some("T1"), "the first SACDTTxt wins");
}

#[test]
fn walk_stops_at_an_unknown_signature() {
    let mut img = roomy();
    set_area_toc_sectors(&mut img, 9);
    // Sector 6: unknown; sector 7: a SACDTRL1 that the walk must not reach.
    put(&mut img, A + 6 * S, b"UNKNOWN!");
    let trl1: Vec<u8> = img[A + 4 * S..A + 5 * S].to_vec();
    put(&mut img, A + 7 * S, &trl1);
    put(&mut img, A + 7 * S + 8, &999u32.to_be_bytes());
    let disc = open(img);
    assert_ne!(disc.toc().areas[0].tracks[0].start_lsn, 999);
    // And an unknown signature before the track lists hides them.
    let mut img = Spec::simple().build();
    chain(&mut img, &[b"UNKNOWN!"]);
    assert!(matches!(Disc::open(Cursor::new(img)), Err(Error::NoAudioAreas)));
}

#[test]
fn unknown_text_type_warns_only_with_a_string() {
    // Track 1's text list is at SACDTTxt + 600: count, 3 reserved, then type, reserved,
    // string.
    let item = A + S + 600 + 4;
    let mut empty = Spec::simple().build();
    put(&mut empty, item, &[0x41, 0x20, 0]);
    let disc = open(empty);
    assert!(!has(&disc, WarningCode::TrackTextTypeUnknown));
    assert!(disc.toc().areas[0].tracks[0].text.is_empty());

    let mut text = Spec::simple().build();
    put(&mut text, item, &[0x41]);
    let disc = open(text);
    assert!(
        disc.warnings().iter().any(|w| w.code == WarningCode::TrackTextTypeUnknown && w.track == Some(1))
    );
}

#[test]
fn stored_language_codes_and_catalogue_numbers() {
    let mut img = Spec::simple().build();
    // Master TOC: two text channels, "e " and 0x00 0x00, both reported as language "e"
    // and "" but stored apart; catalogue numbers with padding and a NUL.
    patch_master(&mut img, 128, &[2]);
    patch_master(&mut img, 136, &[b'e', b' ', 1, 0, 0, 0, 2, 0]);
    patch_master(&mut img, 24, b" CAT\xa0 \0junk\0\0\0\0\0\0");
    patch_master(&mut img, 88, b"0123456789ABCDEF");
    put(&mut img, A + 80, &[1]);
    put(&mut img, A + 88, &[b' ', b' ', 3, 0]);
    let disc = open(img);
    let m = &disc.toc().master;
    let codes: Vec<_> = m.locales.iter().map(|l| (l.language.as_str(), l.language_code, l.charset)).collect();
    assert_eq!(codes, [("e", *b"e ", 1), ("", [0, 0], 2)]);
    assert_eq!(m.album_catalog_number, "CAT");
    assert_eq!(m.album_catalog_number_stored, b" CAT\xa0 ");
    assert_eq!(m.disc_catalog_number_stored, b"0123456789ABCDEF");
    let a = &disc.toc().areas[0].locales;
    assert_eq!((a[0].language.as_str(), a[0].language_code, a[0].charset), ("", *b"  ", 3));
}
