// SPDX-License-Identifier: MIT OR Apache-2.0

//! The whole corpus read through seek indexes (issue #68): every track and track with
//! pause in reverse order, then ranges at pseudo-random offsets, each opened afresh and
//! continued, give exactly what reads without an index give (whose hashes the corpus test
//! checks against the manifest): frames, statuses, missing, invalid and malformed counts,
//! and warnings.

use std::fs::File;
use std::io::BufReader;

use scarletbook::{Disc, FrameStatus, IndexOptions, ReadAtReader, Slot, TrackAudioCursor, Warning};
use scarletbook_conformance::{corpus_dir, load_cases};

#[derive(Debug, PartialEq)]
struct Read {
    frames: Vec<(u32, FrameStatus, u64)>,
    missing: u64,
    invalid: u64,
    malformed: u64,
    warnings: Vec<Warning>,
}

fn hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

fn read<R: std::io::Read + std::io::Seek>(
    disc: &mut Disc<R>,
    cursor: TrackAudioCursor,
) -> (Read, TrackAudioCursor) {
    let mut audio = disc.read_track_audio(cursor);
    let frames = audio
        .by_ref()
        .map(|f| f.unwrap_or_else(|e| panic!("{e}")))
        .map(|f| (f.number, f.status, hash(&f.data)))
        .collect();
    let c = audio.into_cursor();
    let r = Read {
        frames,
        missing: c.missing_frames(),
        invalid: c.invalid_dst_frames(),
        malformed: c.malformed_sectors(),
        warnings: c.warnings().to_vec(),
    };
    (r, c)
}

/// The ranges read: tracks with pause, then tracks, in reverse order; then ranges at
/// pseudo-random offsets.
fn ranges<R: std::io::Read + std::io::Seek>(disc: &Disc<R>, slot: Slot) -> Vec<(u32, u32)> {
    let Some(area) = disc.toc().area(slot) else { return Vec::new() };
    let p = area.playtime_frames();
    let mut out = Vec::new();
    for t in area.tracks.iter().rev() {
        let n = u32::from(t.number);
        if let Ok(r) = disc.track_range_with_pause(slot, n) {
            out.push(r);
        }
        out.push((t.start_frame, t.end_frame()));
    }
    let mut x = 0x9E37_79B9_7F4A_7C15u64 ^ u64::from(p);
    for _ in 0..8 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let first = (x % u64::from(p.max(1))) as u32;
        let len = 1 + (x >> 32) as u32 % 40;
        out.push((first, first.saturating_add(len).min(p)));
    }
    out.retain(|&(a, b)| disc.check_range(slot, a, b).is_ok());
    out
}

#[test]
fn corpus_reads_through_an_index_equal_plain_reads() {
    let dir = corpus_dir();
    let cases = load_cases(&dir).unwrap_or_else(|e| panic!("manifest: {e}"));
    let (mut areas, mut reads) = (0, 0);
    for case in &cases {
        let id = case["id"].as_str().unwrap_or_default();
        let path = dir.join("images").join(format!("{id}.iso"));
        let open = || File::open(&path).ok().and_then(|f| Disc::open(BufReader::new(f)).ok());
        let (Some(mut plain), Some(mut indexed)) = (open(), open()) else { continue };
        for slot in Slot::ALL {
            let ranges = ranges(&plain, slot);
            if ranges.is_empty() {
                continue;
            }
            areas += 1;
            for interval in [1, 64] {
                indexed.enable_seek_index(slot, IndexOptions::new().with_interval(interval)).unwrap();
                let mut prev: Option<TrackAudioCursor> = None;
                for &(first, end) in &ranges {
                    let c = plain.range_audio_cursor(slot, first, end).unwrap();
                    let (want, _) = read(&mut plain, c);
                    let c = indexed.range_audio_cursor(slot, first, end).unwrap();
                    let (got, _) = read(&mut indexed, c);
                    assert_eq!(got, want, "{id} {slot} K {interval} [{first}, {end})");
                    // Continued from the previous range through the index.
                    let c = match prev.take() {
                        Some(p) => indexed.continue_range_audio(p, slot, first, end).unwrap(),
                        None => indexed.range_audio_cursor(slot, first, end).unwrap(),
                    };
                    let (got, c) = read(&mut indexed, c);
                    assert_eq!(
                        got.frames, want.frames,
                        "{id} {slot} K {interval} [{first}, {end}) continued"
                    );
                    assert_eq!((got.missing, got.invalid), (want.missing, want.invalid));
                    prev = Some(c);
                    reads += 2;
                }
            }
        }
    }
    eprintln!("{areas} areas, {reads} reads through an index");
    assert!(areas > 100, "{areas}");
}

/// Every corpus area read on two threads at once, through a clone and a share of one disc
/// that keep one seek index: each thread reads the ranges in its own order (one forwards,
/// one backwards), both building and using the one index, and every read equals the plain
/// read of that range.
#[test]
fn corpus_reads_on_two_threads_through_one_index() {
    let dir = corpus_dir();
    let cases = load_cases(&dir).unwrap_or_else(|e| panic!("manifest: {e}"));
    let mut areas = 0;
    for case in &cases {
        let id = case["id"].as_str().unwrap_or_default();
        let path = dir.join("images").join(format!("{id}.iso"));
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let Ok(mut plain) = Disc::open(std::io::Cursor::new(bytes.clone())) else { continue };
        let source: std::sync::Arc<[u8]> = bytes.clone().into();
        let mut disc = Disc::open(ReadAtReader::new(source)).unwrap_or_else(|e| panic!("{id}: {e}"));
        for slot in Slot::ALL {
            let ranges = ranges(&plain, slot);
            if ranges.is_empty() {
                continue;
            }
            areas += 1;
            let want: Vec<Read> = ranges
                .iter()
                .map(|&(a, b)| {
                    let c = plain.range_audio_cursor(slot, a, b).unwrap();
                    read(&mut plain, c).0
                })
                .collect();
            disc.enable_seek_index(slot, IndexOptions::new().with_interval(4)).unwrap();
            let mut cloned = disc.clone();
            let mut shared = disc.share(std::io::Cursor::new(bytes.clone())).unwrap();
            std::thread::scope(|s| {
                let (ranges, want) = (&ranges, &want);
                s.spawn(move || {
                    for (i, &(a, b)) in ranges.iter().enumerate() {
                        let c = cloned.range_audio_cursor(slot, a, b).unwrap();
                        assert_eq!(read(&mut cloned, c).0, want[i], "{id} {slot} [{a}, {b}) clone");
                    }
                });
                s.spawn(move || {
                    for (i, &(a, b)) in ranges.iter().enumerate().rev() {
                        let c = shared.range_audio_cursor(slot, a, b).unwrap();
                        assert_eq!(read(&mut shared, c).0, want[i], "{id} {slot} [{a}, {b}) share");
                    }
                });
            });
            assert!(disc.seek_index(slot).is_some_and(|i| i.checkpoints() > 0 || i.is_complete()));
        }
    }
    eprintln!("{areas} areas read on two threads through one index");
    assert!(areas > 100, "{areas}");
}
