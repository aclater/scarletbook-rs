// SPDX-License-Identifier: MIT OR Apache-2.0

//! Property tests of the TOC parser and text decoding.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;
mod harness;

use std::io::Cursor;

use proptest::prelude::*;
use scarletbook::text::{self, Charset};
use scarletbook::{Disc, Error};

fn config() -> ProptestConfig {
    if cfg!(miri) {
        // Miri's isolation forbids the file system access of failure persistence.
        ProptestConfig { cases: 4, failure_persistence: None, ..ProptestConfig::default() }
    } else {
        // PROPTEST_CASES, if set, overrides the default number of cases.
        let cases = std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(256);
        ProptestConfig { cases, ..ProptestConfig::default() }
    }
}

proptest! {
    #![proptest_config(config())]

    /// Any bytes after the master TOC signature: never a panic, and the TOC invariants
    /// hold whenever the image is accepted.
    #[test]
    #[cfg_attr(miri, ignore = "slow under Miri")]
    fn arbitrary_toc_bytes(mut data in proptest::collection::vec(any::<u8>(), 0..40_000)) {
        if data.len() >= 8 {
            data[..8].copy_from_slice(b"SACDMTOC");
        }
        harness::toc(&data);
    }

    /// A valid synthetic disc with random bytes overwritten in its TOC sectors.
    #[test]
    #[cfg_attr(miri, ignore = "slow under Miri")]
    fn mutated_synthetic_toc(edits in proptest::collection::vec((510usize * 2048..546 * 2048, any::<u8>()), 1..64)) {
        let mut img = common::Spec::simple().build();
        for (at, b) in edits {
            img[at] = b;
        }
        match Disc::open(Cursor::new(img)) {
            Ok(disc) => harness::check_toc(&disc),
            Err(e) => prop_assert!(e.code().is_some(), "untyped rejection {e}"),
        }
    }

    /// Latin-1 decoding is one code point per byte.
    #[test]
    fn latin1_is_one_to_one(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        prop_assert_eq!(text::decode(&bytes, Charset::Latin1).chars().count(), bytes.len());
    }

    /// Multi-byte decoding never yields more code points than bytes, and text strings
    /// never contain control characters.
    #[test]
    #[cfg_attr(miri, ignore = "parses large tables")]
    fn cjk_decoding_is_bounded(code in 3u8..=6, bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let cs = Charset::from_code(code);
        prop_assert!(text::decode(&bytes, cs).chars().count() <= bytes.len());
        if let Some(s) = text::decode_string(&bytes, cs) {
            prop_assert!(s.chars().all(|c| c >= ' '));
        }
    }

    /// Fixed text fields never start or end with whitespace of the set W.
    #[test]
    fn fixed_text_is_trimmed(bytes in proptest::collection::vec(any::<u8>(), 0..16)) {
        let s = text::fixed_text(&bytes);
        let w = |c: char| matches!(c, '\u{9}'..='\u{d}' | '\u{1c}'..='\u{20}' | '\u{85}' | '\u{a0}');
        prop_assert!(!s.starts_with(w) && !s.ends_with(w));
        prop_assert!(!s.contains('\0'));
    }
}

#[test]
fn synthetic_disc_parses() {
    let disc = Disc::open(Cursor::new(common::Spec::simple().build())).unwrap();
    harness::check_toc(&disc);
    let area = &disc.toc().areas[0];
    assert_eq!(area.tracks.len(), 2);
    assert_eq!(area.tracks[1].text.title.as_deref(), Some("T2"));
    assert_eq!(disc.toc().master.text.album_title.as_deref(), Some("Synthetic"));
    assert!(disc.warnings().is_empty(), "{:?}", disc.warnings());
}

#[test]
fn rejections_are_typed() {
    assert!(matches!(Disc::open(Cursor::new(vec![0u8; 100])), Err(Error::NotScarletbook)));
    let mut img = common::Spec::simple().build();
    img[510 * 2048 + 8] = 2;
    assert!(matches!(
        Disc::open(Cursor::new(img.clone())),
        Err(Error::MasterTocVersionUnsupported { major: 2, minor: 20 })
    ));
    img.truncate(515 * 2048);
    assert!(matches!(Disc::open(Cursor::new(img)), Err(Error::MasterTocTruncated)));
}

fn copies_warning(img: Vec<u8>) -> usize {
    let disc = Disc::open(Cursor::new(img)).unwrap();
    disc.warnings().iter().filter(|w| w.code == scarletbook::WarningCode::MasterTocCopiesDiffer).count()
}

/// Specification 1, 1.5.1 item 3: every byte of a readable copy is compared, the warning
/// is reported at most once, and an unreadable copy is not compared.
#[test]
fn master_toc_copies() {
    let base = common::Spec::simple().build();
    assert_eq!(copies_warning(base.clone()), 0);
    // A reserved byte of copy 2, and a byte of an undecoded text channel of copy 3.
    let mut one = base.clone();
    one[520 * 2048 + 12] ^= 1;
    assert_eq!(copies_warning(one.clone()), 1);
    one[530 * 2048 + 4 * 2048 + 100] ^= 1;
    assert_eq!(copies_warning(one), 1, "reported once even when both copies differ");
    let mut manufacturer = base.clone();
    manufacturer[529 * 2048 + 2047] ^= 0xFF;
    assert_eq!(copies_warning(manufacturer), 1);
    // LSN 510 itself is never replaced by a copy.
    let mut master = base.clone();
    master[510 * 2048 + 24..510 * 2048 + 29].copy_from_slice(b"OTHER");
    let disc = Disc::open(Cursor::new(master)).unwrap();
    assert_eq!(disc.toc().master.album_catalog_number, "OTHER");
}

/// A copy that cannot be read in full is not compared (specification 1, 1.5.1 item 3).
#[test]
fn unreadable_master_toc_copy_is_not_compared() {
    const S: usize = 2048;
    let mut img = common::Spec::simple().build();
    // Move the area TOC below the master TOC (it may lie anywhere, 1.5.2 item 5), and
    // point every master TOC copy at it.
    let toc: Vec<u8> = img[540 * S..546 * S].to_vec();
    img[100 * S..106 * S].copy_from_slice(&toc);
    for copy in [510, 520, 530] {
        img[copy * S + 64..copy * S + 68].copy_from_slice(&100u32.to_be_bytes());
    }
    // Copy 3 starts at LSN 530: make the part that survives differ, then end the image
    // inside it, so that its 10-sector read fails.
    img[530 * S + 12] ^= 1;
    img.truncate(535 * S);
    let disc = Disc::open(Cursor::new(img.clone())).unwrap();
    assert_eq!(disc.toc().areas.len(), 1);
    let w =
        disc.warnings().iter().filter(|w| w.code == scarletbook::WarningCode::MasterTocCopiesDiffer).count();
    assert_eq!(w, 0, "an unreadable copy is not compared");
    // The same difference in copy 2, which is readable, is reported.
    img[520 * S + 12] ^= 1;
    assert_eq!(copies_warning(img), 1);
}

/// A reader that fails with an I/O error on any read touching LSN 520 to 539.
struct FailingCopies(Cursor<Vec<u8>>);

impl std::io::Read for FailingCopies {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let pos = self.0.position();
        let end = pos + buf.len() as u64;
        if pos < 540 * 2048 && end > 520 * 2048 {
            return Err(std::io::Error::other("injected failure"));
        }
        self.0.read(buf)
    }
}

impl std::io::Seek for FailingCopies {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.0.seek(pos)
    }
}

/// Specification 1, 1.5.1 items 2 and 3: an I/O error on a copy is a failed read of an
/// optional comparison; it neither rejects the image nor warns.
#[test]
fn io_error_on_a_master_toc_copy_changes_nothing() {
    let mut img = common::Spec::simple().build();
    img[520 * 2048 + 12] ^= 1; // would differ, if it could be read
    let disc = Disc::open(FailingCopies(Cursor::new(img))).expect("LSN 510 to 519 decide the outcome");
    assert!(disc.warnings().iter().all(|w| w.code != scarletbook::WarningCode::MasterTocCopiesDiffer));
    assert_eq!(disc.toc().areas.len(), 1);
}
