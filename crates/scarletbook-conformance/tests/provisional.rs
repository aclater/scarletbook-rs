// SPDX-License-Identifier: MIT OR Apache-2.0

//! The whole corpus read provisionally (issue #123; specification 4, 4.2 item 8): every
//! track and track with pause, and ranges at pseudo-random offsets, each opened afresh with
//! `SeekPolicy::Speculative`, then every reading checked by a complete seek index. A
//! reading whose positions differ from an exact read is refuted, never confirmed, and its
//! positions from `exact_from` on are exact; on the valid images every reading equals the
//! exact one. The corpus image `area-audio-past-end` (whose 2-channel area runs over the
//! multichannel sectors) gives refuted readings. Each range is also read after a remount,
//! from the seek hints and confirmations a first session saved (#124), with the same checks:
//! a reading confirmed at once from a saved confirmation (4.2 item 9) must equal the exact one.

use std::fs::File;
use std::io::BufReader;

use scarletbook::{
    AreaIndex, CursorOptions, Disc, FrameSpan, IndexOptions, SeekPolicy, Slot, Speculation,
    SpeculativeOptions, TrackFrame,
};
use scarletbook_conformance::{corpus_dir, load_cases};

type Frames = Vec<TrackFrame>;

fn read<R: std::io::Read + std::io::Seek>(
    disc: &mut Disc<R>,
    slot: Slot,
    (first, end): (u32, u32),
) -> (Frames, scarletbook::TrackCursor) {
    let c = disc.frames_cursor(slot, FrameSpan::Range(first, end), CursorOptions::new()).unwrap();
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
    let (mut past_end_refuted, mut from_saved) = (0, 0);
    for case in &cases {
        let id = case["id"].as_str().unwrap_or_default();
        let valid = case["category"].as_str() == Some("valid");
        let path = dir.join("images").join(format!("{id}.iso"));
        let open = || File::open(&path).ok().and_then(|f| Disc::open(BufReader::new(f)).ok());
        let Some(mut plain) = open() else { continue };
        for slot in Slot::ALL {
            let ranges = ranges(&plain, slot);
            if ranges.is_empty() {
                continue;
            }
            let options = SpeculativeOptions::new().with_min_gap_sectors(0);
            // A first session reads every range on one disc, keeping an index, and verifies
            // them: its seek hints and the confirmed readings (#124; 4.2 item 9) are saved,
            // without the exact checkpoints, so that later readings use them.
            let saved = open().and_then(|mut d| {
                d.enable_seek_index(slot, IndexOptions::new()).ok()?;
                d.set_seek_policy(slot, SeekPolicy::Speculative(options)).ok()?;
                for &range in &ranges {
                    read(&mut d, slot, range);
                }
                while !d.verify_speculation(slot, 1 << 20).ok()? {}
                d.seek_index(slot).map(|i| i.without_checkpoints().to_bytes())
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
                    if matches!(c.speculation(), Speculation::Confirmed { .. }) {
                        // From a saved confirmation, before any pass of this session.
                        assert_eq!(got, want, "{id} {slot} {range:?}: confirmed from saved state");
                        from_saved += 1;
                    }
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
    eprintln!(
        "{readings} readings, {provisional} provisional, {refuted} refuted, {from_saved} confirmed from saved state"
    );
    assert!(from_saved > 100, "{from_saved}");
    assert!(provisional > 100, "{provisional}");
    assert!(past_end_refuted > 0, "area-audio-past-end gave no refuted reading");
}

/// The suspect mark (4.2 item 8, sacd-spec `eafaaba`): every provisional reading of the
/// 2-channel area of `area-audio-past-end` (whose audio sectors run past the image's end and
/// over the multichannel sectors) is suspect for its layout, and read exactly when asked;
/// no reading of a valid image is suspect.
#[test]
#[cfg_attr(miri, ignore = "reads the whole corpus")]
fn suspect_readings_of_the_corpus() {
    let dir = corpus_dir();
    let cases = load_cases(&dir).unwrap_or_else(|e| panic!("manifest: {e}"));
    let (mut past_end, mut valid_provisional) = (0, 0);
    for case in &cases {
        let id = case["id"].as_str().unwrap_or_default();
        let valid = case["category"].as_str() == Some("valid");
        let path = dir.join("images").join(format!("{id}.iso"));
        let open = || File::open(&path).ok().and_then(|f| Disc::open(BufReader::new(f)).ok());
        let Some(mut plain) = open() else { continue };
        for slot in Slot::ALL {
            for exact_when_suspect in [false, true] {
                let Some(mut disc) = open() else { continue };
                let options = SpeculativeOptions::new()
                    .with_min_gap_sectors(0)
                    .with_exact_when_suspect(exact_when_suspect);
                if disc.set_seek_policy(slot, SeekPolicy::Speculative(options)).is_err() {
                    continue;
                }
                for range in ranges(&plain, slot) {
                    let c = disc
                        .frames_cursor(slot, FrameSpan::Range(range.0, range.1), CursorOptions::new())
                        .unwrap();
                    let mut it = disc.read_frames(c);
                    let got: Frames = it.by_ref().map(|f| f.unwrap()).collect();
                    let c = it.into_cursor();
                    let provisional =
                        matches!(c.resumed_from(), scarletbook::ResumedFrom::Speculative { .. });
                    let at = format!("{id} {slot} {range:?} exact_when_suspect {exact_when_suspect}");
                    if valid {
                        assert_eq!(c.suspect(), None, "{at}");
                        valid_provisional += usize::from(provisional);
                    }
                    if id == "area-audio-past-end" && slot == Slot::TwoChannel {
                        if exact_when_suspect {
                            assert!(!provisional, "{at}");
                            assert_eq!(got, read(&mut plain, slot, range).0, "{at}");
                        } else if provisional {
                            assert_eq!(c.suspect(), Some(scarletbook::Suspect::AreaLayout), "{at}");
                            past_end += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(past_end > 0 && valid_provisional > 100, "{past_end} {valid_provisional}");
}

/// Counts the read calls and bytes through it.
struct Counting<R>(R, std::rc::Rc<std::cell::Cell<(u64, u64)>>);

impl<R: std::io::Read> std::io::Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.0.read(buf)?;
        let (reads, bytes) = self.1.get();
        self.1.set((reads + 1, bytes + n as u64));
        Ok(n)
    }
}

impl<R: std::io::Seek> std::io::Seek for Counting<R> {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.0.seek(pos)
    }
}

/// Issue #434: on a 2-channel area of uneven DST bit rate (`scarletbook-synth --minutes 6
/// --area 2ch --kind dst --track-minutes 0.41 --dst-frames uneven`), the search for frame
/// 18443 read back from its first probe at the area's average density, began one sector
/// past frame 18442's start, found no sector below 18443 and gave up: an exact scan of
/// 1 113 reads and 72.9 MB. It now searches on below that read. Each range near it, opened
/// on a fresh disc, starts provisionally within a few reads and returns the exact frames.
#[test]
#[cfg_attr(miri, ignore = "a 6-minute image; the search's unit tests cover it")]
fn uneven_dst_search_does_not_fall_back_to_a_scan() {
    use scarletbook::{ResumedFrom, SectorFormat};
    use scarletbook_conformance::synth::{Config, DstSource, Kind, write_image};

    let cfg = Config {
        frames: 6 * 60 * 75,
        two_channel: true,
        multichannel: false,
        mc_channels: 5,
        kind: Kind::Dst,
        format: SectorFormat::Plain,
        track_frames: 1845,
    };
    let src = DstSource::from_corpus(&corpus_dir()).unwrap().uneven();
    let mut image = std::io::Cursor::new(Vec::new());
    write_image(&mut image, &cfg, &src).unwrap();
    let image = image.into_inner();
    let slot = Slot::TwoChannel;
    let (lo, hi) = (18_430, 18_460);
    let mut plain = Disc::open(std::io::Cursor::new(&image)).unwrap();
    let (exact, _) = read(&mut plain, slot, (lo, hi + 7));
    for first in lo..hi {
        let count = std::rc::Rc::new(std::cell::Cell::new((0, 0)));
        let mut disc = Disc::open(Counting(std::io::Cursor::new(&image), count.clone())).unwrap();
        disc.set_seek_policy(slot, SeekPolicy::Speculative(SpeculativeOptions::new())).unwrap();
        count.set((0, 0));
        let (frames, cursor) = read(&mut disc, slot, (first, first + 7));
        let at = (first - lo) as usize;
        assert_eq!(frames, exact[at..at + 7], "frames {first}..{}", first + 7);
        assert!(matches!(cursor.resumed_from(), ResumedFrom::Speculative { .. }), "{first}");
        let (reads, bytes) = count.get();
        assert!(reads <= 6 && bytes <= 256 << 10, "{first}: {reads} reads, {bytes} bytes");
        if first == 18_443 {
            assert_eq!((reads, bytes), (5, 194_560));
        }
    }
}
