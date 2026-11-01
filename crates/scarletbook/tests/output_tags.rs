// SPDX-License-Identifier: MIT OR Apache-2.0

//! DSF tags (specification 8, 8.2.6 and 8.3; issue #131): the tag a DSF layout ends with
//! parses, by a reader written here from 8.3, into the frames 8.3.5 gives the TOC's items,
//! with every ID3 writer's choice of 8.6 round-tripping.

#![cfg(feature = "output")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;
mod output_common;

use std::io::Cursor;

use common::TrackSpec;
use output_common::{corpus, corpus_ranges, plain, synthetic};
use scarletbook::output::{
    AlbumArtistFrame, ArrangerFrame, DateFrame, Format, Id3Encoding, Id3Options, Layout, MAX_TAG_BYTES,
    OutputOptions, RegionKind, SourceTrack, TagItem, TagSet, Tags,
};
use scarletbook::toc::{DiscTextItem, Genre, Toc, Track, TrackTextItem};
use scarletbook::{Disc, Error, Slot};

/// A frame: its ID and its body's strings (text frames), or (language, description, text)
/// for `COMM`.
type Frame = (String, Vec<String>);

/// The strings of a body in encoding `enc` (8.3.4).
fn strings(enc: u8, b: &[u8]) -> Vec<String> {
    let wide = matches!(enc, 1 | 2);
    let mut out = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    let mut i = 0;
    let flush = |cur: &mut Vec<u8>, out: &mut Vec<String>| {
        let s = match enc {
            0 => cur.iter().map(|&b| char::from(b)).collect(),
            3 => String::from_utf8(cur.clone()).unwrap(),
            1 => {
                let le = cur.starts_with(&[0xFF, 0xFE]);
                assert!(le || cur.starts_with(&[0xFE, 0xFF]), "a UTF-16 string starts with a BOM");
                let units: Vec<u16> =
                    cur[2..]
                        .chunks(2)
                        .map(|c| {
                            if le {
                                u16::from_le_bytes([c[0], c[1]])
                            } else {
                                u16::from_be_bytes([c[0], c[1]])
                            }
                        })
                        .collect();
                String::from_utf16(&units).unwrap()
            }
            2 => String::from_utf16(
                &cur.chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect::<Vec<_>>(),
            )
            .unwrap(),
            _ => panic!("encoding {enc}"),
        };
        out.push(s);
        cur.clear();
    };
    while i < b.len() {
        if wide && b[i] == 0 && b[i + 1] == 0 {
            flush(&mut cur, &mut out);
            i += 2;
        } else if !wide && b[i] == 0 {
            flush(&mut cur, &mut out);
            i += 1;
        } else {
            cur.extend_from_slice(&b[i..i + if wide { 2 } else { 1 }]);
            i += if wide { 2 } else { 1 };
        }
    }
    if !cur.is_empty() {
        flush(&mut cur, &mut out);
    }
    out
}

fn synchsafe(b: &[u8]) -> usize {
    assert!(b.iter().all(|x| x & 0x80 == 0), "synchsafe");
    b.iter().fold(0, |v, &x| v << 7 | usize::from(x))
}

/// A tag's frames (8.3.1 to 8.3.4), with its footer and padding: (frames, footer,
/// padding bytes, encodings used).
fn parse(tag: &[u8]) -> (Vec<Frame>, bool, usize, Vec<u8>) {
    assert_eq!(&tag[..5], b"ID3\x04\x00");
    let flags = tag[5];
    assert_eq!(flags & !0x10, 0);
    let size = synchsafe(&tag[6..10]);
    let footer = flags & 0x10 != 0;
    assert_eq!(tag.len(), 10 + size + if footer { 10 } else { 0 });
    if footer {
        assert_eq!(&tag[tag.len() - 10..tag.len() - 7], b"3DI");
        assert_eq!(tag[tag.len() - 7..], tag[3..10]);
    }
    let body = &tag[10..10 + size];
    let mut at = 0;
    let mut frames = Vec::new();
    let mut encodings = Vec::new();
    while at < body.len() && body[at] != 0 {
        let id = String::from_utf8(body[at..at + 4].to_vec()).unwrap();
        assert!(id.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()));
        let n = synchsafe(&body[at + 4..at + 8]);
        assert!(n >= 1);
        assert_eq!(&body[at + 8..at + 10], &[0, 0]);
        let b = &body[at + 10..at + 10 + n];
        encodings.push(b[0]);
        let s = if id == "COMM" {
            let mut v = vec![String::from_utf8(b[1..4].to_vec()).unwrap()];
            v.extend(strings(b[0], &b[4..]));
            v
        } else {
            strings(b[0], &b[1..])
        };
        frames.push((id, s));
        at += 10 + n;
    }
    let padding = body.len() - at;
    assert!(body[at..].iter().all(|&b| b == 0));
    assert!(!(footer && padding > 0), "a tag with a footer has no padding");
    (frames, footer, padding, encodings)
}

/// The tag of a DSF layout, checked against the header's pointer and sizes (8.2.1).
fn tag_of(disc: &mut Disc<Cursor<Vec<u8>>>, layout: &Layout) -> Option<Vec<u8>> {
    let mut f = vec![0; layout.size() as usize];
    layout.read_at(disc, 0, &mut f).unwrap();
    let u64le = |at: usize| u64::from_le_bytes(f[at..at + 8].try_into().unwrap());
    assert_eq!(u64le(12), f.len() as u64);
    let data_end = 80 + u64le(84);
    let pointer = u64le(20);
    let last = layout.regions().last().unwrap();
    if pointer == 0 {
        assert_eq!(data_end, f.len() as u64);
        assert_eq!(last.kind, RegionKind::SoundData);
        None
    } else {
        assert_eq!(pointer, data_end);
        assert_eq!((last.kind, last.offset), (RegionKind::Trailer, pointer));
        Some(f[pointer as usize..].to_vec())
    }
}

/// The control characters of 8.3.4 item 1 removed, line breaks too unless `newlines`.
fn clean(s: &str, newlines: bool) -> String {
    s.chars()
        .filter(|&c| (c == '\n' && newlines) || !matches!(c, '\u{0}'..='\u{1F}' | '\u{7F}'..='\u{9F}'))
        .collect()
}

/// The frames the defaults give a file of `track` (or none) of an area of `count` tracks,
/// from the TOC by 8.3.5, 8.3.7 and 8.3.8, in the table's order.
fn expected(toc: &Toc, track: Option<&Track>, count: usize) -> Vec<Frame> {
    let mut f: Vec<Frame> = Vec::new();
    let mut text = |id: &str, v: Option<&str>| {
        if let Some(v) = v {
            f.push((id.into(), vec![clean(v, false)]));
        }
    };
    let tt = |i| track.and_then(|t| t.text.get(i));
    text("TIT2", tt(TrackTextItem::Title));
    text("TPE1", tt(TrackTextItem::Performer));
    text("TEXT", tt(TrackTextItem::Songwriter));
    text("TCOM", tt(TrackTextItem::Composer));
    if let Some(a) = tt(TrackTextItem::Arranger) {
        f.push(("TIPL".into(), vec!["arranger".into(), clean(a, false)]));
    }
    if let Some(m) = tt(TrackTextItem::Message) {
        f.push(("COMM".into(), vec!["XXX".into(), String::new(), clean(m, true)]));
    }
    if let Some(m) = tt(TrackTextItem::ExtraMessage) {
        f.push(("COMM".into(), vec!["XXX".into(), "extra message".into(), clean(m, true)]));
    }
    if let Some(t) = track {
        f.push(("TRCK".into(), vec![format!("{}/{}", t.number, count)]));
        if let Some(isrc) = &t.isrc {
            f.push(("TSRC".into(), vec![clean(isrc, false)]));
        }
    }
    let genres: Vec<Genre> = track
        .and_then(|t| t.genre)
        .into_iter()
        .chain(toc.master.album_genres)
        .chain(toc.master.disc_genres)
        .collect();
    let mut names: Vec<String> = Vec::new();
    for g in genres {
        if g.category == 1 && (2..=29).contains(&g.index) {
            let n = g.name().unwrap().to_owned();
            if !names.contains(&n) {
                names.push(n);
            }
        }
    }
    if !names.is_empty() {
        f.push(("TCON".into(), names));
    }
    let d = &toc.master.text;
    let pick = |disc, album| d.get(disc).or(d.get(album));
    if let Some(v) = pick(DiscTextItem::DiscTitle, DiscTextItem::AlbumTitle) {
        f.push(("TALB".into(), vec![clean(v, false)]));
    }
    if let Some(v) = pick(DiscTextItem::DiscPublisher, DiscTextItem::AlbumPublisher) {
        f.push(("TPUB".into(), vec![clean(v, false)]));
    }
    let date = toc.master.date;
    if let (Some(v), 1..=9999) = (pick(DiscTextItem::DiscCopyright, DiscTextItem::AlbumCopyright), date.year)
    {
        f.push(("TCOP".into(), vec![format!("{:04} {}", date.year, clean(v, false))]));
    }
    let (q, z) = (toc.master.album_sequence_number, toc.master.album_set_size);
    if q >= 1 && q <= z {
        f.push(("TPOS".into(), vec![format!("{q}/{z}")]));
    }
    if (1..=9999).contains(&date.year) {
        let leap = date.year % 4 == 0 && date.year % 100 != 0 || date.year % 400 == 0;
        let days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        let s = match (date.month, date.day) {
            (1..=12, d) if d >= 1 && d <= days[usize::from(date.month) - 1] => {
                format!("{:04}-{:02}-{:02}", date.year, date.month, d)
            }
            (1..=12, _) => format!("{:04}-{:02}", date.year, date.month),
            _ => format!("{:04}", date.year),
        };
        f.push(("TDRL".into(), vec![s]));
    }
    f
}

#[test]
#[cfg_attr(miri, ignore = "the corpus is too large for Miri")]
fn corpus_tags_are_the_toc_items() {
    let mut tagged = 0;
    let mut frames = 0;
    for (_name, bytes) in corpus() {
        let Ok(mut disc) = Disc::open(Cursor::new(bytes)) else { continue };
        let toc = disc.toc().clone();
        for area in &toc.areas {
            if area.channel_speakers().is_none() {
                continue;
            }
            for (first, end) in corpus_ranges(&disc, area.slot) {
                let Ok(layout) =
                    disc.output_layout(area.slot, first, end, Format::Dsf, &OutputOptions::default())
                else {
                    continue;
                };
                let track = layout.source_track().and_then(|n| area.track(u32::from(n)));
                let want = expected(&toc, track, area.tracks.len());
                match tag_of(&mut disc, &layout) {
                    None => assert!(want.is_empty()),
                    Some(tag) => {
                        let (got, footer, padding, encodings) = parse(&tag);
                        assert_eq!(got, want, "{first}..{end}");
                        assert!(footer && padding == 0 && encodings.iter().all(|&e| e == 3));
                        tagged += 1;
                        frames += got.len();
                    }
                }
            }
        }
    }
    assert!(tagged > 20 && frames > 100, "{tagged} tags, {frames} frames");
}

fn disc() -> Disc<Cursor<Vec<u8>>> {
    synthetic(
        2,
        &plain(2, 2),
        vec![TrackSpec { start: 0, duration: 1 }, TrackSpec { start: 1, duration: 1 }],
        2,
    )
}

/// The tag of track 1 of [`disc`] with these options.
fn tag_with(options: &OutputOptions) -> Result<Option<Vec<u8>>, Error> {
    let mut disc = disc();
    let layout = disc.output_layout(Slot::TwoChannel, 0, 1, Format::Dsf, options)?;
    Ok(tag_of(&mut disc, &layout))
}

fn set() -> TagSet {
    let mut s = TagSet::new();
    s.set(TagItem::TrackTitle, "Tït\u{1}le ✓ 𝄞");
    s.push(TagItem::Performer, "A");
    s.push(TagItem::Performer, "B");
    s.set(TagItem::Arranger, "Arr");
    s.set(TagItem::Message, "line 1\nline 2\u{7F}");
    s.set(TagItem::ExtraMessage, "extra");
    s.set(TagItem::TrackNumber, "3");
    s.set(TagItem::TrackCount, "12");
    s.set(TagItem::AlbumArtist, "Orchestra");
    s.set(TagItem::Copyright, "Label");
    s.set(TagItem::Date, "2004-02-29");
    s.set(TagItem::DiscNumber, "2");
    s.set(TagItem::TitlePhonetic, "Taitoru");
    s.set_custom("mood", "calm");
    s
}

#[test]
fn every_encoding_round_trips() {
    for (enc, byte) in [
        (Id3Encoding::Utf8, 3),
        (Id3Encoding::Utf16Le, 1),
        (Id3Encoding::Utf16Be, 1),
        (Id3Encoding::Utf16BeNoBom, 2),
        (Id3Encoding::Latin1OrUtf8, 0),
    ] {
        for last in [false, true] {
            let id3 = Id3Options::new().with_encoding(enc).with_terminate_last(last);
            let options = OutputOptions::default().with_tags(Tags::Set(set())).with_id3(id3);
            let (frames, ..) = parse(&tag_with(&options).unwrap().unwrap());
            let get =
                |id: &str| frames.iter().filter(|(i, _)| i == id).map(|(_, s)| s.clone()).collect::<Vec<_>>();
            assert_eq!(get("TIT2"), [vec!["Tïtle ✓ 𝄞".to_owned()]]);
            assert_eq!(get("TPE1"), [vec!["A".to_owned(), "B".to_owned()]]);
            assert_eq!(get("COMM")[0], ["XXX", "", "line 1\nline 2"]);
            assert_eq!(get("TRCK"), [vec!["3/12".to_owned()]]);
            assert_eq!(get("TCOP"), [vec!["2004 Label".to_owned()]]);
            assert_eq!(get("TPOS"), [vec!["2".to_owned()]]);
            assert_eq!(get("TXXX"), [vec!["mood".to_owned(), "calm".to_owned()]]);
            assert!(get("TPE2").is_empty() && get("TSOT").is_empty(), "W11 defaults");
            let (_, _, _, encs) = parse(&tag_with(&options).unwrap().unwrap());
            if byte == 0 {
                // ISO 8859-1 where every code point allows it (8.3.4 item 2).
                assert!(encs.contains(&0) && encs.contains(&3));
            } else {
                assert!(encs.iter().all(|&e| e == byte));
            }
        }
    }
}

#[test]
fn frame_choices() {
    let id3 = Id3Options::new()
        .with_footer(false)
        .with_padding(33)
        .with_arranger(ArrangerFrame::Txxx)
        .with_album_artist(AlbumArtistFrame::Tpe2)
        .with_date(DateFrame::Tdrc)
        .with_phonetic(true)
        .with_track_count(false)
        .with_join_messages(Some(" / "))
        .with_language(*b"eng")
        .with_control_replacement(Some('?'))
        .with_split(Some(" ✓ "))
        .with_frame_order(vec![*b"TDRC", *b"TXXX"]);
    let options = OutputOptions::default().with_tags(Tags::Set(set())).with_id3(id3);
    let (frames, footer, padding, _) = parse(&tag_with(&options).unwrap().unwrap());
    assert!(!footer && padding == 33);
    let ids: Vec<&str> = frames.iter().map(|(i, _)| i.as_str()).collect();
    assert_eq!(&ids[..4], ["TDRC", "TXXX", "TXXX", "TIT2"]);
    let get = |id: &str| frames.iter().filter(|(i, _)| i == id).map(|(_, s)| s.clone()).collect::<Vec<_>>();
    assert_eq!(get("TIT2"), [vec!["Tït?le".to_owned(), "𝄞".to_owned()]]);
    assert_eq!(
        get("TXXX"),
        [vec!["arranger".to_owned(), "Arr".to_owned()], vec!["mood".to_owned(), "calm".to_owned()]]
    );
    assert_eq!(get("TPE2"), [vec!["Orchestra".to_owned()]]);
    assert_eq!(get("TSOT"), [vec!["Taitoru".to_owned()]]);
    assert_eq!(get("TRCK"), [vec!["3".to_owned()]]);
    assert_eq!(get("COMM"), [vec!["eng".to_owned(), String::new(), "line 1\nline 2? / extra".to_owned()]]);
    assert_eq!(get("TDRC"), [vec!["2004-02-29".to_owned()]]);
}

#[test]
fn tag_sources_and_refusals() {
    // No tag, an empty set, and a raw tag.
    assert_eq!(tag_with(&OutputOptions::default().with_tags(Tags::None)).unwrap(), None);
    assert_eq!(tag_with(&OutputOptions::default().with_tags(Tags::Set(TagSet::new()))).unwrap(), None);
    let raw = tag_with(&OutputOptions::default().with_tags(Tags::Set(set()))).unwrap().unwrap();
    assert_eq!(
        tag_with(&OutputOptions::default().with_tags(Tags::Raw(raw.clone()))).unwrap(),
        Some(raw.clone())
    );
    let mut short = raw.clone();
    short.pop();
    let mut v3 = raw.clone();
    v3[3] = 3;
    for bad in [short, v3, vec![], b"ID3\x04\x00\x00\x00\x00\x00\x80".to_vec()] {
        assert!(matches!(
            tag_with(&OutputOptions::default().with_tags(Tags::Raw(bad))),
            Err(Error::InvalidArgument(_))
        ));
    }
    // From the TOC, track 1 of the synthetic disc has its title, and track 2's file its own.
    let (frames, ..) = parse(&tag_with(&OutputOptions::default()).unwrap().unwrap());
    assert_eq!(frames[0], ("TIT2".to_owned(), vec!["T1".to_owned()]));
    let mut d = disc();
    let options = OutputOptions::default().with_source_track(SourceTrack::Track(2));
    let layout = d.output_layout(Slot::TwoChannel, 0, 2, Format::Dsf, &options).unwrap();
    let (frames, ..) = parse(&tag_of(&mut d, &layout).unwrap());
    assert_eq!(frames[0], ("TIT2".to_owned(), vec!["T2".to_owned()]));
    assert!(frames.contains(&("TRCK".to_owned(), vec!["2/2".to_owned()])));
    // Refusals: two TXXX of one description, a TXXX of two values, a bad language, a tag too large.
    let mut dup = set();
    dup.set_custom("arranger", "x");
    let mut id3 = Id3Options::default();
    id3.arranger = ArrangerFrame::Txxx;
    let mut two = set();
    two.push(TagItem::Arranger, "Second");
    let mut lang = Id3Options::default();
    lang.language = *b"EN1";
    let mut big = TagSet::new();
    big.set(TagItem::TrackTitle, &"x".repeat(MAX_TAG_BYTES));
    for options in [
        OutputOptions::default().with_tags(Tags::Set(dup)).with_id3(id3.clone()),
        OutputOptions::default().with_tags(Tags::Set(two)).with_id3(id3),
        OutputOptions::default().with_tags(Tags::Set(set())).with_id3(lang),
        OutputOptions::default().with_tags(Tags::Set(big)),
    ] {
        assert!(matches!(tag_with(&options), Err(Error::InvalidArgument(_))));
    }
    // TagSet edits.
    let mut s = set();
    s.remove(TagItem::Performer);
    assert!(s.get(TagItem::Performer).is_empty());
    s.push(TagItem::Performer, "C");
    assert_eq!(s.get(TagItem::Performer), ["C"]);
    s.set_custom("mood", "lively");
    assert_eq!(s.custom(), [("mood".to_owned(), "lively".to_owned())]);
    assert!(!s.is_empty() && TagSet::new().is_empty());
}

/// A caller's ID3v2.3 tag of `body.len() + 10` bytes, with flags byte `flags` and the
/// 28-bit size of 8.2.6 item 4 over the body.
fn v23(flags: u8, body: &[u8]) -> Vec<u8> {
    let n = body.len();
    let mut t = vec![b'I', b'D', b'3', 3, 0, flags];
    t.extend([(n >> 21) as u8 & 0x7F, (n >> 14) as u8 & 0x7F, (n >> 7) as u8 & 0x7F, n as u8 & 0x7F]);
    t.extend_from_slice(body);
    t
}

/// A caller-supplied ID3v2.3 tag (specification 8, 8.2.6 item 4 and 8.6 row 3, sacd-spec
/// e069569): written unchanged as the DSF metadata region, the header's checks refused
/// one by one, and the frames never looked at.
#[test]
fn caller_id3v23_tag() {
    let mut d = disc();
    let none = OutputOptions::default().with_tags(Tags::None);
    let bare = d.output_layout(Slot::TwoChannel, 0, 1, Format::Dsf, &none).unwrap();
    let mut plain = vec![0; bare.size() as usize];
    bare.read_at(&mut d, 0, &mut plain).unwrap();
    // A body that is no valid frame, a size over two of the four bytes (0x01 0x48 = 200),
    // the three flags [ID3v2.3] defines, and the smallest tag, a header alone.
    let body: Vec<u8> = (0..200u8).map(|i| i.wrapping_mul(37) | 0x80).collect();
    for tag in [v23(0, &body), v23(0xE0, &body), v23(0x40, b"\xFF\xFB"), v23(0, b"")] {
        let options = OutputOptions::default().with_tags(Tags::Raw(tag.clone()));
        let layout = d.output_layout(Slot::TwoChannel, 0, 1, Format::Dsf, &options).unwrap();
        let mut f = vec![0; layout.size() as usize];
        layout.read_at(&mut d, 0, &mut f).unwrap();
        // tag_of checks the file size and the metadata pointer (8.2.1, 8.2.6).
        assert_eq!(tag_of(&mut d, &layout).unwrap(), tag);
        assert_eq!(f.len(), plain.len() + tag.len());
        // Only the header's file size and metadata pointer differ before the tag.
        let mut want = plain.clone();
        want[12..20].copy_from_slice(&(f.len() as u64).to_le_bytes());
        want[20..28].copy_from_slice(&(plain.len() as u64).to_le_bytes());
        assert_eq!(f[..plain.len()], want[..]);
        assert_eq!(f[plain.len()..], tag[..]);
    }
    assert_eq!(v23(0, &body)[6..10], [0, 0, 1, 0x48]);
    let good = v23(0, &body);
    let with = |at: usize, b: u8| {
        let mut t = good.clone();
        t[at] = b;
        t
    };
    let mut long = good.clone();
    long.push(0);
    let mut short = good.clone();
    short.pop();
    let mut refused = vec![
        b"ID3\x03\x00\x00\x00\x00\x00".to_vec(), // m < 10
        with(0, b'X'),
        with(2, b'2'),
        with(4, 1),                                      // version 2.3.1
        with(6, 0x80),                                   // a size byte of 0x80 or more
        with(9, 0xC8),                                   // 200 with its high bit set, not 28-bit
        with(9, 0x47),                                   // size m - 11
        long,                                            // size m - 11
        short,                                           // size m - 9
        v23(0, &[0; 10]).into_iter().take(10).collect(), // size 10 of a header alone
    ];
    refused.extend((0..5).map(|bit| with(5, 1 << bit))); // the low five flag bits
    for bad in refused {
        let options = OutputOptions::default().with_tags(Tags::Raw(bad.clone()));
        assert!(
            matches!(
                d.output_layout(Slot::TwoChannel, 0, 1, Format::Dsf, &options),
                Err(Error::InvalidArgument(_))
            ),
            "{bad:02X?}"
        );
    }
    // Above MAX_TAG_BYTES, as every tag.
    let big = v23(0, &vec![0; MAX_TAG_BYTES - 9]);
    let options = OutputOptions::default().with_tags(Tags::Raw(big));
    assert!(matches!(
        d.output_layout(Slot::TwoChannel, 0, 1, Format::Dsf, &options),
        Err(Error::InvalidArgument(_))
    ));
}
