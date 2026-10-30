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
