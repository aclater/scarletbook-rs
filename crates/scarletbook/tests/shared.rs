// SPDX-License-Identifier: MIT OR Apache-2.0

//! Discs of one image read on several threads at once ([`Disc::share`], `clone`): the
//! results are exactly those of one reader, with the same errors, on every image.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use common::{PFrame, Spec, TrackSpec, pack};
use scarletbook::audio::FRAME_BYTES_PER_CHANNEL;
use scarletbook::{Disc, Error, ReadAt, ReadAtReader, SectorFormat, Slot, TrackFrame, Warning};

// Discs of a shared source may be sent to and shared between threads.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Disc<ReadAtReader<Arc<std::fs::File>>>>();
    assert_send_sync::<Disc<ReadAtReader<Arc<[u8]>>>>();
    assert_send_sync::<Disc<ReadAtReader<&[u8]>>>();
};

/// Everything a read of a track gives: every position or error, and the counts.
#[derive(Debug, PartialEq)]
struct Outcome {
    frames: Vec<Result<TrackFrame, String>>,
    missing: u64,
    malformed: u64,
    warnings: Vec<Warning>,
}

fn read_track<R: Read + Seek>(disc: &mut Disc<R>, slot: Slot, n: u32) -> Result<Outcome, String> {
    let cursor = disc.track_frames_cursor(slot, n).map_err(|e| e.to_string())?;
    let mut frames = disc.read_frames(cursor);
    let list = frames.by_ref().map(|f| f.map_err(|e| e.to_string())).collect();
    let c = frames.into_cursor();
    Ok(Outcome {
        frames: list,
        missing: c.missing_frames(),
        malformed: c.malformed_sectors(),
        warnings: c.warnings().to_vec(),
    })
}

fn tracks<R: Read + Seek>(disc: &Disc<R>) -> Vec<(Slot, u32)> {
    disc.toc().areas.iter().flat_map(|a| (1..=a.tracks.len() as u32).map(move |n| (a.slot, n))).collect()
}

/// Reads every track of `disc` on `threads` threads, each through a disc of its own made
/// by `make`, odd threads walking their tracks backwards.
fn read_on_threads<R: Read + Seek, R2: Read + Seek + Send>(
    disc: &Disc<R>,
    threads: usize,
    make: impl Fn(&Disc<R>) -> Disc<R2>,
) -> Vec<Result<Outcome, String>> {
    let jobs = tracks(disc);
    let mut out: Vec<Option<Result<Outcome, String>>> = jobs.iter().map(|_| None).collect();
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|t| {
                let mut mine = make(disc);
                let jobs = &jobs;
                s.spawn(move || {
                    let mut order: Vec<usize> = (t..jobs.len()).step_by(threads).collect();
                    if t % 2 == 1 {
                        order.reverse();
                    }
                    order
                        .into_iter()
                        .map(|j| (j, read_track(&mut mine, jobs[j].0, jobs[j].1)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for h in handles {
            for (j, o) in h.join().unwrap() {
                out[j] = Some(o);
            }
        }
    });
    out.into_iter().map(Option::unwrap).collect()
}

/// Serial reads through a `Cursor` equal reads on 1, 2, 3 and 8 threads through clones and
/// through shares.
fn check_image(bytes: Vec<u8>, name: &str) {
    let Ok(mut serial) = Disc::open(Cursor::new(bytes.clone())) else { return };
    let want: Vec<_> = tracks(&serial).into_iter().map(|(s, n)| read_track(&mut serial, s, n)).collect();
    let source: Arc<[u8]> = bytes.into();
    let disc = Disc::open(ReadAtReader::new(Arc::clone(&source))).unwrap();
    assert_eq!(disc.toc(), serial.toc(), "{name}");
    assert_eq!(disc.warnings(), serial.warnings(), "{name}");
    for threads in [1, 2, 3, 8] {
        assert_eq!(read_on_threads(&disc, threads, Disc::clone), want, "{name}: {threads} threads, clones");
    }
    let shares = read_on_threads(&disc, 3, |d| d.share(ReadAtReader::new(Arc::clone(&source))).unwrap());
    assert_eq!(shares, want, "{name}: shares");
}

fn corpus() -> Vec<(String, Vec<u8>)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/images");
    let mut images: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let p = e.unwrap().path();
            (p.file_name().unwrap().to_string_lossy().into_owned(), std::fs::read(&p).unwrap())
        })
        .collect();
    images.sort();
    assert!(images.len() > 50);
    images
}

/// The image in another sector format: each 2048-byte sector stored in `P` bytes at `D`.
fn reformat(plain: &[u8], format: SectorFormat) -> Vec<u8> {
    let (p, d) = (format.stored_size() as usize, format.data_offset() as usize);
    let mut out = Vec::with_capacity(plain.len() / 2048 * p);
    for sector in plain.chunks_exact(2048) {
        let at = out.len();
        out.resize(at + p, 0x5A);
        out[at + d..at + d + 2048].copy_from_slice(sector);
    }
    out
}

#[test]
#[cfg_attr(miri, ignore = "reads every corpus image")]
fn threads_equal_serial_reads_on_the_corpus() {
    for (name, bytes) in corpus() {
        check_image(bytes, &name);
    }
}

#[test]
#[cfg_attr(miri, ignore = "reads corpus images")]
fn threads_equal_serial_reads_in_raw_formats() {
    for (name, bytes) in
        corpus().into_iter().filter(|(n, _)| n.starts_with("dst-") || n.starts_with("audio-"))
    {
        for format in [SectorFormat::Raw2054, SectorFormat::Raw2064] {
            check_image(reformat(&bytes, format), &format!("{name} as {format:?}"));
        }
    }
}

/// A small disc with plain and DST frames in two tracks, without corpus files (Miri).
fn synthetic() -> Vec<u8> {
    let frames: Vec<PFrame> = (0..10)
        .filter(|n| n % 4 != 3)
        .map(|n| {
            let dst = n % 2 == 1;
            let len = if dst { 40 + n as usize } else { 2 * FRAME_BYTES_PER_CHANNEL };
            PFrame { number: n, dst, data: (0..len).map(|i| (i as u8) ^ (n as u8)).collect() }
        })
        .collect();
    let spec = Spec {
        channels: 2,
        playtime: 10,
        tracks: vec![TrackSpec { start: 0, duration: 4 }, TrackSpec { start: 4, duration: 6 }],
        audio: pack(&frames),
    };
    spec.build()
}

#[test]
fn threads_equal_serial_reads_on_a_synthetic_disc() {
    check_image(synthetic(), "synthetic");
}

/// A source that counts the bytes read and fails (with an error, or as a short read) in a
/// 4 KiB window starting at `bad`.
struct Faulty {
    data: Arc<[u8]>,
    bad: u64,
    eof: bool,
    read: AtomicU64,
}

impl Faulty {
    fn fault(&self, offset: u64) -> Option<io::Result<usize>> {
        (self.bad..self.bad + 4096)
            .contains(&offset)
            .then(|| if self.eof { Ok(0) } else { Err(io::Error::other("injected")) })
    }

    /// How many bytes from `offset` can be read before the fault.
    fn room(&self, offset: u64, len: usize) -> usize {
        if offset < self.bad { len.min((self.bad - offset) as usize) } else { len }
    }
}

impl ReadAt for Faulty {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if let Some(r) = self.fault(offset) {
            return r;
        }
        let n = self.room(offset, buf.len());
        let n = self.data.read_at(offset, &mut buf[..n])?;
        self.read.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }

    fn size(&self) -> io::Result<u64> {
        self.data.size()
    }
}

/// The same faults through `Read + Seek`, as a reader of today shows them.
struct FaultySeek(Arc<Faulty>, u64);

impl Read for FaultySeek {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if let Some(r) = self.0.fault(self.1) {
            return r;
        }
        let n = self.0.room(self.1, buf.len());
        let mut c = Cursor::new(&self.0.data[..]);
        c.set_position(self.1);
        let n = c.read(&mut buf[..n])?;
        self.1 += n as u64;
        Ok(n)
    }
}

impl Seek for FaultySeek {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let mut c = Cursor::new(&self.0.data[..]);
        c.set_position(self.1);
        self.1 = c.seek(pos)?;
        Ok(self.1)
    }
}

#[test]
#[cfg_attr(miri, ignore = "reads corpus images")]
fn read_failures_and_short_reads_are_unchanged() {
    for (name, bytes) in corpus().into_iter().filter(|(n, _)| n.starts_with("dst-features")) {
        let data: Arc<[u8]> = bytes.into();
        let probe = Disc::open(Cursor::new(&data[..])).unwrap();
        for area in &probe.toc().areas {
            let mid = (u64::from(area.audio_start_lsn) + u64::from(area.audio_end_lsn)) / 2;
            for eof in [false, true] {
                // Inside a sector: the chunk read stops short in its middle.
                let source = Arc::new(Faulty {
                    data: Arc::clone(&data),
                    bad: mid * 2048 + 100,
                    eof,
                    read: AtomicU64::new(0),
                });
                let mut seek = Disc::open(FaultySeek(Arc::clone(&source), 0)).unwrap();
                let mut at = Disc::open(ReadAtReader::new(Arc::clone(&source))).unwrap();
                let mut clone = at.clone();
                let mut shared = at.share(ReadAtReader::new(Arc::clone(&source))).unwrap();
                let mut failed = 0;
                for (s, n) in tracks(&at) {
                    let want = read_track(&mut seek, s, n);
                    failed += want.as_ref().map_or(0, |o| o.frames.iter().filter(|f| f.is_err()).count());
                    assert_eq!(read_track(&mut at, s, n), want, "{name} {s} {n} eof {eof}");
                    assert_eq!(read_track(&mut clone, s, n), want, "{name} {s} {n} eof {eof}");
                    assert_eq!(read_track(&mut shared, s, n), want, "{name} {s} {n} eof {eof}");
                }
                assert!(failed > 0, "{name}: the fault must fail some reads");
            }
        }
    }
}

#[test]
fn a_cursor_reads_through_any_disc_of_the_image() {
    let bytes = synthetic();
    let mut serial = Disc::open(Cursor::new(bytes.clone())).unwrap();
    let want = read_track(&mut serial, Slot::TwoChannel, 2).unwrap();
    let mut a = Disc::open(ReadAtReader::new(&bytes[..])).unwrap();
    let mut b = a.clone();
    let mut c = a.share(Cursor::new(bytes.clone())).unwrap();
    // Positions alternate between the three discs.
    let mut cursor = a.track_frames_cursor(Slot::TwoChannel, 2).unwrap();
    let mut got = Vec::new();
    for i in 0.. {
        let f = match i % 3 {
            0 => cursor.next_frame(a.image_mut()),
            1 => cursor.next_frame(b.image_mut()),
            _ => cursor.next_frame(c.image_mut()),
        };
        match f {
            Some(f) => got.push(f.map_err(|e| e.to_string())),
            None => break,
        }
    }
    assert_eq!(got, want.frames);
    assert_eq!(cursor.missing_frames(), want.missing);
    assert_eq!(cursor.malformed_sectors(), want.malformed);
}

#[test]
fn shares_have_their_originals_identifier_and_others_do_not() {
    let bytes = synthetic();
    let mut a = Disc::open(ReadAtReader::new(&bytes[..])).unwrap();
    let mut b = a.clone();
    let mut c = a.share(Cursor::new(bytes.clone())).unwrap();
    let mut other = Disc::open(Cursor::new(bytes.clone())).unwrap();
    let id = a.image_mut().id();
    assert_eq!((b.image_mut().id(), c.image_mut().id()), (id, id));
    assert_ne!(other.image_mut().id(), id);
    // A cursor of the family is refused by an image opened on its own, even of the same bytes.
    // (An area cursor stays usable with its own image after the refusal.)
    let mut cursor = a.stored_frame_list_cursor(Slot::TwoChannel).unwrap();
    assert!(cursor.next_frame(b.image_mut()).unwrap().is_ok());
    assert!(matches!(cursor.next_frame(other.image_mut()), Some(Err(Error::InvalidArgument(_)))));
    assert!(cursor.next_frame(c.image_mut()).unwrap().is_ok());
    // A continuation onto a share keeps the reading state (no sector read again); onto
    // an image opened on its own it scans afresh.
    let source = Arc::new(Faulty {
        data: bytes.clone().into(),
        bad: u64::MAX - 8192,
        eof: false,
        read: AtomicU64::new(0),
    });
    let mut a = Disc::open(ReadAtReader::new(Arc::clone(&source))).unwrap();
    let mut c = a.share(ReadAtReader::new(Arc::clone(&source))).unwrap();
    let mut other = Disc::open(ReadAtReader::new(Arc::clone(&source))).unwrap();
    for (onto_share, rereads) in [(true, false), (false, true)] {
        let mut track = a.track_frames_cursor(Slot::TwoChannel, 1).unwrap();
        assert!(track.next_frame(a.image_mut()).unwrap().is_ok());
        let before = source.read.load(Ordering::Relaxed);
        let (disc, next) = if onto_share {
            let next = c.continue_track_frames(track, Slot::TwoChannel, 2).unwrap();
            (&mut c, next)
        } else {
            let next = other.continue_track_frames(track, Slot::TwoChannel, 2).unwrap();
            (&mut other, next)
        };
        disc.read_frames(next).for_each(|f| drop(f.unwrap()));
        assert_eq!(source.read.load(Ordering::Relaxed) > before, rereads, "onto a share: {onto_share}");
    }
}

#[test]
fn a_continuation_across_discs_of_the_image_reads_each_sector_once() {
    let mut images = vec![("synthetic".to_string(), synthetic())];
    if !cfg!(miri) {
        images.extend(corpus().into_iter().filter(|(n, _)| n.starts_with("dst-features")));
    }
    for (name, bytes) in images {
        let source = Arc::new(Faulty {
            data: bytes.into(),
            bad: u64::MAX - 8192,
            eof: false,
            read: AtomicU64::new(0),
        });
        let first = Disc::open(ReadAtReader::new(Arc::clone(&source))).unwrap();
        let mut discs =
            [first.clone(), first.share(ReadAtReader::new(Arc::clone(&source))).unwrap(), first.clone()];
        let sectors = first.toc().sectors;
        for area in &first.toc().areas {
            let before = source.read.load(Ordering::Relaxed);
            let mut cursor = None;
            // Each track through the next disc, continuing the previous track's cursor.
            for n in 1..=area.tracks.len() as u32 {
                let disc = &mut discs[n as usize % 3];
                if disc.check_track(area.slot, n).is_err() {
                    continue;
                }
                let next = match cursor.take() {
                    Some(prev) => disc.continue_track_frames(prev, area.slot, n).unwrap(),
                    None => disc.track_frames_cursor(area.slot, n).unwrap(),
                };
                let mut frames = disc.read_frames(next);
                frames.by_ref().for_each(|f| drop(f.unwrap()));
                cursor = Some(frames.into_cursor());
            }
            let (start, end) = scarletbook::audio::audio_sector_range(area, sectors);
            let read = source.read.load(Ordering::Relaxed) - before;
            assert!(
                read <= (end - start) * 2048,
                "{name} {}: {read} bytes read, more than one pass",
                area.slot
            );
        }
    }
}

#[test]
fn share_refuses_a_reader_of_another_image() {
    let bytes = synthetic();
    let disc = Disc::open(ReadAtReader::new(&bytes[..])).unwrap();
    assert!(disc.share(Cursor::new(&bytes[..])).is_ok());
    // Another length.
    let mut longer = bytes.clone();
    longer.extend_from_slice(&[0u8; 2048]);
    assert!(matches!(disc.share(Cursor::new(longer)), Err(Error::InvalidArgument(_))));
    // The same length, with a byte changed in the master TOC sector, in the area TOC copy
    // used, or (not checked) in the audio.
    let changed = |at: usize| {
        let mut v = bytes.clone();
        v[at] ^= 0xFF;
        disc.share(Cursor::new(v)).map(drop)
    };
    assert!(matches!(changed(510 * 2048 + 2000), Err(Error::InvalidArgument(_))));
    assert!(matches!(changed(common::AREA_TOC_LSN * 2048 + 3 * 2048 + 7), Err(Error::InvalidArgument(_))));
    assert!(changed(common::AUDIO_LSN * 2048 + 100).is_ok());
    // A reader that fails.
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("broken"))
        }
    }
    impl Seek for Broken {
        fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
            Err(io::Error::other("broken"))
        }
    }
    assert!(matches!(disc.share(Broken), Err(Error::Io(_))));
}

#[test]
fn a_share_reports_to_its_own_log() {
    use scarletbook::log::{Level, Log};
    use std::sync::Mutex;
    let lines = |sink: &Arc<Mutex<Vec<String>>>| {
        let sink = Arc::clone(sink);
        Log::new(Level::Debug, move |_, m| sink.lock().unwrap().push(m.to_string()))
    };
    let (first, second) = (Arc::new(Mutex::new(Vec::new())), Arc::new(Mutex::new(Vec::new())));
    let bytes = synthetic();
    let disc = Disc::open_with_log(ReadAtReader::new(&bytes[..]), lines(&first)).unwrap();
    let opened = first.lock().unwrap().len();
    let mut share = disc.share_with_log(Cursor::new(&bytes[..]), lines(&second)).unwrap();
    assert_eq!(first.lock().unwrap().len(), opened, "nothing reported to the original's log");
    assert!(second.lock().unwrap().iter().any(|l| l.contains("shared reader checked")));
    let cursor = share.track_frames_cursor(Slot::TwoChannel, 1).unwrap();
    share.read_frames(cursor).for_each(drop);
    assert_eq!(first.lock().unwrap().len(), opened);
    // A clone keeps its original's log until it is given another.
    let mut clone = disc.clone();
    clone.set_log(Log::none());
    let cursor = clone.track_frames_cursor(Slot::TwoChannel, 1).unwrap();
    clone.read_frames(cursor).for_each(drop);
    assert_eq!(first.lock().unwrap().len(), opened);
}
