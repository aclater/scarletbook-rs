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
        ProptestConfig { cases: 128, ..ProptestConfig::default() }
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
