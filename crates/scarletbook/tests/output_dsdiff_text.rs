// SPDX-License-Identifier: MIT OR Apache-2.0

//! DSDIFF text (specification 8, 8.4.7 item 5, 8.4.8 and 8.4.9, at sacd-spec `6db1251`;
//! issue #131): the artist and title of `DIIN`, the comments of `COMT` and marker text,
//! written as text bytes with every writer's choice of 8.6 rows 14, 22 to 26 and 31.

#![cfg(feature = "output")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;
mod output_common;

use std::io::Cursor;

use common::TrackSpec;
use output_common::{plain, synthetic};
use scarletbook::output::{
    ArtistTitleSource, Comment, CommentKind, DsdiffDeparture, Format, InfoChunk, LocalChunk, Marker,
    MarkerKind, MarkerText, Markers, OutputOptions, SourceTrack, TagItem, TagSet, Tags,
};
use scarletbook::{Disc, Error, Slot};

fn chunk(out: &mut Vec<u8>, id: &[u8], data: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(data.len() as u64).to_be_bytes());
    out.extend_from_slice(data);
    if data.len() % 2 == 1 {
        out.push(0);
    }
}

/// The chunks inside a chunk's data, from `at`: (ID, data without the pad byte).
fn chunks(data: &[u8], mut at: usize) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut out = Vec::new();
    while at < data.len() {
        let d = u64::from_be_bytes(data[at + 4..at + 12].try_into().unwrap()) as usize;
        out.push((data[at..at + 4].to_vec(), data[at + 12..at + 12 + d].to_vec()));
        at += 12 + d + d % 2;
    }
    assert_eq!(at, data.len());
    out
}

fn file(disc: &mut Disc<Cursor<Vec<u8>>>, first: u32, end: u32, options: &OutputOptions) -> Vec<u8> {
    let layout = disc.output_layout(Slot::TwoChannel, first, end, Format::DsdiffDsd, options).unwrap();
    let mut f = vec![0; layout.size() as usize];
    layout.read_at(disc, 0, &mut f).unwrap();
    f
}

/// A local chunk's data.
fn local(f: &[u8], id: &[u8]) -> Option<Vec<u8>> {
    chunks(f, 16).into_iter().find(|(i, _)| i == id).map(|(_, d)| d)
}

fn disc() -> Disc<Cursor<Vec<u8>>> {
    synthetic(
        2,
        &plain(2, 3),
        vec![TrackSpec { start: 0, duration: 1 }, TrackSpec { start: 1, duration: 2 }],
        3,
    )
}

fn tags() -> TagSet {
    let mut s = TagSet::new();
    s.set(TagItem::AlbumArtist, "Orchestre de la Suisse Romande");
    s.set(TagItem::Album, "Ravel – Boléro");
    s
}

/// `DIAR` or `DITI` data: `k` and the text (8.4.8).
fn text(t: &[u8]) -> Vec<u8> {
    let mut d = (t.len() as u32).to_be_bytes().to_vec();
    d.extend_from_slice(t);
    d
}

#[test]
fn artist_and_title() {
    let mut disc = disc();
    let on = OutputOptions::default()
        .with_dsdiff_edited_master_info(true)
        .with_dsdiff_artist_title(Some(true))
        .with_tags(Tags::Set(tags()));
    let diin = chunks(&local(&file(&mut disc, 0, 3, &on), b"DIIN").unwrap(), 0);
    let get = |id: &[u8]| diin.iter().find(|(i, _)| i == id).map(|(_, d)| d.clone());
    assert_eq!(get(b"DIAR"), Some(text(b"Orchestre de la Suisse Romande")));
    // W26: `?` for every code point outside U+0020 to U+007E (8.4.9).
    assert_eq!(get(b"DITI"), Some(text(b"Ravel ? Bol?ro")));
    let ids: Vec<&[u8]> = diin.iter().map(|(i, _)| i.as_slice()).collect();
    assert_eq!(ids, [b"EMID" as &[u8], b"MARK", b"MARK", b"MARK", b"MARK", b"DIAR", b"DITI"]);

    // W26 removing, and W14 another order.
    let removed = on.clone().with_dsdiff_text_replacement(None).with_dsdiff_info_order([
        InfoChunk::Title,
        InfoChunk::Artist,
        InfoChunk::Markers,
        InfoChunk::EditedMasterId,
    ]);
    let diin = chunks(&local(&file(&mut disc, 0, 3, &removed), b"DIIN").unwrap(), 0);
    assert_eq!(diin[0], (b"DITI".to_vec(), text(b"Ravel  Bolro")));
    assert_eq!(diin.last().unwrap().0, b"EMID");

    // W22 counts the pad byte of an odd text in `D`; the bytes are the same.
    let mut odd = TagSet::new();
    odd.set(TagItem::Album, "abc");
    let plain_pad = on.clone().with_tags(Tags::Set(odd.clone()));
    let counted = plain_pad.clone().with_dsdiff_text_pad_counted(true);
    let a = file(&mut disc, 0, 3, &plain_pad);
    let b = file(&mut disc, 0, 3, &counted);
    assert_eq!(a.len(), b.len());
    let diti = |f: &[u8]| {
        let d = local(f, b"DIIN").unwrap();
        let at = d.windows(4).position(|w| w == b"DITI").unwrap();
        d[at..].to_vec()
    };
    let mut want = Vec::new();
    chunk(&mut want, b"DITI", &text(b"abc"));
    assert_eq!(diti(&a), want);
    let mut want = Vec::new();
    chunk(&mut want, b"DITI", &[text(b"abc"), vec![0]].concat());
    assert_eq!(diti(&b), want);

    // W31: an empty title is left out, or written with `k = 0`.
    let mut empty = TagSet::new();
    empty.set(TagItem::Album, "ÉÉ");
    let left_out = on.clone().with_tags(Tags::Set(empty.clone())).with_dsdiff_text_replacement(None);
    assert!(local(&file(&mut disc, 0, 3, &left_out), b"DIIN").unwrap().windows(4).all(|w| w != b"DITI"));
    let written = left_out.with_dsdiff_empty_text(true);
    assert_eq!(
        diti(&file(&mut disc, 0, 3, &written)),
        [b"DITI".to_vec(), 4u64.to_be_bytes().to_vec(), vec![0; 4]].concat()
    );

    // Off by default outside an edited master, on by default in one; none for raw tags.
    let default = OutputOptions::default().with_dsdiff_edited_master_info(true).with_tags(Tags::Set(tags()));
    assert!(local(&file(&mut disc, 0, 3, &default), b"DIIN").unwrap().windows(4).all(|w| w != b"DIAR"));
    let raw = on.clone().with_tags(Tags::Raw(Vec::new()));
    assert!(local(&file(&mut disc, 0, 3, &raw), b"DIIN").unwrap().windows(4).all(|w| w != b"DIAR"));
    // A replacement that isn't a text byte.
    let bad = on.clone().with_dsdiff_text_replacement(Some(0x7F));
    assert!(matches!(
        disc.output_layout(Slot::TwoChannel, 0, 3, Format::DsdiffDsd, &bad).unwrap_err(),
        Error::InvalidArgument(_)
    ));
    // A bad DIIN order.
    let twice = on.with_dsdiff_info_order([InfoChunk::Title; 4]);
    assert!(matches!(
        disc.output_layout(Slot::TwoChannel, 0, 3, Format::DsdiffDsd, &twice).unwrap_err(),
        Error::InvalidArgument(_)
    ));
}

#[test]
fn marker_text_is_the_track_title() {
    let mut disc = disc();
    let options = OutputOptions::default()
        .with_dsdiff_edited_master_info(true)
        .with_dsdiff_marker_text(MarkerText::TrackTitle);
    let diin = chunks(&local(&file(&mut disc, 0, 3, &options), b"DIIN").unwrap(), 0);
    let texts: Vec<Vec<u8>> = diin
        .iter()
        .filter(|(i, _)| i == b"MARK")
        .map(|(_, d)| {
            let k = u32::from_be_bytes(d[18..22].try_into().unwrap()) as usize;
            d[22..22 + k].to_vec()
        })
        .collect();
    // Program start, track 1 start, track 2 start, track 2 stop.
    assert_eq!(texts, [b"".to_vec(), b"T1".to_vec(), b"T2".to_vec(), b"".to_vec()]);
}

#[test]
fn comments() {
    let mut disc = disc();
    let mut dated = Comment::new(CommentKind::FileHistory(1), "operator: é");
    (dated.year, dated.month, dated.day, dated.hour, dated.minutes) = (2026, 10, 31, 23, 59);
    let list = vec![
        Comment::new(CommentKind::General, "abc"),
        dated,
        Comment::new(CommentKind::Channel(2), ""),
        Comment::new(CommentKind::SoundSource(0), "DSD"),
    ];
    let options = OutputOptions::default().with_dsdiff_comments(list.clone());
    let f = file(&mut disc, 0, 3, &options);
    let ids: Vec<Vec<u8>> = chunks(&f, 16).into_iter().map(|(i, _)| i).collect();
    assert_eq!(ids, [b"FVER".to_vec(), b"PROP".to_vec(), b"COMT".to_vec(), b"DSD ".to_vec()]);
    let comt = local(&f, b"COMT").unwrap();
    let comment = |y: u16, mdhm: [u8; 4], kind: u16, reference: u16, t: &[u8]| {
        let mut c = y.to_be_bytes().to_vec();
        c.extend_from_slice(&mdhm);
        c.extend_from_slice(&kind.to_be_bytes());
        c.extend_from_slice(&reference.to_be_bytes());
        c.extend_from_slice(&(t.len() as u32).to_be_bytes());
        c.extend_from_slice(t);
        if t.len() % 2 == 1 {
            c.push(0);
        }
        c
    };
    // The empty comment is left out (W31).
    let want = [
        3u16.to_be_bytes().to_vec(),
        comment(0, [0; 4], 0, 0, b"abc"),
        comment(2026, [10, 31, 23, 59], 3, 1, b"operator: ?"),
        comment(0, [0; 4], 2, 0, b"DSD"),
    ]
    .concat();
    assert_eq!(comt, want);
    let with_empty = options.clone().with_dsdiff_empty_text(true);
    assert_eq!(
        u16::from_be_bytes(
            local(&file(&mut disc, 0, 3, &with_empty), b"COMT").unwrap()[..2].try_into().unwrap()
        ),
        4
    );
    // Only empty comments: no `COMT`.
    let only_empty =
        OutputOptions::default().with_dsdiff_comments(vec![Comment::new(CommentKind::General, "")]);
    assert!(local(&file(&mut disc, 0, 3, &only_empty), b"COMT").is_none());
    // COMT after the sound data (W13).
    let order = vec![LocalChunk::Property, LocalChunk::SoundData, LocalChunk::Comments];
    let ids: Vec<Vec<u8>> =
        chunks(&file(&mut disc, 0, 3, &options.clone().with_dsdiff_local_order(order)), 16)
            .into_iter()
            .map(|(i, _)| i)
            .collect();
    assert_eq!(ids.last().unwrap(), b"COMT");
    // Out-of-range fields.
    let mut late = Comment::new(CommentKind::General, "x");
    late.hour = 24;
    for bad in [
        late,
        Comment::new(CommentKind::Channel(3), "x"),
        Comment::new(CommentKind::SoundSource(3), "x"),
        Comment::new(CommentKind::FileHistory(5), "x"),
    ] {
        let o = OutputOptions::default().with_dsdiff_comments(vec![bad]);
        assert!(matches!(
            disc.output_layout(Slot::TwoChannel, 0, 3, Format::DsdiffDsd, &o).unwrap_err(),
            Error::InvalidArgument(_)
        ));
    }
}

/// `DIIN` without `EMID` outside an edited master (8.4.8 and 8.6 row 24 at sacd-spec
/// `e069569`), and the refusal in an edited master, which requires it (8.4.11 item 1).
#[test]
fn diin_without_emid() {
    let mut disc = disc();
    let on = OutputOptions::default()
        .with_dsdiff_edited_master_info(true)
        .with_dsdiff_artist_title(Some(true))
        .with_dsdiff_edited_master_id("id".to_owned())
        .with_tags(Tags::Set(tags()));
    // The default writes `EMID`.
    let with = local(&file(&mut disc, 0, 3, &on), b"DIIN").unwrap();
    assert_eq!(chunks(&with, 0)[0], (b"EMID".to_vec(), b"id".to_vec()));
    // Without it, `MARK`, `DIAR` and `DITI` remain, in the order of W14, which still names
    // `EMID`; the file is 14 bytes (`EMID` with 2 text bytes) shorter.
    let without = on.clone().with_dsdiff_edited_master_id_written(false);
    let f = file(&mut disc, 0, 3, &without);
    let diin = local(&f, b"DIIN").unwrap();
    assert_eq!(diin, with[14..]);
    let ids: Vec<Vec<u8>> = chunks(&diin, 0).into_iter().map(|(i, _)| i).collect();
    assert_eq!(ids, [&b"MARK"[..], b"MARK", b"MARK", b"MARK", b"DIAR", b"DITI"]);
    assert_eq!(f.len(), file(&mut disc, 0, 3, &on).len() - 14);
    assert_eq!(u64::from_be_bytes(f[4..12].try_into().unwrap()), f.len() as u64 - 12);
    // The `EMID` text is not checked when it isn't written.
    let bad_id = without.clone().with_dsdiff_edited_master_id("\u{e9}".to_owned());
    assert_eq!(file(&mut disc, 0, 3, &bad_id), f);
    let bad_id = on.clone().with_dsdiff_edited_master_id("\u{e9}".to_owned());
    assert!(matches!(
        disc.output_layout(Slot::TwoChannel, 0, 3, Format::DsdiffDsd, &bad_id).unwrap_err(),
        Error::InvalidArgument(_)
    ));
    // A track without markers, artist or title (W34, 8.4.8 at sacd-spec `048aa1a`): by
    // default `DIIN` with `D = 0`, 12 bytes, or else no `DIIN`, the file 12 bytes shorter
    // and otherwise the same.
    let bare = OutputOptions::default()
        .with_dsdiff_edited_master_info(true)
        .with_dsdiff_edited_master_id_written(false);
    let f = file(&mut disc, 1, 3, &bare);
    let at = f.windows(4).position(|w| w == b"DIIN").unwrap();
    assert_eq!(f[at..at + 12], [&b"DIIN"[..], &0u64.to_be_bytes()].concat());
    assert_eq!(local(&f, b"DIIN"), Some(Vec::new()));
    let left_out = file(&mut disc, 1, 3, &bare.clone().with_dsdiff_empty_info(false));
    assert!(local(&left_out, b"DIIN").is_none());
    assert_eq!(left_out.len(), f.len() - 12);
    assert_eq!(left_out[12..at], f[12..at]);
    assert_eq!(left_out[at..], f[at + 12..]);
    assert_eq!(u64::from_be_bytes(left_out[4..12].try_into().unwrap()), left_out.len() as u64 - 12);
    // As the default without `DIIN` at all.
    assert_eq!(left_out, file(&mut disc, 1, 3, &OutputOptions::default()));
    // An empty title left out (W31) leaves `DIIN` empty too; one written with `k = 0` doesn't.
    let mut empty = TagSet::new();
    empty.set(TagItem::Album, "");
    let text_only = bare.clone().with_dsdiff_artist_title(Some(true)).with_tags(Tags::Set(empty));
    assert!(
        local(&file(&mut disc, 1, 3, &text_only.clone().with_dsdiff_empty_info(false)), b"DIIN").is_none()
    );
    let kept = text_only.with_dsdiff_empty_info(false).with_dsdiff_empty_text(true);
    assert_eq!(local(&file(&mut disc, 1, 3, &kept), b"DIIN").unwrap()[..4], *b"DITI");
    // With content, W34 changes nothing.
    assert_eq!(
        file(&mut disc, 0, 3, &without.clone().with_dsdiff_empty_info(false)),
        file(&mut disc, 0, 3, &without)
    );

    // An edited master requires `EMID`.
    let big = synthetic(
        2,
        &plain(2, 160),
        vec![TrackSpec { start: 0, duration: 75 }, TrackSpec { start: 75, duration: 75 }],
        160,
    );
    let master = OutputOptions::default().with_dsdiff_edited_master(true);
    assert!(big.output_layout(Slot::TwoChannel, 0, 160, Format::DsdiffDsd, &master).is_ok());
    let refused = master.with_dsdiff_edited_master_id_written(false);
    assert!(matches!(
        big.output_layout(Slot::TwoChannel, 0, 160, Format::DsdiffDsd, &refused).unwrap_err(),
        Error::InvalidArgument(_)
    ));
}

/// `DIAR` and `DITI` from the source track's performer and title (8.4.8 and 8.6 row 24 at
/// sacd-spec `e069569`; specification 3, 3.5, types 0x02 and 0x01).
#[test]
fn artist_and_title_of_the_source_track() {
    let mut disc = disc();
    let mut set = tags();
    set.set(TagItem::Performer, "Ansermet");
    set.set(TagItem::TrackTitle, "Bolero");
    let base = OutputOptions::default()
        .with_dsdiff_edited_master_info(true)
        .with_dsdiff_edited_master_id_written(false)
        .with_dsdiff_artist_title(Some(true))
        .with_tags(Tags::Set(set.clone()));
    let diin = |disc: &mut Disc<Cursor<Vec<u8>>>, first, end, o: &OutputOptions| {
        local(&file(disc, first, end, o), b"DIIN").unwrap()
    };
    let want = |artist: &[u8], title: &[u8]| {
        let mut d = Vec::new();
        if !artist.is_empty() {
            chunk(&mut d, b"DIAR", &text(artist));
        }
        if !title.is_empty() {
            chunk(&mut d, b"DITI", &text(title));
        }
        d
    };
    // Track 2, `[1, 3)`, has a source track and no markers by default.
    let album = diin(&mut disc, 1, 3, &base);
    assert_eq!(album, want(b"Orchestre de la Suisse Romande", b"Ravel ? Bol?ro"));
    let track = base.clone().with_dsdiff_artist_title_source(ArtistTitleSource::Track);
    assert_eq!(diin(&mut disc, 1, 3, &track), want(b"Ansermet", b"Bolero"));
    let then = base.clone().with_dsdiff_artist_title_source(ArtistTitleSource::TrackThenAlbum);
    assert_eq!(diin(&mut disc, 1, 3, &then), want(b"Ansermet", b"Bolero"));
    // Each absent track item: none, or the album item.
    let mut no_performer = set.clone();
    no_performer.remove(TagItem::Performer);
    let o = |source| {
        base.clone().with_tags(Tags::Set(no_performer.clone())).with_dsdiff_artist_title_source(source)
    };
    assert_eq!(diin(&mut disc, 1, 3, &o(ArtistTitleSource::Track)), want(b"", b"Bolero"));
    assert_eq!(
        diin(&mut disc, 1, 3, &o(ArtistTitleSource::TrackThenAlbum)),
        want(b"Orchestre de la Suisse Romande", b"Bolero")
    );
    // A frame range without a source track has no track strings, whatever the set holds.
    let none = |o: OutputOptions| o.with_source_track(SourceTrack::None);
    assert_eq!(diin(&mut disc, 1, 3, &none(track.clone())), want(b"", b""));
    assert_eq!(diin(&mut disc, 1, 3, &none(then.clone())), album);
    // From the TOC: the synthetic track 2 has the title `T2` and no performer, and the
    // disc the title `Synthetic` and no artist.
    let toc = track.clone().with_tags(Tags::FromToc);
    assert_eq!(diin(&mut disc, 1, 3, &toc), want(b"", b"T2"));
    assert_eq!(diin(&mut disc, 1, 3, &base.clone().with_tags(Tags::FromToc)), want(b"", b"Synthetic"));

    // An edited master has no source track (8.4.11), so only the album strings.
    let mut big = synthetic(
        2,
        &plain(2, 160),
        vec![TrackSpec { start: 0, duration: 75 }, TrackSpec { start: 75, duration: 75 }],
        160,
    );
    let master = |source| {
        OutputOptions::default()
            .with_dsdiff_edited_master(true)
            .with_tags(Tags::Set(set.clone()))
            .with_dsdiff_artist_title_source(source)
    };
    let mut tail = |o: &OutputOptions| {
        let d = diin(&mut big, 0, 160, o);
        chunks(&d, 0).into_iter().filter(|(i, _)| i == b"DIAR" || i == b"DITI").collect::<Vec<_>>()
    };
    assert!(tail(&master(ArtistTitleSource::Track)).is_empty());
    assert_eq!(tail(&master(ArtistTitleSource::TrackThenAlbum)), tail(&master(ArtistTitleSource::Album)));
    assert_eq!(
        tail(&master(ArtistTitleSource::Album))[0],
        (b"DIAR".to_vec(), text(b"Orchestre de la Suisse Romande"))
    );
}

/// Departure D2 (specification 8, 8.7, at sacd-spec `e069569`): `DIAR`, `DITI`, marker and
/// comment text as ISO 8859-1, every other code point removed whatever W26 says, `k`
/// counting the bytes written; `EMID` unchanged. It is never made unless named.
#[test]
fn departure_d2_writes_iso_8859_1() {
    let mut disc = disc();
    assert!(OutputOptions::default().dsdiff_departures.is_empty());
    let mut set = TagSet::new();
    // U+001F, U+007F and U+009F are removed; U+00A0 and U+00FF are the last bytes kept;
    // U+0100 and U+2013 are removed.
    set.set(TagItem::AlbumArtist, "Dvořák\u{1F}\u{7F}\u{9F}\u{A0}\u{FF}\u{100}");
    set.set(TagItem::Album, "Ravel – Boléro");
    let conforming = OutputOptions::default()
        .with_dsdiff_edited_master_info(true)
        .with_dsdiff_artist_title(Some(true))
        .with_dsdiff_marker_text(MarkerText::TrackTitle)
        .with_dsdiff_comments(vec![Comment::new(CommentKind::FileHistory(1), "operator: é")])
        .with_tags(Tags::Set(set));
    let d2 = conforming.clone().with_dsdiff_departures([DsdiffDeparture::D2]);
    let diin = |f: &[u8]| chunks(&local(f, b"DIIN").unwrap(), 0);
    let get =
        |diin: &[(Vec<u8>, Vec<u8>)], id: &[u8]| diin.iter().find(|(i, _)| i == id).map(|(_, d)| d.clone());

    // Without D2: the replacement, as before.
    let f = file(&mut disc, 0, 3, &conforming);
    assert_eq!(get(&diin(&f), b"DITI"), Some(text(b"Ravel ? Bol?ro")));
    assert_eq!(get(&diin(&f), b"DIAR"), Some(text(b"Dvo??k??????")));

    // With D2: one byte per kept code point; the replacement `?` is not used.
    let f = file(&mut disc, 0, 3, &d2);
    let info = diin(&f);
    assert_eq!(get(&info, b"DITI"), Some(text(b"Ravel  Bol\xE9ro")));
    // `k` is 7, the bytes written, and the pad byte is uncounted in `D` (W22's default).
    assert_eq!(get(&info, b"DIAR"), Some(text(b"Dvo\xE1k\xA0\xFF")));
    let at = f.windows(4).position(|w| w == b"DIAR").unwrap();
    assert_eq!(&f[at + 4..at + 12], &11u64.to_be_bytes());
    // Marker text from the TOC is ASCII and so unchanged.
    let marks: Vec<Vec<u8>> =
        info.iter().filter(|(i, _)| i == b"MARK").map(|(_, d)| d[22..].to_vec()).collect();
    assert_eq!(marks, [b"".to_vec(), b"T1".to_vec(), b"T2".to_vec(), b"".to_vec()]);
    // The comment: `k` 11, then its pad byte (counted in the comment, not in `k`).
    let comt = local(&f, b"COMT").unwrap();
    assert_eq!(&comt[12..16], &11u32.to_be_bytes());
    assert_eq!(&comt[16..], b"operator: \xE9\0");

    // W22 counts the pad byte of odd text in `D` under D2 too: `k` stays the bytes written.
    let counted = d2.clone().with_dsdiff_text_pad_counted(true);
    let f = file(&mut disc, 0, 3, &counted);
    let at = f.windows(4).position(|w| w == b"DIAR").unwrap();
    assert_eq!(&f[at + 4..at + 12], &12u64.to_be_bytes());
    assert_eq!(&f[at + 12..at + 24], &[&7u32.to_be_bytes()[..], b"Dvo\xE1k\xA0\xFF\0"].concat()[..]);

    // W31: a text that D2 empties is left out, or written with `k = 0`.
    let mut gone = TagSet::new();
    gone.set(TagItem::Album, "€\u{100}");
    let left_out = d2.clone().with_tags(Tags::Set(gone)).with_dsdiff_comments(Vec::new());
    assert!(get(&diin(&file(&mut disc, 0, 3, &left_out)), b"DITI").is_none());
    let written = left_out.with_dsdiff_empty_text(true);
    assert_eq!(get(&diin(&file(&mut disc, 0, 3, &written)), b"DITI"), Some(text(b"")));

    // A custom marker's own bytes may be 0xA0 to 0xFF with D2 only; 0x80 to 0x9F never.
    let marker = |t: &[u8]| {
        let mut m = Marker::new(0, MarkerKind::ProgramStart);
        m.text = t.to_vec();
        Markers::Custom(vec![m])
    };
    let latin = conforming.clone().with_dsdiff_markers(marker(b"B\xE9"));
    let e = disc.output_layout(Slot::TwoChannel, 0, 3, Format::DsdiffDsd, &latin).unwrap_err();
    assert!(matches!(e, Error::InvalidArgument(_)), "{e}");
    let f = file(&mut disc, 0, 3, &latin.with_dsdiff_departures([DsdiffDeparture::D2]));
    assert_eq!(get(&diin(&f), b"MARK").unwrap()[18..], [&2u32.to_be_bytes()[..], b"B\xE9"].concat());
    let c1 = d2.clone().with_dsdiff_markers(marker(b"\x80"));
    let e = disc.output_layout(Slot::TwoChannel, 0, 3, Format::DsdiffDsd, &c1).unwrap_err();
    assert!(matches!(e, Error::InvalidArgument(_)), "{e}");

    // `EMID` stays bytes 0x20 to 0x7E.
    let emid = d2.with_dsdiff_edited_master_id("é".to_owned());
    let e = disc.output_layout(Slot::TwoChannel, 0, 3, Format::DsdiffDsd, &emid).unwrap_err();
    assert!(matches!(e, Error::InvalidArgument(_)), "{e}");
}
