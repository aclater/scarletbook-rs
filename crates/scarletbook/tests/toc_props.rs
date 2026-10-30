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
        ProptestConfig { cases: 256, ..ProptestConfig::default() }
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
