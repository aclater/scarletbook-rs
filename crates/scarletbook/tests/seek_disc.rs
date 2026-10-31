// SPDX-License-Identifier: MIT OR Apache-2.0

//! A disc with a seek index enabled (issue #68): every cursor it makes or continues reads
//! exactly what it reads without one, and builds the index as it goes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::cell::Cell;
use std::io::Cursor;
use std::rc::Rc;

use common::hostile::{S, stream};
use common::{PFrame, Spec, TrackSpec, pack};
use proptest::prelude::*;
use scarletbook::{AudioFrame, Disc, Error, IndexOptions, Slot, TrackAudioCursor, Warning};

fn config() -> ProptestConfig {
    if cfg!(miri) {
        ProptestConfig { cases: 2, failure_persistence: None, ..ProptestConfig::default() }
    } else {
        let cases = std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(128);
        ProptestConfig { cases, ..ProptestConfig::default() }
    }
}

/// A 2-channel disc of the given audio sectors with three tracks over its play time (at
/// most 300 frames, within its capacity).
fn image(audio: Vec<[u8; S]>) -> (Vec<u8>, u32) {
    let playtime = (7 * audio.len() as u32).min(300);
    let third = (playtime / 3).max(1);
    let tracks = vec![
        TrackSpec { start: 0, duration: third },
        TrackSpec { start: third.min(playtime - 1), duration: (playtime - third).max(1).min(third) },
        TrackSpec {
            start: (2 * third).min(playtime - 1),
            duration: playtime - (2 * third).min(playtime - 1),
        },
    ];
    (Spec { channels: 2, playtime, tracks, audio }.build(), playtime)
}

/// One read: a range or a track, fresh or continuing the previous cursor, stopped after
/// `take` frames.
#[derive(Clone, Debug)]
enum Op {
    Range { a: f64, b: f64, take: usize, cont: bool },
    Track { n: u32, take: usize, cont: bool },
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0.0f64..1.0, 0.0f64..1.0, 0usize..30, any::<bool>()).prop_map(|(a, b, take, cont)| Op::Range {
            a,
            b,
            take,
            cont
        }),
        (1u32..=3, 0usize..30, any::<bool>()).prop_map(|(n, take, cont)| Op::Track { n, take, cont }),
    ]
}

#[derive(Debug, PartialEq)]
struct Got {
    /// Number, status and a hash of the audio of each frame.
    frames: Vec<(u32, scarletbook::FrameStatus, u64)>,
    missing: u64,
    invalid: u64,
    malformed: u64,
    warnings: Vec<Warning>,
}

fn hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// Runs `ops` on `disc`, returning what each read gave (or its error).
fn run(disc: &mut Disc<Cursor<Vec<u8>>>, playtime: u32, ops: &[Op]) -> Vec<Result<Got, String>> {
    let mut prev: Option<TrackAudioCursor> = None;
    let mut out = Vec::new();
    for op in ops {
        let (cursor, take) = match *op {
            Op::Range { a, b, take, cont } => {
                let first = (a * f64::from(playtime)) as u32;
                let end = first + 1 + (b * f64::from(playtime - first - 1)) as u32;
                let c = match prev.take().filter(|_| cont) {
                    Some(p) => disc.continue_range_audio(p, Slot::TwoChannel, first, end),
                    None => disc.range_audio_cursor(Slot::TwoChannel, first, end),
                };
                (c, take)
            }
            Op::Track { n, take, cont } => {
                let c = match prev.take().filter(|_| cont) {
                    Some(p) => disc.continue_track_audio(p, Slot::TwoChannel, n),
                    None => disc.track_audio_cursor(Slot::TwoChannel, n),
                };
                (c, take)
            }
        };
        let cursor = match cursor {
            Ok(c) => c,
            Err(e) => {
                out.push(Err(e.to_string()));
                continue;
            }
        };
        let mut it = disc.read_track_audio(cursor);
        let frames = it
            .by_ref()
            .take(take)
            .map(Result::unwrap)
            .map(|f: AudioFrame| (f.number, f.status, hash(&f.data)))
            .collect();
        let c = it.into_cursor();
        // Before its first frame, a continued cursor reports the counts of the scan it
        // continues, and one the index moved ahead those of nothing read yet: compare the
        // counts once a frame was read.
        let read = take > 0 || !matches!(*op, Op::Range { cont: true, .. } | Op::Track { cont: true, .. });
        out.push(Ok(Got {
            frames,
            missing: c.missing_frames(),
            invalid: c.invalid_dst_frames(),
            malformed: if read { c.malformed_sectors() } else { 0 },
            warnings: if read { c.warnings().to_vec() } else { Vec::new() },
        }));
        prev = Some(c);
    }
    out
}

proptest! {
    #![proptest_config(config())]

    /// Any sequence of reads, fresh and continued, forwards, backwards and repeated, gives
    /// the same through a disc with a seek index as through one without.
    #[test]
    #[cfg_attr(miri, ignore = "decodes many ranges")]
    fn reads_with_an_index_equal_reads_without(
        audio in stream(),
        interval in prop_oneof![Just(1u32), Just(4), Just(64)],
        eager in proptest::option::of(0u64..400),
        ops in proptest::collection::vec(op(), 1..10),
    ) {
        let (img, playtime) = image(audio);
        let mut plain = Disc::open(Cursor::new(img.clone())).unwrap();
        let mut indexed = Disc::open(Cursor::new(img)).unwrap();
        indexed.enable_seek_index(Slot::TwoChannel, IndexOptions::new().with_interval(interval)).unwrap();
        if let Some(sectors) = eager {
            indexed.extend_seek_index(Slot::TwoChannel, sectors).unwrap();
        }
        let want = run(&mut plain, playtime, &ops);
        let got = run(&mut indexed, playtime, &ops);
        prop_assert_eq!(got, want);
        // And again, with whatever the first round built.
        let got = run(&mut indexed, playtime, &ops);
        let want = run(&mut plain, playtime, &ops);
        prop_assert_eq!(got, want);
    }
}

/// A reader that counts the bytes read through it.
struct Counting<R> {
    inner: R,
    read: Rc<Cell<u64>>,
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

fn long_disc() -> (Vec<u8>, u32, u64) {
    let fs: Vec<PFrame> = (0..1000u32)
        .map(|n| PFrame { number: n, dst: true, data: vec![n as u8; 4000 + (n as usize * 37) % 6000] })
        .collect();
    let audio = pack(&fs);
    let sectors = audio.len() as u64;
    let spec =
        Spec { channels: 2, playtime: 1000, tracks: vec![TrackSpec { start: 0, duration: 1000 }], audio };
    (spec.build(), 1000, sectors)
}

/// Reads through the disc build the index; later reads anywhere cost a bounded number of
/// sectors, and a continuation jumps ahead through the index.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn lazy_index_bounds_later_reads() {
    let (img, playtime, sectors) = long_disc();
    let read = Rc::new(Cell::new(0u64));
    let mut disc = Disc::open(Counting { inner: Cursor::new(img), read: read.clone() }).unwrap();
    disc.enable_seek_index(Slot::TwoChannel, IndexOptions::new()).unwrap();
    // The last frame: a scan of the whole area, which completes the index.
    let start = read.get();
    disc.range_frames(Slot::TwoChannel, playtime - 1, playtime).unwrap().for_each(|f| drop(f.unwrap()));
    assert_eq!((read.get() - start) / S as u64, sectors);
    let index = disc.seek_index(Slot::TwoChannel).unwrap();
    assert!(index.is_complete(), "frontier {}", index.frontier());
    for first in (0..playtime).step_by(53) {
        let start = read.get();
        let f = disc.range_frames(Slot::TwoChannel, first, playtime).unwrap().next().unwrap().unwrap();
        assert_eq!(f.number, first);
        let cost = (read.get() - start) / S as u64;
        assert!(cost <= 32 + 64 + 32 + 5, "first {first}: {cost} sectors");
    }
    // A continuation from the start into a range near the end jumps through the index.
    let c = disc.range_frames_cursor(Slot::TwoChannel, 0, 10).unwrap();
    let mut it = disc.read_frames(c);
    it.next().unwrap().unwrap();
    let c = it.into_cursor();
    let start = read.get();
    let c = disc.continue_range_frames(c, Slot::TwoChannel, 900, 910).unwrap();
    assert!(matches!(c.resumed_from(), scarletbook::ResumedFrom::Checkpoint { .. }));
    let f = disc.read_frames(c).next().unwrap().unwrap();
    assert_eq!(f.number, 900);
    assert!((read.get() - start) / (S as u64) < 200);
}

/// A continuation that goes backwards resumes from the index rather than the first sector.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn backwards_continuation_resumes_from_the_index() {
    let (img, playtime, _) = long_disc();
    let read = Rc::new(Cell::new(0u64));
    let mut disc = Disc::open(Counting { inner: Cursor::new(img), read: read.clone() }).unwrap();
    disc.enable_seek_index(Slot::TwoChannel, IndexOptions::new()).unwrap();
    while !disc.extend_seek_index(Slot::TwoChannel, 1000).unwrap() {}
    let c = disc.range_audio_cursor(Slot::TwoChannel, 900, playtime).unwrap();
    let (_, c) = {
        let mut it = disc.read_track_audio(c);
        let f = it.next().unwrap().unwrap();
        (f, it.into_cursor())
    };
    let start = read.get();
    let c = disc.continue_range_audio(c, Slot::TwoChannel, 500, 510).unwrap();
    let f = disc.read_track_audio(c).next().unwrap().unwrap();
    assert_eq!(f.number, 500);
    assert!((read.get() - start) / (S as u64) < 200);
}

#[test]
fn index_management() {
    let (img, _) = image(vec![[0u8; S]; 4]);
    let mut disc = Disc::open(Cursor::new(img.clone())).unwrap();
    assert!(disc.seek_index(Slot::TwoChannel).is_none());
    assert!(matches!(disc.extend_seek_index(Slot::TwoChannel, 10), Err(Error::InvalidArgument(_))));
    assert!(matches!(
        disc.enable_seek_index(Slot::Multichannel, IndexOptions::new()),
        Err(Error::NoSuchArea(Slot::Multichannel))
    ));
    assert!(disc.enable_seek_index(Slot::TwoChannel, IndexOptions::new().with_interval(0)).is_err());
    disc.enable_seek_index(Slot::TwoChannel, IndexOptions::new().with_interval(2)).unwrap();
    assert!(disc.extend_seek_index(Slot::TwoChannel, 10).unwrap());
    let index = disc.take_seek_index(Slot::TwoChannel).unwrap();
    assert!(index.is_complete());
    assert!(disc.seek_index(Slot::TwoChannel).is_none());
    // Another image size: refused.
    let mut longer = img.clone();
    longer.extend_from_slice(&[0u8; S]);
    let mut other = Disc::open(Cursor::new(longer)).unwrap();
    assert!(matches!(other.set_seek_index(Slot::TwoChannel, index.clone()), Err(Error::InvalidArgument(_))));
    disc.set_seek_index(Slot::TwoChannel, index.clone()).unwrap();
    assert_eq!(disc.seek_index(Slot::TwoChannel), Some(index));
}

/// The opening is logged as reading from the index, and the resume at Debug.
#[test]
fn seeks_are_logged() {
    let fs: Vec<PFrame> =
        (0..6u32).map(|n| PFrame { number: n, dst: true, data: vec![n as u8; 2500] }).collect();
    let (img, playtime) = image(pack(&fs));
    let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = std::sync::Arc::clone(&lines);
    let log = scarletbook::log::Log::new(scarletbook::log::Level::Debug, move |_, m| {
        sink.lock().unwrap().push(m.to_string());
    });
    let mut disc = Disc::open_with_log(Cursor::new(img), log).unwrap();
    disc.enable_seek_index(Slot::TwoChannel, IndexOptions::new().with_interval(1)).unwrap();
    while !disc.extend_seek_index(Slot::TwoChannel, 100).unwrap() {}
    lines.lock().unwrap().clear();
    disc.range_frames(Slot::TwoChannel, 4, playtime).unwrap().take(1).for_each(|f| drop(f.unwrap()));
    let lines = lines.lock().unwrap();
    assert!(lines.iter().any(|l| l.contains("frames 4..") && l.contains("from the seek index")), "{lines:?}");
    assert!(
        lines.iter().any(|l| l.contains("resuming at LSN") && l.contains("from seek index")),
        "{lines:?}"
    );
}

/// A caller's own index, without the disc keeping one: cursors from it and continuations
/// through it read as without, and fill it.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn a_callers_index() {
    let (img, playtime, _) = long_disc();
    let mut disc = Disc::open(Cursor::new(img.clone())).unwrap();
    let mut plain = Disc::open(Cursor::new(img)).unwrap();
    let area = disc.toc().areas[0].clone();
    let mut index = scarletbook::AreaIndex::new(&area, disc.toc().sectors, IndexOptions::new()).unwrap();
    let mut prev: Option<TrackAudioCursor> = None;
    for (first, end) in [(990, 1000), (100, 120), (500, 503), (40, 41), (700, playtime)] {
        let c = match prev.take() {
            None => disc.range_audio_cursor_indexed(Slot::TwoChannel, first, end, &index).unwrap(),
            Some(p) => {
                let next = disc.range_continuation(Slot::TwoChannel, first, end).unwrap();
                next.check_index(&index).unwrap();
                next.log_audio_indexed(&p, &index);
                next.audio_indexed_unlogged(p, &mut index)
            }
        };
        let mut it = disc.read_track_audio(c);
        let got: Vec<_> = it.by_ref().map(Result::unwrap).collect();
        let c = it.into_cursor();
        let want: Vec<_> =
            plain.range_audio(Slot::TwoChannel, first, end).unwrap().map(Result::unwrap).collect();
        assert!(got == want, "[{first}, {end})");
        index.absorb(&c).unwrap();
        prev = Some(c);
    }
    assert!(index.is_complete());
    // Another area's index is refused before anything is read.
    let mut other = area.clone();
    other.audio_end_lsn -= 1;
    let foreign = scarletbook::AreaIndex::new(&other, disc.toc().sectors, IndexOptions::new()).unwrap();
    assert!(matches!(
        disc.range_audio_cursor_indexed(Slot::TwoChannel, 0, 1, &foreign),
        Err(Error::InvalidArgument(_))
    ));
}

/// Discs of one image on two threads ([`Disc::share`], `clone`) keep one index: one
/// thread's reads build it, the other's seek then starts from it; both read exactly what a
/// disc without an index reads.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn clones_on_two_threads_share_one_index() {
    let (img, playtime, _) = long_disc();
    let mut plain = Disc::open(Cursor::new(img.clone())).unwrap();
    let want: Vec<_> = [(999, 1000), (900, 910), (100, 103)]
        .iter()
        .map(|&(a, b)| {
            plain.range_frames(Slot::TwoChannel, a, b).unwrap().map(Result::unwrap).collect::<Vec<_>>()
        })
        .collect();
    let source: std::sync::Arc<[u8]> = img.into();
    let mut disc = Disc::open(scarletbook::ReadAtReader::new(source)).unwrap();
    disc.enable_seek_index(Slot::TwoChannel, IndexOptions::new()).unwrap();
    // A shared disc on another reader, counting its reads.
    let read = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    struct Counted<R>(R, std::sync::Arc<std::sync::atomic::AtomicU64>);
    impl<R: std::io::Read> std::io::Read for Counted<R> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.0.read(buf)?;
            self.1.fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
            Ok(n)
        }
    }
    impl<R: std::io::Seek> std::io::Seek for Counted<R> {
        fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
            self.0.seek(pos)
        }
    }
    let bytes = long_disc().0;
    let mut shared = disc.share(Counted(Cursor::new(bytes), std::sync::Arc::clone(&read))).unwrap();
    let mut builder = disc.clone();
    // Thread 1 reads the last frame: a scan of the whole area, which completes the index.
    let first = std::thread::spawn(move || {
        let got: Vec<_> =
            builder.range_frames(Slot::TwoChannel, 999, playtime).unwrap().map(Result::unwrap).collect();
        got
    })
    .join()
    .unwrap();
    assert_eq!(first, want[0]);
    assert!(disc.seek_index(Slot::TwoChannel).unwrap().is_complete());
    // Thread 2, on the shared disc, seeks with it.
    let (second, third, sectors) = std::thread::spawn(move || {
        let start = read.load(std::sync::atomic::Ordering::Relaxed);
        let second: Vec<_> =
            shared.range_frames(Slot::TwoChannel, 900, 910).unwrap().map(Result::unwrap).collect();
        let sectors = (read.load(std::sync::atomic::Ordering::Relaxed) - start) / S as u64;
        let third: Vec<_> =
            shared.range_frames(Slot::TwoChannel, 100, 103).unwrap().map(Result::unwrap).collect();
        (second, third, sectors)
    })
    .join()
    .unwrap();
    assert_eq!(second, want[1]);
    assert_eq!(third, want[2]);
    assert!(sectors < 200, "{sectors} sectors for a seek through the shared index");
}

/// Two clones reading at once, each building and using the one index, read as a disc
/// without one.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn clones_reading_at_once_build_one_index() {
    let (img, playtime, _) = long_disc();
    let mut plain = Disc::open(Cursor::new(img.clone())).unwrap();
    let ranges: Vec<(u32, u32)> =
        (0..24).map(|i| ((i * 397) % (playtime - 5), (i * 397) % (playtime - 5) + 5)).collect();
    let want: Vec<Vec<_>> = ranges
        .iter()
        .map(|&(a, b)| plain.range_frames(Slot::TwoChannel, a, b).unwrap().map(Result::unwrap).collect())
        .collect();
    let source: std::sync::Arc<[u8]> = img.into();
    let disc = Disc::open(scarletbook::ReadAtReader::new(source)).unwrap();
    let mut disc = disc;
    disc.enable_seek_index(Slot::TwoChannel, IndexOptions::new().with_interval(16)).unwrap();
    std::thread::scope(|s| {
        for t in 0..2 {
            let mut d = disc.clone();
            let (ranges, want) = (&ranges, &want);
            s.spawn(move || {
                for i in (0..ranges.len()).map(|i| if t == 0 { i } else { ranges.len() - 1 - i }) {
                    let (a, b) = ranges[i];
                    let got: Vec<_> =
                        d.range_frames(Slot::TwoChannel, a, b).unwrap().map(Result::unwrap).collect();
                    assert_eq!(got, want[i], "thread {t}, [{a}, {b})");
                }
            });
        }
    });
    let index = disc.seek_index(Slot::TwoChannel).unwrap();
    assert!(index.checkpoints() > 100, "{}", index.checkpoints());
}
