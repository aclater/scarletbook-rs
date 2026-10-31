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
use crate::log::sb_log;
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
    /// (see [`REPLAY_SECTORS`]); 0 stores none. A cursor made from the index may hold as
    /// much again while it records. Saved indexes keep at most 4 GiB of it.
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
    /// At sector `lsn`, provisionally (specification 4, 4.2 item 8, sacd-spec `82eac6c`):
    /// with no frame under assembly and the range's first frame as the next acceptable
    /// frame number, after `probes` reads that chose `lsn` (see [`SeekPolicy`]). The
    /// positions are provisional ([`Speculation`]). Both are 0 until the cursor first
    /// reads, which is when it chooses.
    Speculative {
        /// The sector the provisional reading began at, `L`.
        lsn: u64,
        /// The reads made to choose it.
        probes: u32,
    },
}

/// How cursors start a range that the seek index (if any) cannot reach cheaply.
///
/// [`SeekPolicy::Exact`], the default, starts every range from the area's first sector or
/// from a checkpoint of the seek index (specification 4, 4.2 items 6 and 7), so that its
/// positions are always this specification's. [`SeekPolicy::Speculative`] lets a cursor
/// begin far into an area without reading the sectors before it: **provisional reading**,
/// specification 4, 4.2 item 8 (sacd-spec `82eac6c`). Opting in is the caller's request
/// that item requires.
///
/// # Provisional reading
///
/// A cursor that would have to read more than [`SpeculativeOptions::min_gap_sectors`]
/// sectors before the range's estimated position chooses a sector `L` instead: it searches
/// the frame records' timecodes for a sector close before the range's first frame `u`
/// (interpolation with a bisection safeguard, a few small reads; at most a constant times
/// `log2(n + 1)` sectors for an area of `n` audio sectors, as 7.1 item 3 allows), and
/// begins at the seek index's checkpoint boundary at least [`REPLAY_SECTORS`] before it,
/// with no frame under assembly and `u` as the next acceptable frame number.
///
/// Its positions are exactly the specification's when both (4.2 item 8):
/// - (a) `a_L <= u`: no frame numbered `u` or more was accepted before `L` by a reader
///   processing from the first sector; and
/// - (b) the frame under assembly at `L` is none or has a frame number below `u`.
///
/// They hold on every image whose frames start in increasing frame number order (any
/// well-formed disc), but no read of `L` or later sectors can establish them: one earlier
/// sector can break (a). So the positions are **provisional** ([`Speculation::Pending`])
/// until a pass from the first sector establishes or refutes them. When (a) or (b) fails,
/// positions numbered `a*` or more are still exact, where `a*` is the next acceptable frame
/// number of a reader processing from the first sector just after it delivered or
/// discarded the frame under assembly at `L`; below `a*`, a provisional position can differ
/// whether or not the specification's is missing (a frame accepted before `L` is never
/// seen, and frames the specification discards can be accepted). Every frame a provisional
/// reading returns is a complete frame of the image, in increasing frame number order.
///
/// The malformed sectors and warnings of a provisional reading cover the sectors it
/// processes from `L` only (4.4.5 item 5); [`crate::TrackCursor::counts_from`] says so.
/// Its missing and invalid counts are provisional too (4.7.1 item 6).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum SeekPolicy {
    /// Exact reads only: the default.
    #[default]
    Exact,
    /// Provisional reading where an exact start would read far.
    Speculative(SpeculativeOptions),
}

/// Options of [`SeekPolicy::Speculative`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SpeculativeOptions {
    /// Read exactly when the exact start lies at most this many sectors before the range's
    /// estimated position (in proportion to the play time). Default 2 048 (4 MiB).
    pub min_gap_sectors: u64,
    /// Sectors per probe read while searching, 1 to 32. Default 8 (16 KiB).
    pub probe_sectors: u32,
    /// Most probes per search. Default 48; at most `2 x ceil(log2(n + 1)) + 4` are made in
    /// any case, for an area of `n` audio sectors.
    pub max_probes: u32,
}

impl Default for SpeculativeOptions {
    fn default() -> Self {
        SpeculativeOptions { min_gap_sectors: 2048, probe_sectors: 8, max_probes: 48 }
    }
}

impl SpeculativeOptions {
    /// The defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// These options with `min_gap_sectors`.
    pub fn with_min_gap_sectors(self, sectors: u64) -> Self {
        SpeculativeOptions { min_gap_sectors: sectors, ..self }
    }

    /// These options with `probe_sectors` (clamped to 1 to 32 when used).
    pub fn with_probe_sectors(self, sectors: u32) -> Self {
        SpeculativeOptions { probe_sectors: sectors, ..self }
    }

    /// These options with `max_probes`.
    pub fn with_max_probes(self, probes: u32) -> Self {
        SpeculativeOptions { max_probes: probes, ..self }
    }
}

/// Whether a cursor's positions are this specification's, or provisional (specification
/// 4, 4.2 item 8; see [`SeekPolicy`]).
///
/// A cursor chooses its start on its first read: check this after the first frame.
/// Before it, a cursor allowed to read provisionally shows `Pending { lsn: 0, .. }`, even
/// one that then finds an exact start better and reports [`Speculation::Exact`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Speculation {
    /// Not a provisional reading: the positions are exact.
    Exact,
    /// A provisional reading from sector `lsn` for positions `first` or more, not yet
    /// established or refuted (`lsn` is 0 until the cursor first reads).
    Pending {
        /// `L`.
        lsn: u64,
        /// `u`, the range's first frame when the reading began.
        first: u32,
    },
    /// A provisional reading whose conditions (a) and (b) a pass from the first sector has
    /// established: its positions are this specification's (4.2 item 8).
    Confirmed {
        /// `L`.
        lsn: u64,
        /// `u`.
        first: u32,
    },
    /// A provisional reading whose condition (a) or (b) fails: its positions numbered
    /// `first` to `exact_from - 1` are **possibly different** from this specification's
    /// (4.2 item 8), whether or not they are missing; from `exact_from` on they are exact.
    /// `exact_from` is at least the specification's `a*` (the next acceptable frame number
    /// of a reader from the first sector just after it delivered or discarded the frame
    /// under assembly at `L`): the larger of `a_L` and one more than that frame's number.
    Refuted {
        /// `L`.
        lsn: u64,
        /// `u`.
        first: u32,
        /// The last position the cursor had returned, if any.
        last_returned: Option<u32>,
        /// Positions from here on are exact.
        exact_from: u32,
    },
    /// A provisional reading that will not be checked: dropped past the limit of
    /// [`MAX_CLAIMS`] pending readings per index, or found to be of an index that does not
    /// match the image. Its positions stay provisional.
    Unverified {
        /// `L`.
        lsn: u64,
        /// `u`.
        first: u32,
    },
}

/// Most provisional readings an index keeps waiting to be checked; past it, the oldest is
/// dropped as [`Speculation::Unverified`].
pub const MAX_CLAIMS: usize = 256;

/// Most verdicts kept waiting to be taken, per index ([`AreaIndex::take_events`]) and per
/// disc and area ([`crate::Disc::take_seek_events`]); past it, the oldest are dropped (the
/// cursors still hold their verdicts).
pub const MAX_EVENTS: usize = 4 * MAX_CLAIMS;

/// Appends `events` to `queue`, keeping the last [`MAX_EVENTS`].
pub(crate) fn queue_events(queue: &mut Vec<SeekEvent>, events: impl IntoIterator<Item = SeekEvent>) {
    queue.extend(events);
    if queue.len() > MAX_EVENTS {
        queue.drain(..queue.len() - MAX_EVENTS);
    }
}

/// What became of a provisional reading: an event of [`AreaIndex::take_events`] and
/// [`crate::Disc::take_seek_events`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SeekEvent {
    /// The area.
    pub slot: crate::Slot,
    /// The provisional reading, as the cursors that made it number it
    /// ([`crate::TrackCursor::speculation_id`]).
    pub claim: u64,
    /// Its verdict: [`Speculation::Confirmed`], [`Speculation::Refuted`] or
    /// [`Speculation::Unverified`].
    pub speculation: Speculation,
}

static NEXT_CLAIM: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// The verdict on a provisional reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    Confirmed,
    Refuted { exact_from: u32 },
    Unverified,
}

/// A provisional reading (`L`, `u`), shared by the cursor that made it (and its
/// continuations) and the index that checks it.
#[derive(Debug)]
pub(crate) struct Claim {
    pub(crate) id: u64,
    slot: crate::Slot,
    first: u32,
    lsn: std::sync::atomic::AtomicU64,
    /// One more than the last position returned, 0 for none.
    returned: std::sync::atomic::AtomicU32,
    verdict: std::sync::Mutex<Option<Verdict>>,
}

impl Claim {
    pub(crate) fn new(slot: crate::Slot, first: u32) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Claim {
            id: NEXT_CLAIM.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            slot,
            first,
            lsn: std::sync::atomic::AtomicU64::new(0),
            returned: std::sync::atomic::AtomicU32::new(0),
            verdict: std::sync::Mutex::new(None),
        })
    }

    pub(crate) fn set_lsn(&self, lsn: u64) {
        self.lsn.store(lsn, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn lsn(&self) -> u64 {
        self.lsn.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Notes that position `t` was returned.
    #[inline]
    pub(crate) fn returned(&self, t: u32) {
        self.returned.fetch_max(t.saturating_add(1), std::sync::atomic::Ordering::Relaxed);
    }

    fn verdict(&self) -> Option<Verdict> {
        // Nothing panics while the lock is held.
        *self.verdict.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Settles the claim, once: whether this call did.
    fn settle(&self, verdict: Verdict) -> bool {
        let mut v = self.verdict.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if v.is_some() {
            return false;
        }
        *v = Some(verdict);
        true
    }

    pub(crate) fn speculation(&self) -> Speculation {
        let (lsn, first) = (self.lsn(), self.first);
        match self.verdict() {
            None => Speculation::Pending { lsn, first },
            Some(Verdict::Confirmed) => Speculation::Confirmed { lsn, first },
            Some(Verdict::Refuted { exact_from }) => {
                let r = self.returned.load(std::sync::atomic::Ordering::Relaxed);
                Speculation::Refuted { lsn, first, last_returned: r.checked_sub(1), exact_from }
            }
            Some(Verdict::Unverified) => Speculation::Unverified { lsn, first },
        }
    }

    fn event(&self) -> SeekEvent {
        SeekEvent { slot: self.slot, claim: self.id, speculation: self.speculation() }
    }
}

/// The state of a provisional reading at a checkpoint boundary it crossed, after it had
/// resynchronised (delivered a frame): where its frame under assembly started (`back`, as
/// in [`Checkpoint`], at most [`REPLAY_SECTORS`]), the digest of the sector, and one more
/// than the largest frame number it had delivered complete (`seen`). Used only by later
/// provisional starts (never by exact ones), and not saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Provisional {
    pub(crate) back: u32,
    pub(crate) digest: u64,
    pub(crate) seen: u32,
    /// The number of the frame under assembly there, if any.
    pub(crate) under: Option<u32>,
}

/// The provisional checkpoints a provisional reading records.
#[derive(Debug)]
pub(crate) struct ProvisionalRecorder {
    interval: u64,
    next: u64,
    pub(crate) list: Vec<(u64, Provisional)>,
}

impl ProvisionalRecorder {
    /// A recorder of the boundaries `start + j x interval` after `from`.
    pub(crate) fn new(start: u64, from: u64, interval: u32) -> Box<Self> {
        let k = u64::from(interval.max(1));
        let next = start + (from - start) / k * k + k;
        Box::new(ProvisionalRecorder { interval: k, next, list: Vec::new() })
    }

    #[inline]
    pub(crate) fn due(&self, lsn: u64) -> bool {
        lsn == self.next
    }

    /// Records the boundary before sector `lsn`, if the reading has resynchronised.
    pub(crate) fn record(
        &mut self,
        lsn: u64,
        under: Option<(u64, u32)>,
        seen: u32,
        sector: &[u8; SECTOR_SIZE],
    ) {
        self.next += self.interval;
        let back = under.map_or(0, |(b, _)| b);
        if seen > 0 && back <= REPLAY_SECTORS {
            // At most REPLAY_SECTORS.
            let back = back as u32;
            let under = under.map(|(_, n)| n);
            self.list.push((lsn, Provisional { back, digest: digest(sector), seen, under }));
        }
    }
}

/// A provisional start still to be chosen: on the cursor's first read, from the area's
/// play time and the exact plan the cursor holds meanwhile.
#[derive(Debug)]
pub(crate) struct Speculate {
    pub(crate) first: u32,
    pub(crate) playtime: u32,
    pub(crate) interval: u32,
    pub(crate) options: SpeculativeOptions,
    /// The exact start, used when the search finds nothing better.
    pub(crate) exact: Box<AreaCursor>,
    /// Where the exact start begins: the search looks past it.
    pub(crate) from: u64,
}

/// The first frame record of a sector, as a frame number (specification 4, 4.3.2).
fn first_record(sector: &[u8]) -> Option<u32> {
    let header = *sector.first()?;
    let (f, k) = ((header >> 2) & 7, usize::from(header >> 5));
    if f == 0 {
        return None;
    }
    let r = 1 + 2 * k;
    let tc = sector.get(r..r + 3)?;
    Some((u32::from(tc[0]) * 60 + u32::from(tc[1])) * 75 + u32::from(tc[2]))
}

/// Most sectors one probe reads before giving up on finding a frame record (a DST frame
/// spans at most 31 sectors on well-formed discs, 4.3.3).
const PROBE_SPAN: u64 = 32;

/// Searches `[lo, hi)` for a sector holding a frame record whose timecode is at most
/// `first`, as close before the frame numbered `first` as the probes allow: the sector
/// found (`lo` itself if none) and the number of reads made.
pub(crate) fn locate<R: Read + Seek>(
    image: &mut Image<R>,
    (mut lo, mut hi): (u64, u64),
    first: u32,
    playtime: u32,
    options: &SpeculativeOptions,
) -> Result<(u64, u32), Error> {
    let per = u64::from(options.probe_sectors.clamp(1, 32));
    // Specification 7, 7.1 item 3: at most a constant times log2(n + 1) sectors.
    let n = hi.saturating_sub(lo);
    let log = u64::BITS - n.leading_zeros();
    let max = options.max_probes.min(2 * log + 4);
    let mut buf = vec![0u8; per as usize * SECTOR_SIZE];
    let (mut lo_t, mut hi_t) = (0u32, playtime.max(first.saturating_add(1)));
    let (mut reads, mut halve) = (0u32, false);
    while hi - lo > PROBE_SPAN && reads < max {
        let guess = if halve {
            lo + (hi - lo) / 2
        } else {
            let num = u128::from(first.saturating_sub(lo_t));
            let den = u128::from(hi_t.saturating_sub(lo_t).max(1));
            lo + (u128::from(hi - lo) * num / den) as u64
        }
        .clamp(lo + 1, hi - 1);
        let before = hi - lo;
        // One probe: from `guess`, `per` sectors at a time, until a record, starting reads
        // only below `guess + PROBE_SPAN` (the last may end up to `per - 1` past it) and
        // below `hi`, so a record found lies in `[guess, hi)`.
        let mut found = None;
        let mut at = guess;
        while found.is_none() && at < hi.min(guess + PROBE_SPAN) && reads < max {
            let count = per.min(hi - at);
            let chunk = &mut buf[..count as usize * SECTOR_SIZE];
            if image.read_into(at, chunk)? == crate::image::ReadOutcome::OutOfRange {
                return Err(Error::InvalidArgument("audio sectors outside the image"));
            }
            reads += 1;
            found = (0..count)
                .find_map(|i| first_record(&chunk[i as usize * SECTOR_SIZE..]).map(|t| (at + i, t)));
            at += count;
        }
        match found {
            Some((m, t)) if t <= first => (lo, lo_t) = (m, t),
            Some((_, t)) => (hi, hi_t) = (guess, t.max(lo_t.saturating_add(1))),
            None => hi = guess,
        }
        // A step that did not at least halve the interval is followed by a bisection.
        halve = (hi - lo) * 2 > before;
    }
    Ok((lo, reads))
}

/// The provisional start `L` for a frame record found at `target`: the checkpoint boundary
/// (`start + j x interval`) at least [`REPLAY_SECTORS`] before it.
pub(crate) fn provisional_start(start: u64, target: u64, interval: u32) -> u64 {
    let k = u64::from(interval.max(1));
    let from = target.saturating_sub(REPLAY_SECTORS).max(start);
    start + (from - start) / k * k
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
/// # One index per image
///
/// An index holds the reading state of one area of **one image**. Its key (the area's
/// slot, audio sector range and channel count, and the image's sector count) cannot tell
/// apart two images of the same size and layout, such as two rips of one disc, and an index
/// fed from both would mix their state: a later read of one could report the other's
/// malformed sectors and warnings, or frames under assembly, with no error. So within a
/// process, an index is of the image of the first cursor it takes checkpoints from
/// ([`AreaIndex::absorb`], [`AreaIndex::extend`]; [`crate::Image::id`], which the discs
/// made by [`crate::Disc::share`] and `clone` share), and refuses cursors and reads of any
/// other. That identity is not saved: an index loaded with [`AreaIndex::from_bytes`] (or
/// given to [`crate::Disc::set_seek_index`]) takes the image of its first absorb or
/// extend, and is trusted to be of it (loading is safe, not exact; see the [module
/// documentation](self#trust)).
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
#[derive(Clone, Debug)]
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
    /// The image of the first cursor this index took checkpoints from ([`Image::id`],
    /// unique within the process; not saved), whose cursors alone it takes from then on.
    image_id: Option<u64>,
    /// Provisional readings waiting to be checked (not saved).
    claims: Vec<std::sync::Arc<Claim>>,
    /// Provisional checkpoints past the frontier, for provisional starts only (not saved).
    provisional: std::collections::BTreeMap<u64, Provisional>,
    /// Verdicts not yet taken.
    events: Vec<SeekEvent>,
}

/// Two indexes are equal when they hold the same reading state: the image they were
/// built from in this process is not part of it.
impl PartialEq for AreaIndex {
    fn eq(&self, other: &Self) -> bool {
        (self.key, self.start, self.end, self.interval, self.inline_budget)
            == (other.key, other.start, other.end, other.interval, other.inline_budget)
            && self.checkpoints == other.checkpoints
            && self.stored == other.stored
            && self.end_state == other.end_state
            && self.malformed == other.malformed
    }
}

impl Eq for AreaIndex {}

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
            image_id: None,
            claims: Vec::new(),
            provisional: std::collections::BTreeMap::new(),
            events: Vec::new(),
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
    /// The index is of `image` from then on (see [One index per image](Self#one-index-per-image)).
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`] if `image` has another sector count than the index's, or
    /// is another image than the one the index was built from in this process;
    /// [`Error::Io`] if a read fails. The checkpoints recorded before stay.
    pub fn extend<R: Read + Seek>(&mut self, image: &mut Image<R>, max_sectors: u64) -> Result<bool, Error> {
        if image.sector_count() != self.key.image_sectors {
            return Err(Error::InvalidArgument("seek index of another image size"));
        }
        if self.image_id.is_some_and(|id| id != image.id()) {
            return Err(Error::InvalidArgument("seek index of another image"));
        }
        // Of this image from now on, even if there is nothing left to read.
        self.image_id = Some(image.id());
        if self.is_complete() {
            self.verify(image)?;
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
        self.take_from(&cursor)?;
        scanned?;
        self.verify(image)?;
        Ok(self.is_complete())
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

    /// The resume plan at checkpoint `j`, which must exist.
    pub(crate) fn plan_at(&self, j: usize) -> Plan<'_> {
        let checkpoint = self.checkpoints[j];
        Plan::At { lsn: self.checkpoint_lsn(j), checkpoint, held: self.stored.get(j as u32) }
    }

    /// The number of the last checkpoint at or before `lsn` (which must not lie past the
    /// frontier): `None` if there is none.
    pub(crate) fn checkpoint_before(&self, lsn: u64) -> Option<usize> {
        let j = ((lsn.checked_sub(self.start)?) / u64::from(self.interval)) as usize;
        (!self.checkpoints.is_empty()).then(|| j.min(self.checkpoints.len() - 1))
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
            // The cursor's own bound; this index charges only what it adds when it takes
            // the frames (a frame it holds a prefix of costs its growth).
            budget: self.inline_budget,
            checkpoints: Vec::new(),
            stored: Stored::default(),
            end_state: None,
            kept: false,
        }))
    }

    /// Takes the checkpoints `cursor` recorded past this index's frontier (any of
    /// [`crate::AreaCursor`], [`crate::TrackCursor`] or [`crate::TrackAudioCursor`]); a
    /// cursor made without an index recorded none. Checkpoints the index already holds
    /// are compared, not added. Frames under assembly past the inline budget are not
    /// stored: those checkpoints are replayed instead. The cursor then drops what it
    /// recorded (absorbing it again costs nothing until it records more), and the index
    /// is of the cursor's image from then on (see [One index per
    /// image](Self#one-index-per-image)).
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`], with the index unchanged:
    /// - if the cursor read another image than the one this index was built from in this
    ///   process (it then stops recording);
    /// - if it was made from an index of another area, image size or interval, if its
    ///   checkpoints do not join this index's (it started past the frontier), or if they
    ///   differ from those held (another image with the same key).
    pub fn absorb(&mut self, cursor: &mut impl crate::audio::SeekCursor) -> Result<(), Error> {
        let cursor = crate::audio::sealed::Sealed::area_cursor_mut(cursor);
        match self.take_from(cursor) {
            Ok(()) => {
                Self::drain(cursor);
                self.take_provisional(cursor);
                Ok(())
            }
            Err(e) => {
                if self.of_another_image(cursor) {
                    cursor.stop_recording();
                }
                Err(e)
            }
        }
    }

    /// Whether `cursor` read another image than the one this index is of.
    fn of_another_image(&self, cursor: &AreaCursor) -> bool {
        matches!((self.image_id, cursor.image_id()), (Some(a), Some(b)) if a != b)
    }

    /// Forgets the image this index was built from, as for an index loaded from bytes:
    /// the next cursor it takes checkpoints from, or the next `extend`, sets it again.
    pub(crate) fn forget_image(&mut self) {
        self.image_id = None;
    }

    /// [`AreaIndex::absorb`] without draining the cursor.
    pub(crate) fn take_from(&mut self, cursor: &AreaCursor) -> Result<(), Error> {
        let Some(r) = cursor.recorder() else { return Ok(()) };
        if self.of_another_image(cursor) {
            return Err(Error::InvalidArgument("cursor of another image than the seek index's"));
        }
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
        if self.image_id.is_none() {
            self.image_id = cursor.image_id();
        }
        self.prune_provisional();
        Ok(())
    }
}

impl AreaIndex {
    /// [`AreaIndex::absorb`], then drops what `cursor` recorded, so that absorbing it
    /// again costs nothing until it records more; a cursor whose checkpoints cannot join
    /// this index stops recording.
    pub(crate) fn absorb_drain(&mut self, cursor: &mut AreaCursor) {
        if self.take_from(cursor).is_err() {
            cursor.stop_recording();
            return;
        }
        Self::drain(cursor);
        self.take_provisional(cursor);
    }

    /// Drops what `cursor` recorded, once taken: it records on from there.
    pub(crate) fn drain(cursor: &mut AreaCursor) {
        if let Some(r) = cursor.recorder_mut() {
            if r.end_state.is_some() {
                cursor.stop_recording();
            } else {
                r.first += r.checkpoints.len() as u32;
                r.checkpoints.clear();
                r.stored = Stored::default();
            }
        }
    }

    /// The LSN a cursor of positions `first` or more would start at, `None` for one that
    /// reads nothing.
    pub(crate) fn plan_lsn(&self, first: u32) -> Option<u64> {
        match self.plan(first) {
            Plan::Start => Some(self.start),
            Plan::At { lsn, .. } => Some(lsn),
            Plan::End(_) => None,
        }
    }

    /// The checkpoint interval.
    pub(crate) fn interval(&self) -> u32 {
        self.interval
    }
}

impl AreaIndex {
    /// The provisional readings waiting to be checked ([`AreaIndex::verify`]).
    pub fn pending_claims(&self) -> usize {
        self.claims.len()
    }

    /// Takes the verdicts on provisional readings reached since the last call (see
    /// [`AreaIndex::verify`]).
    pub fn take_events(&mut self) -> Vec<SeekEvent> {
        std::mem::take(&mut self.events)
    }

    /// Takes from `cursor` its provisional reading, to check, and the provisional
    /// checkpoints it recorded past the frontier: only for a cursor of this index's image.
    pub(crate) fn take_provisional(&mut self, cursor: &mut AreaCursor) {
        if self.image_id.is_some_and(|id| cursor.image_id().is_some_and(|c| c != id)) {
            return;
        }
        if let Some(claim) = cursor.unregistered_claim() {
            if self.claims.len() >= MAX_CLAIMS {
                let old = self.claims.remove(0);
                if old.settle(Verdict::Unverified) {
                    queue_events(&mut self.events, [old.event()]);
                }
            }
            self.claims.push(claim);
        }
        let frontier = self.frontier();
        let most = self.boundaries();
        for (lsn, p) in cursor.take_provisional_checkpoints() {
            if lsn > frontier && self.provisional.len() < most {
                self.provisional.insert(lsn, p);
            }
        }
    }

    /// Drops the oldest pending readings past [`MAX_CLAIMS`], as unverified.
    pub(crate) fn cap_claims(&mut self) {
        while self.claims.len() > MAX_CLAIMS {
            let old = self.claims.remove(0);
            if old.settle(Verdict::Unverified) {
                queue_events(&mut self.events, [old.event()]);
            }
        }
    }

    /// Drops what the exact checkpoints now cover.
    fn prune_provisional(&mut self) {
        let frontier = self.frontier();
        self.provisional.retain(|&lsn, _| lsn > frontier);
    }

    /// A provisional start for positions `u` or more from the provisional checkpoints: the
    /// last one whose reading had delivered no frame numbered `u` or more and whose frame
    /// under assembly is none or numbered below `u` (so that, as far as that reading saw,
    /// (a) and (b) of 4.2 item 8 hold there), if a later one had delivered such a frame (so
    /// that `u` lies within what they cover).
    pub(crate) fn plan_provisional(&self, u: u32) -> Option<(u64, Provisional)> {
        let mut best = None;
        for (&lsn, &p) in &self.provisional {
            if p.seen > u {
                return best;
            }
            if p.under.is_none_or(|n| n < u) {
                best = Some((lsn, p));
            }
        }
        None
    }

    /// Checks the provisional readings that the exact checkpoints now reach (specification
    /// 4, 4.2 item 8): for each, the state at its `L` of a reader from the first sector,
    /// rebuilt from the checkpoint at or before `L` (at most [`REPLAY_SECTORS`] plus the
    /// interval in sectors read through `image`). A reading is confirmed when (a) that
    /// reader's next acceptable frame number at `L` is at most `u` and (b) its frame under
    /// assembly there is none or numbered below `u`, and refuted ("possibly different")
    /// otherwise; its cursors see the verdict at once ([`crate::TrackCursor::speculation`]),
    /// and it becomes an event ([`AreaIndex::take_events`]). Refutations are logged at
    /// warning level. Returns the number of readings settled. [`AreaIndex::extend`] does
    /// this itself.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`] if `image` is another image than the index's (see [One
    /// index per image](AreaIndex#one-index-per-image)); [`Error::Io`] if a read fails (the
    /// readings not yet checked stay pending).
    pub fn verify<R: Read + Seek>(&mut self, image: &mut Image<R>) -> Result<usize, Error> {
        if self.image_id.is_some_and(|id| id != image.id()) {
            return Err(Error::InvalidArgument("seek index of another image"));
        }
        // Checked against this image: the index is of it from now on.
        self.image_id = Some(image.id());
        let frontier = self.frontier();
        let mut settled = 0;
        let mut i = 0;
        while i < self.claims.len() {
            let claim = std::sync::Arc::clone(&self.claims[i]);
            let lsn = claim.lsn();
            if claim.verdict().is_some() {
                self.claims.remove(i);
                continue;
            }
            if lsn == 0 || lsn > frontier || lsn >= self.end {
                i += 1;
                continue;
            }
            let verdict = match AreaCursor::state_at(self, lsn, image)? {
                None => Verdict::Unverified,
                Some((a, held, _)) => {
                    let u = claim.first;
                    if a <= u && held.is_none_or(|g| g < u) {
                        Verdict::Confirmed
                    } else {
                        Verdict::Refuted { exact_from: a.max(held.map_or(0, |g| g.saturating_add(1))) }
                    }
                }
            };
            self.claims.remove(i);
            if claim.settle(verdict) {
                settled += 1;
                let event = claim.event();
                match event.speculation {
                    Speculation::Refuted { first, last_returned, exact_from, .. } => sb_log!(
                        image.log(),
                        Warn,
                        "area {}: provisional reading from LSN {lsn} for frames {first}.. possibly different \
                         from the specification below frame {exact_from} (4.2 item 8; returned up to {:?})",
                        claim.slot,
                        last_returned
                    ),
                    _ => sb_log!(
                        image.log(),
                        Debug,
                        "area {}: provisional reading from LSN {lsn}: {:?}",
                        claim.slot,
                        event.speculation
                    ),
                }
                queue_events(&mut self.events, [event]);
            }
        }
        Ok(settled)
    }
}

/// The seek indexes a [`crate::Disc`] keeps, one per slot, each behind its own lock so that
/// cursors can be made and continued through `&Disc`: a read lock while a cursor is made
/// from an index, a write lock while checkpoints are taken into it. No log sink runs under
/// either. Kept apart from the rest of the disc so that discs sharing an image can share
/// it.
#[derive(Debug, Default)]
pub(crate) struct SeekIndexes {
    indexes: [std::sync::RwLock<Option<AreaIndex>>; 2],
    /// Verdicts on provisional readings, for [`crate::Disc::take_seek_events`].
    events: [std::sync::Mutex<Vec<SeekEvent>>; 2],
}

impl SeekIndexes {
    fn at(slot: crate::Slot) -> usize {
        match slot {
            crate::Slot::TwoChannel => 0,
            crate::Slot::Multichannel => 1,
        }
    }

    /// Runs `f` on the index of `slot`, if one is kept, under a read lock.
    pub(crate) fn read<T>(&self, slot: crate::Slot, f: impl FnOnce(Option<&AreaIndex>) -> T) -> T {
        // Nothing panics while a lock is held: the indexes stay consistent.
        let index = self.indexes[Self::at(slot)].read().unwrap_or_else(std::sync::PoisonError::into_inner);
        f(index.as_ref())
    }

    /// Runs `f` on the index of `slot` (or its absence) under a write lock.
    pub(crate) fn write<T>(&self, slot: crate::Slot, f: impl FnOnce(&mut Option<AreaIndex>) -> T) -> T {
        let mut index =
            self.indexes[Self::at(slot)].write().unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut index)
    }

    /// Takes what `cursor` recorded into the index of its slot, if one is kept and the
    /// cursor read the image `image_id` (this disc's, shared by its clones and shares);
    /// costs a test when it recorded nothing new. A cursor that read another image (of the
    /// same size and layout, so that the key cannot tell) stops recording instead: its
    /// checkpoints are that image's.
    #[inline]
    pub(crate) fn absorb(&self, cursor: &mut AreaCursor, image_id: u64) {
        let kept = cursor.recorder().is_none_or(|r| r.kept);
        if kept && cursor.has_news() {
            if cursor.image_id().is_some_and(|id| id != image_id) {
                cursor.stop_recording();
                return;
            }
            let events = self.write(cursor.slot(), |index| match index {
                Some(index) => {
                    index.absorb_drain(cursor);
                    index.take_events()
                }
                None => {
                    cursor.stop_recording();
                    Vec::new()
                }
            });
            self.push_events(cursor.slot(), events);
        }
    }

    /// Queues verdicts for [`crate::Disc::take_seek_events`].
    pub(crate) fn push_events(&self, slot: crate::Slot, events: Vec<SeekEvent>) {
        if !events.is_empty() {
            let mut queue =
                self.events[Self::at(slot)].lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            queue_events(&mut queue, events);
        }
    }

    /// Takes the queued verdicts of `slot`.
    pub(crate) fn take_events(&self, slot: crate::Slot) -> Vec<SeekEvent> {
        let mut queue = self.events[Self::at(slot)].lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::take(&mut *queue)
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
    /// Made from the index a [`crate::Disc`] keeps, which takes the checkpoints; a cursor
    /// from a caller's index is left to the caller.
    pub(crate) kept: bool,
}

impl Recorder {
    /// Whether anything was recorded since the last [`AreaIndex::absorb_drain`].
    pub(crate) fn has_news(&self) -> bool {
        !self.checkpoints.is_empty() || self.end_state.is_some()
    }

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
            image_id: None,
            claims: Vec::new(),
            provisional: std::collections::BTreeMap::new(),
            events: Vec::new(),
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
