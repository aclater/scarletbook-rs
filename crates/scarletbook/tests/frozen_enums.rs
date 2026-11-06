// SPDX-License-Identifier: MIT OR Apache-2.0

//! The enums documented as frozen stay exhaustive: no `match` here has a wildcard arm, so this
//! file stops compiling if one of them gains a variant or becomes `#[non_exhaustive]`. Changing
//! that is a breaking change, and needs a decision first (#39).

use scarletbook::Slot;
use scarletbook::audio::{FrameContent, FrameKind, Stored};
use scarletbook::log::Level;
use scarletbook::text::Charset;
use scarletbook::toc::{AreaKind, AreaTextItem, DiscTextItem};

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
    match Stored::Missing {
        Stored::Dst(_) | Stored::Plain | Stored::Missing => (),
    }
    match Charset::from_code(0) {
        Charset::Latin1 | Charset::ShiftJisX0213 | Charset::EucKr | Charset::EucCn | Charset::Big5 => (),
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
