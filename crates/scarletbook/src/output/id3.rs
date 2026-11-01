// SPDX-License-Identifier: MIT OR Apache-2.0

//! ID3v2.4 tags as DSF carries them (specification 8, 8.3).

use super::tags::{TagItem, TagSet};
use crate::error::Error;

/// The largest tag written, header and footer included. A synchsafe size can say up to
/// 2^28 - 1; TOC strings are at most 4096 bytes each, so this bound matters only for a
/// caller's own values.
pub const MAX_TAG_BYTES: usize = 1 << 20;

/// The text encoding of ID3 frames (specification 8, 8.3.4; W7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Id3Encoding {
    /// 0x03, UTF-8.
    Utf8,
    /// 0x00, ISO 8859-1, for each frame whose code points all allow it (8.3.4 item 2), and
    /// UTF-8 for the others.
    Latin1OrUtf8,
    /// 0x01, UTF-16 with a byte order mark before each string, little-endian.
    Utf16Le,
    /// 0x01, UTF-16 with a byte order mark before each string, big-endian.
    Utf16Be,
    /// 0x02, UTF-16BE without a byte order mark.
    Utf16BeNoBom,
}

/// How the arranger is written (8.3.5; W11).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ArrangerFrame {
    /// `TIPL`: the function `arranger`, then the name.
    Tipl,
    /// `TXXX` with [`Id3Options::arranger_description`].
    Txxx,
}

/// How the album or disc artist is written (8.3.5; W11): ID3 has no album-artist frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AlbumArtistFrame {
    /// No frame.
    None,
    /// `TPE2` (band, orchestra or accompaniment).
    Tpe2,
    /// `TXXX` with [`Id3Options::album_artist_description`].
    Txxx,
}

/// The frame of the date (8.3.5, 8.3.8; W11).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DateFrame {
    /// `TDRL`, the release time.
    Tdrl,
    /// `TDRC`, the recording time.
    Tdrc,
}

/// The ID3 writer's choices of specification 8, 8.6 (rows 4 to 12; W numbers), with their
/// defaults.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Id3Options {
    /// W4, a footer (and so no padding), as the ID3 document requires of a tag after the audio (8.3.1 item 2).
    /// Default `true`.
    pub footer: bool,
    /// W5, bytes of padding after the frames, without a footer. Default 0.
    pub padding: u32,
    /// W6, the frame order: frames are written in the order of their IDs here, and frames
    /// with the same ID in the order they are made. Default the order of the 8.3.5 table.
    /// Frames whose ID is missing are written last.
    pub frame_order: Vec<[u8; 4]>,
    /// W7, the text encoding. Default [`Id3Encoding::Utf8`].
    pub encoding: Id3Encoding,
    /// W8, whether the last string of a text frame, a `TXXX` value and a `COMM` text end
    /// with a terminator. Default `false`.
    pub terminate_last: bool,
    /// W9, what replaces a control character (8.3.4 item 1); `None` removes it. Default
    /// `None`.
    pub control_replacement: Option<char>,
    /// W10, a separator at which every value is split into several values. Default `None`.
    pub split: Option<String>,
    /// W11, the arranger's frame. Default [`ArrangerFrame::Tipl`].
    pub arranger: ArrangerFrame,
    /// W11 and W12, `None` for one `COMM` frame per message, or the separator that joins
    /// the message and the extra message in one. Default `None`.
    pub join_messages: Option<String>,
    /// W11, whether the phonetic title and performer are written (`TSOT`, `TSOP`).
    /// Default `false`.
    pub phonetic: bool,
    /// W11, whether `TRCK` has `/count`. Default `true`.
    pub track_count: bool,
    /// W11, the album or disc artist's frame. Default [`AlbumArtistFrame::None`].
    pub album_artist: AlbumArtistFrame,
    /// W11, whether the copyright is written, as `TCOP` with the year of the date and a
    /// space before it, and only when the date has a year (8.3.5). Default `true`.
    pub copyright: bool,
    /// W11, whether a sequence number outside `1..=set size` is written alone in `TPOS`.
    /// Default `false`.
    pub album_set_alone: bool,
    /// W11, the date's frame. Default [`DateFrame::Tdrl`].
    pub date: DateFrame,
    /// W12, the `COMM` language: three lower-case ISO 639-2 letters, or `XXX`. Default
    /// `XXX`.
    pub language: [u8; 3],
    /// W12, the description of the message's `COMM`. Default empty.
    pub message_description: String,
    /// W12, the description of the extra message's `COMM`. Default `extra message`.
    pub extra_message_description: String,
    /// W12, the `TXXX` description of the arranger. Default `arranger`.
    pub arranger_description: String,
    /// W12, the `TXXX` description of the album artist. Default `album artist`.
    pub album_artist_description: String,
}

/// The frame IDs in the order of the table of 8.3.5.
pub const TABLE_ORDER: [[u8; 4]; 18] = [
    *b"TIT2", *b"TPE1", *b"TEXT", *b"TCOM", *b"TIPL", *b"COMM", *b"TSOT", *b"TSOP", *b"TRCK", *b"TSRC",
    *b"TCON", *b"TALB", *b"TPE2", *b"TXXX", *b"TPUB", *b"TCOP", *b"TPOS", *b"TDRL",
];

impl Default for Id3Options {
    fn default() -> Self {
        let mut order = TABLE_ORDER.to_vec();
        order.push(*b"TDRC");
        Id3Options {
            footer: true,
            padding: 0,
            frame_order: order,
            encoding: Id3Encoding::Utf8,
            terminate_last: false,
            control_replacement: None,
            split: None,
            arranger: ArrangerFrame::Tipl,
            join_messages: None,
            phonetic: false,
            track_count: true,
            album_artist: AlbumArtistFrame::None,
            copyright: true,
            album_set_alone: false,
            date: DateFrame::Tdrl,
            language: *b"XXX",
            message_description: String::new(),
            extra_message_description: "extra message".to_owned(),
            arranger_description: "arranger".to_owned(),
            album_artist_description: "album artist".to_owned(),
        }
    }
}

/// Whether `c` is a control character (8.3.4 item 1).
fn control(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{1F}' | '\u{7F}'..='\u{9F}')
}

/// A string with its control characters removed or replaced (8.3.4 item 1); line breaks
/// stay where `newlines` allows them.
fn clean(s: &str, newlines: bool, o: &Id3Options) -> String {
    s.chars()
        .filter_map(|c| match c {
            '\n' if newlines => Some(c),
            c if control(c) => o.control_replacement.filter(|r| !control(*r) || (*r == '\n' && newlines)),
            c => Some(c),
        })
        .collect()
}

/// The strings of a frame body, encoded with one encoding (8.3.4): the encoding byte, then
/// each string, separated by terminators, the last one terminated when `last` says.
fn text_body(prefix: &[u8], strings: &[String], last: bool, o: &Id3Options) -> Vec<u8> {
    // `prefix` goes after the encoding byte (the `COMM` language).
    let latin1 = strings.iter().all(|s| s.chars().all(|c| c == '\n' || ('\u{20}'..='\u{FF}').contains(&c)));
    let enc = match o.encoding {
        Id3Encoding::Utf8 => 3,
        Id3Encoding::Latin1OrUtf8 if latin1 => 0,
        Id3Encoding::Latin1OrUtf8 => 3,
        Id3Encoding::Utf16Le | Id3Encoding::Utf16Be => 1,
        Id3Encoding::Utf16BeNoBom => 2,
    };
    let mut out = vec![enc];
    out.extend_from_slice(prefix);
    for (i, s) in strings.iter().enumerate() {
        match o.encoding {
            Id3Encoding::Utf16Le => {
                out.extend(std::iter::once(0xFEFF).chain(s.encode_utf16()).flat_map(u16::to_le_bytes))
            }
            Id3Encoding::Utf16Be => {
                out.extend(std::iter::once(0xFEFF).chain(s.encode_utf16()).flat_map(u16::to_be_bytes))
            }
            Id3Encoding::Utf16BeNoBom => out.extend(s.encode_utf16().flat_map(u16::to_be_bytes)),
            // Every code point is below 0x100 (checked above).
            _ if enc == 0 => out.extend(s.chars().map(|c| c as u8)),
            _ => out.extend_from_slice(s.as_bytes()),
        }
        if i + 1 < strings.len() || last {
            out.extend_from_slice(if enc == 1 || enc == 2 { &[0, 0] } else { &[0] });
        }
    }
    out
}

/// A synchsafe integer (8.3.1); `v` is below 2^28.
fn synchsafe(v: usize) -> [u8; 4] {
    [(v >> 21) as u8 & 0x7F, (v >> 14) as u8 & 0x7F, (v >> 7) as u8 & 0x7F, v as u8 & 0x7F]
}

/// The tag of `set` (8.3), or `None` when it has no frame (8.2.6 item 3).
///
/// # Errors
///
/// [`Error::InvalidArgument`] if the tag would exceed [`MAX_TAG_BYTES`] or the language
/// is not three ASCII letters.
pub(super) fn tag(set: &TagSet, o: &Id3Options) -> Result<Option<Vec<u8>>, Error> {
    if !(o.language.iter().all(u8::is_ascii_lowercase) || o.language == *b"XXX") {
        return Err(Error::InvalidArgument("the COMM language is three lower-case letters or XXX"));
    }
    let values = |item| -> Vec<String> {
        let v = set.get(item).iter().map(|s| clean(s, false, o));
        match &o.split {
            Some(sep) if !sep.is_empty() => {
                v.flat_map(|s| s.split(sep.as_str()).map(str::to_owned).collect::<Vec<_>>()).collect()
            }
            _ => v.collect(),
        }
    };
    let digits = |s: &String| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let mut frames: Vec<([u8; 4], Vec<u8>, Option<String>)> = Vec::new();
    let text = |frames: &mut Vec<([u8; 4], Vec<u8>, Option<String>)>, id: &[u8; 4], strings: Vec<String>| {
        if !strings.is_empty() {
            frames.push((*id, text_body(&[], &strings, o.terminate_last, o), None));
        }
    };
    let txxx = |frames: &mut Vec<([u8; 4], Vec<u8>, Option<String>)>, description: &str, value: &str| {
        let strings = [clean(description, false, o), clean(value, false, o)];
        let key = Some(format!("TXXX {}", strings[0]));
        frames.push((*b"TXXX", text_body(&[], &strings, o.terminate_last, o), key));
    };
    // A `TXXX` holds one value (8.3.6).
    let one_txxx = |frames: &mut Vec<([u8; 4], Vec<u8>, Option<String>)>,
                    description: &str,
                    values: &[String]| match values {
        [] => Ok(()),
        [v] => {
            txxx(frames, description, v);
            Ok(())
        }
        _ => Err(Error::InvalidArgument("a TXXX item holds one value")),
    };
    let comm = |frames: &mut Vec<([u8; 4], Vec<u8>, Option<String>)>, description: &str, value: &str| {
        let strings = [clean(description, false, o), clean(value, true, o)];
        let key = Some(format!("COMM {}", strings[0]));
        frames.push((*b"COMM", text_body(&o.language, &strings, o.terminate_last, o), key));
    };
    text(&mut frames, b"TIT2", values(TagItem::TrackTitle));
    text(&mut frames, b"TPE1", values(TagItem::Performer));
    text(&mut frames, b"TEXT", values(TagItem::Songwriter));
    text(&mut frames, b"TCOM", values(TagItem::Composer));
    let arrangers = values(TagItem::Arranger);
    match o.arranger {
        // One frame of function and name pairs (8.3.3 item 3).
        ArrangerFrame::Tipl => text(
            &mut frames,
            b"TIPL",
            arrangers.into_iter().flat_map(|a| ["arranger".to_owned(), a]).collect(),
        ),
        ArrangerFrame::Txxx => one_txxx(&mut frames, &o.arranger_description, &arrangers)?,
    }
    let message = set.get(TagItem::Message).join("\n");
    let extra = set.get(TagItem::ExtraMessage).join("\n");
    match &o.join_messages {
        Some(sep) if !message.is_empty() && !extra.is_empty() => {
            comm(&mut frames, &o.message_description, &format!("{message}{sep}{extra}"))
        }
        _ => {
            if !set.get(TagItem::Message).is_empty() {
                comm(&mut frames, &o.message_description, &message);
            }
            if !set.get(TagItem::ExtraMessage).is_empty() {
                let d = if o.join_messages.is_some() {
                    &o.message_description
                } else {
                    &o.extra_message_description
                };
                comm(&mut frames, d, &extra);
            }
        }
    }
    if o.phonetic {
        text(&mut frames, b"TSOT", values(TagItem::TitlePhonetic));
        text(&mut frames, b"TSOP", values(TagItem::PerformerPhonetic));
    }
    if let Some(n) = set.get(TagItem::TrackNumber).first().filter(|n| digits(n)) {
        let count = set.get(TagItem::TrackCount).first().filter(|c| o.track_count && digits(c));
        text(&mut frames, b"TRCK", vec![count.map_or(n.clone(), |c| format!("{n}/{c}"))]);
    }
    text(&mut frames, b"TSRC", values(TagItem::Isrc));
    text(&mut frames, b"TCON", values(TagItem::Genre));
    text(&mut frames, b"TALB", values(TagItem::Album));
    let artists = values(TagItem::AlbumArtist);
    match o.album_artist {
        AlbumArtistFrame::None => {}
        AlbumArtistFrame::Tpe2 => text(&mut frames, b"TPE2", artists),
        AlbumArtistFrame::Txxx => one_txxx(&mut frames, &o.album_artist_description, &artists)?,
    }
    for (d, v) in set.custom() {
        txxx(&mut frames, d, v);
    }
    text(&mut frames, b"TPUB", values(TagItem::Publisher));
    let date = set.get(TagItem::Date).first();
    let year =
        date.and_then(|d| d.get(..4)).filter(|y| y.bytes().all(|b| b.is_ascii_digit()) && *y != "0000");
    if let (true, Some(year)) = (o.copyright, year) {
        let v: Vec<String> = values(TagItem::Copyright).into_iter().map(|c| format!("{year} {c}")).collect();
        text(&mut frames, b"TCOP", v);
    }
    if let Some(q) = set.get(TagItem::DiscNumber).first().filter(|q| digits(q)) {
        let z = set.get(TagItem::DiscCount).first().filter(|z| digits(z));
        text(&mut frames, b"TPOS", vec![z.map_or(q.clone(), |z| format!("{q}/{z}"))]);
    }
    let date_id = match o.date {
        DateFrame::Tdrl => b"TDRL",
        DateFrame::Tdrc => b"TDRC",
    };
    text(&mut frames, date_id, date.map(|d| vec![clean(d, false, o)]).unwrap_or_default());

    if frames.is_empty() {
        return Ok(None);
    }
    // At most one `TXXX` per description and one `COMM` per language and description
    // (8.3.3 item 3, 8.3.6).
    let mut keys: Vec<&String> = frames.iter().filter_map(|(_, _, k)| k.as_ref()).collect();
    let n = keys.len();
    keys.sort_unstable();
    keys.dedup();
    if keys.len() != n {
        return Err(Error::InvalidArgument("two TXXX or COMM frames have one description"));
    }
    // W6: stable, so frames with one ID keep the order they were made in.
    frames.sort_by_key(|(id, _, _)| o.frame_order.iter().position(|x| x == id).unwrap_or(usize::MAX));
    let mut body = Vec::new();
    for (id, b, _) in &frames {
        if body.len() + 10 + b.len() > MAX_TAG_BYTES {
            return Err(Error::InvalidArgument("the tag is larger than MAX_TAG_BYTES"));
        }
        body.extend_from_slice(id);
        body.extend_from_slice(&synchsafe(b.len()));
        body.extend_from_slice(&[0, 0]);
        body.extend_from_slice(b);
    }
    if !o.footer {
        let padding = o.padding as usize;
        if body.len() + padding + 20 > MAX_TAG_BYTES {
            return Err(Error::InvalidArgument("the tag is larger than MAX_TAG_BYTES"));
        }
        body.resize(body.len() + padding, 0);
    }
    let flags = if o.footer { 0x10 } else { 0 };
    let header = |id: &[u8; 3]| {
        let mut h = id.to_vec();
        h.extend_from_slice(&[4, 0, flags]);
        h.extend_from_slice(&synchsafe(body.len()));
        h
    };
    let mut out = header(b"ID3");
    out.extend_from_slice(&body);
    if o.footer {
        out.extend_from_slice(&header(b"3DI"));
    }
    Ok(Some(out))
}

/// Checks a caller's tag (W3): an ID3v2.4 header whose synchsafe size, with the footer its
/// flags announce, is the tag's length, at most [`MAX_TAG_BYTES`] (8.3.1).
pub(super) fn check_raw(tag: &[u8]) -> Result<(), Error> {
    let bad = Err(Error::InvalidArgument("not an ID3v2.4 tag of its stated size"));
    if tag.len() < 10
        || tag.len() > MAX_TAG_BYTES
        || &tag[..5] != b"ID3\x04\x00"
        || tag[6..10].iter().any(|b| b & 0x80 != 0)
    {
        return bad;
    }
    let size = tag[6..10].iter().fold(0usize, |v, &b| v << 7 | usize::from(b));
    let footer = if tag[5] & 0x10 != 0 { 10 } else { 0 };
    if 10 + size + footer != tag.len() {
        return bad;
    }
    Ok(())
}
