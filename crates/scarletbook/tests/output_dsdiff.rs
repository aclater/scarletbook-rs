// SPDX-License-Identifier: MIT OR Apache-2.0

//! Plain DSD DSDIFF layouts (specification 8, 8.4.1 to 8.4.5; issue #131): the size known
//! from the TOC equals a sequential write's, the chunks are those of 8.4.2 and 8.4.3, and
//! byte ranges read at random equal the sequential write's. Caller chunks (8.4.2 item 5 and
//! 8.6 row 33, at sacd-spec `e069569`; issue #169) follow the local chunks.

#![cfg(feature = "output")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;
mod output_common;

use std::io::Cursor;

use common::TrackSpec;
use output_common::{Lcg, check, contents, corpus, corpus_ranges, enough, plain, synthetic, variants};
use proptest::prelude::*;
use scarletbook::output::{
    CallerChunk, Comment, CommentKind, Format, LocalChunk, OutputOptions, PropertyChunk, RegionKind,
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

/// What the property chunk holds besides `FS  ` and `CMPR`.
struct Props<'a> {
    ids: &'a [&'a [u8; 4]],
    lsco: Option<u16>,
    abss: Option<u32>,
}

/// A plain DSD DSDIFF file written sequentially from 8.4, chunks in table order.
fn sequential(content: &[Vec<u8>], p: &Props<'_>) -> Vec<u8> {
    let mut prop = b"SND ".to_vec();
    chunk(&mut prop, b"FS  ", &2_822_400u32.to_be_bytes());
    let mut chnl = (p.ids.len() as u16).to_be_bytes().to_vec();
    p.ids.iter().for_each(|id| chnl.extend_from_slice(*id));
    chunk(&mut prop, b"CHNL", &chnl);
    let mut cmpr = b"DSD ".to_vec();
    cmpr.push(14);
    cmpr.extend_from_slice(b"not compressed");
    chunk(&mut prop, b"CMPR", &cmpr);
    if let Some(a) = p.abss {
        let a = u64::from(a);
        let mut d = ((a / 270_000) as u16).to_be_bytes().to_vec();
        d.push((a / 4500 % 60) as u8);
        d.push((a / 75 % 60) as u8);
        d.extend_from_slice(&((a % 75 * 37_632) as u32).to_be_bytes());
        chunk(&mut prop, b"ABSS", &d);
    }
    if let Some(v) = p.lsco {
        chunk(&mut prop, b"LSCO", &v.to_be_bytes());
    }
    let mut form = b"DSD ".to_vec();
    chunk(&mut form, b"FVER", &[1, 5, 0, 0]);
    chunk(&mut form, b"PROP", &prop);
    chunk(&mut form, b"DSD ", &content.concat());
    let mut f = Vec::new();
    chunk(&mut f, b"FRM8", &form);
    f
}

#[test]
#[cfg_attr(miri, ignore = "the corpus is too large for Miri")]
fn corpus_layouts_equal_sequential_writes() {
    let mut rng = Lcg(84);
    let mut checked = 0;
    for (_name, bytes) in corpus() {
        let Ok(mut disc) = Disc::open(Cursor::new(bytes)) else { continue };
        for area in disc.toc().areas.clone() {
            let (ids, lsco): (&[&[u8; 4]], u16) = match area.channel_speakers().map(<[_]>::len) {
                Some(2) => (&[b"SLFT", b"SRGT"], 0),
                Some(5) => (&[b"MLFT", b"MRGT", b"C   ", b"LS  ", b"RS  "], 3),
                Some(6) => (&[b"MLFT", b"MRGT", b"C   ", b"LFE ", b"LS  ", b"RS  "], 4),
                _ => continue,
            };
            for (first, end) in corpus_ranges(&disc, area.slot) {
                if let Err(e) = disc.check_range(area.slot, first, end) {
                    let o = disc.output_layout(
                        area.slot,
                        first,
                        end,
                        Format::DsdiffDsd,
                        &OutputOptions::default(),
                    );
                    assert_eq!(o.unwrap_err().to_string(), e.to_string());
                    continue;
                }
                let content = contents(&mut disc, area.slot, first, end);
                for (lsco_on, abss) in variants(&[(true, None), (false, Some(first))], checked) {
                    let options = OutputOptions::default()
                        .with_dsdiff_loudspeaker_configuration(lsco_on)
                        .with_dsdiff_absolute_start(abss);
                    let layout =
                        disc.output_layout(area.slot, first, end, Format::DsdiffDsd, &options).unwrap();
                    let expected = sequential(&content, &Props { ids, lsco: lsco_on.then_some(lsco), abss });
                    check(&mut disc, &layout, &expected, &content, &mut rng);
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > enough(), "only {checked} layouts checked");
}

#[test]
fn unspecified_layouts_take_numbered_channel_ids() {
    let mut disc = synthetic(4, &plain(4, 3), vec![TrackSpec { start: 0, duration: 3 }], 3);
    let content = contents(&mut disc, Slot::TwoChannel, 0, 3);
    let layout =
        disc.output_layout(Slot::TwoChannel, 0, 3, Format::DsdiffDsd, &OutputOptions::default()).unwrap();
    let ids: &[&[u8; 4]] = &[b"C000", b"C001", b"C002", b"C003"];
    check(
        &mut disc,
        &layout,
        &sequential(&content, &Props { ids, lsco: None, abss: None }),
        &content,
        &mut Lcg(1),
    );

    let options = OutputOptions::default()
        .with_dsdiff_unspecified_channel_numbers(Some(vec![7, 10, 999, 123]))
        .with_dsdiff_unspecified_loudspeaker_configuration(true);
    let layout = disc.output_layout(Slot::TwoChannel, 0, 3, Format::DsdiffDsd, &options).unwrap();
    let ids: &[&[u8; 4]] = &[b"C007", b"C010", b"C999", b"C123"];
    check(
        &mut disc,
        &layout,
        &sequential(&content, &Props { ids, lsco: Some(65_535), abss: None }),
        &content,
        &mut Lcg(2),
    );

    for bad in [vec![0, 1, 2], vec![0, 1, 2, 2], vec![0, 1, 2, 1000]] {
        let options = OutputOptions::default().with_dsdiff_unspecified_channel_numbers(Some(bad.clone()));
        let e = disc.output_layout(Slot::TwoChannel, 0, 3, Format::DsdiffDsd, &options).unwrap_err();
        assert!(matches!(e, Error::InvalidArgument(_)), "{bad:?}: {e}");
    }
}

#[test]
fn property_order_and_absolute_start_are_checked() {
    let mut disc = synthetic(2, &plain(2, 2), vec![TrackSpec { start: 0, duration: 2 }], 2);
    let mut order = PropertyChunk::TABLE_ORDER;
    order.reverse();
    let options =
        OutputOptions::default().with_dsdiff_property_order(order).with_dsdiff_absolute_start(Some(75));
    let layout = disc.output_layout(Slot::TwoChannel, 0, 2, Format::DsdiffDsd, &options).unwrap();
    let mut head = vec![0; 200];
    layout.read_at(&mut disc, 0, &mut head).unwrap();
    // `FRM8`, `DSD `, `FVER` (16 bytes), then `PROP`, `SND ` and the chunks reversed.
    let ids: Vec<&[u8]> = vec![&head[48..52], &head[62..66], &head[82..86]];
    assert_eq!(ids, [b"LSCO" as &[u8], b"ABSS", b"CMPR"]);
    // ABSS: 0 h 0 min 1 s 0 samples.
    assert_eq!(&head[74..82], &[0, 0, 0, 1, 0, 0, 0, 0]);
    assert_eq!(layout.size(), u64::from_be_bytes(head[4..12].try_into().unwrap()) + 12);
    assert_eq!(layout.size() % 2, 0);

    let mut twice = PropertyChunk::TABLE_ORDER;
    twice[0] = PropertyChunk::Channels;
    let e = disc
        .output_layout(
            Slot::TwoChannel,
            0,
            2,
            Format::DsdiffDsd,
            &OutputOptions::default().with_dsdiff_property_order(twice),
        )
        .unwrap_err();
    assert!(matches!(e, Error::InvalidArgument(_)), "{e}");
    let too_late = OutputOptions::default().with_dsdiff_absolute_start(Some(6_479_998));
    let e = disc.output_layout(Slot::TwoChannel, 0, 2, Format::DsdiffDsd, &too_late).unwrap_err();
    assert!(matches!(e, Error::InvalidArgument(_)), "{e}");
    let last = OutputOptions::default().with_dsdiff_absolute_start(Some(6_479_997));
    disc.output_layout(Slot::TwoChannel, 0, 2, Format::DsdiffDsd, &last).unwrap();
}

fn config() -> ProptestConfig {
    if cfg!(miri) {
        ProptestConfig { cases: 2, failure_persistence: None, ..ProptestConfig::default() }
    } else {
        ProptestConfig { cases: 32, ..ProptestConfig::default() }
    }
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn synthetic_ranges(channels in 1u8..=if cfg!(miri) { 2 } else { 6 }, n in 1u32..if cfg!(miri) { 3 } else { 6 }, gaps in proptest::collection::vec(any::<bool>(), 6), a in 0u32..6, len in 1u32..6, seed in any::<u64>()) {
        let frames: Vec<_> = plain(channels, n).into_iter().filter(|f| !gaps[f.number as usize]).collect();
        let first = a % n;
        let end = (first + len).min(n);
        let mut disc = synthetic(channels, &frames, vec![TrackSpec { start: 0, duration: n }], n);
        let layout = disc.output_layout(Slot::TwoChannel, first, end, Format::DsdiffDsd, &OutputOptions::default()).unwrap();
        let content = contents(&mut disc, Slot::TwoChannel, first, end);
        let names: Vec<[u8; 4]> = (0..channels).map(|c| *format!("C{c:03}").as_bytes().first_chunk().unwrap()).collect();
        let stereo: [[u8; 4]; 2] = [*b"SLFT", *b"SRGT"];
        let ids: Vec<&[u8; 4]> = if channels == 2 { stereo.iter().collect() } else { names.iter().collect() };
        let lsco = (channels == 2).then_some(0);
        check(&mut disc, &layout, &sequential(&content, &Props { ids: &ids, lsco, abss: None }), &content, &mut Lcg(seed));
    }
}

/// Caller chunks of every kind 8.4.2 item 5 allows: odd and even data, empty data, a
/// repeated ID and an ID of spaces only.
fn caller_chunks() -> Vec<CallerChunk> {
    vec![
        CallerChunk::new(*b"ID3 ", vec![1, 2, 3, 4, 5]).unwrap(),
        CallerChunk::new(*b"ID3 ", vec![6, 7, 8, 9]).unwrap(),
        CallerChunk::new(*b"x   ", Vec::new()).unwrap(),
        CallerChunk::new(*b"    ", vec![0xFF]).unwrap(),
    ]
}

/// `file` with `chunks` appended by 8.4.1 item 1 and its `FRM8` size counting them (8.4.2
/// items 4 and 5).
fn with_callers(file: &[u8], chunks: &[CallerChunk]) -> Vec<u8> {
    let mut f = file.to_vec();
    for c in chunks {
        chunk(&mut f, &c.id(), c.data());
    }
    let frm8 = f.len() as u64 - 12;
    f[4..12].copy_from_slice(&frm8.to_be_bytes());
    f
}

fn whole(disc: &mut Disc<Cursor<Vec<u8>>>, first: u32, end: u32, options: &OutputOptions) -> Vec<u8> {
    let layout = disc.output_layout(Slot::TwoChannel, first, end, Format::DsdiffDsd, options).unwrap();
    let mut f = vec![0; layout.size() as usize];
    layout.read_at(disc, 0, &mut f).unwrap();
    f
}

#[test]
fn caller_chunks_follow_the_local_chunks() {
    let mut disc = synthetic(2, &plain(2, 3), vec![TrackSpec { start: 0, duration: 3 }], 3);
    let content = contents(&mut disc, Slot::TwoChannel, 0, 3);
    let options = OutputOptions::default().with_dsdiff_caller_chunks(caller_chunks());
    let layout = disc.output_layout(Slot::TwoChannel, 0, 3, Format::DsdiffDsd, &options).unwrap();
    let ids: &[&[u8; 4]] = &[b"SLFT", b"SRGT"];
    let plain_file = sequential(&content, &Props { ids, lsco: Some(0), abss: None });
    let expected = with_callers(&plain_file, &caller_chunks());
    // The chunks' bytes, written out from 8.4.1 item 1: `D` 5 with a pad byte, 4, 0 and 1
    // with a pad byte.
    let mut tail = b"ID3 ".to_vec();
    tail.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 5, 1, 2, 3, 4, 5, 0]);
    tail.extend_from_slice(b"ID3 ");
    tail.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 4, 6, 7, 8, 9]);
    tail.extend_from_slice(b"x   ");
    tail.extend_from_slice(&[0; 8]);
    tail.extend_from_slice(b"    ");
    tail.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1, 0xFF, 0]);
    assert_eq!(expected[plain_file.len()..], tail);
    assert_eq!(expected.len() % 2, 0);
    check(&mut disc, &layout, &expected, &content, &mut Lcg(3));
    // The size and the regions count them: the last region is the trailer that holds them.
    assert_eq!(layout.size(), plain_file.len() as u64 + 60);
    let last = layout.regions().last().unwrap();
    assert_eq!((last.kind, last.offset, last.len), (RegionKind::Trailer, plain_file.len() as u64, 60));

    // After the last local chunk, wherever the order puts the local chunks.
    let comments = OutputOptions::default()
        .with_dsdiff_comments(vec![Comment::new(CommentKind::General, "abc")])
        .with_dsdiff_local_order(vec![LocalChunk::Property, LocalChunk::SoundData, LocalChunk::Comments]);
    let base = whole(&mut disc, 0, 3, &comments);
    let options = comments.with_dsdiff_caller_chunks(caller_chunks());
    let layout = disc.output_layout(Slot::TwoChannel, 0, 3, Format::DsdiffDsd, &options).unwrap();
    check(&mut disc, &layout, &with_callers(&base, &caller_chunks()), &content, &mut Lcg(4));

    // A DSF file ignores them.
    let dsf = |o: &OutputOptions| disc.output_layout(Slot::TwoChannel, 0, 3, Format::Dsf, o).unwrap().size();
    assert_eq!(dsf(&options), dsf(&OutputOptions::default()));
}

#[test]
fn caller_chunk_ids_are_checked() {
    for id in [*b"ID3\x1F", *b"\x7F   ", *b" ID3", *b"A B ", *b"\xC3\xA9  "] {
        let e = CallerChunk::new(id, Vec::new()).unwrap_err();
        assert!(matches!(e, Error::InvalidArgument(_)), "{id:?}: {e}");
    }
    // The chunks specification 8 defines (8.4.2 item 5).
    for id in [b"FRM8", b"FVER", b"PROP", b"DSD ", b"DST ", b"DSTI", b"COMT", b"DIIN", b"MANF"] {
        let e = CallerChunk::new(*id, vec![0]).unwrap_err();
        assert!(matches!(e, Error::InvalidArgument(_)), "{id:?}: {e}");
    }
    // Others are allowed, the chunks inside `PROP` and `DIIN` among them.
    for id in [b"ID3 ", b"FS  ", b"MARK", b"A~!-", b"    "] {
        CallerChunk::new(*id, Vec::new()).unwrap();
    }
}

#[test]
#[cfg_attr(miri, ignore = "75-frame tracks are too large for Miri")]
fn caller_chunks_in_an_edited_master() {
    let mut disc = synthetic(2, &plain(2, 75), vec![TrackSpec { start: 0, duration: 75 }], 75);
    let content = contents(&mut disc, Slot::TwoChannel, 0, 75);
    let master = OutputOptions::default().with_dsdiff_edited_master(true);
    let base = whole(&mut disc, 0, 75, &master);
    let options = master.with_dsdiff_caller_chunks(caller_chunks());
    let layout = disc.output_layout(Slot::TwoChannel, 0, 75, Format::DsdiffDsd, &options).unwrap();
    check(&mut disc, &layout, &with_callers(&base, &caller_chunks()), &content, &mut Lcg(5));
}
