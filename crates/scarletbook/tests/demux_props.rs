// SPDX-License-Identifier: MIT OR Apache-2.0

//! Property tests of audio sector demultiplexing and track timelines.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;
mod harness;

use std::io::Cursor;

use common::{PFrame, Spec, TrackSpec, pack};
use proptest::prelude::*;
use scarletbook::audio::FRAME_BYTES_PER_CHANNEL;
use scarletbook::{Demuxer, Disc, FrameContent, FrameKind, Slot};

fn config() -> ProptestConfig {
    if cfg!(miri) {
        ProptestConfig { cases: 2, failure_persistence: None, ..ProptestConfig::default() }
    } else {
        // PROPTEST_CASES, if set, overrides the default number of cases.
        let cases = std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(128);
        ProptestConfig { cases, ..ProptestConfig::default() }
    }
}

/// Frames with strictly increasing numbers; DST frames of 1 to 12 000 bytes, plain frames
/// of exactly C x 4704 bytes.
fn frames(channels: u8) -> impl Strategy<Value = Vec<PFrame>> {
    let plain = usize::from(channels) * FRAME_BYTES_PER_CHANNEL;
    proptest::collection::vec(
        (0u32..4, any::<bool>(), 1usize..12_000, any::<u8>()),
        0..if cfg!(miri) { 3 } else { 12 },
    )
    .prop_map(move |v| {
        let mut n = 0;
        v.into_iter()
            .map(|(gap, dst, len, seed)| {
                n += gap;
                let len = if dst { len } else { plain };
                let data = (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect();
                let f = PFrame { number: n, dst, data };
                n += 1;
                f
            })
            .collect()
    })
}

fn demux_all(channels: u8, sectors: &[[u8; 2048]]) -> Vec<PFrame> {
    let mut d = Demuxer::new(channels);
    let mut out = Vec::new();
    for s in sectors {
        assert!(!d.push_sector(s), "packer produced a malformed sector");
        while let Some(f) = d.pop() {
            out.push(PFrame { number: f.number, dst: f.kind == FrameKind::Dst, data: f.data });
        }
    }
    d.finish();
    while let Some(f) = d.pop() {
        out.push(PFrame { number: f.number, dst: f.kind == FrameKind::Dst, data: f.data });
    }
    out
}

proptest! {
    #![proptest_config(config())]

    /// Packing frames into sectors and demultiplexing them gives the frames back.
    #[test]
    fn pack_then_demux_round_trips((channels, fs) in (1u8..=2).prop_flat_map(|c| (Just(c), frames(c)))) {
        let sectors = pack(&fs);
        prop_assert_eq!(demux_all(channels, &sectors), fs);
    }

    /// Arbitrary sector bytes: invariants of the harness hold.
    #[test]
    #[cfg_attr(miri, ignore = "slow under Miri")]
    fn arbitrary_sectors(channels in any::<u8>(), data in proptest::collection::vec(any::<u8>(), 0..8 * 2048)) {
        let mut input = vec![channels];
        input.extend_from_slice(&data);
        harness::demux(&input);
    }

    /// A track's timeline holds exactly its accepted frames, with gaps missing.
    #[test]
    #[cfg_attr(miri, ignore = "slow under Miri")]
    fn track_timeline(fs in frames(2), start in 0u32..20, duration in 1u32..20) {
        let sectors = pack(&fs);
        let spec = Spec {
            channels: 2,
            playtime: start + duration,
            tracks: vec![TrackSpec { start, duration }],
            audio: if sectors.is_empty() { vec![[0u8; 2048]] } else { sectors },
        };
        let capacity = 7 * spec.audio.len() as u32;
        let mut disc = Disc::open(Cursor::new(spec.build())).unwrap();
        let result = disc.track_frames(Slot::TwoChannel, 1);
        if start + duration > capacity {
            prop_assert!(result.is_err());
            return Ok(());
        }
        let got: Vec<_> = result.unwrap().map(Result::unwrap).collect();
        prop_assert_eq!(got.len() as u32, duration);
        for (i, tf) in got.iter().enumerate() {
            let t = start + i as u32;
            prop_assert_eq!(tf.number, t);
            match fs.iter().find(|f| f.number == t) {
                Some(f) if f.dst => prop_assert_eq!(&tf.content, &FrameContent::Dst(f.data.clone())),
                Some(f) => prop_assert_eq!(&tf.content, &FrameContent::Dsd(f.data.clone())),
                None => prop_assert_eq!(&tf.content, &FrameContent::Missing),
            }
        }
    }
}

/// The missing-frame count follows the frames returned, with or without `parallel`
/// (whose batches decode ahead of the caller).
#[test]
fn counts_follow_returned_frames() {
    let spec = Spec {
        channels: 2,
        playtime: 12,
        tracks: vec![TrackSpec { start: 0, duration: 12 }],
        audio: vec![[0u8; 2048]; 2],
    };
    let mut disc = Disc::open(Cursor::new(spec.build())).unwrap();
    let mut audio = disc.track_audio(Slot::TwoChannel, 1).unwrap();
    for returned in 1..=12u64 {
        let frame = audio.next().unwrap().unwrap();
        assert_eq!(frame.status, scarletbook::FrameStatus::Missing);
        assert_eq!(audio.missing_frames(), returned);
    }
    assert!(audio.next().is_none());
    assert_eq!(audio.missing_frames(), 12);
}

/// A reader that counts the bytes read through it.
struct Counting<R> {
    inner: R,
    read: std::rc::Rc<std::cell::Cell<u64>>,
}

impl<R: std::io::Read> std::io::Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read.set(self.read.get() + n as u64);
        Ok(n)
    }
}

impl<R: std::io::Seek> std::io::Seek for Counting<R> {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// Three consecutive tracks over 12 plain frames.
fn three_track_disc() -> (Vec<u8>, usize) {
    let frames: Vec<PFrame> = (0..12u32)
        .map(|n| PFrame { number: n, dst: false, data: vec![n as u8; 2 * FRAME_BYTES_PER_CHANNEL] })
        .collect();
    let audio = pack(&frames);
    let sectors = audio.len();
    let spec = Spec {
        channels: 2,
        playtime: 12,
        tracks: vec![
            TrackSpec { start: 0, duration: 4 },
            TrackSpec { start: 4, duration: 4 },
            TrackSpec { start: 8, duration: 4 },
        ],
        audio,
    };
    (spec.build(), sectors)
}

fn decode_all<R: std::io::Read + std::io::Seek>(
    disc: &mut Disc<R>,
    cursor: scarletbook::TrackAudioCursor,
) -> (Vec<scarletbook::AudioFrame>, scarletbook::TrackAudioCursor) {
    let mut audio = disc.read_track_audio(cursor);
    let frames = audio.by_ref().map(Result::unwrap).collect();
    (frames, audio.into_cursor())
}

#[test]
#[cfg_attr(miri, ignore = "decodes several tracks")]
fn continuing_reads_each_audio_sector_once() {
    let (img, audio_sectors) = three_track_disc();
    let read = std::rc::Rc::new(std::cell::Cell::new(0));
    let mut disc = Disc::open(Counting { inner: Cursor::new(img.clone()), read: read.clone() }).unwrap();

    // Fresh cursors: each track rescans from the area's first sector.
    let start = read.get();
    let mut fresh = Vec::new();
    for n in 1..=3 {
        let c = disc.track_audio_cursor(Slot::TwoChannel, n).unwrap();
        fresh.push(decode_all(&mut disc, c).0);
    }
    let fresh_bytes = read.get() - start;

    // One cursor continued from track to track.
    let start = read.get();
    let mut continued = Vec::new();
    let mut cursor = disc.track_audio_cursor(Slot::TwoChannel, 1).unwrap();
    for n in 1..=3 {
        if n > 1 {
            cursor = disc.continue_track_audio(cursor, Slot::TwoChannel, n).unwrap();
        }
        let (frames, back) = decode_all(&mut disc, cursor);
        continued.push(frames);
        cursor = back;
    }
    let continued_bytes = read.get() - start;

    assert_eq!(continued, fresh);
    assert!(continued.iter().flatten().all(|f| f.status == scarletbook::FrameStatus::Dsd));
    assert_eq!(continued_bytes, (audio_sectors * 2048) as u64, "each audio sector read exactly once");
    assert!(fresh_bytes > continued_bytes);
}

#[test]
#[cfg_attr(miri, ignore = "decodes several tracks")]
fn continuing_backwards_rescans_with_the_same_result() {
    let (img, _) = three_track_disc();
    let mut disc = Disc::open(Cursor::new(img)).unwrap();
    let c3 = disc.track_audio_cursor(Slot::TwoChannel, 3).unwrap();
    let (_, after3) = decode_all(&mut disc, c3);
    // Track 1 starts before the cursor's position: a fresh scan, same frames.
    let c1 = disc.continue_track_audio(after3, Slot::TwoChannel, 1).unwrap();
    let (back, _) = decode_all(&mut disc, c1);
    let fresh1 = disc.track_audio_cursor(Slot::TwoChannel, 1).unwrap();
    assert_eq!(back, decode_all(&mut disc, fresh1).0);
}

/// A sector whose single packet runs past its end (specification 4, 4.4.3 item 1).
fn overrun_sector() -> [u8; 2048] {
    let mut s = [0u8; 2048];
    s[0] = 1 << 5; // one packet record, no frame records
    s[1..3].copy_from_slice(&((2u16 << 11) | 2047).to_be_bytes()); // audio, 2047 bytes from offset 3
    s
}

#[test]
fn malformed_sector_is_reported_with_its_lsn() {
    let mut audio = vec![[0u8; 2048]; 3];
    audio[1] = overrun_sector();
    let spec = Spec { channels: 2, playtime: 1, tracks: vec![TrackSpec { start: 0, duration: 1 }], audio };
    let mut disc = Disc::open(Cursor::new(spec.build())).unwrap();
    let mut frames = disc.area_frames(Slot::TwoChannel).unwrap();
    assert!(frames.by_ref().all(|f| f.is_ok()));
    assert_eq!(frames.malformed_sectors(), 1);
    let w = frames.warnings();
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].code, scarletbook::WarningCode::AudioSectorMalformed);
    assert_eq!(w[0].slot, Some(Slot::TwoChannel));
    assert_eq!(w[0].lsn, Some(common::AUDIO_LSN as u32 + 1));
    assert_eq!(w[0].track, None);
}

#[test]
#[cfg_attr(miri, ignore = "processes thousands of sectors")]
fn malformed_warnings_are_capped_with_a_full_count() {
    let n = scarletbook::audio::MAX_MALFORMED_WARNINGS + 4;
    let spec = Spec {
        channels: 2,
        playtime: 1,
        tracks: vec![TrackSpec { start: 0, duration: 1 }],
        audio: vec![overrun_sector(); n],
    };
    let mut disc = Disc::open(Cursor::new(spec.build())).unwrap();
    let mut frames = disc.area_frames(Slot::TwoChannel).unwrap();
    frames.by_ref().for_each(drop);
    assert_eq!(frames.malformed_sectors(), n as u64);
    assert_eq!(frames.warnings().len(), scarletbook::audio::MAX_MALFORMED_WARNINGS);
    // The kept warnings are the first ones, in processing order.
    assert_eq!(frames.warnings()[0].lsn, Some(common::AUDIO_LSN as u32));
}

/// Specification 4, 4.4.5 item 1: a reader that stops early does not warn for sectors it
/// did not process.
#[test]
#[cfg_attr(miri, ignore = "decodes several frames")]
fn early_stop_does_not_report_unprocessed_sectors() {
    let frames: Vec<PFrame> = (0..3u32)
        .map(|n| PFrame { number: n, dst: false, data: vec![7; 2 * FRAME_BYTES_PER_CHANNEL] })
        .collect();
    let mut audio = pack(&frames);
    audio.push(overrun_sector());
    let spec = Spec { channels: 2, playtime: 3, tracks: vec![TrackSpec { start: 0, duration: 1 }], audio };
    let mut disc = Disc::open(Cursor::new(spec.build())).unwrap();
    let mut track = disc.track_audio(Slot::TwoChannel, 1).unwrap();
    assert_eq!(track.by_ref().count(), 1);
    assert_eq!(track.malformed_sectors(), 0);
    assert!(track.warnings().is_empty());
    // A whole-area scan does reach it.
    let mut all = disc.area_frames(Slot::TwoChannel).unwrap();
    all.by_ref().for_each(drop);
    assert_eq!(all.warnings().len(), 1);
}

/// Decodes up to `limit` frames of a cursor (all if `None`), returning them and the cursor.
fn take_frames<R: std::io::Read + std::io::Seek>(
    disc: &mut Disc<R>,
    cursor: scarletbook::TrackAudioCursor,
    limit: Option<usize>,
) -> (Vec<scarletbook::AudioFrame>, scarletbook::TrackAudioCursor) {
    let mut audio = disc.read_track_audio(cursor);
    let frames = audio.by_ref().take(limit.unwrap_or(usize::MAX)).map(Result::unwrap).collect();
    (frames, audio.into_cursor())
}

/// A disc of 40 frames' play time with the given frames and tracks; padded so that the
/// area's capacity (7 frames per sector) covers every track.
fn timeline_disc(fs: &[PFrame], tracks: &[(u32, u32)]) -> Vec<u8> {
    let mut audio = pack(fs);
    audio.resize(audio.len().max(6), [0u8; 2048]);
    Spec {
        channels: 2,
        playtime: 40,
        tracks: tracks.iter().map(|&(start, duration)| TrackSpec { start, duration }).collect(),
        audio,
    }
    .build()
}

proptest! {
    #![proptest_config(config())]

    /// Continuing one cursor through tracks in any order (gaps, overlaps, zero-length,
    /// backwards), stopping each after some frames, gives exactly the frames of a fresh
    /// cursor per track.
    #[test]
    #[cfg_attr(miri, ignore = "decodes many frames")]
    fn continuation_equals_fresh_scans(
        fs in frames(2),
        tracks in proptest::collection::vec((0u32..30, 0u32..10), 1..6),
        stops in proptest::collection::vec(proptest::option::of(0usize..6), 6),
    ) {
        let mut disc = Disc::open(Cursor::new(timeline_disc(&fs, &tracks))).unwrap();
        let mut cursor: Option<scarletbook::TrackAudioCursor> = None;
        for (i, stop) in (1..=tracks.len() as u32).zip(stops) {
            if disc.check_track(Slot::TwoChannel, i).is_err() {
                continue;
            }
            let fresh_cursor = disc.track_audio_cursor(Slot::TwoChannel, i).unwrap();
            let (fresh, _) = take_frames(&mut disc, fresh_cursor, stop);
            let next = match cursor.take() {
                Some(prev) => disc.continue_track_audio(prev, Slot::TwoChannel, i).unwrap(),
                None => disc.track_audio_cursor(Slot::TwoChannel, i).unwrap(),
            };
            let (continued, back) = take_frames(&mut disc, next, stop);
            prop_assert_eq!(continued, fresh, "track {}", i);
            cursor = Some(back);
        }
    }
}

/// Both master TOC slots point at the same area TOC (valid, specification 3.1): two areas
/// with the same audio range, which a cursor must still treat as different areas.
#[test]
#[cfg_attr(miri, ignore = "decodes several tracks")]
fn continuation_into_another_area_rescans() {
    let fs: Vec<PFrame> = (0..8u32)
        .map(|n| PFrame { number: n, dst: false, data: vec![n as u8; 2 * FRAME_BYTES_PER_CHANNEL] })
        .collect();
    let mut img = timeline_disc(&fs, &[(0, 4), (4, 4)]);
    for lsn in [510usize, 520, 530] {
        img[lsn * 2048 + 72..lsn * 2048 + 76].copy_from_slice(&540u32.to_be_bytes());
        img[lsn * 2048 + 86..lsn * 2048 + 88].copy_from_slice(&6u16.to_be_bytes());
    }
    let mut disc = Disc::open(Cursor::new(img)).unwrap();
    assert_eq!(disc.toc().areas.len(), 2);
    let c = disc.track_audio_cursor(Slot::TwoChannel, 2).unwrap();
    let (_, after) = take_frames(&mut disc, c, None);
    // Track 1 of the other slot starts at frame 0, before the cursor: but even a later
    // start in another area must not reuse the state; compare with a fresh cursor.
    let continued = disc.continue_track_audio(after, Slot::Multichannel, 1).unwrap();
    let (got, _) = take_frames(&mut disc, continued, None);
    let fresh = disc.track_audio_cursor(Slot::Multichannel, 1).unwrap();
    assert_eq!(got, take_frames(&mut disc, fresh, None).0);
}

/// A reader that fails every read while `fail` is set.
struct Flaky {
    inner: Cursor<Vec<u8>>,
    fail: std::rc::Rc<std::cell::Cell<bool>>,
}

impl std::io::Read for Flaky {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.fail.get() {
            return Err(std::io::Error::other("injected failure"));
        }
        self.inner.read(buf)
    }
}

impl std::io::Seek for Flaky {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

#[test]
#[cfg_attr(miri, ignore = "decodes several tracks")]
fn continuation_after_a_read_error_rescans() {
    let fs: Vec<PFrame> = (0..8u32)
        .map(|n| PFrame { number: n, dst: false, data: vec![n as u8; 2 * FRAME_BYTES_PER_CHANNEL] })
        .collect();
    let fail = std::rc::Rc::new(std::cell::Cell::new(false));
    let img = timeline_disc(&fs, &[(0, 4), (4, 4)]);
    let mut disc = Disc::open(Flaky { inner: Cursor::new(img), fail: fail.clone() }).unwrap();
    let c = disc.track_audio_cursor(Slot::TwoChannel, 1).unwrap();
    fail.set(true);
    let mut audio = disc.read_track_audio(c);
    assert!(matches!(audio.next(), Some(Err(scarletbook::Error::Io(_)))));
    let failed = audio.into_cursor();
    fail.set(false);
    let continued = disc.continue_track_audio(failed, Slot::TwoChannel, 2).unwrap();
    let (got, _) = take_frames(&mut disc, continued, None);
    let fresh = disc.track_audio_cursor(Slot::TwoChannel, 2).unwrap();
    let (want, _) = take_frames(&mut disc, fresh, None);
    assert_eq!(got, want);
    assert!(got.iter().all(|f| f.status == scarletbook::FrameStatus::Dsd));
}

#[test]
#[cfg_attr(miri, ignore = "decodes several tracks")]
fn a_cursor_never_reads_another_image() {
    // Two discs with the same layout but different audio.
    let make = |fill: u8| {
        let fs: Vec<PFrame> = (0..8u32)
            .map(|n| PFrame {
                number: n,
                dst: false,
                data: vec![fill ^ n as u8; 2 * FRAME_BYTES_PER_CHANNEL],
            })
            .collect();
        Disc::open(Cursor::new(timeline_disc(&fs, &[(0, 4), (4, 4)]))).unwrap()
    };
    let (mut x, mut y) = (make(0x10), make(0x20));
    let cx = x.track_audio_cursor(Slot::TwoChannel, 1).unwrap();
    let (_, after) = take_frames(&mut x, cx, None);
    // Through Disc: the other disc is detected and scanned afresh.
    let continued = y.continue_track_audio(after, Slot::TwoChannel, 2).unwrap();
    let (got, _) = take_frames(&mut y, continued, None);
    let fresh = y.track_audio_cursor(Slot::TwoChannel, 2).unwrap();
    assert_eq!(got, take_frames(&mut y, fresh, None).0);
    // Through the cursor alone (which cannot see images): reading another image is refused.
    let cx = x.track_audio_cursor(Slot::TwoChannel, 1).unwrap();
    let (_, after) = take_frames(&mut x, cx, None);
    let area = y.toc().areas[0].clone();
    let raw = after.continue_to(&area, &area.tracks[1], y.toc().sectors).unwrap();
    let mut audio = y.read_track_audio(raw);
    assert!(matches!(audio.next(), Some(Err(scarletbook::Error::InvalidArgument(_)))));
}
