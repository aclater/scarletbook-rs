// SPDX-License-Identifier: MIT OR Apache-2.0

//! Seeking within an area: saved reading state, so that a frame range far into an area
//! does not need a scan of the area from its first audio sector (issue #68).
//!
//! # Why an index
//!
//! The frames a range holds depend on every audio sector before it: the next acceptable
//! frame number `a` (specification 4, 4.5) is one more than the largest number of any
//! complete frame delivered so far, and a single sector anywhere before the range can
//! raise it past every frame of the range (4.5 item 2). A reader that has not read that
//! sector cannot tell the two images apart, so no exact reader can seek a first time
//! without reading the area up to the range. What it can do is keep the state of that
//! read: an [`AreaIndex`] holds the reading state of an area every `K` sectors
//! ([`IndexOptions::interval`]) from its first audio sector up to the index's
//! **frontier**. A cursor that starts from it resumes at the last checkpoint whose `a` is
//! at most the range's first frame, rebuilds the frame under assembly there, and reads
//! on as a scan from the first sector would: the frames, statuses, counts and warnings it
//! returns are exactly those of such a scan; only its reads and log lines differ.
//!
//! # Building one
//!
//! An index is built by reading: a cursor made from an index
//! ([`crate::TrackCursor::for_range_indexed`], [`crate::TrackAudioCursor::for_range_indexed`])
//! records a checkpoint each time its scan crosses a boundary past the index's frontier,
//! and [`AreaIndex::absorb`] takes those checkpoints into the index. Each audio sector is
//! then read at most once per index for building it, and once the frontier lies past a
//! range, opening that range reads at most `W + K` sectors before the sectors of its own
//! frames (`W` = [`REPLAY_SECTORS`], unless the frame under assembly there is longer and
//! does not fit the inline budget).
//!
//! # Trust
//!
//! An index is the reading state of one area of one image. It is keyed by the area (slot,
//! audio sector range, channel count) and the image's sector count, and a cursor refuses
//! an index of another key. Each checkpoint also holds a digest (64-bit FNV-1a) of the
//! sector at its LSN, which a cursor resuming there compares with that sector when it
//! reads it (it reads it anyway), before it returns anything: on a mismatch it discards
//! the index, logs a warning and scans from the first sector ([`ResumedFrom::Discarded`]).
//!
//! What that digest catches: an index of another image of the same size and layout, or of
//! an image changed since, whenever the checkpoint's own sector differs (with a 2^-64
//! chance of missing it). What it does not: a change only in the other sectors before the
//! checkpoint, which can change the reading state there without changing that sector (the
//! replayed sectors and the stored frame under assembly are not checked either); and a
//! cursor that reads nothing at all (a range wholly past the last accepted frame of a
//! complete index), which has no sector to compare. An index is thus **trusted for correctness and validated for safety**, like
//! the image itself: [`AreaIndex::from_bytes`] checks every bound and invariant before it
//! allocates, so that hostile bytes give an error, never a panic or an unbounded
//! allocation, but an index forged to pass those checks can make a cursor return other
//! frames than the image holds (still within the bounds of any read).
//!
//! # Specification
//!
//! Beginning at a checkpoint is specification 4, 4.2 item 7 (sacd-spec `da6b900`): the
//! frame under assembly there is the one processing from the first sector leaves (stored,
//! or rebuilt by replaying from the sector it started in), and the cursor begins with the
//! checkpoint's next acceptable frame number, the scan's own, which is at most the range's
//! first frame `u`; 4.5 item 6 gives why that is exact. Malformed counts and warnings
//! include those the earlier pass found (4.4.5 item 4), and replayed sectors are not
//! reported again (4.4.5 item 2). Specification 7, 7.1 items 2 and 3 bound the memory and
//! the passes.

mod persist;

use std::io::{Read, Seek};

use crate::audio::{
    AreaCursor, AreaKey, FrameKind, MAX_FRAME_BYTES, MAX_MALFORMED_WARNINGS, audio_sector_range,
};
use crate::error::Error;
use crate::image::{Image, SECTOR_SIZE};
use crate::toc::Area;

/// Default checkpoint interval `K` in sectors.
pub const DEFAULT_INTERVAL: u32 = 64;

/// Largest checkpoint interval `K` in sectors.
pub const MAX_INTERVAL: u32 = 65_536;

/// Default bound on the bytes of frames under assembly an index stores (1 MiB).
pub const DEFAULT_INLINE_BUDGET: usize = 1 << 20;

/// Most sectors a resume replays to rebuild the frame under assembly, `W`, unless that
/// frame does not fit the inline budget.
///
/// On well-formed images a frame spans at most 31 sectors (a DST frame's sector count,
/// specification 4, 4.3.3; a plain frame at most `ceil(6 x 4704 / 2045) + 1 = 15`), so a
/// checkpoint never stores a frame's bytes. A hostile frame may be spread over any number
/// of sectors; a checkpoint whose frame under assembly started more than `W` sectors
/// before it stores the frame's bytes (at most [`MAX_FRAME_BYTES`]) while the index's
/// inline budget lasts, and is replayed from the frame's first sector otherwise.
pub const REPLAY_SECTORS: u64 = 32;

/// Options of an [`AreaIndex`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct IndexOptions {
    /// The checkpoint interval `K` in sectors, 1 to [`MAX_INTERVAL`]: the index holds
    /// about `24` bytes per `K` audio sectors, and a resume reads at most `K` sectors
    /// past its checkpoint before the range's first frame is known.
    pub interval: u32,
    /// The most bytes of frames under assembly the index stores, over all its checkpoints
    /// (see [`REPLAY_SECTORS`]); 0 stores none.
    pub inline_budget: usize,
}

impl Default for IndexOptions {
    fn default() -> Self {
        IndexOptions { interval: DEFAULT_INTERVAL, inline_budget: DEFAULT_INLINE_BUDGET }
    }
}

impl IndexOptions {
    /// The defaults: `K` = [`DEFAULT_INTERVAL`], [`DEFAULT_INLINE_BUDGET`].
    pub fn new() -> Self {
        Self::default()
    }

    /// These options with checkpoint interval `interval`.
    pub fn with_interval(self, interval: u32) -> Self {
        IndexOptions { interval, ..self }
    }

    /// These options with inline budget `bytes`.
    pub fn with_inline_budget(self, bytes: usize) -> Self {
        IndexOptions { inline_budget: bytes, ..self }
    }
}

/// Where a cursor made from an index started its scan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResumedFrom {
    /// At the area's first audio sector, as a cursor made without an index.
    Start,
    /// At the checkpoint at sector `lsn`, after replaying the sectors from `replay_from`
    /// (equal to `lsn` when nothing was replayed) to rebuild the frame under assembly.
    Checkpoint {
        /// The checkpoint's LSN.
        lsn: u64,
        /// The first sector read.
        replay_from: u64,
    },
    /// Nowhere: the index is complete and no frame at or after the range's first is
    /// accepted, so every position is missing and nothing is read.
    End,
    /// At the area's first audio sector, after finding that the index does not hold
    /// sector `lsn` as the image has it (its digest differs): a stale index, or one of
    /// another image of the same size. The index was not used; the cursor records nothing.
    Discarded {
        /// The checkpoint whose sector differs.
        lsn: u64,
    },
}

/// The reading state of the scan at a boundary `c = S0 + jK`, before sector `c`
/// (specification 4, 4.4.1 and 4.5): `a`, the malformed sectors before `c`, and where the
/// frame under assembly started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Checkpoint {
    /// `a` at `c`.
    pub(crate) next_acceptable: u32,
    /// `c - r` for a frame under assembly that started in sector `r`, 0 for none.
    pub(crate) back: u32,
    /// Malformed sectors in `[S0, c)`.
    pub(crate) malformed_before: u64,
    /// FNV-1a of sector `c`, the first sector a resume reads after its replay.
    pub(crate) digest: u64,
}

/// The bytes of a frame under assembly that started in sector `start`: the longest prefix
/// any checkpoint holds. A frame under assembly only grows, so the checkpoints within one
/// frame share one buffer, and a frame spread over many checkpoints costs its bytes once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Frame {
    pub(crate) start: u64,
    pub(crate) bytes: Vec<u8>,
}

/// The frame under assembly of the checkpoint numbered `at`: the first `len` bytes of
/// frame `frame`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Held {
    pub(crate) at: u32,
    pub(crate) frame: u32,
    pub(crate) len: u32,
    pub(crate) number: u32,
    pub(crate) kind: FrameKind,
    pub(crate) remaining: i32,
}

/// Stored frames under assembly, by increasing checkpoint number and start sector.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Stored {
    pub(crate) frames: Vec<Frame>,
    pub(crate) held: Vec<Held>,
    /// Bytes of `frames`.
    pub(crate) used: usize,
}

impl Stored {
    /// Stores the frame under assembly of checkpoint `at` (after every one stored), which
    /// started in sector `start` and holds `data`, if its new bytes fit in `budget` bytes
    /// in all: whether it was stored. Bytes the last frame already holds cost nothing.
    pub(crate) fn add(
        &mut self,
        at: u32,
        start: u64,
        (number, kind, remaining): (u32, FrameKind, i32),
        data: &[u8],
        budget: usize,
    ) -> bool {
        if data.len() > MAX_FRAME_BYTES {
            return false;
        }
        let frame = match self.frames.last_mut() {
            Some(f) if f.start == start => {
                let common = f.bytes.len().min(data.len());
                // Always a prefix of the same frame; anything else is not stored.
                if f.bytes[..common] != data[..common] {
                    return false;
                }
                let grow = data.len() - common;
                if self.used + grow > budget {
                    return false;
                }
                f.bytes.extend_from_slice(&data[common..]);
                self.used += grow;
                self.frames.len() - 1
            }
            _ => {
                if self.used + data.len() > budget {
                    return false;
                }
                self.frames.push(Frame { start, bytes: data.to_vec() });
                self.used += data.len();
                self.frames.len() - 1
            }
        };
        // At most MAX_FRAME_BYTES bytes, and fewer frames than checkpoints.
        let (frame, len) = (frame as u32, data.len() as u32);
        self.held.push(Held { at, frame, len, number, kind, remaining });
        true
    }

    /// The frame under assembly of checkpoint `at`, if stored.
    pub(crate) fn get(&self, at: u32) -> Option<(&Held, &[u8])> {
        let h = &self.held[self.held.binary_search_by_key(&at, |h| h.at).ok()?];
        Some((h, self.bytes(h)))
    }

    pub(crate) fn bytes(&self, h: &Held) -> &[u8] {
        self.frames.get(h.frame as usize).and_then(|f| f.bytes.get(..h.len as usize)).unwrap_or(&[])
    }

    fn memory(&self) -> usize {
        self.held.capacity() * std::mem::size_of::<Held>()
            + self.frames.capacity() * std::mem::size_of::<Frame>()
            + self.frames.iter().map(|f| f.bytes.capacity()).sum::<usize>()
    }

    /// Copies the frame under assembly of `other`'s checkpoint `at` (which started in
    /// sector `start`) as this one's, within `budget`.
    fn take(&mut self, other: &Stored, at: u32, start: u64, budget: usize) {
        if let Some((h, bytes)) = other.get(at) {
            self.add(at, start, (h.number, h.kind, h.remaining), bytes, budget);
        }
    }
}

/// The state once the scan has ended (specification 4, 4.4.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EndState {
    /// `a` after the last delivery.
    pub(crate) next_acceptable: u32,
    /// Malformed sectors of the area.
    pub(crate) malformed: u64,
}

/// How a cursor made from an index starts.
#[derive(Debug)]
pub(crate) enum Plan<'a> {
    /// At the area's first audio sector.
    Start,
    /// At a checkpoint.
    At {
        lsn: u64,
        checkpoint: Checkpoint,
        /// Its frame under assembly when stored, else replayed from `lsn - back`.
        held: Option<(&'a Held, &'a [u8])>,
    },
    /// After the end.
    End(EndState),
}

/// The frame under assembly at a boundary, as the cursor holds it.
pub(crate) struct Assembling<'a> {
    pub(crate) start_lsn: u64,
    pub(crate) number: u32,
    pub(crate) kind: FrameKind,
    pub(crate) remaining: i32,
    pub(crate) data: &'a [u8],
}

/// FNV-1a (64-bit) of a sector.
pub(crate) fn digest(sector: &[u8; SECTOR_SIZE]) -> u64 {
    sector.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

// Indexes are built on one thread and used on others.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<AreaIndex>();
};

/// Saved reading state of one area: the scan's state every [`IndexOptions::interval`]
/// sectors from the area's first audio sector up to the index's frontier, so that a cursor
/// can start a frame range there instead of at the first sector (see the [module
/// documentation](self)).
///
/// # Memory
///
/// At most 24 bytes per `K` existing audio sectors (one checkpoint each), plus the
/// frames under assembly stored within [`IndexOptions::inline_budget`] (each at most
/// [`MAX_FRAME_BYTES`]), plus the LSNs of the first [`MAX_MALFORMED_WARNINGS`] malformed
/// sectors (16 KiB): [`AreaIndex::memory`] reports it. A 50-minute 2-channel DST area
/// (475 000 sectors) at `K = 64` takes about 180 KB.
// Specification 7, 7.1 item 2 (sacd-spec `da6b900`): a constant number of bytes per
// existing audio sector, plus at most one 65 536-byte frame under assembly per saved state
// that holds one, their number chosen by the caller (the inline budget).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AreaIndex {
    key: AreaKey,
    /// The area's existing audio sectors, `[start, end)`.
    start: u64,
    end: u64,
    interval: u32,
    inline_budget: usize,
    /// The checkpoints at `start + j x interval`, `j = 0, 1, ...`, without gaps.
    checkpoints: Vec<Checkpoint>,
    /// Stored frames under assembly.
    stored: Stored,
    end_state: Option<EndState>,
    /// The first [`MAX_MALFORMED_WARNINGS`] malformed sectors below the frontier.
    malformed: Vec<u32>,
}

impl AreaIndex {
    /// An empty index of `area` in an image of `image_sectors` sectors: its frontier is the
    /// area's first audio sector.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`] if the interval is not 1 to [`MAX_INTERVAL`].
    pub fn new(area: &Area, image_sectors: u64, options: IndexOptions) -> Result<Self, Error> {
        if !(1..=MAX_INTERVAL).contains(&options.interval) {
            return Err(Error::InvalidArgument("seek index interval must be 1 to 65536"));
        }
        let (start, end) = audio_sector_range(area, image_sectors);
        Ok(AreaIndex {
            key: AreaKey::new(area, image_sectors),
            start,
            end,
            interval: options.interval,
            inline_budget: options.inline_budget,
            checkpoints: Vec::new(),
            stored: Stored::default(),
            end_state: None,
            malformed: Vec::new(),
        })
    }

    /// The options the index was made with.
    pub fn options(&self) -> IndexOptions {
        IndexOptions { interval: self.interval, inline_budget: self.inline_budget }
    }

    /// The LSN up to which the index holds the scan's state: the area's first audio sector
    /// for an empty index, one past its last for a complete one.
    pub fn frontier(&self) -> u64 {
        if self.end_state.is_some() {
            self.end
        } else {
            self.checkpoint_lsn(self.checkpoints.len().saturating_sub(1))
        }
    }

    /// Whether the index covers the whole area: every range can then be opened from it.
    pub fn is_complete(&self) -> bool {
        self.end_state.is_some()
    }

    /// The number of checkpoints held.
    pub fn checkpoints(&self) -> usize {
        self.checkpoints.len()
    }

    /// The bytes the index holds, for the caller's accounting (see [Memory](Self#memory)).
    pub fn memory(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.checkpoints.capacity() * std::mem::size_of::<Checkpoint>()
            + self.stored.memory()
            + self.malformed.capacity() * std::mem::size_of::<u32>()
    }

    /// Reads the area on from the frontier, about `max_sectors` sectors (at least to the
    /// next checkpoint, so that each call makes progress; plus a frame under assembly
    /// replayed to resume there, and up to the next read chunk), and
    /// records its checkpoints: whether the index is now complete. Building an index
    /// ahead of the reads that use it ("eager") costs one pass over the area in all
    /// (specification 7, 7.1 item 3), which can be split into calls of any size, for
    /// example on another thread with an image of its own; [`AreaIndex::merge`] then joins
    /// that index with one the reads of the first thread filled ([`AreaIndex::absorb`]).
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`] if `image` has another sector count than the index's,
    /// [`Error::Io`] if a read fails; the checkpoints recorded before stay.
    pub fn extend<R: Read + Seek>(&mut self, image: &mut Image<R>, max_sectors: u64) -> Result<bool, Error> {
        if image.sector_count() != self.key.image_sectors {
            return Err(Error::InvalidArgument("seek index of another image size"));
        }
        if self.is_complete() {
            return Ok(true);
        }
        let mut cursor = AreaCursor::resume(self, u32::MAX);
        // Past the next boundary, which is recorded when its sector is read.
        let next = self.checkpoint_lsn(self.checkpoints.len());
        let stop = self.frontier().saturating_add(max_sectors).max(next.saturating_add(1));
        let scanned = cursor.scan_to(image, stop);
        if let ResumedFrom::Discarded { .. } = cursor.resumed_from() {
            return Err(Error::InvalidArgument("seek index does not match the image"));
        }
        // A cursor of this index joins it.
        self.absorb(&cursor)?;
        scanned.map(|_| self.is_complete())
    }

    pub(crate) fn key(&self) -> AreaKey {
        self.key
    }

    /// Whether the index is of `area` in an image of `image_sectors` sectors.
    pub(crate) fn check_key(&self, area: &Area, image_sectors: u64) -> Result<(), Error> {
        if self.key != AreaKey::new(area, image_sectors) {
            return Err(Error::InvalidArgument("seek index of another area or image"));
        }
        Ok(())
    }

    fn checkpoint_lsn(&self, j: usize) -> u64 {
        self.start + j as u64 * u64::from(self.interval)
    }

    /// The number of boundaries of the area, `ceil((end - start) / K)`.
    fn boundaries(&self) -> usize {
        (self.end - self.start).div_ceil(u64::from(self.interval)) as usize
    }

    /// Where a cursor of the range starting at frame `first` starts: the last checkpoint
    /// with `a <= first` (`a` grows with the LSN), or after the end when no frame at or
    /// after `first` is accepted.
    pub(crate) fn plan(&self, first: u32) -> Plan<'_> {
        if let Some(end) = self.end_state.filter(|e| first >= e.next_acceptable) {
            return Plan::End(end);
        }
        let j = self.checkpoints.partition_point(|c| c.next_acceptable <= first);
        let Some(j) = j.checked_sub(1) else { return Plan::Start };
        let checkpoint = self.checkpoints[j];
        let held = self.stored.get(j as u32);
        Plan::At { lsn: self.checkpoint_lsn(j), checkpoint, held }
    }

    /// The malformed sectors before `lsn`, which must not lie past the frontier.
    pub(crate) fn malformed_below(&self, lsn: u64) -> impl Iterator<Item = u32> + '_ {
        self.malformed.iter().copied().take_while(move |&m| u64::from(m) < lsn)
    }

    /// A recorder of the checkpoints past the frontier, for a cursor that starts at `from`
    /// (at or before the frontier).
    pub(crate) fn recorder(&self, from: u64) -> Option<Box<Recorder>> {
        if self.end_state.is_some() {
            return None;
        }
        let first = self.checkpoints.len();
        Some(Box::new(Recorder {
            key: self.key,
            interval: self.interval,
            first: first as u32,
            next: self.checkpoint_lsn(first),
            from,
            budget: self.inline_budget.saturating_sub(self.stored.used),
            checkpoints: Vec::new(),
            stored: Stored::default(),
            end_state: None,
        }))
    }

    /// Takes the checkpoints `cursor` recorded past this index's frontier (any of
    /// [`crate::AreaCursor`], [`crate::TrackCursor`] or [`crate::TrackAudioCursor`]); a
    /// cursor made without an index recorded none. Checkpoints the index already holds
    /// are compared, not added. Frames under assembly past the inline budget are not
    /// stored: those checkpoints are replayed instead.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`], with the index unchanged, if the cursor was made from
    /// an index of another area, image size or interval, if its checkpoints do not join
    /// this index's (it started past the frontier), or if they differ from those held
    /// (another image with the same key).
    pub fn absorb(&mut self, cursor: &impl crate::audio::SeekCursor) -> Result<(), Error> {
        let cursor = crate::audio::sealed::Sealed::area_cursor(cursor);
        let Some(r) = cursor.recorder() else { return Ok(()) };
        if r.key != self.key || r.interval != self.interval {
            return Err(Error::InvalidArgument("cursor of another area, image size or seek interval"));
        }
        if r.from > self.frontier() || r.first as usize > self.checkpoints.len() {
            return Err(Error::InvalidArgument("cursor's checkpoints do not join the seek index"));
        }
        let have = self.checkpoints.len();
        for (i, c) in r.checkpoints.iter().enumerate() {
            let j = r.first as usize + i;
            if j < have && self.checkpoints[j].next_acceptable != c.next_acceptable
                || j < have && self.checkpoints[j].malformed_before != c.malformed_before
            {
                return Err(Error::InvalidArgument("cursor's checkpoints differ from the seek index"));
            }
        }
        if matches!((self.end_state, r.end_state), (Some(a), Some(b)) if a != b) {
            return Err(Error::InvalidArgument("cursor's end state differs from the seek index"));
        }
        let old_frontier = self.frontier();
        let skip = have - r.first as usize;
        for (i, c) in r.checkpoints.iter().enumerate().skip(skip) {
            let j = r.first + i as u32;
            // A frame under assembly past the budget is replayed from its first sector
            // instead: still exact.
            let start = self.checkpoint_lsn(j as usize) - u64::from(c.back);
            self.stored.take(&r.stored, j, start, self.inline_budget);
            self.checkpoints.push(*c);
        }
        if r.end_state.is_some() && self.checkpoints.len() == self.boundaries() {
            self.end_state = r.end_state;
        }
        let frontier = self.frontier();
        if frontier > old_frontier && self.malformed.len() < MAX_MALFORMED_WARNINGS {
            // The cursor's warnings are those of a scan from the first sector.
            self.malformed = cursor
                .warnings()
                .iter()
                .filter_map(|w| w.lsn)
                .take_while(|&m| u64::from(m) < frontier)
                .take(MAX_MALFORMED_WARNINGS)
                .collect();
        }
        Ok(())
    }
}

/// The checkpoints a cursor records past an index's frontier, for [`AreaIndex::absorb`].
#[derive(Clone, Debug)]
pub(crate) struct Recorder {
    key: AreaKey,
    interval: u32,
    /// The number of the first checkpoint recorded.
    first: u32,
    /// The boundary to record next.
    next: u64,
    /// Where the cursor's scan started.
    from: u64,
    /// Bytes of frames under assembly still to store.
    budget: usize,
    checkpoints: Vec<Checkpoint>,
    stored: Stored,
    end_state: Option<EndState>,
}

impl Recorder {
    /// Whether the scan's state before sector `lsn` is to be recorded.
    #[inline]
    pub(crate) fn due(&self, lsn: u64) -> bool {
        lsn == self.next
    }

    /// Records the state before sector `lsn` (a due boundary), whose bytes are `sector`.
    pub(crate) fn record(
        &mut self,
        lsn: u64,
        next_acceptable: u32,
        malformed_before: u64,
        assembling: Option<Assembling<'_>>,
        sector: &[u8; SECTOR_SIZE],
    ) {
        let at = self.first + self.checkpoints.len() as u32;
        let mut back = 0;
        if let Some(a) = assembling {
            // The frame started in an audio sector at or before this one: below 2^32.
            back = u32::try_from(lsn - a.start_lsn).unwrap_or(u32::MAX).max(1);
            if u64::from(back) > REPLAY_SECTORS {
                self.stored.add(at, a.start_lsn, (a.number, a.kind, a.remaining), a.data, self.budget);
            }
        }
        self.checkpoints.push(Checkpoint { next_acceptable, back, malformed_before, digest: digest(sector) });
        self.next += u64::from(self.interval);
    }

    /// Records the end of the scan.
    pub(crate) fn finish(&mut self, next_acceptable: u32, malformed: u64) {
        self.end_state = Some(EndState { next_acceptable, malformed });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(next: &[u32], end: Option<u32>) -> AreaIndex {
        AreaIndex {
            key: AreaKey::new(&crate::audio::tests_support::area(1).1, 200),
            start: 100,
            end: 200,
            interval: 10,
            inline_budget: 0,
            checkpoints: next
                .iter()
                .map(|&a| Checkpoint { next_acceptable: a, back: 0, malformed_before: 0, digest: 0 })
                .collect(),
            stored: Stored::default(),
            end_state: end.map(|a| EndState { next_acceptable: a, malformed: 0 }),
            malformed: Vec::new(),
        }
    }

    fn at(p: Plan<'_>) -> Option<u64> {
        match p {
            Plan::At { lsn, .. } => Some(lsn),
            Plan::Start | Plan::End(_) => None,
        }
    }

    /// Specification 4, 4.5 item 6: a checkpoint whose straight-pass `a` is above `first`
    /// is never chosen, even when a later one has a smaller frame number in view.
    #[test]
    fn plan_takes_the_last_checkpoint_with_a_at_most_first() {
        // A huge timecode accepted between LSN 120 and 130 raises a past every later frame.
        let ix = index(&[0, 3, 9, 1_166_881, 1_166_881], None);
        assert_eq!(at(ix.plan(0)), Some(100));
        assert_eq!(at(ix.plan(2)), Some(100));
        assert_eq!(at(ix.plan(3)), Some(110));
        assert_eq!(at(ix.plan(8)), Some(110));
        assert_eq!(at(ix.plan(9)), Some(120));
        assert_eq!(at(ix.plan(1_166_880)), Some(120));
        assert_eq!(at(ix.plan(1_166_881)), Some(140));
        assert!(matches!(index(&[], None).plan(5), Plan::Start));
        let done = index(&[0, 3], Some(7));
        assert_eq!(at(done.plan(6)), Some(110));
        assert!(matches!(done.plan(7), Plan::End(EndState { next_acceptable: 7, .. })));
        assert_eq!(done.frontier(), 200);
        assert_eq!(ix.frontier(), 140);
        assert_eq!(index(&[], None).frontier(), 100);
    }

    #[test]
    fn options_are_checked() {
        let (_, area, _) = crate::audio::tests_support::area(4);
        for k in [0, MAX_INTERVAL + 1] {
            let e = AreaIndex::new(&area, 4, IndexOptions::new().with_interval(k)).unwrap_err();
            assert!(matches!(e, Error::InvalidArgument(_)));
        }
        let ix = AreaIndex::new(&area, 4, IndexOptions::new().with_interval(MAX_INTERVAL)).unwrap();
        assert_eq!(ix.options().interval, MAX_INTERVAL);
        assert_eq!((ix.frontier(), ix.is_complete(), ix.checkpoints()), (0, false, 0));
        assert!(ix.memory() >= std::mem::size_of::<AreaIndex>());
    }

    #[test]
    fn digest_is_fnv1a() {
        // FNV-1a of 2048 zero bytes, computed independently.
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for _ in 0..SECTOR_SIZE {
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        assert_eq!(digest(&[0; SECTOR_SIZE]), h);
        assert_ne!(digest(&[0; SECTOR_SIZE]), digest(&[1; SECTOR_SIZE]));
    }
}
