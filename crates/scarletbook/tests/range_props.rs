// SPDX-License-Identifier: MIT OR Apache-2.0

//! Decoded audio of arbitrary frame ranges of an area: a track is one such range.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::io::Cursor;

use common::{PFrame, Spec, TrackSpec, pack};
use proptest::prelude::*;
use scarletbook::audio::FRAME_BYTES_PER_CHANNEL;
use scarletbook::{AudioFrame, Disc, Error, FrameStatus, Slot, TrackAudioCursor};

fn config() -> ProptestConfig {
    if cfg!(miri) {
        ProptestConfig { cases: 2, failure_persistence: None, ..ProptestConfig::default() }
    } else {
        let cases = std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(128);
        ProptestConfig { cases, ..ProptestConfig::default() }
    }
}

const PLAYTIME: u32 = 40;

/// Plain and DST frames with strictly increasing numbers below the play time. The DST
/// frames are a few bytes of anything: most are invalid, some decode.
fn frames() -> impl Strategy<Value = Vec<PFrame>> {
    proptest::collection::vec((0u32..4, any::<bool>(), 1usize..64, any::<u8>()), 0..12).prop_map(|v| {
        let mut n = 0;
        let mut out = Vec::new();
        for (gap, dst, len, seed) in v {
            n += gap;
            if n >= PLAYTIME {
                break;
            }
            let len = if dst { len } else { 2 * FRAME_BYTES_PER_CHANNEL };
            let data = (0..len).map(|i| (i as u8).wrapping_mul(37).wrapping_add(seed)).collect();
            out.push(PFrame { number: n, dst, data });
            n += 1;
        }
        out
    })
}

/// A 2-channel disc of `PLAYTIME` frames whose capacity covers the whole play time.
fn disc(fs: &[PFrame], tracks: &[(u32, u32)]) -> Disc<Cursor<Vec<u8>>> {
    let mut audio = pack(fs);
    audio.resize(audio.len().max(PLAYTIME.div_ceil(7) as usize), [0u8; 2048]);
    let spec = Spec {
        channels: 2,
        playtime: PLAYTIME,
        tracks: tracks.iter().map(|&(start, duration)| TrackSpec { start, duration }).collect(),
        audio,
    };
    Disc::open(Cursor::new(spec.build())).unwrap()
}

fn read_all<R: std::io::Read + std::io::Seek>(
    disc: &mut Disc<R>,
    cursor: TrackAudioCursor,
) -> (Vec<AudioFrame>, TrackAudioCursor) {
    let mut audio = disc.read_track_audio(cursor);
    let frames = audio.by_ref().map(Result::unwrap).collect();
    (frames, audio.into_cursor())
}

fn range(disc: &mut Disc<Cursor<Vec<u8>>>, first: u32, end: u32) -> Vec<AudioFrame> {
    disc.range_audio(Slot::TwoChannel, first, end).unwrap().map(Result::unwrap).collect()
}

#[test]
#[cfg_attr(miri, ignore = "decodes a whole area")]
fn whole_area_is_tracks_plus_pauses() {
    let fs: Vec<PFrame> = (0..PLAYTIME)
        .filter(|n| n % 5 != 3)
        .map(|n| PFrame { number: n, dst: false, data: vec![n as u8; 2 * FRAME_BYTES_PER_CHANNEL] })
        .collect();
    // A lead-in (0..2), pauses (10..12, 20..25) and a tail after the last track (35..40).
    let tracks = [(2, 8), (12, 8), (25, 10)];
    let mut d = disc(&fs, &tracks);
    let mut whole = d.range_audio(Slot::TwoChannel, 0, PLAYTIME).unwrap();
    let all: Vec<AudioFrame> = whole.by_ref().map(Result::unwrap).collect();
    assert_eq!(whole.missing_frames(), 8);
    assert_eq!(all.iter().map(|f| f.number).collect::<Vec<_>>(), (0..PLAYTIME).collect::<Vec<_>>());
    let mut pieces = range(&mut d, 0, 2);
    for (i, &(s, len)) in tracks.iter().enumerate() {
        pieces.extend(d.track_audio(Slot::TwoChannel, i as u32 + 1).unwrap().map(Result::unwrap));
        let next = tracks.get(i + 1).map_or(PLAYTIME, |t| t.0);
        pieces.extend(range(&mut d, s + len, next));
    }
    assert_eq!(pieces, all);
    // The pauses hold what the disc has: plain frames, or the missing fill.
    assert_eq!(all[11].status, FrameStatus::Dsd);
    assert!(all[11].data.iter().all(|&b| b == 11));
    assert_eq!(all[23].status, FrameStatus::Missing);
    assert!(all[23].data.iter().all(|&b| b == 0x69));
}

#[test]
fn range_bounds_and_errors() {
    let mut d = disc(&[], &[(0, 4)]);
    let invalid = |e| matches!(e, Error::FrameRangeInvalid { playtime: PLAYTIME, .. });
    for (first, end) in
        [(8, 3), (0, PLAYTIME + 1), (PLAYTIME, PLAYTIME + 1), (0, u32::MAX), (u32::MAX, u32::MAX)]
    {
        let e = d.check_range(Slot::TwoChannel, first, end).unwrap_err();
        assert!(invalid(e), "[{first}, {end})");
        assert!(d.range_audio_cursor(Slot::TwoChannel, first, end).is_err());
    }
    // An empty range in the play range is skipped as an empty track is (4.7.1 item 4).
    for (first, end) in [(0, 0), (7, 7), (PLAYTIME, PLAYTIME)] {
        let e = d.check_range(Slot::TwoChannel, first, end).unwrap_err();
        assert!(matches!(e, Error::TrackEmpty), "[{first}, {end}): {e:?}");
    }
    let e = d.check_range(Slot::TwoChannel, 3, 1).unwrap_err();
    assert_eq!(e.class(), scarletbook::ErrorClass::Usage);
    assert_eq!(e.code(), None);
    assert!(matches!(d.check_range(Slot::Multichannel, 0, 1), Err(Error::NoSuchArea(Slot::Multichannel))));
    // The first and last positions of the timeline, and the whole of it.
    for (first, end) in [(0, 1), (PLAYTIME - 1, PLAYTIME), (0, PLAYTIME)] {
        d.check_range(Slot::TwoChannel, first, end).unwrap();
        let got = range(&mut d, first, end);
        assert_eq!(got.len() as u32, end - first);
        assert_eq!((got[0].number, got.last().unwrap().number), (first, end - 1));
    }

    // Capacity: 2 audio sectors hold at most 14 frames, whatever the play time.
    let spec = Spec {
        channels: 2,
        playtime: 20,
        tracks: vec![TrackSpec { start: 0, duration: 1 }],
        audio: vec![[0; 2048]; 2],
    };
    let mut small = Disc::open(Cursor::new(spec.build())).unwrap();
    small.check_range(Slot::TwoChannel, 0, 14).unwrap();
    let e = small.check_range(Slot::TwoChannel, 10, 15).unwrap_err();
    assert!(matches!(e, Error::TrackExceedsCapacity { end_frame: 15, capacity: 14 }), "{e:?}");
    assert_eq!(e.code(), Some("track-exceeds-capacity"));
    // A range that fails its checks consumes the cursor; a good one continues it.
    let c = small.range_audio_cursor(Slot::TwoChannel, 0, 2).unwrap();
    assert!(small.continue_range_audio(c, Slot::TwoChannel, 0, 15).is_err());
    let c = small.range_audio_cursor(Slot::TwoChannel, 0, 2).unwrap();
    let (got, c) = read_all(&mut small, c);
    assert_eq!(got.len(), 2);
    let c = small.continue_range_audio(c, Slot::TwoChannel, 2, 14).unwrap();
    assert_eq!(read_all(&mut small, c).0.len(), 12);

    // Channel count first, as for tracks (specification 4, 4.8 item 1).
    let spec = Spec {
        channels: 0,
        playtime: 4,
        tracks: vec![TrackSpec { start: 0, duration: 1 }],
        audio: vec![[0; 2048]; 1],
    };
    let none = Disc::open(Cursor::new(spec.build())).unwrap();
    assert!(matches!(
        none.check_range(Slot::TwoChannel, 3, 1),
        Err(Error::ChannelCountUnsupported { channels: 0 })
    ));
}

proptest! {
    #![proptest_config(config())]

    /// Any range of the timeline is that slice of the whole area, a track is the range of
    /// its own frames, and continuing a cursor through ranges (and tracks) in any order
    /// gives what fresh cursors give.
    #[test]
    #[cfg_attr(miri, ignore = "decodes many frames")]
    fn ranges_are_slices_of_the_area(
        fs in frames(),
        tracks in proptest::collection::vec((0u32..PLAYTIME, 1u32..10), 1..4),
        ranges in proptest::collection::vec((0u32..PLAYTIME, 1u32..=PLAYTIME, proptest::option::of(0usize..6)), 1..6),
    ) {
        let tracks: Vec<(u32, u32)> = tracks.into_iter().map(|(s, d)| (s, d.min(PLAYTIME - s))).collect();
        let mut d = disc(&fs, &tracks);
        let whole = range(&mut d, 0, PLAYTIME);
        prop_assert_eq!(whole.len() as u32, PLAYTIME);
        for (i, &(s, len)) in tracks.iter().enumerate() {
            let got: Vec<AudioFrame> =
                d.track_audio(Slot::TwoChannel, i as u32 + 1).unwrap().map(Result::unwrap).collect();
            prop_assert_eq!(&got[..], &whole[s as usize..(s + len) as usize]);
        }
        let mut cursor: Option<TrackAudioCursor> = None;
        for (a, b, stop) in ranges {
            let (first, end) = (a.min(b), a.max(b));
            if first == end {
                prop_assert!(matches!(d.check_range(Slot::TwoChannel, first, end), Err(Error::TrackEmpty)));
                continue;
            }
            let fresh = range(&mut d, first, end);
            prop_assert_eq!(&fresh[..], &whole[first as usize..end as usize]);
            let next = match cursor.take() {
                Some(prev) => d.continue_range_audio(prev, Slot::TwoChannel, first, end).unwrap(),
                None => d.range_audio_cursor(Slot::TwoChannel, first, end).unwrap(),
            };
            let mut audio = d.read_track_audio(next);
            let got: Vec<AudioFrame> = audio.by_ref().take(stop.unwrap_or(usize::MAX)).map(Result::unwrap).collect();
            let n = got.len();
            prop_assert_eq!(&got[..], &fresh[..n]);
            let missing = got.iter().filter(|f| f.status == FrameStatus::Missing).count() as u64;
            let invalid = got.iter().filter(|f| matches!(f.status, FrameStatus::Invalid(_))).count() as u64;
            prop_assert_eq!((audio.missing_frames(), audio.invalid_dst_frames()), (missing, invalid));
            cursor = Some(audio.into_cursor());
        }
    }
}

/// Specification 4, 4.7.1 item 5, including tracks out of order, overlapping and of
/// duration 0.
#[test]
fn tracks_with_pauses() {
    type Ranges = &'static [(u32, u32)];
    let cases: [(Ranges, Ranges); 4] = [
        (&[(2, 8), (12, 8), (25, 10)], &[(0, 12), (12, 25), (25, PLAYTIME)]),
        (&[(10, 5), (0, 4), (20, 3)], &[(0, 15), (0, 20), (20, PLAYTIME)]),
        (&[(0, 10), (5, 3)], &[(0, 10), (5, PLAYTIME)]),
        (&[(0, 4), (4, 0), (4, 6)], &[(0, 4), (4, 4), (4, PLAYTIME)]),
    ];
    for (tracks, expected) in cases {
        let mut d = disc(&[], tracks);
        for (i, &want) in expected.iter().enumerate() {
            let n = i as u32 + 1;
            assert_eq!(d.track_range_with_pause(Slot::TwoChannel, n).unwrap(), want, "{tracks:?} track {n}");
            let (first, end) = want;
            if first == end {
                assert!(matches!(d.range_audio(Slot::TwoChannel, first, end), Err(Error::TrackEmpty)));
            } else {
                assert_eq!(range(&mut d, first, end).len() as u32, end - first);
            }
        }
        let n = expected.len() as u32 + 1;
        assert!(matches!(d.track_range_with_pause(Slot::TwoChannel, n), Err(Error::NoSuchTrack { .. })));
        assert!(matches!(d.track_range_with_pause(Slot::Multichannel, 1), Err(Error::NoSuchArea(_))));
    }
}

/// DST frames of several sectors each: every range, whatever sector its edges fall in, is
/// that slice of the whole area.
#[test]
#[cfg_attr(miri, ignore = "decodes many ranges")]
fn dst_frames_spanning_sectors_at_range_edges() {
    let fs: Vec<PFrame> = (0..10u32)
        .filter(|&n| n != 4)
        .map(|n| {
            let len = 2500 + 300 * n as usize;
            PFrame { number: n, dst: true, data: (0..len).map(|i| (i as u8) ^ (n as u8 * 17)).collect() }
        })
        .collect();
    let sectors = pack(&fs);
    assert!(sectors.len() > fs.len(), "frames span sectors");
    let mut d = disc(&fs, &[(0, 10)]);
    let whole = range(&mut d, 0, PLAYTIME);
    assert_eq!(whole[4].status, FrameStatus::Missing);
    for first in 0..12 {
        for end in first + 1..=12 {
            assert_eq!(range(&mut d, first, end), whole[first as usize..end as usize], "[{first}, {end})");
        }
    }
}

/// A sector whose single packet runs past its end (specification 4, 4.4.3 item 1).
fn overrun_sector() -> [u8; 2048] {
    let mut s = [0u8; 2048];
    s[0] = 1 << 5;
    s[1..3].copy_from_slice(&((2u16 << 11) | 2047).to_be_bytes());
    s
}

/// A malformed sector just before the sector where frame 3 starts: ranges that end before
/// frame 2 is complete do not reach it; ranges that need frame 2 or later report it.
#[test]
#[cfg_attr(miri, ignore = "decodes several ranges")]
fn malformed_sector_at_a_range_edge() {
    let plain = |n: u32| PFrame { number: n, dst: false, data: vec![n as u8; 2 * FRAME_BYTES_PER_CHANNEL] };
    let head: Vec<PFrame> = (0..3).map(plain).collect();
    let tail: Vec<PFrame> = (3..6).map(plain).collect();
    let mut audio = pack(&head);
    let lsn = (common::AUDIO_LSN + audio.len()) as u32;
    audio.push(overrun_sector());
    audio.extend(pack(&tail));
    let spec = Spec { channels: 2, playtime: 6, tracks: vec![TrackSpec { start: 0, duration: 6 }], audio };
    let mut d = Disc::open(Cursor::new(spec.build())).unwrap();
    let whole: Vec<AudioFrame> = range(&mut d, 0, 6);
    assert!(whole.iter().all(|f| f.status == FrameStatus::Dsd));
    for (first, end, malformed) in [(0, 2, 0), (1, 2, 0), (2, 3, 1), (3, 4, 1), (3, 6, 1), (0, 6, 1)] {
        let mut audio = d.range_audio(Slot::TwoChannel, first, end).unwrap();
        let got: Vec<AudioFrame> = audio.by_ref().map(Result::unwrap).collect();
        assert_eq!(got, whole[first as usize..end as usize], "[{first}, {end})");
        assert_eq!(audio.malformed_sectors(), malformed, "[{first}, {end})");
        let lsns: Vec<Option<u32>> = audio.warnings().iter().map(|w| w.lsn).collect();
        assert_eq!(lsns, vec![Some(lsn); malformed as usize], "[{first}, {end})");
    }
}

/// Both master TOC slots point at the same area TOC: continuing a range cursor into the
/// other slot (even at a later frame) rescans, with the frames of a fresh cursor.
#[test]
#[cfg_attr(miri, ignore = "decodes several ranges")]
fn continuing_across_slots() {
    let fs: Vec<PFrame> = (0..12u32)
        .map(|n| PFrame { number: n, dst: n % 3 == 0, data: vec![n as u8; 2 * FRAME_BYTES_PER_CHANNEL] })
        .collect();
    let mut audio = pack(&fs);
    audio.resize(audio.len().max(6), [0u8; 2048]);
    let spec =
        Spec { channels: 2, playtime: PLAYTIME, tracks: vec![TrackSpec { start: 0, duration: 12 }], audio };
    let mut img = spec.build();
    for lsn in [510usize, 520, 530] {
        img[lsn * 2048 + 72..lsn * 2048 + 76].copy_from_slice(&540u32.to_be_bytes());
        img[lsn * 2048 + 86..lsn * 2048 + 88].copy_from_slice(&6u16.to_be_bytes());
    }
    let mut d = Disc::open(Cursor::new(img)).unwrap();
    assert_eq!(d.toc().areas.len(), 2);
    let steps = [
        (Slot::TwoChannel, 2, 5),
        (Slot::Multichannel, 6, 9),
        (Slot::TwoChannel, 9, 12),
        (Slot::Multichannel, 0, 3),
    ];
    let mut cursor: Option<TrackAudioCursor> = None;
    for (slot, first, end) in steps {
        let fresh = d.range_audio_cursor(slot, first, end).unwrap();
        let (expected, _) = read_all(&mut d, fresh);
        let next = match cursor.take() {
            Some(prev) => d.continue_range_audio(prev, slot, first, end).unwrap(),
            None => d.range_audio_cursor(slot, first, end).unwrap(),
        };
        let (got, back) = read_all(&mut d, next);
        assert_eq!(got, expected, "{slot} [{first}, {end})");
        assert_eq!(got.len() as u32, end - first);
        cursor = Some(back);
    }
}
