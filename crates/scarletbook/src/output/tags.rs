// SPDX-License-Identifier: MIT OR Apache-2.0

//! Tag items from the TOC (specification 8, 8.3.5 and 8.3.7 to 8.3.8), neutral to the
//! format that carries them, with the caller's overrides.

use super::OutputOptions;
use crate::toc::{Area, Date, Genre, Toc, Track};

/// A tag item (specification 8, 8.3.5): what a file's tag or text says, before a format
/// maps it to its frames or chunks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TagItem {
    /// Track title (3.8, type 0x01).
    TrackTitle,
    /// Track performer (0x02).
    Performer,
    /// Songwriter (0x03).
    Songwriter,
    /// Composer (0x04).
    Composer,
    /// Arranger (0x05).
    Arranger,
    /// Message (0x06).
    Message,
    /// Extra message (0x07).
    ExtraMessage,
    /// Track title, phonetic (0x81).
    TitlePhonetic,
    /// Track performer, phonetic (0x82).
    PerformerPhonetic,
    /// The source track's number, as decimal digits (3.9.1).
    TrackNumber,
    /// The area's track count, as decimal digits.
    TrackCount,
    /// The ISRC (3.7.3).
    Isrc,
    /// Genre names (8.3.7), each once, in the order track, album, disc.
    Genre,
    /// The album or disc title (2.5.4), as [`OutputOptions::text_source`] chooses.
    Album,
    /// The album or disc artist (2.5.4), likewise.
    AlbumArtist,
    /// The album or disc publisher (2.5.4), likewise.
    Publisher,
    /// The album or disc copyright (2.5.4), likewise.
    Copyright,
    /// The album sequence number (2.3.1), as decimal digits.
    DiscNumber,
    /// The album set size (2.3.1), as decimal digits.
    DiscCount,
    /// The date (2.3.2) as 8.3.8 gives it: `yyyy-MM-dd`, `yyyy-MM` or `yyyy`.
    Date,
}

/// Which of the album and disc text items (specification 2, 2.5.4) the album, album artist,
/// publisher and copyright items take (8.3.5 "Choice"; W11 and W24).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TextSource {
    /// The disc's.
    Disc,
    /// The album's.
    Album,
    /// The disc's, or the album's when the disc's is absent.
    DiscThenAlbum,
}

/// Which genre entries the genre item takes (8.3.5, 8.3.7; W11): in the order track,
/// album, disc.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct GenreSources {
    /// The source track's genre (3.7.3).
    pub track: bool,
    /// The album genres (2.3, offset 40).
    pub album: bool,
    /// The disc genres (2.3, offset 104).
    pub disc: bool,
}

impl Default for GenreSources {
    fn default() -> Self {
        GenreSources { track: true, album: true, disc: true }
    }
}

/// The tag items of a file, each with one or more values, and user-defined items with a
/// description of their own. [`TagSet::from_toc`] fills one from the TOC; the caller may
/// change it and pass it in [`super::Tags::Set`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct TagSet {
    items: Vec<(TagItem, Vec<String>)>,
    custom: Vec<(String, String)>,
}

/// The name of a genre entry (8.3.7 items 1 and 2): table 1, indexes 2 to 29.
fn genre_name(g: Genre) -> Option<&'static str> {
    if g.category == 1 && (2..=29).contains(&g.index) { g.name() } else { None }
}

/// The days of `month` in `year` of the Gregorian calendar (8.3.8 item 2).
fn days(year: u16, month: u8) -> u8 {
    match month {
        2 if year % 4 == 0 && year % 100 != 0 || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// The ID3 timestamp of a date (8.3.8), or `None`.
fn timestamp(d: Date) -> Option<String> {
    if d.year == 0 || d.year > 9999 {
        return None;
    }
    Some(if (1..=12).contains(&d.month) && (1..=days(d.year, d.month)).contains(&d.day) {
        format!("{:04}-{:02}-{:02}", d.year, d.month, d.day)
    } else if (1..=12).contains(&d.month) {
        format!("{:04}-{:02}", d.year, d.month)
    } else {
        format!("{:04}", d.year)
    })
}

impl TagSet {
    /// An empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// The items of a file of `area` with source track `source` (specification 8, 8.1 item
    /// 7), from the reported TOC (8.1 item 6): the per-track items only with a source
    /// track, the album and disc items as `options` choose (W11), absent items left out.
    pub fn from_toc(toc: &Toc, area: &Area, source: Option<&Track>, options: &OutputOptions) -> TagSet {
        use crate::toc::{DiscTextItem as D, TrackTextItem as T};
        let mut set = TagSet::new();
        if let Some(t) = source {
            for (item, from) in [
                (TagItem::TrackTitle, T::Title),
                (TagItem::Performer, T::Performer),
                (TagItem::Songwriter, T::Songwriter),
                (TagItem::Composer, T::Composer),
                (TagItem::Arranger, T::Arranger),
                (TagItem::Message, T::Message),
                (TagItem::ExtraMessage, T::ExtraMessage),
                (TagItem::TitlePhonetic, T::TitlePhonetic),
                (TagItem::PerformerPhonetic, T::PerformerPhonetic),
            ] {
                if let Some(v) = t.text.get(from) {
                    set.set(item, v);
                }
            }
            set.set(TagItem::TrackNumber, &t.number.to_string());
            set.set(TagItem::TrackCount, &area.tracks.len().to_string());
            if let Some(isrc) = &t.isrc {
                set.set(TagItem::Isrc, isrc);
            }
        }
        let g = options.genres;
        let track_genre = source.and_then(|t| t.genre).filter(|_| g.track);
        let album = toc.master.album_genres.iter().filter(|_| g.album);
        let disc = toc.master.disc_genres.iter().filter(|_| g.disc);
        for name in track_genre.iter().chain(album).chain(disc).filter_map(|&g| genre_name(g)) {
            if !set.get(TagItem::Genre).iter().any(|n| n == name) {
                set.push(TagItem::Genre, name);
            }
        }
        let text = &toc.master.text;
        for (item, disc, album) in [
            (TagItem::Album, D::DiscTitle, D::AlbumTitle),
            (TagItem::AlbumArtist, D::DiscArtist, D::AlbumArtist),
            (TagItem::Publisher, D::DiscPublisher, D::AlbumPublisher),
            (TagItem::Copyright, D::DiscCopyright, D::AlbumCopyright),
        ] {
            let v = match options.text_source {
                TextSource::Disc => text.get(disc),
                TextSource::Album => text.get(album),
                TextSource::DiscThenAlbum => text.get(disc).or(text.get(album)),
            };
            if let Some(v) = v {
                set.set(item, v);
            }
        }
        let (q, z) = (toc.master.album_sequence_number, toc.master.album_set_size);
        if (1..=z).contains(&q) {
            set.set(TagItem::DiscNumber, &q.to_string());
            set.set(TagItem::DiscCount, &z.to_string());
        } else if q >= 1 && options.id3.album_set_alone {
            set.set(TagItem::DiscNumber, &q.to_string());
        }
        if let Some(d) = timestamp(toc.master.date) {
            set.set(TagItem::Date, &d);
        }
        set
    }

    /// The values of `item`, in order; empty if it is absent.
    pub fn get(&self, item: TagItem) -> &[String] {
        self.items.iter().find(|(i, _)| *i == item).map_or(&[], |(_, v)| v)
    }

    /// Sets `item` to the single value `value`.
    pub fn set(&mut self, item: TagItem, value: &str) {
        self.remove(item);
        self.items.push((item, vec![value.to_owned()]));
    }

    /// Adds `value` as a further value of `item`.
    pub fn push(&mut self, item: TagItem, value: &str) {
        match self.items.iter_mut().find(|(i, _)| *i == item) {
            Some((_, v)) => v.push(value.to_owned()),
            None => self.items.push((item, vec![value.to_owned()])),
        }
    }

    /// Removes `item`.
    pub fn remove(&mut self, item: TagItem) {
        self.items.retain(|(i, _)| *i != item);
    }

    /// Sets the user-defined item `description` to `value` (an ID3 `TXXX` frame, 8.3.6).
    pub fn set_custom(&mut self, description: &str, value: &str) {
        self.custom.retain(|(d, _)| d != description);
        self.custom.push((description.to_owned(), value.to_owned()));
    }

    /// The user-defined items, in order.
    pub fn custom(&self) -> &[(String, String)] {
        &self.custom
    }

    /// Whether it has no item.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty() && self.custom.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::timestamp;
    use crate::toc::Date;

    /// The rules of specification 8, 8.3.8.
    #[test]
    fn timestamps() {
        let ts = |year, month, day| timestamp(Date { year, month, day });
        assert_eq!(ts(2004, 2, 29).as_deref(), Some("2004-02-29"));
        assert_eq!(ts(2000, 2, 29).as_deref(), Some("2000-02-29"));
        assert_eq!(ts(1900, 2, 29).as_deref(), Some("1900-02"));
        assert_eq!(ts(2001, 4, 31).as_deref(), Some("2001-04"));
        assert_eq!(ts(2001, 12, 31).as_deref(), Some("2001-12-31"));
        assert_eq!(ts(2001, 13, 1).as_deref(), Some("2001"));
        assert_eq!(ts(2001, 0, 0).as_deref(), Some("2001"));
        assert_eq!(ts(5, 1, 0).as_deref(), Some("0005-01"));
        assert_eq!(ts(0, 1, 1), None);
        assert_eq!(ts(10_000, 1, 1), None);
    }
}
