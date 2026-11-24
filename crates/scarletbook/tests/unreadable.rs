// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unreadable sectors (#435): every timeline position whose frame has a byte in an
//! unreadable sector, or starts in one, is lost, never decoded from part of its data; every
//! other position is as without them, at its own frame number; marks reach cursors through
//! the set and through readers that report sectors unreadable; seek indexes never hold
//! state from past an unreadable sector.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;
#[path = "common/dst_enc.rs"]
mod dst_enc;

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};

use common::{AUDIO_LSN, PFrame, S, Spec, TrackSpec, pack};
use proptest::prelude::*;
use scarletbook::audio::FRAME_BYTES_PER_CHANNEL;
use scarletbook::read_at::unreadable_error;
use scarletbook::{
    AudioFrame, Disc, Error, FrameContent, FrameStatus, IndexOptions, ReadAt, ReadAtReader, SeekPolicy, Slot,
    Speculation, SpeculativeOptions, TrackFrame,
};

const SLOT: Slot = Slot::TwoChannel;

/// The audio sectors (indexes from the area's first) that each frame's packets lie in,
/// first and last, parsed from the sectors as specification 4, 4.3 lays them out.
fn spans(audio: &[[u8; S]]) -> BTreeMap<u32, (usize, usize)> {
    let mut out = BTreeMap::new();
    let mut cur: Option<u32> = None;
    for (i, s) in audio.iter().enumerate() {
        let h = s[0];
        let rec = if h & 1 != 0 { 4 } else { 3 };
        let (f, k) = (usize::from((h >> 2) & 7), usize::from(h >> 5));
        let mut x = 0;
        for p in 0..k {
            let w = u16::from_be_bytes([s[1 + 2 * p], s[2 + 2 * p]]);
            if (w >> 11) & 7 != 2 {
                continue;
            }
            if w & 0x8000 != 0 {
                assert!(x < f, "test images give every start a record");
                let r = 1 + 2 * k + x * rec;
                x += 1;
                let n = (u32::from(s[r]) * 60 + u32::from(s[r + 1])) * 75 + u32::from(s[r + 2]);
                cur = Some(n);
                out.insert(n, (i, i));
            } else if let Some(n) = cur {
                out.get_mut(&n).unwrap().1 = i;
            }
        }
    }
    out
}

/// The frames whose packets touch the audio sectors `[a, b)` (indexes from the first).
fn touched(spans: &BTreeMap<u32, (usize, usize)>, (a, b): (usize, usize)) -> BTreeSet<u32> {
    spans.iter().filter(|&(_, &(s, e))| s < b && e >= a).map(|(&n, _)| n).collect()
}

/// A 2-channel area of `n` DST frames numbered 0 to `n - 1`, as `pack` lays them out: coded
/// frames of 1-bit audio from the test encoder, or under Miri short uncoded ones. Returns
/// the image and its audio sectors.
fn dst_disc(n: u32) -> (Vec<u8>, Vec<[u8; S]>) {
    let frames: Vec<PFrame> = if cfg!(miri) {
        (0..n)
            .map(|number| {
                let payload: Vec<u8> = (0..3000).map(|i| (i as u32 * 7 + number) as u8).collect();
                PFrame { number, dst: true, data: [&[0x40], &payload[..]].concat() }
            })
            .collect()
    } else {
        let coefs: Vec<i32> = (0..20).map(|i| [96, -40, 17, -5][i % 4] >> (i / 4)).collect();
        let ptable: Vec<u32> = (0..24).map(|i| 128 - 5 * i).collect();
        let mut rng = dst_enc::Rng::new(0x435);
        (0..n)
            .map(|number| {
                // Noise for a part of each channel and an idle pattern for the rest: coded
                // frames of about 1 to 3 sectors, as on a disc.
                let noisy = 700 + (number as usize * 977) % 1900;
                let signal: Vec<Vec<u8>> = (0..2)
                    .map(|_| {
                        (0..FRAME_BYTES_PER_CHANNEL)
                            .map(|j| if j < noisy { rng.next_u64() as u8 } else { 0x00 })
                            .collect()
                    })
                    .collect();
                let data = dst_enc::encode_simple(&signal, &coefs, &ptable, &[false, true]);
                PFrame { number, dst: true, data }
            })
            .collect()
    };
    disc_of(&frames, n)
}

/// A 2-channel area of `n` plain frames.
fn dsd_disc(n: u32) -> (Vec<u8>, Vec<[u8; S]>) {
    let frames: Vec<PFrame> = (0..n)
        .map(|number| PFrame {
            number,
            dst: false,
            data: vec![number as u8 ^ 0xA5; 2 * FRAME_BYTES_PER_CHANNEL],
        })
        .collect();
    disc_of(&frames, n)
}

fn disc_of(frames: &[PFrame], playtime: u32) -> (Vec<u8>, Vec<[u8; S]>) {
    let audio = pack(frames);
    let tracks = vec![TrackSpec { start: 0, duration: playtime }];
    (Spec { channels: 2, playtime, tracks, audio: audio.clone() }.build(), audio)
}

/// `image` with the audio sectors `[a, b)` (indexes) zero-filled, as a salvage reader
/// supplies them.
fn zeroed(image: &[u8], (a, b): (usize, usize)) -> Vec<u8> {
    let mut img = image.to_vec();
    img[(AUDIO_LSN + a) * S..(AUDIO_LSN + b) * S].fill(0);
    img
}

/// A disc of `image` with the audio sectors `[a, b)` (indexes) marked unreadable.
fn marked(image: Vec<u8>, (a, b): (usize, usize)) -> Disc<Cursor<Vec<u8>>> {
    let disc = Disc::open(Cursor::new(image)).unwrap();
    disc.unreadable_sectors().insert((AUDIO_LSN + a) as u64, (b - a) as u64);
    disc
}

/// The positions `[first, end)` as stored, with the missing and lost counts.
fn raw<R: Read + Seek>(disc: &mut Disc<R>, (first, end): (u32, u32)) -> (Vec<TrackFrame>, u64, u64) {
    let mut frames = disc.range_frames(SLOT, first, end).unwrap();
    let v: Vec<TrackFrame> = frames.by_ref().map(Result::unwrap).collect();
    (v, frames.missing_frames(), frames.lost_frames())
}

/// The positions `[first, end)` decoded, with the missing, invalid and lost counts.
fn decoded<R: Read + Seek>(disc: &mut Disc<R>, (first, end): (u32, u32)) -> (Vec<AudioFrame>, [u64; 3]) {
    let mut audio = disc.range_audio(SLOT, first, end).unwrap();
    let v: Vec<AudioFrame> = audio.by_ref().map(Result::unwrap).collect();
    (v, [audio.missing_frames(), audio.invalid_dst_frames(), audio.lost_frames()])
}

/// Checks the positions read with `[a, b)` marked against `clean`, read without marks:
/// lost exactly where a frame touches the sectors, the same everywhere else.
fn check_raw(what: &str, got: &[TrackFrame], clean: &[TrackFrame], lost: &BTreeSet<u32>) {
    assert_eq!(got.len(), clean.len(), "{what}");
    for (g, c) in got.iter().zip(clean) {
        assert_eq!(g.number, c.number, "{what}");
        if lost.contains(&g.number) {
            assert!(
                g.is_lost() && g.content == FrameContent::Missing,
                "{what}: position {} not lost",
                g.number
            );
        } else {
            assert!(g == c, "{what}: position {} changed", g.number);
        }
    }
}

/// Every run of 1, 2, 3 or 5 unreadable sectors at every place in the area of `image`,
/// zero-filled and marked: raw, decoded around the damage and the stored frame list.
/// Returns the number of cases, and of cases where without marks (zero-filled only) a frame
/// touching the sectors decodes, or is an invalid DST frame: frames assembled from part of
/// their data (and other frames' pieces), which the marks prevent.
fn sweep(image: &[u8], audio: &[[u8; S]], decode: bool) -> (usize, (usize, usize)) {
    let spans = spans(audio);
    let playtime = spans.len() as u32;
    let all = (0, playtime);
    let mut clean_disc = Disc::open(Cursor::new(image.to_vec())).unwrap();
    let (clean, _, _) = raw(&mut clean_disc, all);
    let (mut cases, mut partial) = (0, (0, 0));
    let runs: &[usize] = if cfg!(miri) { &[1] } else { &[1, 2, 3, 5] };
    for &k in runs {
        for a in 0..=audio.len() - k {
            let bad = (a, a + k);
            let what = format!("{k} sectors at audio sector {a}");
            let lost = touched(&spans, bad);
            let img = zeroed(image, bad);
            let mut disc = marked(img.clone(), bad);
            let (got, missing, n_lost) = raw(&mut disc, all);
            check_raw(&what, &got, &clean, &lost);
            assert_eq!((missing, n_lost), (0, lost.len() as u64), "{what}");
            // The stored frame list: the frames that do not touch the sectors, and stretches
            // that cover exactly those that do.
            let list: Vec<_> = disc.stored_frame_list(SLOT).unwrap().map(Result::unwrap).collect();
            let numbers: BTreeSet<u32> = list.iter().map(|f| f.number).collect();
            let kept: BTreeSet<u32> = (0..playtime).filter(|t| !lost.contains(t)).collect();
            assert_eq!(numbers, kept, "{what}");
            let mut stretches: BTreeSet<u32> = list
                .iter()
                .filter_map(|f| f.lost_from.map(|l| (l, f.number)))
                .flat_map(|(l, t)| l..t)
                .collect();
            let mut frames = disc.stored_frame_list(SLOT).unwrap();
            frames.by_ref().for_each(drop);
            if let Some(l) = frames.lost_from() {
                stretches.extend(l..playtime);
            }
            assert_eq!(stretches, lost, "{what}");
            if decode && !lost.is_empty() {
                let lo = lost.first().unwrap().saturating_sub(2);
                let hi = (lost.last().unwrap() + 3).min(playtime);
                let (got, counts) = decoded(&mut disc, (lo, hi));
                let (want, _) = decoded(&mut clean_disc, (lo, hi));
                for (g, w) in got.iter().zip(&want) {
                    if lost.contains(&g.number) {
                        assert_eq!(g.status, FrameStatus::Lost, "{what}: position {}", g.number);
                        assert!(g.data.iter().all(|&b| b == 0x69), "{what}");
                    } else {
                        assert!(g == w, "{what}: position {} changed", g.number);
                    }
                }
                assert_eq!(counts, [0, 0, lost.len() as u64], "{what}");
                // Zero-filled only, as before #435.
                let mut unmarked = Disc::open(Cursor::new(img)).unwrap();
                let (old, _) = decoded(&mut unmarked, (lo, hi));
                if old.iter().any(|f| lost.contains(&f.number) && f.status == FrameStatus::Decoded) {
                    partial.0 += 1;
                }
                if old.iter().any(|f| lost.contains(&f.number) && matches!(f.status, FrameStatus::Invalid(_)))
                {
                    partial.1 += 1;
                }
            }
            cases += 1;
        }
    }
    (cases, partial)
}

/// The sweep of #435 on a generated disc of 150 coded DST stereo frames: no position whose
/// frame touches an unreadable sector is ever decoded; under Miri, 3 short frames and runs
/// of one sector.
#[test]
fn dst_sweep_reports_lost_never_decodes() {
    let (image, audio) = dst_disc(if cfg!(miri) { 3 } else { 150 });
    let (cases, (decodes, invalid)) = sweep(&image, &audio, true);
    // For the pull request: what the marks prevent.
    eprintln!(
        "DST sweep: {} sectors, {cases} cases; zero-filled only, a touching frame decodes in {decodes} and is \
         invalid in {invalid}",
        audio.len()
    );
}

/// The same on plain DSD frames.
#[test]
fn dsd_sweep_reports_lost() {
    let (image, audio) = dsd_disc(if cfg!(miri) { 2 } else { 24 });
    sweep(&image, &audio, !cfg!(miri));
}

/// No marks: the same as before, and an empty set changes nothing.
#[test]
fn no_marks_changes_nothing() {
    let (image, audio) = dst_disc(if cfg!(miri) { 4 } else { 20 });
    let playtime = spans(&audio).len() as u32;
    let mut a = Disc::open(Cursor::new(image.clone())).unwrap();
    let mut b = Disc::open(Cursor::new(image)).unwrap();
    b.unreadable_sectors().insert(AUDIO_LSN as u64, 3);
    b.unreadable_sectors().remove(0, u64::MAX);
    assert!(b.unreadable_sectors().is_empty());
    assert_eq!(raw(&mut a, (0, playtime)), raw(&mut b, (0, playtime)));
    assert_eq!(decoded(&mut a, (0, playtime)), decoded(&mut b, (0, playtime)));
}

/// The set: merged, split, queried, shared by shares and clones.
#[test]
fn the_set_merges_and_splits() {
    let (image, _) = dsd_disc(2);
    let disc = Disc::open(ReadAtReader::new(Arc::new(image))).unwrap();
    let set = disc.unreadable_sectors();
    set.insert(10, 2);
    set.insert(14, 1);
    set.insert(12, 2);
    assert_eq!(set.ranges(), [(10, 5)]);
    set.remove(11, 2);
    assert_eq!(set.ranges(), [(10, 1), (13, 2)]);
    assert!(set.contains(10) && !set.contains(11) && set.contains(14) && !set.contains(15));
    set.insert(u64::MAX - 1, 5);
    assert_eq!(set.ranges().last(), Some(&(u64::MAX - 1, 1)));
    set.insert(5, 0);
    assert_eq!(set.ranges().len(), 3);
    let clone = disc.clone();
    assert_eq!(clone.unreadable_sectors().ranges(), set.ranges(), "clones share the set");
    set.clear();
    assert!(clone.unreadable_sectors().is_empty());
}

/// A source that reports the bytes of some sectors unreadable, as a positional source or
/// as a reader.
#[derive(Clone)]
struct Flaky {
    bytes: Arc<Vec<u8>>,
    /// The byte range reported unreadable, which a test may move.
    bad: Arc<Mutex<(u64, u64)>>,
    /// Every request that failed, as (offset, length).
    failed: Arc<Mutex<Vec<(u64, usize)>>>,
    /// Fails only requests of one sector, as a drive whose retries of a sector fail.
    single_only: Arc<std::sync::atomic::AtomicBool>,
    pos: u64,
}

/// The byte range of audio sectors `[a, b)` (indexes).
fn bytes_of((a, b): (usize, usize)) -> (u64, u64) {
    (((AUDIO_LSN + a) * S) as u64, ((AUDIO_LSN + b) * S) as u64)
}

impl Flaky {
    fn new(bytes: Vec<u8>, bad: (usize, usize)) -> Self {
        let bad = Arc::new(Mutex::new(bytes_of(bad)));
        Flaky { bytes: Arc::new(bytes), bad, failed: Arc::default(), single_only: Arc::default(), pos: 0 }
    }
}

impl ReadAt for Flaky {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let bad = *self.bad.lock().unwrap();
        let single = self.single_only.load(std::sync::atomic::Ordering::Relaxed);
        if offset < bad.1 && offset + buf.len() as u64 > bad.0 && (!single || buf.len() == S) {
            self.failed.lock().unwrap().push((offset, buf.len()));
            return Err(unreadable_error());
        }
        self.bytes.read_at(offset, buf)
    }

    fn size(&self) -> io::Result<u64> {
        Ok(self.bytes.len() as u64)
    }
}

impl Read for Flaky {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.read_at(self.pos, buf)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for Flaky {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.pos = match pos {
            SeekFrom::Start(n) => n,
            SeekFrom::End(d) => (self.bytes.len() as i64 + d) as u64,
            SeekFrom::Current(d) => (self.pos as i64 + d) as u64,
        };
        Ok(self.pos)
    }
}

/// A reader that reports sectors unreadable gives what marking them gives, positional or
/// not, and the sectors join the set; `read_sectors` refuses them.
#[test]
fn readers_report_unreadable_sectors() {
    let (image, audio) = dst_disc(if cfg!(miri) { 4 } else { 12 });
    let playtime = spans(&audio).len() as u32;
    let bad = (2, 4);
    let want = raw(&mut marked(image.clone(), bad), (0, playtime));
    let sectors = [((AUDIO_LSN + 2) as u64, 2)];
    let mut at = Disc::open(ReadAtReader::new(Flaky::new(image.clone(), bad))).unwrap();
    assert_eq!(raw(&mut at, (0, playtime)), want, "positional");
    assert_eq!(at.unreadable_sectors().ranges(), sectors);
    let mut seq = Disc::open(Flaky::new(image.clone(), bad)).unwrap();
    assert_eq!(raw(&mut seq, (0, playtime)), want, "sequential");
    assert_eq!(seq.unreadable_sectors().ranges(), sectors);
    // Raw sector reads: refused, whether reported by the reader or marked.
    let mut fresh = Disc::open(Flaky::new(image, bad)).unwrap();
    let mut buf = vec![0u8; 4 * S];
    let e = fresh.image_mut().read_sectors((AUDIO_LSN + 1) as u64, &mut buf).unwrap_err();
    assert!(matches!(e, Error::SectorUnreadable { lsn, .. } if lsn == (AUDIO_LSN + 2) as u64), "{e:?}");
    assert_eq!(e.class(), scarletbook::ErrorClass::Io);
    let e = seq.image_mut().read_sectors((AUDIO_LSN + 3) as u64, &mut buf[..S]).unwrap_err();
    assert!(matches!(e, Error::SectorUnreadable { .. }), "{e:?}");
    seq.image_mut().read_sectors(AUDIO_LSN as u64, &mut buf[..2 * S]).unwrap();
}

/// Seek indexes: no checkpoint from past an unreadable sector, a pass stops there with an
/// error, and a cursor does not resume from a checkpoint at or past a mark made later.
#[test]
fn seek_indexes_hold_nothing_past_unreadable_sectors() {
    let (image, audio) = dst_disc(if cfg!(miri) { 4 } else { 40 });
    let spans = spans(&audio);
    let playtime = spans.len() as u32;
    let bad = (audio.len() / 2, audio.len() / 2 + 1);
    let bad_lsn = (AUDIO_LSN + bad.0) as u64;
    let options = IndexOptions::new().with_interval(2);
    // Built while reading past a mark: the frontier stays before it.
    let mut disc = marked(image.clone(), bad);
    disc.enable_seek_index(SLOT, options).unwrap();
    let want = raw(&mut disc, (0, playtime));
    let index = disc.seek_index(SLOT).unwrap();
    assert!(index.frontier() <= bad_lsn, "frontier {} past {bad_lsn}", index.frontier());
    let e = loop {
        match disc.extend_seek_index(SLOT, 1) {
            Ok(done) => assert!(!done, "an index cannot complete past a mark"),
            Err(e) => break e,
        }
    };
    assert!(matches!(e, Error::SectorUnreadable { lsn, .. } if lsn == bad_lsn), "{e:?}");
    // Every range reads as without the index.
    let first = playtime * 3 / 4;
    let mut plain = marked(image.clone(), bad);
    assert_eq!(raw(&mut disc, (first, playtime)), raw(&mut plain, (first, playtime)));
    // Built complete without marks, then a mark: ranges past it read as a scan meeting it.
    let mut disc = Disc::open(Cursor::new(image)).unwrap();
    disc.enable_seek_index(SLOT, options).unwrap();
    while !disc.extend_seek_index(SLOT, 64).unwrap() {}
    disc.unreadable_sectors().insert(bad_lsn, 1);
    for first in [0, playtime / 2, first] {
        let (got, _, _) = raw(&mut disc, (first, playtime));
        assert_eq!(got, want.0[first as usize..], "from {first}");
    }
    assert!(matches!(disc.extend_seek_index(SLOT, 1), Ok(true)), "a complete index stays complete");
}

/// Provisional readings meet unreadable sectors as exact ones do, and one with a mark
/// before its `L` is never confirmed.
#[test]
fn provisional_readings_with_unreadable_sectors() {
    let n = if cfg!(miri) { 6 } else { 60 };
    let (image, audio) = dst_disc(n);
    let playtime = spans(&audio).len() as u32;
    let k = audio.len();
    let options = SpeculativeOptions::new().with_min_gap_sectors(0);
    let cases = [(k / 4, k / 4 + 1), (k * 3 / 4, k * 3 / 4 + 2)];
    for &bad in &cases[..if cfg!(miri) { 1 } else { 2 }] {
        let first = playtime / 2;
        let mut exact = marked(image.clone(), bad);
        let want = raw(&mut exact, (first, playtime));
        let mut disc = marked(image.clone(), bad);
        disc.enable_seek_index(SLOT, IndexOptions::new().with_interval(2)).unwrap();
        disc.set_seek_policy(SLOT, SeekPolicy::Speculative(options)).unwrap();
        let mut frames = disc.range_frames(SLOT, first, playtime).unwrap();
        let got: Vec<TrackFrame> = frames.by_ref().map(Result::unwrap).collect();
        let (speculation, missing, lost) =
            (frames.speculation(), frames.missing_frames(), frames.lost_frames());
        drop(frames);
        assert_eq!((got, missing, lost), want, "{bad:?}");
        while let Ok(false) = disc.verify_speculation(SLOT, 64) {}
        let marked_before =
            matches!(speculation, Speculation::Pending { lsn, .. } if lsn > (AUDIO_LSN + bad.0) as u64);
        for event in disc.take_seek_events(SLOT) {
            if marked_before {
                assert!(matches!(event.speculation, Speculation::Unverified { .. }), "{bad:?}: {event:?}");
            }
        }
    }
}

fn config() -> ProptestConfig {
    if cfg!(miri) {
        ProptestConfig { cases: 2, failure_persistence: None, ..ProptestConfig::default() }
    } else {
        let cases = std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(64);
        ProptestConfig { cases, ..ProptestConfig::default() }
    }
}

proptest! {
    #![proptest_config(config())]

    /// Frames of any size and kind, numbered one apart, and any unreadable sectors: lost
    /// exactly where a frame touches them, every other position as without them.
    #[test]
    fn lost_exactly_where_frames_touch(
        kinds in proptest::collection::vec((any::<bool>(), 1usize..if cfg!(miri) { 3000 } else { 9000 }, any::<u8>()), 1..if cfg!(miri) { 3 } else { 16 }),
        marks in proptest::collection::vec((0usize..64, 1usize..4), 0..4),
    ) {
        let frames: Vec<PFrame> = kinds
            .iter()
            .enumerate()
            .map(|(i, &(dst, len, seed))| {
                let len = if dst { len } else { 2 * FRAME_BYTES_PER_CHANNEL };
                PFrame { number: i as u32, dst, data: (0..len).map(|j| seed ^ j as u8).collect() }
            })
            .collect();
        let playtime = frames.len() as u32;
        let (image, audio) = disc_of(&frames, playtime);
        let spans = spans(&audio);
        let mut clean = Disc::open(Cursor::new(image.clone())).unwrap();
        let (want, _, _) = raw(&mut clean, (0, playtime));
        let disc = Disc::open(Cursor::new(image)).unwrap();
        let mut lost = BTreeSet::new();
        for &(a, k) in &marks {
            let a = a % audio.len();
            let b = (a + k).min(audio.len());
            disc.unreadable_sectors().insert((AUDIO_LSN + a) as u64, (b - a) as u64);
            lost.extend(touched(&spans, (a, b)));
        }
        let mut disc = disc;
        let (got, missing, n_lost) = raw(&mut disc, (0, playtime));
        check_raw("property", &got, &want, &lost);
        prop_assert_eq!((missing, n_lost), (0, lost.len() as u64));
    }
}

/// A request the reader reports unreadable is never made again: it is split at once, and
/// each sector is tried alone once (physical drives retry slowly).
#[test]
fn failing_reads_are_not_repeated() {
    let (image, audio) = dst_disc(if cfg!(miri) { 4 } else { 12 });
    let playtime = spans(&audio).len() as u32;
    for sequential in [false, true] {
        let flaky = Flaky::new(image.clone(), (2, 3));
        let failed = Arc::clone(&flaky.failed);
        if sequential {
            let mut disc = Disc::open(flaky).unwrap();
            raw(&mut disc, (0, playtime));
            let mut buf = vec![0u8; 4 * S];
            let e = disc.image_mut().read_sectors(AUDIO_LSN as u64 + 3, &mut buf[..S]);
            assert!(e.is_ok(), "{e:?}");
            let flaky = Flaky::new(image.clone(), (2, 3));
            let failed = Arc::clone(&flaky.failed);
            let mut fresh = Disc::open(flaky).unwrap();
            let e = fresh.image_mut().read_sectors(AUDIO_LSN as u64, &mut buf).unwrap_err();
            assert!(matches!(e, Error::SectorUnreadable { .. }), "{e:?}");
            let f = failed.lock().unwrap();
            assert_eq!(f.len(), 2, "read_sectors: the request once, then the sector once: {f:?}");
        } else {
            let mut disc = Disc::open(ReadAtReader::new(flaky)).unwrap();
            raw(&mut disc, (0, playtime));
        }
        let f = failed.lock().unwrap();
        let distinct: BTreeSet<_> = f.iter().collect();
        assert_eq!(distinct.len(), f.len(), "sequential {sequential}: a failing request repeated: {f:?}");
        assert!(f.iter().filter(|r| r.1 == S).count() == 1, "sequential {sequential}: {f:?}");
    }
}

/// A sector marked while a cursor is open, after the cursor read it into its buffer but
/// before it processed it, is unreadable to that cursor too.
#[test]
fn marks_made_mid_read_apply_to_buffered_sectors() {
    let (image, audio) = dst_disc(if cfg!(miri) { 4 } else { 12 });
    let spans = spans(&audio);
    let playtime = spans.len() as u32;
    // A sector within the cursor's first read (32 sectors), past the first frame.
    let a = spans[&1].1.min(30);
    let bad = (a, a + 1);
    let lost = touched(&spans, bad);
    assert!(!lost.contains(&0), "the first frame is read before the mark");
    let (want, _, _) = raw(&mut marked(image.clone(), bad), (0, playtime));
    let mut disc = Disc::open(Cursor::new(image)).unwrap();
    let set = disc.unreadable_sectors();
    let mut frames = disc.range_frames(SLOT, 0, playtime).unwrap();
    let mut got = vec![frames.next().unwrap().unwrap()];
    set.insert((AUDIO_LSN + a) as u64, 1);
    got.extend(frames.map(Result::unwrap));
    assert_eq!(got, want);
}

/// A disc of `n` DST frames in two tracks.
fn two_tracks(n: u32) -> (Vec<u8>, Vec<[u8; S]>) {
    let (_, audio) = dst_disc(n);
    let tracks =
        vec![TrackSpec { start: 0, duration: n / 2 }, TrackSpec { start: n / 2, duration: n - n / 2 }];
    (Spec { channels: 2, playtime: n, tracks, audio: audio.clone() }.build(), audio)
}

/// Continuing a cursor across unreadable sectors, with and without a seek index, gives
/// what fresh cursors give, and a stretch that spans the two ranges is lost in both.
#[test]
fn continuations_across_damage() {
    let n = if cfg!(miri) { 4 } else { 16 };
    let (image, audio) = two_tracks(n);
    let spans = spans(&audio);
    let half = n / 2;
    // The sector where frame `half` starts: the stretch reaches across the track boundary.
    let a = spans[&(half - 1)].1;
    let bad = (a, a + 1);
    for indexed in [false, true] {
        let mut fresh = marked(image.clone(), bad);
        let first = raw(&mut fresh, (0, half));
        let second = raw(&mut fresh, (half, n));
        let mut disc = marked(image.clone(), bad);
        if indexed {
            disc.enable_seek_index(SLOT, IndexOptions::new().with_interval(2)).unwrap();
        }
        let cursor =
            disc.frames_cursor(SLOT, scarletbook::FrameSpan::Range(0, half), Default::default()).unwrap();
        let mut frames = disc.read_frames(cursor);
        let got: Vec<TrackFrame> = frames.by_ref().map(Result::unwrap).collect();
        assert_eq!((got, frames.missing_frames(), frames.lost_frames()), first, "indexed {indexed}");
        let cursor = frames.into_cursor();
        let next = disc.continue_range_frames(cursor, SLOT, half, n).unwrap();
        let mut frames = disc.read_frames(next);
        let got: Vec<TrackFrame> = frames.by_ref().map(Result::unwrap).collect();
        assert_eq!((got, frames.missing_frames(), frames.lost_frames()), second, "indexed {indexed}");
        // Decoded, continued the same way.
        let mut audio_a = disc.range_audio(SLOT, 0, half).unwrap();
        let a_frames: Vec<AudioFrame> = audio_a.by_ref().map(Result::unwrap).collect();
        let c = audio_a.into_cursor();
        let c = disc.continue_range_audio(c, SLOT, half, n).unwrap();
        let b_frames: Vec<AudioFrame> = disc.read_track_audio(c).map(Result::unwrap).collect();
        let lost = touched(&spans, bad);
        for f in a_frames.iter().chain(&b_frames) {
            assert_eq!(
                f.status == FrameStatus::Lost,
                lost.contains(&f.number),
                "indexed {indexed}: {}",
                f.number
            );
        }
    }
}

/// A provisional range whose first frame lies inside a lost stretch, and one read through
/// a reader that reports sectors unreadable during the search, give the exact positions.
#[test]
fn provisional_ranges_inside_lost_stretches() {
    let n = if cfg!(miri) { 6 } else { 60 };
    let (image, audio) = dst_disc(n);
    let spans = spans(&audio);
    let k = audio.len();
    let bad = (k / 2, k / 2 + 2);
    let lost = touched(&spans, bad);
    let first = *lost.iter().nth(lost.len() / 2).unwrap();
    let options = SpeculativeOptions::new().with_min_gap_sectors(0);
    let want = raw(&mut marked(image.clone(), bad), (first, n));
    let mut disc = marked(image.clone(), bad);
    disc.set_seek_policy(SLOT, SeekPolicy::Speculative(options)).unwrap();
    assert_eq!(raw(&mut disc, (first, n)), want, "marked");
    // Reported by the reader, whose sectors the search's probes may read.
    for first in [first, n * 3 / 4] {
        let want = raw(&mut marked(image.clone(), bad), (first, n));
        let mut disc = Disc::open(ReadAtReader::new(Flaky::new(image.clone(), bad))).unwrap();
        disc.set_seek_policy(SLOT, SeekPolicy::Speculative(options)).unwrap();
        assert_eq!(raw(&mut disc, (first, n)), want, "reported, from {first}");
    }
}

/// `verify_speculation` ends (`Ok(true)`) when a mark stops the seek index, with every
/// reading settled; a claim whose own sector turns out unreadable is unverified, not an
/// error.
#[test]
fn verification_ends_with_unreadable_sectors() {
    let n = if cfg!(miri) { 6 } else { 60 };
    let (image, audio) = dst_disc(n);
    let k = audio.len();
    let options = SpeculativeOptions::new().with_min_gap_sectors(0);
    // A mark ahead of a provisional reading's L.
    let mut disc = marked(image.clone(), (k * 3 / 4, k * 3 / 4 + 1));
    disc.enable_seek_index(SLOT, IndexOptions::new().with_interval(2)).unwrap();
    disc.set_seek_policy(SLOT, SeekPolicy::Speculative(options)).unwrap();
    let mut frames = disc.range_frames(SLOT, n / 2, n).unwrap();
    frames.by_ref().for_each(drop);
    let speculation = frames.speculation();
    drop(frames);
    let mut rounds = 0;
    while !disc.verify_speculation(SLOT, 8).unwrap() {
        rounds += 1;
        assert!(rounds < 10_000, "verify_speculation never ends");
    }
    if matches!(speculation, Speculation::Pending { .. }) {
        assert_eq!(disc.take_seek_events(SLOT).len(), 1, "the reading is settled");
    }
    // The claim's own sector fails when the check reads it alone, after the pass read it
    // with its neighbours.
    let flaky = Flaky::new(image, (k, k));
    let bad = Arc::clone(&flaky.bad);
    flaky.single_only.store(true, std::sync::atomic::Ordering::Relaxed);
    let mut disc = Disc::open(ReadAtReader::new(flaky)).unwrap();
    disc.enable_seek_index(SLOT, IndexOptions::new().with_interval(2)).unwrap();
    disc.set_seek_policy(SLOT, SeekPolicy::Speculative(options)).unwrap();
    let mut frames = disc.range_frames(SLOT, n / 2, n).unwrap();
    frames.by_ref().for_each(drop);
    let speculation = frames.speculation();
    drop(frames);
    let Speculation::Pending { lsn, .. } = speculation else {
        // Under Miri the area is too short for a provisional start.
        if !cfg!(miri) {
            panic!("not provisional: {speculation:?}");
        }
        return;
    };
    *bad.lock().unwrap() = ((lsn as usize * S) as u64, ((lsn as usize + 1) * S) as u64);
    let mut rounds = 0;
    while !disc.verify_speculation(SLOT, 64).unwrap() {
        rounds += 1;
        assert!(rounds < 10_000, "verify_speculation never ends");
    }
    let events = disc.take_seek_events(SLOT);
    assert!(
        events.iter().all(|e| matches!(e.speculation, Speculation::Unverified { .. })) && !events.is_empty(),
        "{events:?}"
    );
}
