// SPDX-License-Identifier: MIT OR Apache-2.0

//! Cancelling and observing long reads (issue #106): a cancellation request stops a read
//! within 32 sectors with [`Error::Cancelled`], leaves the cursor where it was, and once
//! withdrawn the reading goes on exactly as without it; [`scarletbook::Progress`] follows
//! the reading from another thread.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::cell::{Cell, OnceCell};
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use common::{PFrame, S, Spec, TrackSpec, pack};
use scarletbook::audio::FRAME_BYTES_PER_CHANNEL;
use scarletbook::{
    CancelToken, Disc, Error, IndexOptions, Progress, SeekPolicy, Slot, SpeculativeOptions, TrackFrame,
};

/// Sectors a cursor reads at most once cancellation is requested.
const BOUND: u64 = 32;

/// A 2-channel area of `n` frames, plain and DST, in two tracks.
fn image(n: u32) -> Vec<u8> {
    let frames: Vec<PFrame> = (0..n)
        .map(|k| {
            let dst = k % 3 != 0;
            let len = if dst { 3000 + (k as usize * 53) % 5000 } else { 2 * FRAME_BYTES_PER_CHANNEL };
            PFrame { number: k, dst, data: (0..len).map(|i| (i as u8) ^ (k as u8)).collect() }
        })
        .collect();
    let half = n / 2;
    let tracks = vec![TrackSpec { start: 0, duration: half }, TrackSpec { start: half, duration: n - half }];
    Spec { channels: 2, playtime: n, tracks, audio: pack(&frames) }.build()
}

/// What the reader does: counts bytes and, once the cursor's token is set, cancels each
/// time `every` more bytes have been read; records the progress seen at each read.
#[derive(Default)]
struct Hook {
    bytes: Cell<u64>,
    token: OnceCell<CancelToken>,
    every: Cell<u64>,
    next: Cell<u64>,
    /// Bytes read when cancellation was last requested.
    at_cancel: Cell<u64>,
    progress: OnceCell<(Progress, u64)>,
    /// The largest lag of the published count behind the sectors actually read.
    lag: Cell<u64>,
    /// Cancel at the start of a read instead of after it: a request that arrives while
    /// the read is in progress, which then completes.
    at_start: Cell<bool>,
    /// The largest read, in bytes.
    largest: Cell<u64>,
    /// The most bytes read after a request before the call returned `Cancelled`.
    max_after: Cell<u64>,
    /// Each read: bytes read before it, and its length.
    reads: std::cell::RefCell<Vec<(u64, u64)>>,
}

struct Hooked {
    inner: Cursor<Vec<u8>>,
    hook: Rc<Hook>,
}

impl Read for Hooked {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let h = &self.hook;
        if let Some((p, base)) = h.progress.get() {
            let read = (h.bytes.get() - base) / S as u64;
            let (published, expected) = p.get();
            assert!(published <= read && published <= expected);
            h.lag.set(h.lag.get().max(read - published));
        }
        let cancel_now = |h: &Hook| {
            if let Some(t) = h.token.get() {
                if h.every.get() > 0 && h.bytes.get() >= h.next.get() {
                    h.next.set(h.bytes.get() + h.every.get());
                    h.at_cancel.set(h.bytes.get());
                    t.cancel();
                }
            }
        };
        if h.at_start.get() {
            cancel_now(h);
        }
        let n = self.inner.read(buf)?;
        h.bytes.set(h.bytes.get() + n as u64);
        h.largest.set(h.largest.get().max(n as u64));
        h.reads.borrow_mut().push((h.bytes.get() - n as u64, n as u64));
        if h.at_start.get() {
            return Ok(n);
        }
        if let Some(t) = h.token.get() {
            if h.every.get() > 0 && h.bytes.get() >= h.next.get() {
                h.next.set(h.bytes.get() + h.every.get());
                h.at_cancel.set(h.bytes.get());
                t.cancel();
            }
        }
        Ok(n)
    }
}

impl Seek for Hooked {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

fn hooked(img: &[u8]) -> (Disc<Hooked>, Rc<Hook>) {
    let hook = Rc::new(Hook::default());
    let disc = Disc::open(Hooked { inner: Cursor::new(img.to_vec()), hook: Rc::clone(&hook) }).unwrap();
    hook.token.set(disc.cancel_token()).unwrap();
    (disc, hook)
}

/// Reads to the end through `next`, withdrawing each cancellation after checking that it
/// came within [`BOUND`] sectors: the items and the number of cancellations.
fn through<T>(hook: &Hook, next: impl FnMut() -> Option<Result<T, Error>>) -> (Vec<T>, u32) {
    through_within(hook, BOUND, next)
}

/// [`through`] with a bound of `bound` sectors.
fn through_within<T>(
    hook: &Hook,
    bound: u64,
    mut next: impl FnMut() -> Option<Result<T, Error>>,
) -> (Vec<T>, u32) {
    let (mut out, mut cancels) = (Vec::new(), 0);
    let token = hook.token.get().unwrap();
    while let Some(r) = next() {
        match r {
            Ok(v) => out.push(v),
            Err(Error::Cancelled) => {
                let after = hook.bytes.get() - hook.at_cancel.get();
                assert!(after <= bound * S as u64, "{after} bytes read after the request");
                hook.max_after.set(hook.max_after.get().max(after));
                cancels += 1;
                token.withdraw();
            }
            Err(e) => panic!("{e}"),
        }
    }
    (out, cancels)
}

fn decoded(disc: &mut Disc<impl Read + Seek>, first: u32, end: u32) -> Vec<scarletbook::AudioFrame> {
    let c = disc.range_audio_cursor(Slot::TwoChannel, first, end).unwrap();
    disc.read_track_audio(c).map(Result::unwrap).collect()
}

fn stored(disc: &mut Disc<impl Read + Seek>, first: u32, end: u32) -> Vec<TrackFrame> {
    let c = disc.range_frames_cursor(Slot::TwoChannel, first, end).unwrap();
    disc.read_frames(c).map(Result::unwrap).collect()
}

/// Cancelled again and again, on every path that reads (a scan from the first sector, a
/// seek index, a provisional search), decoded and stored reads give exactly the frames of
/// an uninterrupted read.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn cancelled_reads_resume_exactly() {
    let img = image(900);
    let mut plain = Disc::open(Cursor::new(img.clone())).unwrap();
    for (first, end) in [(0, 900), (700, 760), (450, 451)] {
        let want_audio = decoded(&mut plain, first, end);
        let want_frames = stored(&mut plain, first, end);
        for path in ["scan", "index", "speculative"] {
            // A provisional search cancelled midway starts again: give it room to finish.
            let everies: &[u64] = if path == "speculative" {
                &[60 * S as u64, 200 * S as u64]
            } else {
                &[1, 7 * S as u64, 50 * S as u64]
            };
            for &every in everies {
                let (mut disc, hook) = hooked(&img);
                match path {
                    "index" => {
                        disc.enable_seek_index(Slot::TwoChannel, IndexOptions::new()).unwrap();
                        while !disc.extend_seek_index(Slot::TwoChannel, 4096).unwrap() {}
                    }
                    "speculative" => {
                        let options =
                            SpeculativeOptions::new().with_min_gap_sectors(16).with_probe_sectors(1);
                        disc.set_seek_policy(Slot::TwoChannel, SeekPolicy::Speculative(options)).unwrap();
                    }
                    _ => {}
                }
                hook.every.set(every);
                hook.next.set(hook.bytes.get() + every);
                let c = disc.range_audio_cursor(Slot::TwoChannel, first, end).unwrap();
                let mut it = disc.read_track_audio(c);
                let (got, cancels) = through(&hook, || it.next());
                drop(it);
                // A well-formed area: provisional readings give the same positions.
                assert_eq!(got, want_audio, "{path} {first}..{end} every {every}");
                // A provisional reading may read fewer sectors than `every`.
                assert!(cancels > 0 || path == "speculative", "{path} {first}..{end} every {every}");
                hook.next.set(hook.bytes.get() + every);
                let c = disc.range_frames_cursor(Slot::TwoChannel, first, end).unwrap();
                let mut it = disc.read_frames(c);
                let (got, _) = through(&hook, || it.next());
                drop(it);
                assert_eq!(got, want_frames, "{path} {first}..{end} every {every}");
                // Withdrawn, the disc reads as before.
                hook.every.set(0);
                hook.token.get().unwrap().withdraw();
                assert_eq!(decoded(&mut disc, first, end).len(), want_audio.len());
            }
        }
    }
}

/// The reads after a provisional start are sized to the range (issue #123), each at most
/// `MAX_READ_SECTORS`: a request arriving while one is in progress cannot stop it; the call
/// returns `Cancelled` once it completes, and every other read stays within 32 sectors. Progress stays consistent across it (`Hooked::read` checks it).
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn a_sized_read_is_the_only_longer_one() {
    sized_read_cancelled(|disc| {
        let options = SpeculativeOptions::new().with_min_gap_sectors(16);
        disc.set_seek_policy(Slot::TwoChannel, SeekPolicy::Speculative(options)).unwrap();
    });
}

/// The same for the read sized to the range after an exact resume from a seek index
/// (issue #124): one read of the replay, the interval and up to 64 sectors.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn a_sized_read_from_an_index() {
    sized_read_cancelled(|disc| {
        disc.enable_seek_index(Slot::TwoChannel, IndexOptions::new()).unwrap();
        while !disc.extend_seek_index(Slot::TwoChannel, 1 << 20).unwrap() {}
    });
}

/// A request at the start of the first read longer than [`BOUND`] that `prepare`d discs
/// make for a far range: that read completes, then the call returns `Cancelled`, and
/// withdrawn, the reading goes on to the same frames; the long reads are at most
/// `MAX_READ_SECTORS` and follow each other.
fn sized_read_cancelled(prepare: impl Fn(&mut Disc<Hooked>)) {
    use scarletbook::seek::MAX_READ_SECTORS;
    let img = image(3000);
    let mut plain = Disc::open(Cursor::new(img.clone())).unwrap();
    let (first, end) = (2000, 2200);
    let want = decoded(&mut plain, first, end);
    // Where the sized read starts, read without interruption.
    let (mut disc, hook) = hooked(&img);
    prepare(&mut disc);
    let base = hook.bytes.get();
    assert_eq!(decoded(&mut disc, first, end), want);
    let reads = hook.reads.borrow().clone();
    let &(at, len) = reads.iter().filter(|r| r.0 >= base).find(|r| r.1 > BOUND * S as u64).unwrap();
    let (mut disc, hook) = hooked(&img);
    prepare(&mut disc);
    assert_eq!(hook.bytes.get(), base, "the same reads up to the range");
    hook.at_start.set(true);
    let c = disc.range_audio_cursor(Slot::TwoChannel, first, end).unwrap();
    let mut it = disc.read_track_audio(c);
    let p = it.progress();
    hook.progress.set((p.clone(), base)).unwrap();
    hook.every.set(BOUND * S as u64);
    hook.next.set(at);
    let (got, cancels) = through_within(&hook, MAX_READ_SECTORS, || it.next());
    assert_eq!(got, want);
    assert!(cancels > 0);
    assert_eq!(hook.max_after.get(), len, "the sized read completes after the request");
    let (read, expected) = p.get();
    assert_eq!(read, expected, "complete");
    let long: Vec<_> = reads.iter().filter(|r| r.0 >= base && r.1 > BOUND * S as u64).collect();
    assert!(long.iter().all(|r| r.1 <= MAX_READ_SECTORS * S as u64), "{long:?}");
    assert!(long.windows(2).all(|w| w[1].0 == w[0].0 + w[0].1), "{long:?}");
}

/// With a pool decoding ahead, a cancelled cursor keeps the frames in flight and resumes
/// exactly; one dropped while cancelled frees them, and a new one reads correctly.
#[cfg(feature = "parallel")]
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn cancelled_with_a_pool() {
    let img = image(600);
    let want = decoded(&mut Disc::open(Cursor::new(img.clone())).unwrap(), 0, 600);
    let pool = Arc::new(rayon::ThreadPoolBuilder::new().num_threads(4).build().unwrap());
    let (mut disc, hook) = hooked(&img);
    hook.every.set(40 * S as u64);
    hook.next.set(hook.bytes.get() + 40 * S as u64);
    let mut c = disc.range_audio_cursor(Slot::TwoChannel, 0, 600).unwrap();
    c.set_decode_on(scarletbook::DecodeOn::Pool(Arc::clone(&pool)));
    let mut it = disc.read_track_audio(c);
    let (got, cancels) = through(&hook, || it.next());
    assert_eq!(got, want);
    assert!(cancels > 3);
    drop(it);
    // Dropped while cancelled, with frames in flight.
    hook.every.set(0);
    let mut c = disc.range_audio_cursor(Slot::TwoChannel, 0, 600).unwrap();
    c.set_decode_on(scarletbook::DecodeOn::Pool(pool));
    let mut it = disc.read_track_audio(c);
    for _ in 0..100 {
        it.next().unwrap().unwrap();
    }
    hook.token.get().unwrap().cancel();
    assert!(matches!(it.next(), Some(Err(Error::Cancelled))));
    drop(it);
    hook.token.get().unwrap().withdraw();
    assert_eq!(decoded(&mut disc, 0, 600), want);
}

/// The token is the image's: a clone's cancellation stops the original's reads, and the
/// stored frame list and seek index extension are cancelled too.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn every_disc_and_read_of_the_image() {
    let img: Arc<[u8]> = image(300).into();
    let disc = Disc::open(scarletbook::ReadAtReader::new(Arc::clone(&img))).unwrap();
    let mut clone = disc.clone();
    let shared = disc.share(Cursor::new(img.to_vec())).unwrap();
    shared.cancel_token().cancel();
    assert!(disc.cancel_token().is_cancelled() && clone.cancel_token().is_cancelled());
    let mut it = clone.track_audio(Slot::TwoChannel, 1).unwrap();
    assert!(matches!(it.next(), Some(Err(Error::Cancelled))));
    drop(it);
    let mut list = clone.stored_frame_list(Slot::TwoChannel).unwrap();
    assert!(matches!(list.next(), Some(Err(Error::Cancelled))));
    drop(list);
    clone.enable_seek_index(Slot::TwoChannel, IndexOptions::new()).unwrap();
    assert!(matches!(clone.extend_seek_index(Slot::TwoChannel, 1000), Err(Error::Cancelled)));
    assert_eq!(Error::Cancelled.class(), scarletbook::ErrorClass::Cancelled);
    disc.cancel_token().withdraw();
    assert!(clone.extend_seek_index(Slot::TwoChannel, 1 << 20).unwrap());
    // A disc opened apart has a token of its own.
    let other = Disc::open(Cursor::new(img.to_vec())).unwrap();
    disc.cancel_token().cancel();
    assert!(!other.cancel_token().is_cancelled());
}

/// Progress advances every chunk of at most 32 sectors through the scan before a far
/// range's first frame, and ends with all expected sectors read.
#[test]
#[cfg_attr(miri, ignore = "reads thousands of sectors")]
fn progress_through_a_cold_scan() {
    let img = image(900);
    for path in ["scan", "speculative", "list"] {
        let (mut disc, hook) = hooked(&img);
        if path == "speculative" {
            let options = SpeculativeOptions::new().with_min_gap_sectors(16).with_probe_sectors(1);
            disc.set_seek_policy(Slot::TwoChannel, SeekPolicy::Speculative(options)).unwrap();
        }
        let base = hook.bytes.get();
        let mut seen = Vec::new();
        let last = if path == "list" {
            let mut it = disc.stored_frame_list(Slot::TwoChannel).unwrap();
            let p = it.progress();
            hook.progress.set((p.clone(), base)).unwrap();
            assert_eq!(p.get().0, 0);
            assert!(p.get().1 > 0);
            for f in it.by_ref() {
                f.unwrap();
                seen.push(p.get());
            }
            p.get()
        } else {
            let c = disc.range_audio_cursor(Slot::TwoChannel, 800, 830).unwrap();
            let mut it = disc.read_track_audio(c);
            let p = it.progress();
            assert_eq!(p.get(), it.progress().get(), "the same object");
            hook.progress.set((p.clone(), base)).unwrap();
            let (read, expected) = p.get();
            assert_eq!(read, 0);
            assert!(expected > 0);
            for f in it.by_ref() {
                f.unwrap();
                seen.push(p.get());
            }
            p.get()
        };
        let read = (hook.bytes.get() - base) / S as u64;
        assert_eq!(last, (read, read), "{path}: complete");
        assert!(hook.lag.get() <= BOUND, "{path}: lag {}", hook.lag.get());
        assert!(seen.windows(2).all(|w| w[0].0 <= w[1].0), "{path}: monotonic");
    }
}

/// A continued cursor counts its own reading; the earlier progress stays where it was.
#[test]
#[cfg_attr(miri, ignore = "reads hundreds of sectors")]
fn a_continuation_counts_afresh() {
    let img = image(300);
    let mut disc = Disc::open(Cursor::new(img)).unwrap();
    let c = disc.track_audio_cursor(Slot::TwoChannel, 1).unwrap();
    let mut it = disc.read_track_audio(c);
    let p1 = it.progress();
    it.by_ref().for_each(|f| drop(f.unwrap()));
    let c = it.into_cursor();
    let first = p1.get();
    assert!(first.0 > 0 && first.0 == first.1);
    let mut c = disc.continue_track_audio(c, Slot::TwoChannel, 2).unwrap();
    let p2 = c.progress();
    assert_eq!(p2.get().0, 0);
    disc.read_track_audio(c).for_each(|f| drop(f.unwrap()));
    assert_eq!(p1.get(), first);
    let (read, expected) = p2.get();
    assert!(read > 0 && read == expected);
}

/// Another thread cancels a read through a slow reader: the reading thread sees
/// `Cancelled` within the bound, with its progress observed meanwhile.
#[test]
#[cfg_attr(miri, ignore = "sleeps")]
fn cancelled_from_another_thread() {
    struct Slow {
        inner: Cursor<Vec<u8>>,
        bytes: Arc<AtomicU64>,
    }
    impl Read for Slow {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            std::thread::sleep(std::time::Duration::from_millis(2));
            let n = self.inner.read(buf)?;
            self.bytes.fetch_add(n as u64, Ordering::SeqCst);
            Ok(n)
        }
    }
    impl Seek for Slow {
        fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(pos)
        }
    }
    let bytes = Arc::new(AtomicU64::new(0));
    let mut disc = Disc::open(Slow { inner: Cursor::new(image(900)), bytes: Arc::clone(&bytes) }).unwrap();
    let token = disc.cancel_token();
    let mut cursor = disc.range_audio_cursor(Slot::TwoChannel, 850, 900).unwrap();
    let progress = cursor.progress();
    let reader = std::thread::spawn(move || {
        let r = disc.read_track_audio(cursor).next();
        (r, disc)
    });
    while progress.sectors_read() < 100 {
        std::thread::yield_now();
    }
    token.cancel();
    let at = bytes.load(Ordering::SeqCst);
    let (r, _disc) = reader.join().unwrap();
    assert!(matches!(r, Some(Err(Error::Cancelled))), "{r:?}");
    let after = bytes.load(Ordering::SeqCst) - at;
    assert!(after <= BOUND * S as u64, "{after} bytes after the request");
    let (read, expected) = progress.get();
    assert!(read >= 100 && expected > read);
}
