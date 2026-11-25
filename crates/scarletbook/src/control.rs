// SPDX-License-Identifier: MIT OR Apache-2.0

//! Cancelling and observing long reads from another thread: [`CancelToken`] and
//! [`Progress`]; and the sectors that cannot be read: [`UnreadableSectors`].

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use crate::error::Error;

/// A request, from any thread, that the reads of an image stop: one per image, shared by
/// every disc made from it ([`crate::Disc::share`], `clone`) and their cursors. Get it with
/// [`crate::Disc::cancel_token`] (or [`crate::Image::cancel_token`]) before handing the
/// disc to the thread that reads.
///
/// While the request stands, every call that reads audio sectors through any disc of the
/// image returns [`Error::Cancelled`]: a call that starts then at once, a call in progress
/// after at most 32 more sectors read (a cursor reads 32 at a time, a provisional search
/// at most 32 per probe), or, if the request arrives during one of the reads sized to the
/// range after a provisional start or a resume from a seek index, after that read of at most
/// [`crate::seek::MAX_READ_SECTORS`] sectors (1 MiB), plus the decoding of one group of frames already started for the
/// frame it is returning. Nothing is lost: the cursor stays where it was, the frames it
/// holds ahead included (decoding jobs already sent to a pool finish), and once the
/// request is [withdrawn](CancelToken::withdraw) it continues as if never interrupted (a
/// provisional search, [`crate::SeekPolicy::Speculative`], cancelled before it chose its
/// start, starts again). A cancelled cursor can also just be dropped. Opening a disc and
/// reading its TOC are not cancelled, nor is [`crate::Image::read_sectors`].
///
/// The request stays until withdrawn: a caller that keeps the disc (or any disc of the
/// image) withdraws it before reading again. A disc opened afresh, even of the same file,
/// has a token of its own.
///
/// ```no_run
/// # fn main() -> Result<(), scarletbook::Error> {
/// use scarletbook::{Disc, Error, Slot};
/// let mut disc = Disc::open(std::io::BufReader::new(std::fs::File::open("disc.iso")?))?;
/// let token = disc.cancel_token();
/// let reader = std::thread::spawn(move || -> Result<u32, Error> {
///     let mut frames = 0;
///     for frame in disc.track_audio(Slot::TwoChannel, 1)? {
///         match frame {
///             Ok(_) => frames += 1,
///             Err(Error::Cancelled) => break,
///             Err(e) => return Err(e),
///         }
///     }
///     Ok(frames)
/// });
/// token.cancel(); // from this thread, for example on Ctrl-C
/// let _frames = reader.join().expect("reader thread")?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A token of its own, not yet cancelled.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Requests that the reads of the image stop, until [`CancelToken::withdraw`].
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Withdraws the request: reads go on again.
    pub fn withdraw(&self) {
        self.0.store(false, Ordering::Relaxed);
    }

    /// Whether cancellation is requested.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// [`Error::Cancelled`] if cancellation is requested.
    #[inline]
    pub(crate) fn check(&self) -> Result<(), Error> {
        if self.is_cancelled() { Err(Error::Cancelled) } else { Ok(()) }
    }
}

/// The audio sectors of an image that cannot be read (#435): one set per image, shared by
/// every disc made from it ([`crate::Disc::share`], `clone`) and their cursors, like
/// [`CancelToken`]. Get it with [`crate::Disc::unreadable_sectors`] (or
/// [`crate::Image::unreadable_sectors`]) and mark sectors with
/// [`UnreadableSectors::insert`]; a reader can also report sectors as it reads them, with
/// [`crate::read_at::unreadable_error`], and they are added here.
///
/// A cursor never asks the reader for a marked sector, and never parses one: the frame it
/// was assembling is dropped unless it is already complete, and the timeline positions from
/// the next acceptable frame number to the next frame accepted after the damage are
/// **lost** ([`crate::FrameStatus::Lost`], [`crate::TrackFrame::is_lost`]), never decoded
/// from part of their data. Reading resumes at the next frame that starts in a readable
/// sector, at its own frame number. On a well-formed disc the lost positions are exactly the
/// frames with a byte in a marked sector or starting in one; every other position is as
/// without marks. A mark applies to every sector a cursor processes after the mark is
/// made, sectors already in its read buffer included; only frames it has already
/// demultiplexed (a [`crate::TrackAudioCursor`] holds a bounded window of them ahead of the
/// caller) keep what they had. Mark before opening the cursors that read past the sectors,
/// or reopen them, for a result that does not depend on timing.
///
/// A seek index's checkpoints hold the reading state of a scan without marks, and a cursor
/// does not resume from one at or after a marked sector. Past marked sectors, the index
/// keeps the state of the scans that met them instead, each checkpoint keyed by the marked
/// runs before it, and a cursor resumes there only when the marks before it are the same
/// (#441; see [`crate::seek`]). So the first range opened past a marked sector scans the
/// area from its start, and later ones start near their first frame, as long as the marks
/// before them stay as they were; adding or removing a mark before a point makes the state
/// there unused until a scan records it again. Continued cursors read on.
///
/// The TOC, read when the disc is opened, is not affected. [`crate::Image::read_sectors`]
/// refuses marked sectors with [`Error::SectorUnreadable`].
///
/// The set holds merged ranges: its memory is one entry per run of marked sectors the
/// caller keeps apart.
// TODO(sacd-spec#55): reading with unreadable sectors is not yet in the specification.
#[derive(Clone, Debug, Default)]
pub struct UnreadableSectors(Arc<Marks>);

#[derive(Debug, Default)]
struct Marks {
    /// Whether any sector is marked: the test every audio read makes.
    any: AtomicBool,
    /// Sorted, disjoint, non-adjacent half-open ranges `[start, end)`.
    ranges: RwLock<Vec<(u64, u64)>>,
}

impl UnreadableSectors {
    /// A set of its own, empty.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Vec<(u64, u64)>> {
        // Nothing panics while the lock is held.
        self.0.ranges.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn update(&self, f: impl FnOnce(&mut Vec<(u64, u64)>)) {
        let mut ranges = self.0.ranges.write().unwrap_or_else(PoisonError::into_inner);
        f(&mut ranges);
        self.0.any.store(!ranges.is_empty(), Ordering::Release);
    }

    /// Marks `count` sectors from `lsn` as unreadable (a range past `u64::MAX` ends there).
    pub fn insert(&self, lsn: u64, count: u64) {
        let end = lsn.saturating_add(count);
        if end == lsn {
            return;
        }
        self.update(|ranges| {
            // The ranges that touch or overlap [lsn, end) merge with it.
            let first = ranges.partition_point(|&(_, e)| e < lsn);
            let last = ranges.partition_point(|&(s, _)| s <= end);
            let (mut s, mut e) = (lsn, end);
            if let Some(&(a, _)) = ranges.get(first).filter(|_| first < last) {
                s = s.min(a);
            }
            if let Some(&(_, b)) = last.checked_sub(1).and_then(|i| ranges.get(i)).filter(|_| first < last) {
                e = e.max(b);
            }
            ranges.splice(first..last, [(s, e)]);
        });
    }

    /// Marks `count` sectors from `lsn` as readable again. Cursors that already read
    /// them keep what they read.
    pub fn remove(&self, lsn: u64, count: u64) {
        let end = lsn.saturating_add(count);
        if end == lsn {
            return;
        }
        self.update(|ranges| {
            let mut kept = Vec::with_capacity(ranges.len() + 1);
            for &(s, e) in ranges.iter() {
                if e <= lsn || s >= end {
                    kept.push((s, e));
                    continue;
                }
                if s < lsn {
                    kept.push((s, lsn));
                }
                if e > end {
                    kept.push((end, e));
                }
            }
            *ranges = kept;
        });
    }

    /// Marks every sector readable again.
    pub fn clear(&self) {
        self.update(Vec::clear);
    }

    /// Whether sector `lsn` is marked.
    pub fn contains(&self, lsn: u64) -> bool {
        self.first_in(lsn, lsn.saturating_add(1)).is_some()
    }

    /// Whether no sector is marked.
    pub fn is_empty(&self) -> bool {
        !self.0.any.load(Ordering::Acquire)
    }

    /// The marked runs, in increasing order, each as (first sector, count).
    pub fn ranges(&self) -> Vec<(u64, u64)> {
        self.read().iter().map(|&(s, e)| (s, e - s)).collect()
    }

    /// The first marked sector in `[start, end)`.
    pub(crate) fn first_in(&self, start: u64, end: u64) -> Option<u64> {
        if self.is_empty() || start >= end {
            return None;
        }
        let ranges = self.read();
        let i = ranges.partition_point(|&(_, e)| e <= start);
        ranges.get(i).filter(|&&(s, _)| s < end).map(|&(s, _)| s.max(start))
    }

    /// The marked runs within `[start, end)`, clipped to it, as half-open ranges.
    pub(crate) fn runs_in(&self, start: u64, end: u64) -> Vec<(u64, u64)> {
        if self.is_empty() || start >= end {
            return Vec::new();
        }
        let ranges = self.read();
        let i = ranges.partition_point(|&(_, e)| e <= start);
        ranges[i..].iter().take_while(|&&(s, _)| s < end).map(|&(s, e)| (s.max(start), e.min(end))).collect()
    }
}

/// How far a cursor has read, readable from any thread while the cursor reads: the
/// sectors it has read through the image so far, and the sectors it expects to have read
/// once it is done. Get it from the cursor (for example
/// [`crate::TrackAudioCursor::progress`]) before handing the cursor to the thread that
/// reads; it stays valid after the cursor is dropped.
///
/// The counts are updated with every read the cursor makes (at most 32 sectors at a
/// time, except the reads sized to the range after a provisional start or a resume from a
/// seek index, of at most
/// [`crate::seek::MAX_READ_SECTORS`]): the scan before a range's first frame, a provisional search, a seek index's
/// replay and the range itself. `sectors_read` is exact; `sectors_expected` is an
/// estimate (the range's sectors are not known until they are read: its end is placed in
/// proportion to the area's play time), revised as the cursor reads, at least
/// `sectors_read`, and equal to it once the cursor has read its range to the end. A
/// continued cursor ([`crate::Disc::continue_track_audio`] and the like) is a new reading
/// with counts of its own: ask it for its own `Progress`; the earlier one no longer moves.
#[derive(Clone, Debug, Default)]
pub struct Progress(Arc<Counts>);

#[derive(Debug, Default)]
struct Counts {
    read: AtomicU64,
    expected: AtomicU64,
}

impl Progress {
    /// The sectors read so far and the sectors expected, `read <= expected`.
    pub fn get(&self) -> (u64, u64) {
        let read = self.0.read.load(Ordering::Acquire);
        let expected = self.0.expected.load(Ordering::Acquire);
        (read, expected.max(read))
    }

    /// The sectors read so far.
    pub fn sectors_read(&self) -> u64 {
        self.get().0
    }

    /// The sectors expected (see [`Progress`]).
    pub fn sectors_expected(&self) -> u64 {
        self.get().1
    }

    /// Publishes the counts.
    pub(crate) fn set(&self, read: u64, expected: u64) {
        self.0.read.store(read, Ordering::Release);
        self.0.expected.store(expected.max(read), Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_and_progress() {
        let t = CancelToken::new();
        let u = t.clone();
        assert!(t.check().is_ok());
        u.cancel();
        assert!(matches!(t.check(), Err(Error::Cancelled)));
        t.withdraw();
        assert!(!u.is_cancelled());
        let p = Progress::default();
        p.set(10, 4);
        assert_eq!(p.get(), (10, 10));
        p.set(10, 40);
        assert_eq!((p.sectors_read(), p.sectors_expected()), (10, 40));
    }
}
