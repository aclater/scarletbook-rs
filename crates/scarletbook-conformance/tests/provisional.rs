// SPDX-License-Identifier: MIT OR Apache-2.0

//! The whole corpus read provisionally (issue #123; specification 4, 4.2 item 8): every
//! track and track with pause, and ranges at pseudo-random offsets, each opened afresh with
//! `SeekPolicy::Speculative`, then every reading checked by a complete seek index. A
//! reading whose positions differ from an exact read is refuted, never confirmed, and its
//! positions from `exact_from` on are exact; on the valid images every reading equals the
//! exact one. The corpus image `area-audio-past-end` (whose 2-channel area runs over the
//! multichannel sectors) gives refuted readings. Each range is also read after a remount,
//! from the seek hints a first session saved (#124), with the same checks.

use std::fs::File;
use std::io::BufReader;

use scarletbook::{
    AreaIndex, Disc, IndexOptions, SeekPolicy, Slot, Speculation, SpeculativeOptions, TrackFrame,
};
use scarletbook_conformance::{corpus_dir, load_cases};

type Frames = Vec<TrackFrame>;

fn read<R: std::io::Read + std::io::Seek>(
    disc: &mut Disc<R>,
    slot: Slot,
    (first, end): (u32, u32),
) -> (Frames, scarletbook::TrackCursor) {
    let c = disc.range_frames_cursor(slot, first, end).unwrap();
    let mut it = disc.read_frames(c);
    let frames = it.by_ref().map(|f| f.unwrap_or_else(|e| panic!("{e}"))).collect();
    (frames, it.into_cursor())
}

/// Tracks with pause and tracks, then ranges at pseudo-random offsets.
fn ranges<R: std::io::Read + std::io::Seek>(disc: &Disc<R>, slot: Slot) -> Vec<(u32, u32)> {
    let Some(area) = disc.toc().area(slot) else { return Vec::new() };
    let p = area.playtime_frames();
    let mut out = Vec::new();
    for t in &area.tracks {
        if let Ok(r) = disc.track_range_with_pause(slot, u32::from(t.number)) {
            out.push(r);
        }
        out.push((t.start_frame, t.end_frame()));
    }
    let mut x = 0x2545_F491_4F6C_DD1Du64 ^ u64::from(p);
    for _ in 0..12 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let first = (x % u64::from(p.max(1))) as u32;
        let len = 1 + (x >> 32) as u32 % 20;
        out.push((first, first.saturating_add(len).min(p)));
    }
    out.retain(|&(a, b)| disc.check_range(slot, a, b).is_ok());
    out
}

#[test]
#[cfg_attr(miri, ignore = "reads the whole corpus")]
fn corpus_provisional_readings_are_exact_or_refuted() {
    let dir = corpus_dir();
    let cases = load_cases(&dir).unwrap_or_else(|e| panic!("manifest: {e}"));
    let (mut readings, mut provisional, mut refuted) = (0, 0, 0);
    let mut past_end_refuted = 0;
    for case in &cases {
        let id = case["id"].as_str().unwrap_or_default();
        let valid = case["category"].as_str() == Some("valid");
        let path = dir.join("images").join(format!("{id}.iso"));
        let open = || File::open(&path).ok().and_then(|f| Disc::open(BufReader::new(f)).ok());
        let Some(mut plain) = open() else { continue };
        for slot in Slot::ALL {
            let ranges = ranges(&plain, slot);
            let options = SpeculativeOptions::new().with_min_gap_sectors(0);
            // A first session reads every range on one disc, keeping an index: its seek
            // hints (#124) are saved.
            let saved = open().and_then(|mut d| {
                d.enable_seek_index(slot, IndexOptions::new()).ok()?;
                d.set_seek_policy(slot, SeekPolicy::Speculative(options)).ok()?;
                for &range in &ranges {
                    read(&mut d, slot, range);
                }
                d.seek_index(slot).map(|i| i.to_bytes())
            });
            let mut cursors = Vec::new();
            for &range in &ranges {
                let (want, _) = read(&mut plain, slot, range);
                // A disc of its own per reading: a cold seek, then one after a remount (from
                // the saved hints); the index checks each.
                for hinted in [false, true] {
                    let mut disc = open().unwrap();
                    match saved.as_ref().filter(|_| hinted) {
                        Some(bytes) => {
                            let area = disc.toc().area(slot).unwrap().clone();
                            let n = disc.toc().sectors;
                            let index = AreaIndex::from_bytes(&area, n, bytes, usize::MAX).unwrap();
                            disc.set_seek_index(slot, index).unwrap();
                        }
                        None => disc.enable_seek_index(slot, IndexOptions::new()).unwrap(),
                    }
                    disc.set_seek_policy(slot, SeekPolicy::Speculative(options)).unwrap();
                    let (got, c) = read(&mut disc, slot, range);
                    while !disc.verify_speculation(slot, 1 << 20).unwrap() {}
                    cursors.push((range, want.clone(), got, c.speculation()));
                }
            }
            for ((first, end), want, got, speculation) in cursors {
                readings += 1;
                let at = format!("{id} {slot} [{first}, {end}): {speculation:?}");
                match speculation {
                    Speculation::Exact => assert_eq!(got, want, "{at}"),
                    Speculation::Confirmed { .. } => {
                        provisional += 1;
                        assert_eq!(got, want, "{at}");
                    }
                    Speculation::Refuted { exact_from, .. } => {
                        provisional += 1;
                        refuted += 1;
                        if id == "area-audio-past-end" {
                            past_end_refuted += 1;
                        }
                        assert!(!valid, "{at}");
                        let from = |v: &Frames| {
                            v.iter().filter(|f| f.number >= exact_from).cloned().collect::<Vec<_>>()
                        };
                        assert_eq!(from(&got), from(&want), "{at}: exact from {exact_from}");
                    }
                    other => panic!("{at}: not settled: {other:?}"),
                }
            }
        }
    }
    eprintln!("{readings} readings, {provisional} provisional, {refuted} refuted");
    assert!(provisional > 100, "{provisional}");
    assert!(past_end_refuted > 0, "area-audio-past-end gave no refuted reading");
}
