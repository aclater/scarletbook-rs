// SPDX-License-Identifier: MIT OR Apache-2.0

//! DSDIFF text (specification 8, 8.4.8 and 8.4.9): the artist and title of `DIIN`, the
//! comments of `COMT`, and reported strings written as text bytes.

use super::OutputOptions;
use super::dsdiff::{DsdiffDeparture, chunk};
use crate::error::Error;

/// The kind of a DSDIFF comment (specification 8, 8.4.8), with its reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CommentKind {
    /// General (type 0); its reference is 0.
    General,
    /// About a channel (type 1): 0 for all channels, or a channel number from 1 to `C`.
    Channel(u16),
    /// About the sound source (type 2): 0 DSD, 1 analogue, 2 PCM.
    SoundSource(u16),
    /// File history (type 3): 0 general remark, 1 operator, 2 machine, 3 time zone, 4
    /// revision.
    FileHistory(u16),
}

/// A DSDIFF comment (specification 8, 8.4.8; W25).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Comment {
    /// Year, 0 to 65 535.
    pub year: u16,
    /// Month, 0 to 12.
    pub month: u8,
    /// Day, 0 to 31.
    pub day: u8,
    /// Hour, 0 to 23.
    pub hour: u8,
    /// Minutes, 0 to 59.
    pub minutes: u8,
    /// The kind and reference.
    pub kind: CommentKind,
    /// The text, written as text bytes (8.4.9), or as ISO 8859-1 with departure D2 (8.7).
    pub text: String,
}

impl Comment {
    /// A comment of `kind` with `text` and the time stamp 0 (the year counting comments
    /// from 0, as 8.4.8 describes for a device without a clock).
    pub fn new(kind: CommentKind, text: &str) -> Self {
        Comment { year: 0, month: 0, day: 0, hour: 0, minutes: 0, kind, text: text.to_owned() }
    }
}

/// A reported string as text bytes (8.4.9; W26): U+0020 to U+007E as themselves, any other
/// code point as the replacement, or removed. With departure D2 (8.7, at sacd-spec
/// `e069569`), U+0020 to U+007E and U+00A0 to U+00FF as themselves and any other code point
/// removed, whatever the replacement.
pub(super) fn text_bytes(s: &str, options: &OutputOptions) -> Vec<u8> {
    if options.departs(DsdiffDeparture::D2) {
        return s.chars().filter(|&c| latin1(c)).map(|c| c as u8).collect();
    }
    let replacement = options.dsdiff_text_replacement;
    s.chars().filter_map(|c| if (' '..='~').contains(&c) { Some(c as u8) } else { replacement }).collect()
}

/// Whether D2 writes `c` (8.7): U+0020 to U+007E or U+00A0 to U+00FF.
fn latin1(c: char) -> bool {
    (' '..='~').contains(&c) || ('\u{A0}'..='\u{FF}').contains(&c)
}

/// Appends an artist or title chunk (`DIAR`, `DITI`; 8.4.8): `k`, the text, a pad byte when
/// `k` is odd, counted in `D` as W22 says. An empty text is left out unless W31 writes it.
pub(super) fn diin_text(out: &mut Vec<u8>, id: &[u8; 4], text: Option<&str>, options: &OutputOptions) {
    let Some(text) = text else { return };
    let t = text_bytes(text, options);
    if t.is_empty() && !options.dsdiff_empty_text {
        return;
    }
    let mut d = (t.len() as u32).to_be_bytes().to_vec();
    d.extend_from_slice(&t);
    if options.dsdiff_text_pad_counted && t.len() % 2 == 1 {
        d.push(0);
    }
    chunk(out, id, &d);
}

/// The `COMT` chunk of the options' comments (8.4.8), or `None` when none is written.
///
/// # Errors
///
/// [`Error::InvalidArgument`] if a field is out of its range, or there are more than
/// 65 535 comments.
pub(super) fn comt(channels: u8, options: &OutputOptions) -> Result<Option<Vec<u8>>, Error> {
    let mut data = vec![0, 0];
    let mut count = 0u16;
    for c in &options.dsdiff_comments {
        if c.month > 12 || c.day > 31 || c.hour > 23 || c.minutes > 59 {
            return Err(Error::InvalidArgument("a comment's time stamp is out of range"));
        }
        let (kind, reference) = match c.kind {
            CommentKind::General => (0u16, 0u16),
            CommentKind::Channel(r) if r <= u16::from(channels) => (1, r),
            CommentKind::SoundSource(r) if r <= 2 => (2, r),
            CommentKind::FileHistory(r) if r <= 4 => (3, r),
            _ => return Err(Error::InvalidArgument("a comment's reference is out of range")),
        };
        let t = text_bytes(&c.text, options);
        if t.is_empty() && !options.dsdiff_empty_text {
            continue;
        }
        count = count.checked_add(1).ok_or(Error::InvalidArgument("at most 65 535 comments"))?;
        data.extend_from_slice(&c.year.to_be_bytes());
        data.extend_from_slice(&[c.month, c.day, c.hour, c.minutes]);
        data.extend_from_slice(&kind.to_be_bytes());
        data.extend_from_slice(&reference.to_be_bytes());
        data.extend_from_slice(&(t.len() as u32).to_be_bytes());
        data.extend_from_slice(&t);
        // Counted in the comment, not in `k`.
        if t.len() % 2 == 1 {
            data.push(0);
        }
    }
    if count == 0 {
        return Ok(None);
    }
    data[..2].copy_from_slice(&count.to_be_bytes());
    let mut out = Vec::new();
    chunk(&mut out, b"COMT", &data);
    Ok(Some(out))
}
