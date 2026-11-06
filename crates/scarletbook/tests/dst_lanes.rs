// SPDX-License-Identifier: MIT OR Apache-2.0

//! `TrackAudioCursor::with_dst_lanes` (issue #203): for every lane count the positions, their
//! order, counts and warnings are those of the default, on the calling thread and on a pool,
//! and only the groups decoded together change.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;
#[path = "common/dst_enc.rs"]
mod dst_enc;

use std::io::Cursor;
use std::path::Path;
use std::sync::{Arc, Mutex};

use common::{PFrame, Spec, TrackSpec, pack};
use scarletbook::audio::{FRAME_BYTES_PER_CHANNEL, TrackAudioCursor};
use scarletbook::dst::{self, MAX_LANES};
use scarletbook::log::{Level, Log};
use scarletbook::{AudioFrame, Disc, Error, ErrorClass, FrameStatus, Slot};

/// Where a read decodes: on the calling thread (`None`) or on a pool of this many threads.
type Threads = Option<usize>;

/// Where to read, for a test: the pool only with `parallel`, and not under Miri (threads).
fn places() -> Vec<Threads> {
    if cfg!(feature = "parallel") && !cfg!(miri) { vec![None, Some(1), Some(2), Some(4)] } else { vec![None] }
}

#[cfg(feature = "parallel")]
fn place(mut cursor: TrackAudioCursor, threads: Threads) -> TrackAudioCursor {
    cursor.set_decode_on(match threads {
        None => scarletbook::DecodeOn::CallingThread,
        Some(n) => scarletbook::DecodeOn::Pool(Arc::new(
            rayon::ThreadPoolBuilder::new().num_threads(n).build().unwrap(),
        )),
    });
    cursor
}

#[cfg(not(feature = "parallel"))]
fn place(cursor: TrackAudioCursor, _: Threads) -> TrackAudioCursor {
    cursor
}

/// What a read of every track of every area gives.
#[derive(Default, PartialEq)]
struct Read {
    frames: Vec<AudioFrame>,
    /// The missing and invalid counts at the end of each track.
    counts: Vec<(u64, u64)>,
    warnings: Vec<String>,
}

/// Reads every track of `image`, one cursor per area, with `lanes` frames together. Returns
/// the sizes of the groups decoded, in timeline order, too.
fn read(image: &[u8], lanes: Option<usize>, threads: Threads) -> (Read, Vec<usize>) {
    let lines: Arc<Mutex<Vec<(Level, String)>>> = Arc::default();
    let sink = Arc::clone(&lines);
    let log = Log::new(Level::Trace, move |l, m| sink.lock().unwrap().push((l, m.to_string())));
    let mut disc = Disc::open_with_log(Cursor::new(image.to_vec()), log).unwrap();
    let areas: Vec<(Slot, Vec<u32>)> = disc
        .toc()
        .areas
        .iter()
        .map(|a| (a.slot, a.tracks.iter().map(|t| u32::from(t.number)).collect()))
        .collect();
    let mut out = Read::default();
    for (slot, tracks) in areas {
        let mut cursor: Option<TrackAudioCursor> = None;
        for n in tracks {
            let next = match cursor.take() {
                Some(prev) => disc.continue_track_audio(prev, slot, n).unwrap(),
                None => {
                    let c = place(disc.track_audio_cursor(slot, n).unwrap(), threads);
                    match lanes {
                        Some(k) => c.with_dst_lanes(k).unwrap(),
                        None => c,
                    }
                }
            };
            assert_eq!(next.dst_lanes(), lanes, "continuations keep the choice");
            let mut audio = disc.read_track_audio(next);
            out.frames.extend(audio.by_ref().map(Result::unwrap));
            out.counts.push((audio.missing_frames(), audio.invalid_dst_frames()));
            cursor = Some(audio.into_cursor());
        }
    }
    let lines = lines.lock().unwrap();
    out.warnings = lines.iter().filter(|(l, _)| *l == Level::Warn).map(|(_, m)| m.clone()).collect();
    // "decoding DST frames A..=B on thread T"; the pool's threads log in any order.
    let mut groups: Vec<(u32, u32)> = lines
        .iter()
        .filter_map(|(_, m)| m.strip_prefix("decoding DST frames "))
        .map(|m| {
            let (a, b) = m.split_once(' ').unwrap().0.split_once("..=").unwrap();
            (a.parse().unwrap(), b.parse().unwrap())
        })
        .collect();
    groups.sort_unstable();
    (out, groups.iter().map(|(a, b)| (b - a + 1) as usize).collect())
}

/// Positions with the status and audio they must have.
type Want = Vec<(u32, FrameStatus, Vec<u8>)>;

/// An area of `nch` channels. Track 1 is `coded` coded DST frames; track 2 is an invalid DST
/// frame, a plain frame, a missing position and a coded DST frame. Returns the image and each
/// position that is not the invalid one's expected status and audio.
fn image(nch: u8, coded: u32) -> (Vec<u8>, Want) {
    let nch_len = usize::from(nch);
    let mut seed = 0x51_3C_7A_D2u64 ^ u64::from(nch);
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let coefs: Vec<i32> = (0..20).map(|i| [96, -40, 17, -5][i % 4] >> (i / 4)).collect();
    let ptable: Vec<u32> = (0..24).map(|i| 128 - 5 * i).collect();
    let mut frames = Vec::new();
    let mut want = Vec::new();
    let mut code = |number: u32, frames: &mut Vec<PFrame>, want: &mut Vec<_>| {
        // Under Miri, DST frames of a short plain payload (DST_coded = 0, zero-filled): the
        // cursor's groups are what is tested, and the lockstep loop has its own Miri tests,
        // `dst::tests::fast_loop_matches_general_*`.
        if cfg!(miri) {
            let payload: Vec<u8> = (0..8).map(|_| rnd() as u8).collect();
            let mut audio = payload.clone();
            audio.resize(nch_len * FRAME_BYTES_PER_CHANNEL, 0);
            frames.push(PFrame { number, dst: true, data: [&[0x40], &payload[..]].concat() });
            want.push((number, FrameStatus::Decoded, audio));
            return;
        }
        let signal: Vec<Vec<u8>> =
            (0..nch).map(|_| (0..FRAME_BYTES_PER_CHANNEL).map(|_| rnd() as u8).collect()).collect();
        let half: Vec<bool> = (0..nch_len).map(|c| (c + number as usize) % 3 == 0).collect();
        let data = dst_enc::encode_simple(&signal, &coefs, &ptable, &half);
        frames.push(PFrame { number, dst: true, data });
        want.push((number, FrameStatus::Decoded, dst_enc::interleave(&signal)));
    };
    for number in 0..coded {
        code(number, &mut frames, &mut want);
    }
    // Too short for its own header: invalid whatever its bits say.
    frames.push(PFrame { number: coded, dst: true, data: vec![0xFF] });
    let plain = vec![0x5A; nch_len * FRAME_BYTES_PER_CHANNEL];
    frames.push(PFrame { number: coded + 1, dst: false, data: plain.clone() });
    want.push((coded + 1, FrameStatus::Dsd, plain));
    want.push((coded + 2, FrameStatus::Missing, vec![0x69; nch_len * FRAME_BYTES_PER_CHANNEL]));
    code(coded + 3, &mut frames, &mut want);
    let mut audio = pack(&frames);
    audio.resize(audio.len().max(6), [0u8; 2048]);
    let tracks = vec![TrackSpec { start: 0, duration: coded }, TrackSpec { start: coded, duration: 4 }];
    (Spec { channels: nch, playtime: coded + 4, tracks, audio }.build(), want)
}

/// Every lane count against the default and the audio that was encoded, with the groups each
/// count decodes. Under Miri: two channels, on the calling thread, few short frames.
#[test]
fn every_lane_count_gives_the_same_positions() {
    let (channels, coded): (&[u8], u32) = if cfg!(miri) { (&[2], 2) } else { (&[1, 2, 3, 4, 5, 6], 8) };
    for &nch in channels {
        let (img, want) = image(nch, coded);
        for threads in places() {
            let what = format!("{nch} channels, {threads:?}");
            let (default, groups) = read(&img, None, threads);
            assert_eq!(groups[0], dst::lanes(nch).min(coded as usize), "{what}");
            assert_eq!(default.frames.len(), coded as usize + 4, "{what}");
            for (number, status, data) in &want {
                let f = &default.frames[*number as usize];
                assert_eq!((f.number, f.status), (*number, *status), "{what}");
                assert!(f.data == *data, "{what}: position {number}: audio differs");
            }
            assert!(matches!(default.frames[coded as usize].status, FrameStatus::Invalid(_)), "{what}");
            assert_eq!(default.counts.last(), Some(&(1, 1)), "{what}");
            for k in 1..=MAX_LANES {
                let (got, groups) = read(&img, Some(k), threads);
                assert!(got == default, "{what}, {k} lanes");
                assert_eq!(groups[0], k.min(coded as usize), "{what}, {k} lanes");
                assert!(groups.iter().all(|&g| g <= k), "{what}, {k} lanes: {groups:?}");
            }
        }
    }
}

/// The same on the corpus's DST images: every feature of a frame, and invalid frames.
#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images; see every_lane_count_gives_the_same_positions")]
fn every_lane_count_decodes_the_corpus_the_same() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/images");
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            let name = p.file_name().unwrap().to_string_lossy();
            name.starts_with("dst-features-") || name.starts_with("dst-invalid-frames-")
        })
        .collect();
    paths.sort();
    assert!(paths.len() >= 6, "{paths:?}");
    for path in &paths {
        let img = std::fs::read(path).unwrap();
        // One thread and a pool: the groups are built apart on those two paths.
        for threads in places().into_iter().filter(|t| matches!(t, None | Some(2))) {
            let (default, _) = read(&img, None, threads);
            assert!(!default.frames.is_empty(), "{path:?}");
            for k in 1..=MAX_LANES {
                let (got, _) = read(&img, Some(k), threads);
                assert!(got == default, "{path:?}, {threads:?}, {k} lanes");
            }
        }
    }
}

/// Chosen after the first read, with positions already held ahead: the same positions.
#[test]
#[cfg_attr(miri, ignore = "decodes full frames; see every_lane_count_gives_the_same_positions")]
fn choosing_during_a_read_gives_the_same_positions() {
    let (img, _) = image(2, 8);
    let (want, _) = read(&img, None, None);
    for threads in places() {
        for k in 1..=MAX_LANES {
            let mut disc = Disc::open(Cursor::new(img.clone())).unwrap();
            let c = place(disc.track_audio_cursor(Slot::TwoChannel, 1).unwrap(), threads);
            let mut audio = disc.read_track_audio(c);
            let mut frames: Vec<AudioFrame> = audio.by_ref().take(3).map(Result::unwrap).collect();
            let c = audio.into_cursor().with_dst_lanes(k).unwrap();
            frames.extend(disc.read_track_audio(c).map(Result::unwrap));
            assert!(frames == want.frames[..8], "{threads:?}, {k} lanes");
        }
    }
}

/// Out of range is refused with its own error, a caller's error; in range is kept.
#[test]
fn out_of_range_is_refused() {
    let disc = Disc::open(Cursor::new(Spec::simple().build())).unwrap();
    assert_eq!(disc.track_audio_cursor(Slot::TwoChannel, 1).unwrap().dst_lanes(), None);
    for k in [0, MAX_LANES + 1, usize::MAX] {
        let c = disc.track_audio_cursor(Slot::TwoChannel, 1).unwrap();
        let e = c.with_dst_lanes(k).unwrap_err();
        assert!(matches!(e, Error::DstLanesInvalid { lanes, .. } if lanes == k), "{e}");
        assert_eq!(e.class(), ErrorClass::Usage);
        assert_eq!(e.code(), None);
    }
    for k in 1..=MAX_LANES {
        let c = disc.track_audio_cursor(Slot::TwoChannel, 1).unwrap().with_dst_lanes(k).unwrap();
        assert_eq!(c.dst_lanes(), Some(k));
        let c = disc.continue_track_audio(c, Slot::TwoChannel, 2).unwrap();
        assert_eq!(c.dst_lanes(), Some(k));
    }
}
