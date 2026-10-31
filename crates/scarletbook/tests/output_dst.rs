// SPDX-License-Identifier: MIT OR Apache-2.0

//! DSDIFF layouts with DST sound data (specification 8, 8.4.6; issue #131): the size
//! known from the TOC and the frame lengths equals a sequential write's, the `DSTF` frames
//! decode to the range's audio (8.4.6 "Decoding"), the `DSTC` CRCs and `DSTI` entries are
//! those of 8.4.6.1 and 8.4.6.2, and byte ranges read at random equal the sequential
//! write's.

#![cfg(feature = "output")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;
mod output_common;

use std::borrow::Cow;
use std::io::{Cursor, Read, Seek};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Invalid DST frames replaced in the tests (W18), so that the corpus test covers some.
static REPLACED: AtomicUsize = AtomicUsize::new(0);

use common::{PFrame, TrackSpec};
use output_common::{
    Lcg, check_with, contents, corpus, corpus_ranges, enough, plain, sampled, synthetic, variants,
};
use proptest::prelude::*;
use scarletbook::audio::FRAME_BYTES_PER_CHANNEL;
use scarletbook::output::{
    DstIndexOffset, DstLengths, Format, InvalidDst, Layout, LocalChunk, OutputOptions, PositionSource,
};
use scarletbook::{Disc, DstDecoder, Error, FrameContent, Slot};

fn chunk(out: &mut Vec<u8>, id: &[u8], data: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(data.len() as u64).to_be_bytes());
    out.extend_from_slice(data);
    if data.len() % 2 == 1 {
        out.push(0);
    }
}

/// The CRC of 8.4.6.1, bit by bit.
fn crc(content: &[u8]) -> u32 {
    let mut r: u64 = 0;
    for bit in content.iter().flat_map(|b| (0..8).rev().map(move |i| (b >> i) & 1)).chain([0; 32]) {
        r = (r << 1) | u64::from(bit);
        if r & (1 << 32) != 0 {
            r ^= 0x1_8000_0011;
        }
    }
    r as u32
}

/// Positions of a range: their DST equivalents as the file holds them, and their content.
struct Positions {
    first: u32,
    equivalents: Vec<Vec<u8>>,
    content: Vec<Vec<u8>>,
}

impl PositionSource for Positions {
    fn content(&mut self, t: u32) -> Result<Cow<'_, [u8]>, Error> {
        Ok(Cow::Borrowed(&self.content[(t - self.first) as usize]))
    }

    fn dst_equivalent(&mut self, t: u32) -> Result<Cow<'_, [u8]>, Error> {
        Ok(Cow::Borrowed(&self.equivalents[(t - self.first) as usize]))
    }
}

/// The positions of a range, with invalid DST frames as `invalid` says.
fn positions<R: Read + Seek>(
    disc: &mut Disc<R>,
    slot: Slot,
    first: u32,
    end: u32,
    invalid: InvalidDst,
) -> Positions {
    let channels = disc.toc().area(slot).unwrap().channel_count;
    let content = contents(disc, slot, first, end);
    let stored: Vec<FrameContent> =
        disc.range_frames(slot, first, end).unwrap().map(|f| f.unwrap().content).collect();
    let mut decoder = DstDecoder::new();
    let equivalents = stored
        .iter()
        .map(|c| {
            let eq = c.dst_equivalent(channels).unwrap().into_owned();
            let mut out = vec![0; usize::from(channels) * FRAME_BYTES_PER_CHANNEL];
            let bad = matches!(c, FrameContent::Dst(b) if decoder.decode(b, channels, &mut out).is_err());
            if bad && invalid == InvalidDst::Replaced {
                let mut v = vec![0x55; 1 + out.len()];
                v[0] = 0;
                v
            } else {
                eq
            }
        })
        .collect();
    Positions { first, equivalents, content }
}

#[derive(Clone)]
struct Choices {
    crc: bool,
    index: Option<DstIndexOffset>,
    index_first: bool,
}

/// A DST DSDIFF file written sequentially from 8.4: `FVER`, `PROP` (`FS  `, `CHNL`, `CMPR`,
/// `LSCO`), then `DSTI` before or after the sound data.
fn sequential(p: &Positions, ids: &[&[u8; 4]], lsco: u16, c: &Choices) -> Vec<u8> {
    let mut prop = b"SND ".to_vec();
    chunk(&mut prop, b"FS  ", &2_822_400u32.to_be_bytes());
    let mut chnl = (ids.len() as u16).to_be_bytes().to_vec();
    ids.iter().for_each(|id| chnl.extend_from_slice(*id));
    chunk(&mut prop, b"CHNL", &chnl);
    let mut cmpr = b"DST ".to_vec();
    cmpr.push(11);
    cmpr.extend_from_slice(b"DST Encoded");
    chunk(&mut prop, b"CMPR", &cmpr);
    chunk(&mut prop, b"LSCO", &lsco.to_be_bytes());
    let mut head = b"DSD ".to_vec();
    chunk(&mut head, b"FVER", &[1, 5, 0, 0]);
    chunk(&mut head, b"PROP", &prop);

    let mut dst = Vec::new();
    let mut frte = (p.equivalents.len() as u32).to_be_bytes().to_vec();
    frte.extend_from_slice(&75u16.to_be_bytes());
    chunk(&mut dst, b"FRTE", &frte);
    let mut starts = Vec::new();
    for (eq, content) in p.equivalents.iter().zip(&p.content) {
        starts.push(dst.len());
        chunk(&mut dst, b"DSTF", eq);
        if c.crc {
            chunk(&mut dst, b"DSTC", &crc(content).to_be_bytes());
        }
    }
    let index_len = if c.index.is_some() { 12 + 12 * p.equivalents.len() } else { 0 };
    // Where the `DST ` chunk's data begins in the file: FRM8 header, then `head`, then the
    // index if it comes first, then the `DST ` chunk header.
    let dst_data = 12 + head.len() + if c.index_first { index_len } else { 0 } + 12;
    let mut index = Vec::new();
    if let Some(offset) = c.index {
        let shift = if offset == DstIndexOffset::FrameData { 12 } else { 0 };
        let mut entries = Vec::new();
        for (s, eq) in starts.iter().zip(&p.equivalents) {
            entries.extend_from_slice(&((dst_data + s + shift) as u64).to_be_bytes());
            entries.extend_from_slice(&(eq.len() as u32).to_be_bytes());
        }
        chunk(&mut index, b"DSTI", &entries);
    }
    let mut form = head;
    if c.index_first {
        form.extend_from_slice(&index);
    }
    chunk(&mut form, b"DST ", &dst);
    if !c.index_first {
        form.extend_from_slice(&index);
    }
    let mut f = Vec::new();
    chunk(&mut f, b"FRM8", &form);
    f
}

fn options(c: &Choices) -> OutputOptions {
    let order = if c.index_first {
        vec![LocalChunk::Property, LocalChunk::DstIndex, LocalChunk::SoundData]
    } else {
        LocalChunk::DEFAULT_ORDER.to_vec()
    };
    OutputOptions::default()
        .with_dsdiff_dst_crc(c.crc)
        .with_dsdiff_dst_index(c.index.is_some())
        .with_dsdiff_dst_index_offset(c.index.unwrap_or(DstIndexOffset::ChunkHeader))
        .with_dsdiff_local_order(order)
}

/// Decodes the `DSTF` frames of a file in order (8.4.6 "Decoding").
fn decode_file(file: &[u8], channels: u8) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut decoder = DstDecoder::new();
    let mut at = 16;
    let mut walk = |id: &[u8], data: &[u8]| {
        if id == b"DSTF" {
            let mut frame = vec![0; usize::from(channels) * FRAME_BYTES_PER_CHANNEL];
            if decoder.decode(data, channels, &mut frame).is_err() {
                frame.fill(0x55);
            }
            out.push(frame);
        }
    };
    fn each(bytes: &[u8], at: &mut usize, end: usize, f: &mut dyn FnMut(&[u8], &[u8])) {
        while *at < end {
            let id = &bytes[*at..*at + 4];
            let d = u64::from_be_bytes(bytes[*at + 4..*at + 12].try_into().unwrap()) as usize;
            let data = &bytes[*at + 12..*at + 12 + d];
            if id == b"DST " {
                let mut inner = *at + 12;
                each(bytes, &mut inner, *at + 12 + d, f);
            } else {
                f(id, data);
            }
            *at += 12 + d + d % 2;
        }
    }
    each(file, &mut at, file.len(), &mut walk);
    out
}

fn ids(channels: usize) -> Option<(&'static [&'static [u8; 4]], u16)> {
    match channels {
        2 => Some((&[b"SLFT", b"SRGT"], 0)),
        5 => Some((&[b"MLFT", b"MRGT", b"C   ", b"LS  ", b"RS  "], 3)),
        6 => Some((&[b"MLFT", b"MRGT", b"C   ", b"LFE ", b"LS  ", b"RS  "], 4)),
        _ => None,
    }
}

fn check_range<R: Read + Seek>(
    disc: &mut Disc<R>,
    slot: Slot,
    first: u32,
    end: u32,
    case: usize,
    rng: &mut Lcg,
) -> usize {
    let area = disc.toc().area(slot).unwrap().clone();
    let Some((ids, lsco)) = area.channel_speakers().and_then(|s| ids(s.len())) else { return 0 };
    let mut n = 0;
    for (invalid, c) in variants(
        &[
            (
                InvalidDst::Stored,
                Choices { crc: false, index: Some(DstIndexOffset::ChunkHeader), index_first: false },
            ),
            (
                InvalidDst::Replaced,
                Choices { crc: true, index: Some(DstIndexOffset::FrameData), index_first: true },
            ),
            (InvalidDst::Stored, Choices { crc: true, index: None, index_first: false }),
        ],
        case,
    ) {
        let lengths = DstLengths::scan(disc, slot, first, end, invalid).unwrap();
        let mut p = positions(disc, slot, first, end, invalid);
        if invalid == InvalidDst::Replaced {
            let stored = positions(disc, slot, first, end, InvalidDst::Stored);
            let replaced = stored.equivalents.iter().zip(&p.equivalents).filter(|(a, b)| a != b).count();
            REPLACED.fetch_add(replaced, Ordering::Relaxed);
        }
        assert_eq!(lengths.lengths(), p.equivalents.iter().map(|e| e.len() as u32).collect::<Vec<_>>());
        let layout = disc.output_layout_dst(&lengths, &options(&c)).unwrap();
        let expected = sequential(&p, ids, lsco, &c);
        assert_eq!(
            decode_file(&expected, area.channel_count),
            p.content,
            "the DSTF frames decode to the range's audio"
        );
        check_with(disc, &layout, &expected, &mut p, rng);
        n += 1;
    }
    n
}

#[test]
#[cfg_attr(miri, ignore = "the corpus is too large for Miri")]
fn corpus_layouts_equal_sequential_writes() {
    let mut rng = Lcg(86);
    let mut checked = 0;
    for (_name, bytes) in corpus() {
        let Ok(mut disc) = Disc::open(Cursor::new(bytes)) else { continue };
        for area in disc.toc().areas.clone() {
            for (first, end) in corpus_ranges(&disc, area.slot) {
                if let Err(e) = disc.check_range(area.slot, first, end) {
                    let s = DstLengths::scan(&mut disc, area.slot, first, end, InvalidDst::Stored);
                    assert_eq!(s.unwrap_err().to_string(), e.to_string());
                    continue;
                }
                checked += check_range(&mut disc, area.slot, first, end, checked, &mut rng);
            }
        }
    }
    assert!(checked > enough(), "only {checked} layouts checked");
    assert!(
        REPLACED.load(Ordering::Relaxed) > if sampled() { 0 } else { 10 },
        "too few invalid DST frames replaced"
    );
}

#[test]
fn lengths_are_checked() {
    let mut disc = synthetic(2, &plain(2, 3), vec![TrackSpec { start: 0, duration: 3 }], 3);
    let area = disc.toc().area(Slot::TwoChannel).unwrap().clone();
    let e =
        disc.output_layout(Slot::TwoChannel, 0, 3, Format::DsdiffDst, &OutputOptions::default()).unwrap_err();
    assert!(matches!(e, Error::InvalidArgument(_)), "{e}");

    let scanned = DstLengths::scan(&mut disc, Slot::TwoChannel, 0, 3, InvalidDst::Stored).unwrap();
    assert_eq!(scanned.lengths(), [9409; 3]);
    let loaded =
        DstLengths::from_lengths(&area, 0, 3, InvalidDst::Stored, scanned.lengths().to_vec()).unwrap();
    assert_eq!(loaded, scanned);
    for bad in [vec![9409; 2], vec![9409, 0, 9409], vec![9409, 65_537, 9409]] {
        let e = DstLengths::from_lengths(&area, 0, 3, InvalidDst::Stored, bad).unwrap_err();
        assert!(matches!(e, Error::InvalidArgument(_)), "{e}");
    }
    // Lengths of another range, and lengths that disagree with the image.
    let other = DstLengths::from_lengths(&area, 0, 2, InvalidDst::Stored, vec![9409; 2]).unwrap();
    let layout = disc.output_layout_dst(&other, &OutputOptions::default()).unwrap();
    assert_eq!(layout.range(), (0, 2));
    let wrong = DstLengths::from_lengths(&area, 0, 3, InvalidDst::Stored, vec![100; 3]).unwrap();
    let layout = disc.output_layout_dst(&wrong, &OutputOptions::default()).unwrap();
    let mut buf = vec![0; layout.size() as usize];
    let e = layout.read_at(&mut disc, 0, &mut buf).unwrap_err();
    assert!(matches!(e, Error::InvalidArgument(_)), "{e}");
    // A local order without the sound data, or with it before `PROP`.
    for order in [
        vec![LocalChunk::Property, LocalChunk::DstIndex],
        vec![LocalChunk::SoundData, LocalChunk::Property, LocalChunk::DstIndex],
    ] {
        let e = disc
            .output_layout_dst(&scanned, &OutputOptions::default().with_dsdiff_local_order(order))
            .unwrap_err();
        assert!(matches!(e, Error::InvalidArgument(_)), "{e}");
    }
    // Without an index, the order needn't name it.
    let o = OutputOptions::default()
        .with_dsdiff_dst_index(false)
        .with_dsdiff_local_order(vec![LocalChunk::Property, LocalChunk::SoundData]);
    disc.output_layout_dst(&scanned, &o).unwrap();
    // A content source can't fill a DST layout.
    let layout: Layout = disc.output_layout_dst(&scanned, &OutputOptions::default()).unwrap();
    let mut held = output_common::Held { first: 0, content: &[], asked: Vec::new() };
    let e = layout.fill(0, &mut vec![0; layout.size() as usize], &mut held).unwrap_err();
    assert!(matches!(e, Error::InvalidArgument(_)), "{e}");
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

    /// Stereo areas of plain frames, DST frames of a few arbitrary bytes (mostly invalid)
    /// and missing positions: any range, every choice.
    #[test]
    fn synthetic_ranges(kinds in proptest::collection::vec(0u8..3, 1..if cfg!(miri) { 3 } else { 6 }), a in 0u32..6, len in 1u32..6, seed in any::<u64>()) {
        let n = kinds.len() as u32;
        let mut frames = Vec::new();
        for (number, kind) in kinds.iter().enumerate() {
            let number = number as u32;
            match kind {
                0 => frames.push(plain(2, number + 1).pop().unwrap()),
                1 => frames.push(PFrame { number, dst: true, data: (0..(number as u8 * 17 + 5)).collect() }),
                _ => {}
            }
        }
        let first = a % n;
        let end = (first + len).min(n);
        let mut disc = synthetic(2, &frames, vec![TrackSpec { start: 0, duration: n }], n);
        prop_assert!(check_range(&mut disc, Slot::TwoChannel, first, end, seed as usize, &mut Lcg(seed)) == variants(&[0; 3], 0).len());
    }
}
