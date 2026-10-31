// SPDX-License-Identifier: MIT OR Apache-2.0

//! Stored and decoded frames of one range in one pass (issue #107): a cursor that keeps
//! each position's stored content gives exactly the positions of a stored read and of a
//! decoded read, from one read of the area's sectors.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::cell::Cell;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::rc::Rc;

use common::{PFrame, Spec, TrackSpec, pack};
use scarletbook::audio::FRAME_BYTES_PER_CHANNEL;
use scarletbook::{AudioFrame, Disc, Error, FrameContent, Slot, TrackFrame, Warning};

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

/// A reader that counts the bytes read through it.
struct Counting {
    inner: Cursor<Vec<u8>>,
    bytes: Rc<Cell<u64>>,
}

impl Read for Counting {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bytes.set(self.bytes.get() + n as u64);
        Ok(n)
    }
}

impl Seek for Counting {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// What a read gives: every position or the error that ended it, and the counts.
#[derive(Debug, PartialEq)]
struct Read2<T> {
    items: Vec<Result<T, String>>,
    counts: (u64, u64, u64),
    warnings: Vec<Warning>,
}

/// The ranges of an area: each track and the whole area.
fn ranges<R: Read + Seek>(disc: &Disc<R>, slot: Slot) -> Vec<(u32, u32)> {
    let Some(area) = disc.toc().area(slot) else { return Vec::new() };
    let mut v: Vec<_> = area.tracks.iter().map(|t| (t.start_frame, t.end_frame())).collect();
    v.push((0, area.playtime_frames()));
    v.into_iter().filter(|&(f, e)| disc.check_range(slot, f, e).is_ok()).collect()
}

#[cfg(feature = "parallel")]
fn on(c: &mut scarletbook::TrackAudioCursor, pool: Option<&std::sync::Arc<rayon::ThreadPool>>) {
    c.set_decode_on(match pool {
        Some(p) => scarletbook::DecodeOn::Pool(std::sync::Arc::clone(p)),
        None => scarletbook::DecodeOn::CallingThread,
    });
}

/// Decoded and stored reads of `[first, end)` apart, and in one pass.
#[allow(clippy::type_complexity)]
fn three<R: Read + Seek>(
    disc: &mut Disc<R>,
    slot: Slot,
    (first, end): (u32, u32),
    #[cfg(feature = "parallel")] pool: Option<&std::sync::Arc<rayon::ThreadPool>>,
) -> (Read2<AudioFrame>, Read2<TrackFrame>, Read2<AudioFrame>, Read2<TrackFrame>) {
    #[allow(unused_mut)]
    let mut c = disc.range_audio_cursor(slot, first, end).unwrap();
    #[cfg(feature = "parallel")]
    on(&mut c, pool);
    let mut it = disc.read_track_audio(c);
    let items = it.by_ref().map(|f| f.map_err(|e| e.to_string())).collect();
    let c = it.into_cursor();
    let decoded = Read2 {
        items,
        counts: (c.missing_frames(), c.invalid_dst_frames(), c.malformed_sectors()),
        warnings: c.warnings().to_vec(),
    };
    let c = disc.range_frames_cursor(slot, first, end).unwrap();
    let mut it = disc.read_frames(c);
    let items = it.by_ref().map(|f| f.map_err(|e| e.to_string())).collect();
    let c = it.into_cursor();
    let stored = Read2 {
        items,
        counts: (c.missing_frames(), 0, c.malformed_sectors()),
        warnings: c.warnings().to_vec(),
    };
    #[allow(unused_mut)]
    let mut c = disc.range_audio_cursor(slot, first, end).unwrap();
    #[cfg(feature = "parallel")]
    on(&mut c, pool);
    c.keep_stored().unwrap();
    let mut it = disc.read_track_audio(c);
    let (mut a, mut s) = (Vec::new(), Vec::new());
    while let Some(r) = it.next_with_stored() {
        match r {
            Ok((f, stored)) => {
                s.push(Ok(TrackFrame { number: f.number, content: stored.content(&f) }));
                a.push(Ok(f));
            }
            Err(e) => {
                a.push(Err(e.to_string()));
                s.push(Err(e.to_string()));
            }
        }
    }
    let c = it.into_cursor();
    let w = c.warnings().to_vec();
    let both_a = Read2 {
        items: a,
        counts: (c.missing_frames(), c.invalid_dst_frames(), c.malformed_sectors()),
        warnings: w.clone(),
    };
    let both_s = Read2 { items: s, counts: (c.missing_frames(), 0, c.malformed_sectors()), warnings: w };
    (decoded, stored, both_a, both_s)
}

/// On every corpus image, every track and whole area of every area: the one-pass read
/// gives the decoded read's frames, statuses and counts, and the stored read's positions.
#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn equals_the_separate_reads_on_the_corpus() {
    #[cfg(feature = "parallel")]
    let pool = std::sync::Arc::new(rayon::ThreadPoolBuilder::new().num_threads(4).build().unwrap());
    let mut checked = 0;
    for (name, bytes) in corpus() {
        let Ok(mut disc) = Disc::open(Cursor::new(bytes)) else { continue };
        for slot in Slot::ALL {
            for range in ranges(&disc, slot) {
                #[cfg(feature = "parallel")]
                for p in [None, Some(&pool)] {
                    let (d, s, a, b) = three(&mut disc, slot, range, p);
                    assert_eq!(a, d, "{name} {slot} {range:?} decoded");
                    assert_eq!(b.items, s.items, "{name} {slot} {range:?} stored");
                    assert_eq!((b.counts.0, b.counts.2, &b.warnings), (s.counts.0, s.counts.2, &s.warnings));
                }
                #[cfg(not(feature = "parallel"))]
                {
                    let (d, s, a, b) = three(&mut disc, slot, range);
                    assert_eq!(a, d, "{name} {slot} {range:?} decoded");
                    assert_eq!(b.items, s.items, "{name} {slot} {range:?} stored");
                    assert_eq!((b.counts.0, b.counts.2, &b.warnings), (s.counts.0, s.counts.2, &s.warnings));
                }
                checked += 1;
            }
        }
    }
    assert!(checked > 100, "{checked} ranges");
}

/// A 2-channel area of `n` frames, plain, DST and missing ones, in two tracks.
fn image(n: u32) -> Vec<u8> {
    let frames: Vec<PFrame> = (0..n)
        .filter(|k| k % 11 != 5)
        .map(|k| {
            let dst = k % 3 != 0;
            let len = if dst { 40 + (k as usize * 53) % 3000 } else { 2 * FRAME_BYTES_PER_CHANNEL };
            PFrame { number: k, dst, data: (0..len).map(|i| (i as u8) ^ (k as u8)).collect() }
        })
        .collect();
    let half = n / 2;
    let tracks = vec![TrackSpec { start: 0, duration: half }, TrackSpec { start: half, duration: n - half }];
    Spec { channels: 2, playtime: n, tracks, audio: pack(&frames) }.build()
}

/// The one pass reads each audio sector once: the bytes of a decoded-only read, and of a
/// stored-only read. The DST equivalents are the stored read's.
#[test]
fn reads_each_sector_once() {
    let img = image(if cfg!(miri) { 12 } else { 200 });
    let bytes = Rc::new(Cell::new(0));
    let mut disc = Disc::open(Counting { inner: Cursor::new(img), bytes: Rc::clone(&bytes) }).unwrap();
    let n = disc.toc().area(Slot::TwoChannel).unwrap().playtime_frames();
    let b0 = bytes.get();
    let c = disc.range_audio_cursor(Slot::TwoChannel, 0, n).unwrap();
    let decoded: Vec<_> = disc.read_track_audio(c).map(Result::unwrap).collect();
    let once = bytes.get() - b0;
    let b0 = bytes.get();
    let c = disc.range_frames_cursor(Slot::TwoChannel, 0, n).unwrap();
    let stored: Vec<_> = disc.read_frames(c).map(Result::unwrap).collect();
    assert_eq!(bytes.get() - b0, once);
    let b0 = bytes.get();
    let mut c = disc.range_audio_cursor(Slot::TwoChannel, 0, n).unwrap();
    c.keep_stored().unwrap();
    assert!(c.keeps_stored());
    let mut it = disc.read_track_audio(c);
    let mut both = Vec::new();
    let mut equivalents = Vec::new();
    // Mixed: every third position through `next` (its stored content dropped).
    let mut k = 0;
    loop {
        k += 1;
        if k % 3 == 0 {
            let Some(f) = it.next() else { break };
            both.push((f.unwrap(), None));
        } else {
            let Some(r) = it.next_with_stored() else { break };
            let (f, s) = r.unwrap();
            assert_eq!(
                s.bytes(&f),
                match s.content(&f) {
                    FrameContent::Dst(b) | FrameContent::Dsd(b) => b,
                    FrameContent::Missing => Vec::new(),
                }
            );
            let s = s.content(&f);
            both.push((f, Some(s)));
        }
    }
    let c = it.into_cursor();
    for (_, s) in &both {
        if let Some(s) = s {
            equivalents.push(c.dst_equivalent(s).into_owned());
        }
    }
    assert_eq!(bytes.get() - b0, once, "one read of the sectors");
    assert!(both.iter().map(|(f, _)| f).eq(decoded.iter()));
    let mut want_eq = Vec::new();
    for (i, (_, s)) in both.iter().enumerate() {
        if let Some(s) = s {
            assert_eq!(s, &stored[i].content, "position {i}");
            want_eq.push(s.dst_equivalent(2).unwrap().into_owned());
        }
    }
    assert_eq!(equivalents, want_eq);
    assert!(stored.iter().any(|f| f.content == FrameContent::Missing));
}

/// Asked for once positions are held ahead it is refused; continuations keep the mode;
/// without it, `next_frame_with_stored` is refused.
#[test]
fn when_it_can_be_asked_for() {
    let img = image(if cfg!(miri) { 12 } else { 120 });
    let mut disc = Disc::open(Cursor::new(img)).unwrap();
    let c = disc.track_audio_cursor(Slot::TwoChannel, 1).unwrap();
    let mut it = disc.read_track_audio(c);
    assert!(matches!(it.next_with_stored(), Some(Err(Error::InvalidArgument(_)))));
    it.next().unwrap().unwrap();
    // Plain DSD and DST positions are held ahead of the caller after the first read.
    assert!(matches!(it.keep_stored(), Err(Error::InvalidArgument(_))));
    let c = it.into_cursor();
    let mut c = disc.continue_track_audio(c, Slot::TwoChannel, 2).unwrap();
    c.keep_stored().unwrap();
    let mut it = disc.read_track_audio(c);
    let (first, _) = it.next_with_stored().unwrap().unwrap();
    let c = it.into_cursor();
    // Continued backwards (a new scan), it still keeps stored content.
    let c = disc.continue_track_audio(c, Slot::TwoChannel, 1).unwrap();
    assert!(c.keeps_stored());
    let mut it = disc.read_track_audio(c);
    let (f, _) = it.next_with_stored().unwrap().unwrap();
    assert_eq!(f.number, 0);
    assert!(first.number > 0);
}
