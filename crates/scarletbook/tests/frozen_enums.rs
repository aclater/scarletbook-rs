// SPDX-License-Identifier: MIT OR Apache-2.0

//! The enums and structs documented as frozen stay exhaustive: no `match` here has a wildcard arm
//! and no pattern has `..`, so this file stops compiling if one of them gains a variant or a field,
//! or becomes `#[non_exhaustive]`. Changing that is a breaking change, and needs a decision first
//! (#39).

use scarletbook::Slot;
use scarletbook::audio::{FrameContent, FrameKind, Stored};
use scarletbook::log::Level;
use scarletbook::toc::{AreaKind, AreaPointer, AreaTextItem, Date, DiscTextItem, Genre, Timecode, Version};

/// Compile-time in substance, and so small enough for Miri.
#[test]
fn frozen_enums_match_without_a_wildcard() {
    let slots = Slot::ALL.map(|s| match s {
        Slot::TwoChannel | Slot::Multichannel => (),
    });
    assert_eq!(slots.len(), 2);

    match AreaKind::TwoChannel {
        AreaKind::TwoChannel | AreaKind::Multichannel => (),
    }
    match FrameKind::Dst {
        FrameKind::Dst | FrameKind::Dsd => (),
    }
    match FrameContent::Missing {
        FrameContent::Dsd(_) | FrameContent::Dst(_) | FrameContent::Missing => (),
    }
    // `Stored::Plain` may be the variant or a deprecated constant of the renamed one (#300): the
    // match is exhaustive either way, and `allow` keeps it building under `-D warnings`.
    #[allow(deprecated)]
    match Stored::Missing {
        Stored::Dst(_) | Stored::Plain | Stored::Missing => (),
    }

    assert_eq!(Level::ALL.len(), 5);
    for level in Level::ALL {
        match level {
            Level::Error | Level::Warn | Level::Info | Level::Debug | Level::Trace => (),
        }
    }

    assert_eq!(AreaTextItem::ALL.len(), 4);
    for item in AreaTextItem::ALL {
        match item {
            AreaTextItem::Description
            | AreaTextItem::Copyright
            | AreaTextItem::DescriptionPhonetic
            | AreaTextItem::CopyrightPhonetic => (),
        }
    }

    assert_eq!(DiscTextItem::ALL.len(), 16);
    for item in DiscTextItem::ALL {
        match item {
            DiscTextItem::AlbumTitle
            | DiscTextItem::AlbumArtist
            | DiscTextItem::AlbumPublisher
            | DiscTextItem::AlbumCopyright
            | DiscTextItem::AlbumTitlePhonetic
            | DiscTextItem::AlbumArtistPhonetic
            | DiscTextItem::AlbumPublisherPhonetic
            | DiscTextItem::AlbumCopyrightPhonetic
            | DiscTextItem::DiscTitle
            | DiscTextItem::DiscArtist
            | DiscTextItem::DiscPublisher
            | DiscTextItem::DiscCopyright
            | DiscTextItem::DiscTitlePhonetic
            | DiscTextItem::DiscArtistPhonetic
            | DiscTextItem::DiscPublisherPhonetic
            | DiscTextItem::DiscCopyrightPhonetic => (),
        }
    }
}

/// Built by literal and taken apart without `..`, as a caller outside the crate may do.
#[test]
fn frozen_structs_build_and_match_without_rest() {
    let Version { major, minor } = Version { major: 1, minor: 20 };
    assert_eq!((major, minor), (1, 20));
    let Genre { category, index } = Genre { category: 1, index: 2 };
    assert_eq!((category, index), (1, 2));
    let Date { year, month, day } = Date { year: 2026, month: 11, day: 6 };
    assert_eq!((year, month, day), (2026, 11, 6));
    let Timecode { minutes, seconds, frames } = Timecode { minutes: 1, seconds: 2, frames: 3 };
    assert_eq!((minutes, seconds, frames), (1, 2, 3));
    let AreaPointer { toc1_lsn, toc2_lsn, toc_sectors } =
        AreaPointer { toc1_lsn: 540, toc2_lsn: 546, toc_sectors: 6 };
    assert_eq!((toc1_lsn, toc2_lsn, toc_sectors), (540, 546, 6));
}
