// SPDX-License-Identifier: MIT OR Apache-2.0

//! Cancelling and observing long reads from another thread: [`CancelToken`] and
//! [`Progress`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::error::Error;

/// A request, from any thread, that the reads of an image stop: one per image, shared by
/// every disc made from it ([`crate::Disc::share`], `clone`) and their cursors. Get it with
/// [`crate::Disc::cancel_token`] (or [`crate::Image::cancel_token`]) before handing the
/// disc to the thread that reads.
///
/// While the request stands, every call that reads audio sectors through any disc of the
/// image returns [`Error::Cancelled`]: a call that starts then at once, a call in progress
/// after at most 32 more sectors read (a cursor reads 32 at a time, a provisional search
/// at most 32 per probe), plus the decoding of one group of frames already started for the
/// frame it is returning. Nothing is lost: the cursor stays where it was, the frames it
/// holds ahead included (decoding jobs already sent to a pool finish), and once the
/// request is [withdrawn](CancelToken::withdraw) it continues as if never interrupted (a
/// provisional search, [`crate::SeekPolicy::Speculative`], cancelled before it chose its
/// start, starts again). A cancelled cursor can also just be dropped. Opening a disc and reading its TOC are not
/// cancelled, nor is [`crate::Image::read_sectors`].
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

/// How far a cursor has read, readable from any thread while the cursor reads: the
/// sectors it has read through the image so far, and the sectors it expects to have read
/// once it is done. Get it from the cursor (for example [`crate::TrackAudioCursor::progress`])
/// before handing the cursor to the thread that reads; it stays valid after the cursor is
/// dropped.
///
/// The counts are updated with every read the cursor makes (at most 32 sectors at a
/// time): the scan before a range's first frame, a provisional search, a seek index's
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
