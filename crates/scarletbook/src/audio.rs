// SPDX-License-Identifier: MIT OR Apache-2.0

//! Audio sector multiplexing (specification 4): frame reassembly, the timeline and tracks.

use std::collections::VecDeque;
use std::io::{Read, Seek};

use crate::control::Progress;
use crate::dst::{DstDecoder, DstError};
use crate::error::{Error, Slot, Warning, WarningCode};
use crate::image::{Image, ReadOutcome, SECTOR_SIZE};
use crate::log::{Level, Log, sb_log};
use crate::seek::{
    AreaIndex, Assembling, Claim, HintRecorder, Plan, Provisional, ProvisionalRecorder, Recorder,
    ResumedFrom, SeekIndexes, SeekPolicy, Speculate, Speculation, digest, first_record, locate,
};
use crate::toc::{Area, Track};

/// DSD bytes per channel in one frame (specification 0, 0.5).
pub const FRAME_BYTES_PER_CHANNEL: usize = 4704;

/// Largest frame under assembly in bytes (specification 4, 4.4.3 item 4).
pub const MAX_FRAME_BYTES: usize = 65_536;

/// Fill of a frame the disc does not supply (specification 0, 0.6).
pub const MISSING_FILL: u8 = 0x69;

/// Fill of a DST frame that cannot be decoded (specification 0, 0.6).
pub const INVALID_FILL: u8 = 0x55;

/// Largest channel count for which audio is produced (specification 4, 4.8).
pub const MAX_CHANNELS: u8 = 6;

/// Most `audio-sector-malformed` warnings kept per area cursor (specification 4, 4.4.5
/// item 3); the count of malformed sectors is kept in full.
pub const MAX_MALFORMED_WARNINGS: usize = 4096;

/// Most log lines of each kind of problem a cursor writes: malformed sectors and discarded
/// frames per area cursor, missing and invalid frames per track cursor. The first line
/// past the cap says that further ones are suppressed; the counts are kept in full.
pub const MAX_PROBLEM_LINES: usize = MAX_MALFORMED_WARNINGS;

/// The number of log lines written for one kind of problem, against
/// [`MAX_PROBLEM_LINES`].
#[derive(Clone, Copy, Debug, Default)]
struct LineCap(usize);

impl LineCap {
    /// Whether one more `level` line about `what` may be written to `log`. The first line
    /// refused writes a line saying that further ones are suppressed. Counts only when
    /// the level is enabled, so it costs one test without a log.
    #[inline]
    fn admit(&mut self, log: &Log, level: Level, slot: &str, what: &str) -> bool {
        if !log.enabled(level) {
            return false;
        }
        self.0 = self.0.saturating_add(1);
        if self.0 == MAX_PROBLEM_LINES + 1 {
            log.write(
                level,
                format_args!(
                    "area {slot}: further {what} lines suppressed (at most {MAX_PROBLEM_LINES} per cursor)"
                ),
            );
        }
        self.0 <= MAX_PROBLEM_LINES
    }
}

/// Why a sector is malformed (specification 4, 4.4.3), for the log.
#[derive(Clone, Copy, Debug)]
enum Malformed {
    /// Packet `p` of `k`, `len` bytes at `pos`, runs past the sector.
    Overrun { p: usize, k: usize, len: usize, pos: usize },
    /// A frame starts in packet `p` but the sector has only `f` frame records.
    NoRecord { p: usize, f: usize },
    /// Frame `number` grew past [`MAX_FRAME_BYTES`] and was discarded.
    Oversized { number: u32 },
}

impl std::fmt::Display for Malformed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Malformed::Overrun { p, k, len, pos } => {
                write!(f, "packet {p} of {k} ({len} bytes at {pos}) overruns the sector")
            }
            Malformed::NoRecord { p, f: records } => {
                write!(f, "frame start in packet {p} without a frame record ({records} records)")
            }
            Malformed::Oversized { number } => {
                write!(f, "frame {number} exceeds {MAX_FRAME_BYTES} bytes and is discarded")
            }
        }
    }
}

/// Most timeline positions a cursor of the `parallel` feature holds ahead of its caller.
///
/// With `parallel`, a track's positions are demultiplexed on the calling thread up to
/// [`PARALLEL_FRAMES_PER_THREAD`] per thread of the cursor's pool (the current rayon pool
/// unless [`TrackAudioCursor::set_decode_on`] chose another) ahead of the frame
/// returned last, at most this many (on a pool of one thread, one group, as below). Each DST frame
/// becomes one job on the pool as soon as it is demultiplexed, so that the pool decodes
/// while the caller consumes the frames before it; every call tops the window up again.
/// Plain DSD and missing positions are filled in on the calling thread when they are
/// returned. Frames are returned in timeline order whatever order the jobs finish in.
///
/// A job decodes up to [`crate::dst::lanes`] consecutive DST frames together
/// ([`DstDecoder::decode_many`]), so the window is topped up in whole groups: a call
/// demultiplexes ahead only once at least a group fits below the bound, and never past it.
/// On the calling thread (without `parallel`, or on a pool of one thread) the window is
/// one such group, decoded on the calling thread when its first frame is asked for.
///
/// Each position held ahead is at most one DST frame's bytes (at most 65 536,
/// [`MAX_FRAME_BYTES`]) and, once decoded, its audio (at most 6 x 4704 bytes): a cursor
/// holds at most `512 x (65 536 + 6 x 4704)` bytes, about 48 MB, whatever the host (plus
/// the frame under assembly, and its read buffer: 64 KiB, or up to 1 MiB,
/// [`crate::seek::MAX_READ_SECTORS`] sectors, kept for the cursor's life once it has made a
/// read sized to its range, after a provisional start or a resume from a seek index); DST frames of 40 % of their decoded size need about 6.8 MB
/// at 2 channels and 20 MB at 6. The cap is a constant so that the memory held does not
/// grow with the host's thread count. It keeps 16 positions per thread up to 32 threads:
/// with groups of 2 or 3 frames per job, a cap of 256 left under 3 jobs per thread at 32
/// threads, and the pool ran short of work while the calling thread was busy (#61).
///
/// A cursor continued ([`TrackAudioCursor::continue_to`] and its range form) or dropped
/// while DST frames are in flight drops its handles on them: queued jobs then skip
/// decoding, and only jobs already decoding run to their end, at most one group of
/// [`crate::dst::lanes`] frames per pool thread. That memory is held briefly beside the
/// new cursor's window, and freed as those jobs finish.
///
/// Besides the window, each decoder holds its tables: per lane of
/// [`DstDecoder::decode_many`], at most 12 filters' byte tables (64 KiB each) and 12
/// ptables' probability tables (8 KiB each), about 0.85 MB. A cursor's own decoder, and
/// each pool thread's, holds up to [`crate::dst::lanes`] lanes of them.
pub const MAX_PARALLEL_BATCH: usize = 512;

/// Positions held ahead per pool thread with the `parallel` feature (see
/// [`MAX_PARALLEL_BATCH`]).
pub const PARALLEL_FRAMES_PER_THREAD: usize = 16;

/// Where a [`TrackAudioCursor`] decodes its DST frames, with the `parallel` feature
/// ([`TrackAudioCursor::set_decode_on`]). Whichever is chosen, the frames returned, their
/// order, counts and log lines are the same; only the threads that decode differ.
#[cfg(feature = "parallel")]
#[derive(Clone, Debug, Default)]
pub enum DecodeOn {
    /// The rayon pool current at each call: the pool of the calling thread if it is one of
    /// a pool's threads (inside [`rayon::ThreadPool::install`]), rayon's global pool
    /// otherwise. The default.
    #[default]
    CurrentPool,
    /// This pool, whichever thread calls: the calling thread demultiplexes and the pool's
    /// threads decode. A pool of one thread decodes on the calling thread, as
    /// [`DecodeOn::CallingThread`]. The cursor holds the pool, so it stays alive while
    /// the cursor does.
    Pool(std::sync::Arc<rayon::ThreadPool>),
    /// The calling thread only, as without the `parallel` feature: no pool is used, not
    /// even rayon's global one.
    CallingThread,
}

/// Most frames that can start in one sector (specification 4, 4.8 item 3).
pub const FRAMES_PER_SECTOR: u64 = 7;

/// How a frame is coded, fixed by the DST flag of the sector it starts in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FrameKind {
    /// DST-coded (specification 5).
    Dst,
    /// Plain DSD, exactly `C x 4704` bytes in interleaved layout.
    Dsd,
}

/// A frame accepted onto an area's timeline (specification 4, 4.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AreaFrame {
    /// Its frame number (timeline position).
    pub number: u32,
    /// Its kind.
    pub kind: FrameKind,
    /// Its bytes: `C x 4704` for plain DSD, 1 to 65 536 for DST.
    pub data: Vec<u8>,
}

#[derive(Debug)]
struct Assembly {
    number: u32,
    kind: FrameKind,
    data: Vec<u8>,
    remaining: i32,
    /// The LSN of the sector it starts in, for the log.
    start_lsn: u64,
}

/// Where the reassembler stands, for the log: the area and the LSN of the sector being
/// processed (0 when unknown).
#[derive(Clone, Copy)]
struct At<'a> {
    log: &'a Log,
    slot: Option<Slot>,
    lsn: u64,
}

impl At<'_> {
    fn slot(&self) -> &'static str {
        self.slot.map_or("?", Slot::name)
    }
}

/// The frame reassembler and timeline of one area (specification 4, 4.4 and 4.5).
///
/// Push the area's sectors in increasing LSN order with [`Demuxer::push_sector`], then
/// call [`Demuxer::finish`]. Accepted frames are queued and taken with
/// [`Demuxer::pop`]. At most 8 frames are queued after one push (7 started in the sector,
/// plus none extra: a frame is delivered only when the next one starts, or at the end).
#[derive(Debug)]
pub struct Demuxer {
    channels: u8,
    assembling: Option<Assembly>,
    next_acceptable: u32,
    ready: VecDeque<AreaFrame>,
    malformed: u64,
    /// The area, for the log.
    slot: Option<Slot>,
    /// Why the last sector pushed was malformed (its first reason), for the log.
    reason: Option<Malformed>,
    /// Lines written about discarded frames.
    discarded_lines: LineCap,
    /// One more than the largest number of a complete frame delivered, accepted or not; 0
    /// before any (for provisional checkpoints).
    seen: u32,
}

impl Demuxer {
    /// A reassembler for an area of `channels` channels (the area TOC's channel count).
    pub fn new(channels: u8) -> Self {
        Demuxer {
            channels,
            assembling: None,
            next_acceptable: 0,
            ready: VecDeque::new(),
            malformed: 0,
            slot: None,
            reason: None,
            discarded_lines: LineCap::default(),
            seen: 0,
        }
    }

    fn at<'a>(&self, log: &'a Log, lsn: u64) -> At<'a> {
        At { log, slot: self.slot, lsn }
    }

    /// Number of malformed sectors seen (specification 4, 4.4.5).
    pub fn malformed_sectors(&self) -> u64 {
        self.malformed
    }

    /// Takes the next accepted frame, if any.
    pub fn pop(&mut self) -> Option<AreaFrame> {
        self.ready.pop_front()
    }

    /// Processes one 2048-byte audio sector (specification 4, 4.4.3). Returns whether
    /// the sector was malformed.
    pub fn push_sector(&mut self, sector: &[u8; SECTOR_SIZE]) -> bool {
        self.push_sector_at(sector, self.at(&Log::none(), 0))
    }

    /// [`Demuxer::push_sector`], reporting to `at.log` with the sector's LSN.
    fn push_sector_at(&mut self, sector: &[u8; SECTOR_SIZE], at: At<'_>) -> bool {
        let header = sector[0];
        let dst = header & 1 != 0;
        let f = usize::from((header >> 2) & 7);
        let k = usize::from(header >> 5);
        let record = if dst { 4 } else { 3 };
        // H <= 1 + 14 + 28 = 43: every record lies inside the sector.
        let h = 1 + 2 * k + f * record;
        let byte = |i: usize| sector.get(i).copied().unwrap_or(0);
        self.reason = None;
        let mut malformed = false;
        let mut x = 0usize;
        let mut pos = h;
        for p in 0..k {
            let w = u16::from_be_bytes([byte(1 + 2 * p), byte(2 + 2 * p)]);
            let start = w & 0x8000 != 0;
            let packet_type = (w >> 11) & 7;
            let len = usize::from(w & 0x07FF);
            let Some(payload) = sector.get(pos..pos + len) else {
                self.reason.get_or_insert(Malformed::Overrun { p, k, len, pos });
                malformed = true;
                break;
            };
            if packet_type == 2 {
                if start {
                    self.deliver_if_complete(at);
                    if x < f {
                        let r = 1 + 2 * k + x * record;
                        let number =
                            (u32::from(byte(r)) * 60 + u32::from(byte(r + 1))) * 75 + u32::from(byte(r + 2));
                        let remaining = if dst { i32::from((byte(r + 3) >> 2) & 0x1F) } else { 0 };
                        let kind = if dst { FrameKind::Dst } else { FrameKind::Dsd };
                        self.assembling =
                            Some(Assembly { number, kind, data: Vec::new(), remaining, start_lsn: at.lsn });
                        x += 1;
                    } else {
                        self.reason.get_or_insert(Malformed::NoRecord { p, f });
                        malformed = true;
                    }
                }
                if let Some(a) = &mut self.assembling {
                    if a.data.len() + len > MAX_FRAME_BYTES {
                        if self.discarded_lines.admit(at.log, Level::Warn, at.slot(), "discarded-frame") {
                            sb_log!(
                                at.log,
                                Warn,
                                "area {} frame {} (from LSN {}): discarded at LSN {}, more than {MAX_FRAME_BYTES} \
                                 bytes (4.4.3)",
                                at.slot(),
                                a.number,
                                a.start_lsn,
                                at.lsn
                            );
                        }
                        self.reason.get_or_insert(Malformed::Oversized { number: a.number });
                        self.assembling = None;
                        malformed = true;
                    } else {
                        a.data.extend_from_slice(payload);
                        // Bounded below by -(7 x sectors): no overflow in an i32 for any
                        // image of fewer than 300 million sectors per frame; saturate anyway.
                        a.remaining = a.remaining.saturating_sub(1);
                    }
                }
            }
            pos += len;
        }
        if malformed {
            // The demuxer does not know LSNs; AreaCursor turns this into warnings.
            self.malformed += 1;
        }
        malformed
    }

    /// Ends the area (specification 4, 4.4.4).
    pub fn finish(&mut self) {
        self.finish_at(self.at(&Log::none(), 0));
    }

    fn finish_at(&mut self, at: At<'_>) {
        self.deliver_if_complete(at);
    }

    fn deliver_if_complete(&mut self, at: At<'_>) {
        let Some(a) = self.assembling.take() else { return };
        let complete = !a.data.is_empty()
            && match a.kind {
                FrameKind::Dst => a.remaining == 0,
                FrameKind::Dsd => a.data.len() == usize::from(self.channels) * FRAME_BYTES_PER_CHANNEL,
            };
        if complete {
            self.seen = self.seen.max(a.number.saturating_add(1));
        }
        if complete && a.number >= self.next_acceptable {
            let (start_lsn, len) = (a.start_lsn, a.data.len());
            // Frame numbers are at most 1 166 880.
            self.next_acceptable = a.number + 1;
            self.ready.push_back(AreaFrame { number: a.number, kind: a.kind, data: a.data });
            // Logged once the frame is queued, so that a sink that panics loses nothing.
            sb_log!(
                at.log,
                Trace,
                "area {} frame {} (from LSN {start_lsn}): accepted, {:?}, {len} bytes",
                at.slot(),
                a.number,
                a.kind
            );
            return;
        }
        if !self.discarded_lines.admit(at.log, Level::Warn, at.slot(), "discarded-frame") {
            return;
        }
        if !complete {
            sb_log!(
                at.log,
                Warn,
                "area {} frame {} (from LSN {}): discarded, incomplete ({:?}, {} bytes{}; 4.4.2)",
                at.slot(),
                a.number,
                a.start_lsn,
                a.kind,
                a.data.len(),
                match a.kind {
                    FrameKind::Dst if a.remaining != 0 => ", sector count does not match",
                    _ => "",
                }
            );
        } else {
            sb_log!(
                at.log,
                Warn,
                "area {} frame {} (from LSN {}): discarded, not after the last accepted frame {} (4.5)",
                at.slot(),
                a.number,
                a.start_lsn,
                self.next_acceptable.saturating_sub(1)
            );
        }
    }
}

/// The reading state of an area's accepted frames, independent of the image it reads
/// from: pass the same image to every call of [`AreaCursor::next_frame`].
///
/// [`AreaFrames`] is the borrowing iterator over the same state.
#[derive(Debug)]
pub struct AreaCursor {
    demux: Demuxer,
    /// The image this cursor reads, bound on first use: its read cache and frames belong
    /// to that image.
    image_id: Option<u64>,
    slot: Slot,
    warnings: Vec<Warning>,
    next_lsn: u64,
    end_lsn: u64,
    buf: Vec<u8>,
    buf_lsn: u64,
    buf_sectors: u64,
    finished: bool,
    /// Lines written about malformed sectors.
    malformed_lines: LineCap,
    /// Records checkpoints for a seek index, when made from one.
    recorder: Option<Box<Recorder>>,
    /// Sectors processed before `replay.until` only rebuild the frame under assembly; the
    /// state is then set to the checkpoint's.
    replay: Option<Replay>,
    /// Where the scan started.
    resumed: ResumedFrom,
    /// [`AreaCursor::next_frame`] reads no sector at or past this LSN (returns `None`), for
    /// [`AreaIndex::extend`].
    stop_lsn: u64,
    /// The checkpoint's LSN and the digest its sector must have, until it is read.
    verify: Option<(u64, u64)>,
    /// The area, to scan it afresh when a seek index does not match the image.
    key: AreaKey,
    /// The malformed count and warnings reported before the first read: those of the
    /// scan a continuation moved this cursor ahead of (see [`AreaCursor::carry_counts`]).
    before_read: Option<Box<(u64, Vec<Warning>)>>,
    /// A provisional start to choose on the first read (specification 4, 4.2 item 8).
    speculate: Option<Speculate>,
    /// The provisional reading this cursor makes, if any (shared with the index that
    /// checks it), and whether that index has it.
    claim: Option<std::sync::Arc<Claim>>,
    claim_registered: bool,
    /// Records provisional checkpoints, for a provisional reading.
    provisional: Option<Box<ProvisionalRecorder>>,
    /// Reads below this LSN reach up to it, in calls of at most
    /// [`crate::seek::MAX_READ_SECTORS`] (reads sized to the range after a provisional start
    /// or a resume from a seek index); others read
    /// [`READ_CHUNK`] sectors at a time.
    read_to: u64,
    /// Records seek hints, when made from an index that keeps them.
    hints: Option<Box<HintRecorder>>,
    /// A seek hint this cursor began at did not match the image: the index drops its hints.
    hints_stale: bool,
    /// Sectors read through the image by this cursor (its reading, see [`Progress`]).
    sectors_read: u64,
    /// Where the reading is expected to end: the sector after the range's estimated last.
    progress_end: u64,
    /// The counts published for other threads, once asked for.
    progress: Option<Progress>,
}

/// The state a resumed scan takes once its replay reaches the checkpoint.
#[derive(Clone, Copy, Debug)]
struct Replay {
    until: u64,
    next_acceptable: u32,
    malformed: u64,
}

/// Sectors read per call to the reader.
const READ_CHUNK: u64 = 32;

impl AreaCursor {
    /// A cursor at the start of `area`'s audio, in an image of `image_sectors` sectors.
    pub fn new(area: &Area, image_sectors: u64) -> Self {
        Self::with_key(AreaKey::new(area, image_sectors))
    }

    /// A cursor at the start of the area `key` names.
    fn with_key(key: AreaKey) -> Self {
        let (start, end) = key.audio_sector_range();
        let mut demux = Demuxer::new(key.channels);
        demux.slot = Some(key.slot);
        AreaCursor {
            demux,
            image_id: None,
            slot: key.slot,
            warnings: Vec::new(),
            next_lsn: start,
            end_lsn: end,
            buf: Vec::new(),
            buf_lsn: 0,
            buf_sectors: 0,
            finished: false,
            malformed_lines: LineCap::default(),
            recorder: None,
            replay: None,
            resumed: ResumedFrom::Start,
            stop_lsn: u64::MAX,
            verify: None,
            key,
            before_read: None,
            speculate: None,
            claim: None,
            claim_registered: false,
            provisional: None,
            read_to: 0,
            hints: None,
            hints_stale: false,
            sectors_read: 0,
            progress_end: end,
            progress: None,
        }
    }

    /// The progress of this cursor's reading, readable from other threads (see
    /// [`Progress`]); the same object every time.
    pub fn progress(&mut self) -> Progress {
        let read = self.sectors_read;
        let expected = if self.finished { read } else { self.expected(self.next_lsn) };
        self.progress
            .get_or_insert_with(|| {
                let p = Progress::default();
                p.set(read, expected);
                p
            })
            .clone()
    }

    /// The sectors expected in all when the next sector to read is `next`.
    fn expected(&self, next: u64) -> u64 {
        self.sectors_read + self.progress_end.saturating_sub(next)
    }

    /// Counts `n` sectors read, the next to read being `next`.
    #[inline]
    fn count_read(&mut self, n: u64, next: u64) {
        self.sectors_read += n;
        if let Some(p) = &self.progress {
            p.set(self.sectors_read, self.expected(next));
        }
    }

    /// The reading is complete: nothing more is expected.
    pub(crate) fn progress_done(&self) {
        if let Some(p) = &self.progress {
            p.set(self.sectors_read, self.sectors_read);
        }
    }

    /// Sets where the reading is expected to end (the sector after the range's estimated
    /// last), also for the exact start a provisional one may fall back to.
    pub(crate) fn set_progress_end(&mut self, lsn: u64) {
        self.progress_end = lsn.min(self.end_lsn);
        if let Some(sp) = self.speculate.as_mut() {
            sp.exact.set_progress_end(lsn);
        }
    }

    /// Starts a new reading: counts from 0, and a [`Progress`] of its own once asked for.
    pub(crate) fn restart_progress(&mut self) {
        self.sectors_read = 0;
        self.progress = None;
    }

    /// Takes the counts of `from`, which this cursor replaces within one reading.
    fn keep_progress(&mut self, from: (u64, u64, Option<Progress>)) {
        (self.sectors_read, self.progress_end, self.progress) = from;
    }

    fn take_progress(&mut self) -> (u64, u64, Option<Progress>) {
        (self.sectors_read, self.progress_end, self.progress.take())
    }

    /// A cursor that chooses on its first read whether to begin provisionally (specification
    /// 4, 4.2 item 8) for positions `first` or more, or as `exact` does.
    pub(crate) fn speculative(
        exact: AreaCursor,
        (first, end): (u32, u32),
        (playtime, knots): (u32, Vec<(u64, u32)>),
        (interval, hint): (u32, Option<(u64, u32, u64)>),
        options: crate::seek::SpeculativeOptions,
    ) -> Self {
        let from = match exact.resumed {
            ResumedFrom::Checkpoint { lsn, .. } => lsn,
            _ => exact.next_lsn,
        };
        let mut cursor = Self::with_key(exact.key);
        cursor.next_lsn = from;
        cursor.resumed = ResumedFrom::Speculative { lsn: 0, probes: 0 };
        cursor.claim = Some(Claim::new(cursor.slot, first));
        cursor.speculate = Some(Speculate {
            first,
            end,
            playtime,
            interval,
            options,
            knots,
            exact: Box::new(exact),
            from,
            hint,
        });
        cursor
    }

    /// Chooses the provisional start (see [`AreaCursor::speculative`]), reading through
    /// `image`; or takes the exact start, if the search finds nothing past it.
    /// A cancelled search keeps `sp` for the next read.
    fn begin<R: Read + Seek>(&mut self, image: &mut Image<R>, mut sp: Speculate) -> Result<(), Error> {
        let (start, _) = self.key.audio_sector_range();
        if let Some((l, t, to)) = sp.hint {
            // A seek hint: one read from its sector to the range's end. Its first record
            // must be the hint's, or the hints are of another image (or a stale one).
            self.read_to = to;
            let sector = match self.sector(image, l) {
                Err(Error::Cancelled) => {
                    self.speculate = Some(sp);
                    return Err(Error::Cancelled);
                }
                r => r?,
            };
            if first_record(&sector) == Some(t) && t < sp.first {
                // TODO(sacd-spec#31): a hint that was the `L` of a confirmed reading could
                // report its positions confirmed at once (sacd-spec#32, 4.2 item 9); until
                // then they are provisional like any other.
                self.start_provisional(start, (l, 0), &sp);
                return Ok(());
            }
            sb_log!(
                image.log(),
                Warn,
                "area {}: seek hint at LSN {l} (frame {t}) does not match the image (first record {:?}): \
                 hints dropped, searching",
                self.slot,
                first_record(&sector)
            );
            self.hints_stale = true;
            self.read_to = 0;
            sp.hint = None;
        }
        let range = (sp.from, self.end_lsn);
        let located = locate(image, range, (sp.first, sp.playtime), &sp.knots, &sp.options, &mut |n| {
            self.count_read(n, sp.from)
        });
        let located = match located {
            Err(Error::Cancelled) => {
                self.speculate = Some(sp);
                return Err(Error::Cancelled);
            }
            r => r?,
        };
        let (l, probes) = (located.lsn, located.reads);
        if l <= sp.from {
            let (image_id, counts, stale) = (self.image_id, self.take_progress(), self.hints_stale);
            *self = *sp.exact;
            self.image_id = image_id;
            self.hints_stale = stale;
            self.keep_progress(counts);
            sb_log!(
                image.log(),
                Debug,
                "area {}: no provisional start past LSN {} for frame {} ({probes} reads): exact",
                self.slot,
                sp.from,
                sp.first
            );
            return Ok(());
        }
        // Begins at the sector found (4.2 item 8): on an image whose frames start in
        // non-decreasing number order, every frame that starts before it is numbered at most
        // its first record's, which is below `first`, so (a) and (b) hold there.
        self.start_provisional(start, (l, probes), &sp);
        // Reads on from the probe that found `l`, without reading it again.
        if let Some((at, bytes)) = located.chunk {
            self.buf_sectors = (bytes.len() / SECTOR_SIZE) as u64;
            self.buf_lsn = at;
            self.buf = bytes;
        }
        // The next read reaches the range's estimated end: its frames from `l` at the
        // density found, plus a quarter and a few sectors.
        let (sectors, frames) = located.density;
        let span =
            u128::from(sp.end.saturating_sub(located.frame)) * u128::from(sectors) / u128::from(frames);
        let span = u64::try_from(span + span / 4 + 4).unwrap_or(u64::MAX);
        self.read_to = l.saturating_add(span);
        Ok(())
    }

    /// Begins a provisional reading at sector `l`, chosen after `probes` reads (4.2 item 8):
    /// no frame under assembly, `sp.first` as the next acceptable frame number.
    fn start_provisional(&mut self, start: u64, (l, probes): (u64, u32), sp: &Speculate) {
        self.next_lsn = l;
        self.demux.next_acceptable = sp.first;
        self.resumed = ResumedFrom::Speculative { lsn: l, probes };
        if let Some(claim) = &self.claim {
            claim.set_lsn(l);
        }
        self.provisional = Some(ProvisionalRecorder::new(start, l, sp.interval));
    }

    /// The seek hints recorded since the last call.
    pub(crate) fn take_hints(&mut self) -> Vec<(u32, u32)> {
        self.hints.as_mut().map(|h| h.take()).unwrap_or_default()
    }

    /// The reading has ended: its last seek hint is recorded, and the hints are news.
    pub(crate) fn flush_hints(&mut self) {
        if let Some(h) = self.hints.as_deref_mut() {
            h.flush();
        }
    }

    /// Whether a seek hint this cursor began at did not match the image (once).
    pub(crate) fn hints_stale(&mut self) -> bool {
        std::mem::take(&mut self.hints_stale)
    }

    /// Records seek hints from now on, with `recorder`.
    pub(crate) fn record_hints(&mut self, recorder: Option<Box<HintRecorder>>) {
        if let Some(sp) = self.speculate.as_mut() {
            sp.exact.hints = recorder.as_ref().map(|r| Box::new(HintRecorder::clone(r)));
        }
        self.hints = recorder;
    }

    /// Reads up to `lsn` (exclusive) in one call when the next read starts below it.
    pub(crate) fn size_first_read(&mut self, lsn: u64) {
        self.read_to = lsn;
    }

    /// A provisional reading for positions `first` or more that starts at the provisional
    /// checkpoint `p` at sector `lsn` (an earlier provisional reading's), instead of
    /// searching; `exact` gives the area.
    pub(crate) fn provisional_at(
        exact: &AreaCursor,
        (lsn, p): (u64, Provisional),
        first: u32,
        interval: u32,
    ) -> Self {
        let mut cursor = Self::with_key(exact.key);
        let (start, _) = cursor.key.audio_sector_range();
        let from = lsn - u64::from(p.back);
        cursor.next_lsn = from;
        cursor.demux.next_acceptable = first;
        if from < lsn {
            // Lemma B of #68, as for an exact resume: rebuilds the frame under assembly.
            cursor.replay = Some(Replay { until: lsn, next_acceptable: first, malformed: 0 });
        }
        cursor.verify = Some((lsn, p.digest));
        cursor.resumed = ResumedFrom::Speculative { lsn, probes: 0 };
        let claim = Claim::new(cursor.slot, first);
        claim.set_lsn(lsn);
        cursor.claim = Some(claim);
        cursor.provisional = Some(ProvisionalRecorder::new(start, lsn, interval));
        cursor
    }

    /// The state at `lsn` (at or before the frontier of `index`) of a reader processing
    /// from the area's first sector: its next acceptable frame number, the number of its
    /// frame under assembly, and its malformed count; rebuilt from the checkpoint at or
    /// before `lsn`, reading through `image` (for a checkpoint boundary `lsn`, at most
    /// [`crate::seek::REPLAY_SECTORS`] sectors to replay and the checkpoint's own sector;
    /// otherwise up to the interval more). `None` if the index does not match the image
    /// there: the checkpoint's sector does not have its digest. The digest is always
    /// compared, also when no sector before `lsn` needs reading.
    pub(crate) fn state_at<R: Read + Seek>(
        index: &AreaIndex,
        lsn: u64,
        image: &mut Image<R>,
    ) -> Result<Option<(u32, Option<u32>, u64)>, Error> {
        let mut cursor = Self::with_key(index.key());
        if let Some(j) = index.checkpoint_before(lsn) {
            cursor.resume_at(index, index.plan_at(j));
        }
        cursor.recorder = None;
        cursor.scan_to(image, lsn)?;
        if let ResumedFrom::Discarded { .. } = cursor.resumed {
            return Ok(None);
        }
        // The scan stops before sector `lsn`: when that is the checkpoint's own sector,
        // its digest is still to compare (a stale index, or one of another image of the
        // same size, must not confirm anything).
        if let Some((at, want)) = cursor.verify {
            if at < cursor.end_lsn && digest(&cursor.sector(image, at)?) != want {
                return Ok(None);
            }
        }
        let d = &cursor.demux;
        Ok(Some((d.next_acceptable, d.assembling.as_ref().map(|a| a.number), d.malformed)))
    }

    /// Whether this cursor's positions are provisional (see [`SeekPolicy`]).
    pub fn speculation(&self) -> Speculation {
        self.claim.as_ref().map_or(Speculation::Exact, |c| c.speculation())
    }

    /// The number of this cursor's provisional reading, as events name it
    /// ([`crate::seek::SeekEvent::claim`]); `None` for an exact cursor.
    pub fn speculation_id(&self) -> Option<u64> {
        self.claim.as_ref().map(|c| c.id)
    }

    /// The sector the malformed count and warnings begin at: `None` for the area's first
    /// sector, `Some(L)` for a provisional reading (specification 4, 4.4.5 item 5).
    pub fn counts_from(&self) -> Option<u64> {
        self.claim.as_ref().map(|c| c.lsn()).filter(|&l| l > 0)
    }

    /// The provisional reading, the first time it is asked for once `L` is chosen.
    pub(crate) fn unregistered_claim(&mut self) -> Option<std::sync::Arc<Claim>> {
        let claim = self.claim.as_ref().filter(|c| !self.claim_registered && c.lsn() > 0)?;
        let claim = std::sync::Arc::clone(claim);
        self.claim_registered = true;
        Some(claim)
    }

    /// The provisional checkpoints recorded since the last call.
    pub(crate) fn take_provisional_checkpoints(&mut self) -> Vec<(u64, Provisional)> {
        self.provisional.as_mut().map(|p| std::mem::take(&mut p.list)).unwrap_or_default()
    }

    /// Whether this cursor has anything for an index: checkpoints, a provisional reading
    /// or provisional checkpoints.
    #[inline]
    pub(crate) fn has_news(&self) -> bool {
        self.recorder.as_deref().is_some_and(Recorder::has_news)
            || (!self.claim_registered && self.claim.as_ref().is_some_and(|c| c.lsn() > 0))
            || self.provisional.as_ref().is_some_and(|p| !p.list.is_empty())
            || self.hints_stale
            || self.hints.as_deref().is_some_and(HintRecorder::has_news)
    }

    /// A cursor of the area of `index` that starts from it for positions `first` or more
    /// (specification 4, 4.2 item 7): at the last checkpoint whose next acceptable frame
    /// number is at most `first`, with its malformed count and warnings those of the scan
    /// from the first sector, recording checkpoints past the index's frontier.
    pub(crate) fn resume(index: &AreaIndex, first: u32) -> Self {
        let mut cursor = Self::with_key(index.key());
        let slot = cursor.slot;
        let warn = |lsn: u32| Warning {
            code: WarningCode::AudioSectorMalformed,
            slot: Some(slot),
            track: None,
            lsn: Some(lsn),
        };
        match index.plan(first) {
            Plan::Start => cursor.recorder = index.recorder(cursor.next_lsn),
            Plan::End(end) => {
                // No frame numbered `first` or more is accepted: nothing to read. The counts
                // are those of a scan to the end, which a scan from the first sector makes.
                cursor.next_lsn = cursor.end_lsn;
                cursor.finished = true;
                cursor.demux.next_acceptable = end.next_acceptable;
                cursor.demux.malformed = end.malformed;
                cursor.warnings = index.malformed_below(cursor.end_lsn).map(warn).collect();
                cursor.resumed = ResumedFrom::End;
            }
            plan @ Plan::At { .. } => cursor.resume_at(index, plan),
        }
        cursor
    }

    /// Resumes at the checkpoint `plan` names (a [`Plan::At`] of `index`).
    fn resume_at(&mut self, index: &AreaIndex, plan: Plan<'_>) {
        let slot = self.slot;
        let warn = |lsn: u32| Warning {
            code: WarningCode::AudioSectorMalformed,
            slot: Some(slot),
            track: None,
            lsn: Some(lsn),
        };
        let Plan::At { lsn, checkpoint, held } = plan else { return };
        // Specification 4, 4.4.5 item 4: a late reader MAY report the malformed
        // sectors an earlier pass found; this one reports them all, as a scan from
        // the first sector would.
        self.warnings = index.malformed_below(lsn).map(warn).collect();
        let replay_from = match held {
            Some((h, bytes)) => {
                self.demux.assembling = Some(Assembly {
                    number: h.number,
                    kind: h.kind,
                    data: bytes.to_vec(),
                    remaining: h.remaining,
                    start_lsn: lsn - u64::from(checkpoint.back),
                });
                lsn
            }
            None => lsn - u64::from(checkpoint.back),
        };
        self.demux.next_acceptable = checkpoint.next_acceptable;
        self.demux.malformed = checkpoint.malformed_before;
        if replay_from < lsn {
            // Lemma B1 of #68: from the first start packet the replay processes, its
            // frame under assembly is the scan's; the frames it delivers are all
            // numbered below the checkpoint's `a`, and are dropped.
            self.replay = Some(Replay {
                until: lsn,
                next_acceptable: checkpoint.next_acceptable,
                malformed: checkpoint.malformed_before,
            });
        }
        self.next_lsn = replay_from;
        self.verify = Some((lsn, checkpoint.digest));
        self.recorder = index.recorder(lsn);
        self.resumed = ResumedFrom::Checkpoint { lsn, replay_from };
    }

    /// Starts afresh at the area's first audio sector: the seek index this cursor started
    /// from does not hold sector `lsn` as it is in the image (a stale index, or one of
    /// another image of the same size). Nothing was returned from it yet: the sectors
    /// processed so far only rebuilt the frame under assembly.
    fn discard_index(&mut self, log: &Log, lsn: u64) {
        let (image_id, stop, counts) = (self.image_id, self.stop_lsn, self.take_progress());
        *self = Self::with_key(self.key);
        self.image_id = image_id;
        self.keep_progress(counts);
        self.resumed = ResumedFrom::Discarded { lsn };
        if stop != u64::MAX {
            // Extending the index (`scan_to`): stop here; reading from the first sector
            // would only record nothing.
            self.stop_lsn = self.next_lsn;
        }
        sb_log!(
            log,
            Warn,
            "area {}: seek index does not match the image at LSN {lsn}, discarded; scanning from LSN {}",
            self.slot,
            self.next_lsn
        );
    }

    /// Where this cursor's scan started (see [`ResumedFrom`]).
    pub fn resumed_from(&self) -> ResumedFrom {
        self.resumed
    }

    /// Scans up to (not including) sector `stop`, or to the end, dropping the frames:
    /// whether the end was reached.
    pub(crate) fn scan_to<R: Read + Seek>(&mut self, image: &mut Image<R>, stop: u64) -> Result<bool, Error> {
        self.stop_lsn = stop;
        let result = loop {
            match self.next_frame(image) {
                Some(Ok(_)) => {}
                Some(Err(e)) => break Err(e),
                None => break Ok(self.finished),
            }
        };
        self.stop_lsn = u64::MAX;
        result
    }

    /// The checkpoints recorded for a seek index.
    pub(crate) fn recorder(&self) -> Option<&Recorder> {
        self.recorder.as_deref()
    }

    pub(crate) fn recorder_mut(&mut self) -> Option<&mut Recorder> {
        self.recorder.as_deref_mut()
    }

    /// Marks the checkpoints this cursor records as those of the disc's own index.
    pub(crate) fn keep_for_disc(&mut self) {
        if let Some(r) = self.recorder.as_deref_mut() {
            r.kept = true;
        }
        if let Some(sp) = self.speculate.as_mut() {
            sp.exact.keep_for_disc();
        }
    }

    /// Records no more checkpoints.
    pub(crate) fn stop_recording(&mut self) {
        self.recorder = None;
    }

    /// Reports, until this cursor first reads, the malformed count and warnings of `from`,
    /// whose scan a continuation moved ahead through a seek index: a scan from the area's
    /// first sector, as `from` was, reports them at that point (and this cursor, once it
    /// reads, those of the same scan further on).
    pub(crate) fn carry_counts(&mut self, from: &AreaCursor) {
        self.before_read = Some(Box::new((from.malformed_sectors(), from.warnings().to_vec())));
    }

    /// The image this cursor read, once it has read.
    pub(crate) fn image_id(&self) -> Option<u64> {
        self.image_id
    }

    /// The area's slot.
    pub(crate) fn slot(&self) -> Slot {
        self.slot
    }

    /// The next sector the scan reads.
    pub(crate) fn scan_lsn(&self) -> u64 {
        self.next_lsn
    }

    /// Number of malformed sectors processed so far by this cursor's scan of its area. The
    /// scan starts at the area's first audio sector, not at the track, and a continued
    /// cursor keeps counting from where the earlier tracks left off.
    /// Track cursors read ahead of the frames they return (a group of
    /// [`crate::dst::lanes`] frames, or with `parallel` up to [`MAX_PARALLEL_BATCH`]
    /// positions), so this may count sectors of frames not yet returned.
    pub fn malformed_sectors(&self) -> u64 {
        // A cursor started from a seek index holds the counts of the sectors before its
        // checkpoint, which a scan from the first sector reports only once it has read.
        match (&self.image_id, &self.before_read) {
            (Some(_), _) => self.demux.malformed_sectors(),
            (None, Some(b)) => b.0,
            (None, None) => 0,
        }
    }

    /// The `audio-sector-malformed` warnings of the sectors processed so far, with slot
    /// and LSN, in processing order (specification 4, 4.4.5). At most
    /// [`MAX_MALFORMED_WARNINGS`] are kept; [`AreaCursor::malformed_sectors`] counts them
    /// all. Like that count, they may include sectors of frames not yet returned.
    pub fn warnings(&self) -> &[Warning] {
        match (&self.image_id, &self.before_read) {
            (Some(_), _) => &self.warnings,
            (None, Some(b)) => &b.1,
            (None, None) => &[],
        }
    }

    fn sector<R: Read + Seek>(&mut self, image: &mut Image<R>, lsn: u64) -> Result<[u8; SECTOR_SIZE], Error> {
        if !(self.buf_lsn..self.buf_lsn + self.buf_sectors).contains(&lsn) {
            let n = READ_CHUNK
                .max(self.read_to.saturating_sub(lsn))
                .min(crate::seek::MAX_READ_SECTORS)
                .min(self.end_lsn - lsn);
            image.check_cancel()?;
            self.buf.resize(n as usize * SECTOR_SIZE, 0);
            // For the image the cursor was made for, the range lies below its sector
            // count; another image may be shorter, which is reported, not assumed.
            if image.read_into(lsn, &mut self.buf)? == ReadOutcome::OutOfRange {
                sb_log!(
                    image.log(),
                    Error,
                    "area {}: audio sectors from LSN {lsn} outside the image",
                    self.slot
                );
                return Err(Error::InvalidArgument("audio sectors outside the image"));
            }
            self.count_read(n, lsn + n);
            self.buf_lsn = lsn;
            self.buf_sectors = n;
        }
        let at = (lsn - self.buf_lsn) as usize * SECTOR_SIZE;
        let mut out = [0u8; SECTOR_SIZE];
        if let Some(src) = self.buf.get(at..at + SECTOR_SIZE) {
            out.copy_from_slice(src);
        }
        Ok(out)
    }

    /// The next accepted frame, `None` at the end of the area.
    ///
    /// The cursor binds to the first image it reads; with an image of another identifier
    /// ([`Image::id`]) it returns [`Error::InvalidArgument`] (and stays usable with its
    /// own). The images of a disc's [`crate::Disc::share`] and `clone` have the same
    /// identifier: they read the same bytes, so the cursor reads through any of them.
    pub fn next_frame<R: Read + Seek>(&mut self, image: &mut Image<R>) -> Option<Result<AreaFrame, Error>> {
        match self.image_id {
            None => {
                self.image_id = Some(image.id());
                if let Some(claim) = &self.claim {
                    claim.set_image(image.id());
                }
                if let Some(sp) = self.speculate.take() {
                    if let Err(e) = self.begin(image, sp) {
                        if let Error::Cancelled = e {
                            // Searched again, from the start, on the next read.
                            self.image_id = None;
                        } else {
                            self.finished = true;
                        }
                        return Some(Err(e));
                    }
                }
                match self.resumed {
                    ResumedFrom::Start => sb_log!(
                        image.log(),
                        Debug,
                        "area {}: scanning audio sectors LSN {}..{}",
                        self.slot,
                        self.next_lsn,
                        self.end_lsn
                    ),
                    ResumedFrom::Checkpoint { lsn, replay_from } => sb_log!(
                        image.log(),
                        Debug,
                        "area {}: resuming at LSN {lsn} from seek index (next acceptable {}, replay from LSN \
                         {replay_from}), scanning to LSN {}",
                        self.slot,
                        self.replay.map_or(self.demux.next_acceptable, |r| r.next_acceptable),
                        self.end_lsn
                    ),
                    ResumedFrom::End => sb_log!(
                        image.log(),
                        Debug,
                        "area {}: seek index: no frame accepted at or after the range, nothing to read",
                        self.slot
                    ),
                    ResumedFrom::Speculative { lsn, probes } => sb_log!(
                        image.log(),
                        Info,
                        "area {}: provisional reading from LSN {lsn} for frames {}.. ({probes} reads to choose it; \
                         4.2 item 8): positions provisional, counts from LSN {lsn}",
                        self.slot,
                        self.demux.next_acceptable
                    ),
                    // Set only once the cursor reads.
                    ResumedFrom::Discarded { .. } => {}
                }
            }
            Some(id) if id != image.id() => {
                return Some(Err(Error::InvalidArgument("audio cursor used with another image")));
            }
            Some(_) => {}
        }
        loop {
            if let Some(f) = self.demux.pop() {
                return Some(Ok(f));
            }
            if self.finished {
                return None;
            }
            if let Some(r) = self.replay.filter(|r| self.next_lsn >= r.until) {
                // The replay has rebuilt the frame under assembly: the checkpoint's state
                // from here (before reading sector `until`, so that a scan stopping there
                // sees the state at it).
                self.demux.ready.clear();
                self.demux.next_acceptable = r.next_acceptable;
                self.demux.malformed = r.malformed;
                self.replay = None;
            }
            if self.next_lsn >= self.stop_lsn && self.replay.is_none() && self.next_lsn < self.end_lsn {
                return None;
            }
            if self.next_lsn < self.end_lsn {
                let lsn = self.next_lsn;
                self.next_lsn += 1;
                match self.sector(image, lsn) {
                    Ok(s) => {
                        if let Some(r) = self.replay {
                            if lsn < r.until {
                                // Rebuilds the frame under assembly only: no log lines, no
                                // warnings, nothing delivered, and the malformed count stays
                                // the checkpoint's, which counts these sectors already, also
                                // if a read fails before the replay ends (4.4.5 item 2).
                                let malformed = self.demux.malformed;
                                self.demux.push_sector_at(&s, self.demux.at(&Log::none(), lsn));
                                self.demux.malformed = malformed;
                                self.demux.ready.clear();
                                continue;
                            }
                        }
                        if let Some((at, want)) = self.verify.filter(|&(at, _)| at == lsn) {
                            self.verify = None;
                            if digest(&s) != want {
                                self.discard_index(image.log(), at);
                                continue;
                            }
                        }
                        if let Some(h) = self.hints.as_deref_mut() {
                            h.see(lsn, &s);
                        }
                        if let Some(pr) = self.provisional.as_deref_mut().filter(|p| p.due(lsn)) {
                            let under = self.demux.assembling.as_ref().map(|a| (lsn - a.start_lsn, a.number));
                            pr.record(lsn, under, self.demux.seen, &s);
                        }
                        if let Some(rec) = self.recorder.as_deref_mut().filter(|r| r.due(lsn)) {
                            let d = &self.demux;
                            let assembling = d.assembling.as_ref().map(|a| Assembling {
                                start_lsn: a.start_lsn,
                                number: a.number,
                                kind: a.kind,
                                remaining: a.remaining,
                                data: &a.data,
                            });
                            rec.record(lsn, d.next_acceptable, d.malformed, assembling, &s);
                        }
                        let at = self.demux.at(image.log(), lsn);
                        let malformed = self.demux.push_sector_at(&s, at);
                        if let Some(reason) = self.demux.reason.filter(|_| malformed) {
                            let code = WarningCode::AudioSectorMalformed.code();
                            let log = image.log();
                            if self.malformed_lines.admit(log, Level::Warn, self.slot.name(), code) {
                                sb_log!(
                                    log,
                                    Warn,
                                    "area {} audio sector LSN {lsn}: {code}: {reason}",
                                    self.slot
                                );
                            }
                        }
                        if malformed && self.warnings.len() < MAX_MALFORMED_WARNINGS {
                            // LSNs of audio sectors come from u32 fields.
                            let lsn = u32::try_from(lsn).ok();
                            self.warnings.push(Warning {
                                code: WarningCode::AudioSectorMalformed,
                                slot: Some(self.slot),
                                track: None,
                                lsn,
                            });
                        }
                    }
                    Err(Error::Cancelled) => {
                        // Nothing was read: the same sector next time.
                        self.next_lsn = lsn;
                        return Some(Err(Error::Cancelled));
                    }
                    Err(e) => {
                        self.finished = true;
                        return Some(Err(e));
                    }
                }
            } else {
                self.demux.finish_at(self.demux.at(image.log(), self.end_lsn));
                self.finished = true;
                self.progress_done();
                if let Some(rec) = self.recorder.as_deref_mut() {
                    rec.finish(self.demux.next_acceptable, self.demux.malformed);
                }
                sb_log!(
                    image.log(),
                    Debug,
                    "area {}: scan ended at LSN {}, {} malformed sectors",
                    self.slot,
                    self.end_lsn,
                    self.demux.malformed_sectors()
                );
            }
        }
    }
}

/// The accepted frames of an area, read from an image in increasing LSN order.
#[derive(Debug)]
pub struct AreaFrames<'a, R> {
    image: &'a mut Image<R>,
    cursor: AreaCursor,
}

impl<'a, R: Read + Seek> AreaFrames<'a, R> {
    pub(crate) fn new(image: &'a mut Image<R>, area: &Area) -> Self {
        let cursor = AreaCursor::new(area, image.sector_count());
        AreaFrames { image, cursor }
    }

    pub(crate) fn with_cursor(image: &'a mut Image<R>, cursor: AreaCursor) -> Self {
        AreaFrames { image, cursor }
    }

    /// Number of malformed sectors processed so far by this cursor's scan of its area. The
    /// scan starts at the area's first audio sector, not at the track, and a continued
    /// cursor keeps counting from where the earlier tracks left off.
    /// Track cursors read ahead of the frames they return (a group of
    /// [`crate::dst::lanes`] frames, or with `parallel` up to [`MAX_PARALLEL_BATCH`]
    /// positions), so this may count sectors of frames not yet returned.
    pub fn malformed_sectors(&self) -> u64 {
        self.cursor.malformed_sectors()
    }

    /// The `audio-sector-malformed` warnings of this cursor's scan of its area so far
    /// (see [`AreaCursor::warnings`]): per area, not per track, and each sector once.
    pub fn warnings(&self) -> &[Warning] {
        self.cursor.warnings()
    }

    /// The progress of the reading, readable from other threads (see [`Progress`]).
    pub fn progress(&mut self) -> Progress {
        self.cursor.progress()
    }
}

impl<R: Read + Seek> Iterator for AreaFrames<'_, R> {
    type Item = Result<AreaFrame, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        self.cursor.next_frame(self.image)
    }
}

/// The sectors `[start, end)` of an area that exist in an image of `n` sectors
/// (specification 4, 4.2).
pub fn audio_sector_range(area: &Area, n: u64) -> (u64, u64) {
    let start = u64::from(area.audio_start_lsn);
    let end = (u64::from(area.audio_end_lsn) + 1).min(n);
    (start, end.max(start))
}

/// Checks whether a track produces audio (specification 4, 4.8).
///
/// # Errors
///
/// [`Error::ChannelCountUnsupported`], [`Error::TrackEmpty`] or
/// [`Error::TrackExceedsCapacity`], in that order of precedence.
pub fn check_track(area: &Area, track: &Track, image_sectors: u64) -> Result<(), Error> {
    check_channels(area)?;
    if track.duration_frames == 0 {
        return Err(Error::TrackEmpty);
    }
    check_capacity(area, track.end_frame(), image_sectors)
}

/// Checks whether the frame range `[first, end)` of an area produces audio
/// (specification 4, 4.7.1 items 3 and 4).
///
/// A range must lie in the area's play range `[0, P)`, `first <= end <= P` with `P` the
/// area's play time ([`Area::playtime_frames`]); anything else is a caller error. The
/// checks of 4.8 then apply as to a track, with `end - first` as the duration and `end`
/// as the end frame.
///
/// # Errors
///
/// [`Error::ChannelCountUnsupported`], [`Error::FrameRangeInvalid`] (the caller error),
/// [`Error::TrackEmpty`] (`first == end`) or [`Error::TrackExceedsCapacity`], in that
/// order of precedence. The 4.8 outcomes keep their track codes (specification 7, 7.7.4
/// reports them per track or per frame range).
pub fn check_range(area: &Area, first: u32, end: u32, image_sectors: u64) -> Result<(), Error> {
    check_channels(area)?;
    let playtime = area.playtime_frames();
    if first > end || end > playtime {
        return Err(Error::FrameRangeInvalid { first, end, playtime });
    }
    if first == end {
        return Err(Error::TrackEmpty);
    }
    check_capacity(area, end, image_sectors)
}

/// Specification 4, 4.8 item 1.
pub(crate) fn check_channels(area: &Area) -> Result<(), Error> {
    if !(1..=MAX_CHANNELS).contains(&area.channel_count) {
        return Err(Error::ChannelCountUnsupported { channels: area.channel_count });
    }
    Ok(())
}

/// Specification 4, 4.8 item 3, for a track or range ending at `end_frame`.
fn check_capacity(area: &Area, end_frame: u32, image_sectors: u64) -> Result<(), Error> {
    let (start, end) = audio_sector_range(area, image_sectors);
    let capacity = (end - start).saturating_mul(FRAMES_PER_SECTOR);
    if u64::from(end_frame) > capacity {
        return Err(Error::TrackExceedsCapacity { end_frame, capacity });
    }
    Ok(())
}

/// Track `number`'s **track with pause** (specification 4, 4.7.1 item 5): `[a, b)` with
/// `a = 0` for track 1 and its own start `s_i` for every other track, and
/// `b = max(s_i + d_i, s_j)` when the track numbered `j = i + 1` exists, `max(s_i + d_i, P)`
/// for the last track. With tracks out of order or overlapping these ranges may overlap,
/// and are used as they are. `None` if the area has no such track.
///
/// Check the range with [`check_range`] as for any range: it may be empty (a track of
/// duration 0 whose successor starts where it does), which is `track-empty`.
pub fn track_range_with_pause(area: &Area, number: u32) -> Option<(u32, u32)> {
    let track = area.track(number)?;
    let first = if number == 1 { 0 } else { track.start_frame };
    let next =
        number.checked_add(1).and_then(|n| area.track(n)).map_or(area.playtime_frames(), |t| t.start_frame);
    Some((first, next.max(track.end_frame())))
}

/// How a continuation opens its range ([`Continuation::log_audio`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Opening {
    /// The earlier cursor's scan goes on.
    Continues,
    /// A start from the seek index.
    FromIndex,
    /// A new scan from the area's first sector.
    Fresh,
}

/// A track or frame range of an area, checked ([`check_track`] or [`check_range`]), that
/// a cursor can continue into without failing: see [`crate::Disc::track_continuation`]
/// and [`crate::Disc::range_continuation`].
#[derive(Clone, Copy, Debug)]
pub struct Continuation<'a> {
    area: &'a Area,
    first: u32,
    end: u32,
    image_sectors: u64,
    image_id: Option<u64>,
    /// The disc's log, and the track number when this is a track, for the log.
    log: &'a Log,
    track: Option<u32>,
    /// The disc's seek indexes.
    seek: &'a SeekIndexes,
    /// The disc's seek policy for the area.
    policy: SeekPolicy,
}

impl<'a> Continuation<'a> {
    pub(crate) fn new(
        area: &'a Area,
        (first, end): (u32, u32),
        (image_sectors, image_id): (u64, Option<u64>),
        log: &'a Log,
        track: Option<u32>,
        (seek, policy): (&'a SeekIndexes, SeekPolicy),
    ) -> Self {
        Continuation { area, first, end, image_sectors, image_id, log, track, seek, policy }
    }

    /// The disc's index for this range's area, if it keeps one, under a read lock.
    fn with_index<T>(&self, f: impl FnOnce(Option<&AreaIndex>) -> T) -> T {
        self.seek.read(self.area.slot, |index| f(index.filter(|i| self.check_index(i).is_ok())))
    }

    /// Logs (at [`Level::Info`]) the opening of `what` as `opening`.
    fn log_how(&self, what: &str, opening: Opening) {
        if self.log.enabled(Level::Info) {
            let how = match opening {
                Opening::Continues => "continuing the earlier scan",
                Opening::FromIndex => "from the seek index",
                Opening::Fresh => "new scan of the area",
            };
            log_open(self.log, self.area, self.track, (self.first, self.end), what, how);
        }
    }

    /// Takes what `cursor` recorded into the disc's index, if the cursor read this image.
    fn absorb(&self, cursor: &mut AreaCursor) {
        if let Some(id) = self.image_id {
            self.seek.absorb(cursor, id);
        }
    }

    /// Takes what `cursor` recorded into the caller's `index`, if the cursor read this
    /// image and its checkpoints join the index; otherwise leaves them (they may be the
    /// disc's).
    fn absorb_into(&self, cursor: &mut AreaCursor, index: &mut AreaIndex) {
        let other_image = matches!((cursor.image_id(), self.image_id), (Some(a), Some(b)) if a != b);
        if !other_image && index.take_from(cursor).is_ok() {
            AreaIndex::drain(cursor);
        }
    }

    /// The frame range, `[first, end)`.
    pub fn range(&self) -> (u32, u32) {
        (self.first, self.end)
    }

    /// `prev` continued into this range (see [`TrackAudioCursor::continue_to_range`]),
    /// or a fresh cursor when its state cannot be reused, or a start from the disc's seek
    /// index. Logs the opening first ([`Continuation::log_audio`]).
    pub fn audio(self, prev: TrackAudioCursor) -> TrackAudioCursor {
        let opening = self.log_audio(&prev);
        self.audio_as(prev, opening)
    }

    /// How this range's audio opens from `prev`, decided once: logged (at
    /// [`Level::Info`]) and returned, for [`Continuation::audio_as`] to open it so.
    /// [`Continuation::audio`] does both.
    pub fn log_audio(&self, prev: &TrackAudioCursor) -> Opening {
        let opening = self.with_index(|index| {
            prev.opening(self.area, self.first, (self.image_sectors, self.image_id), index)
        });
        self.log_how("decoded audio", opening);
        opening
    }

    /// [`Continuation::audio`] without logging: no log sink runs during the call. For a
    /// caller that must not run one while it hands `prev` over (the C interface, whose
    /// callbacks could otherwise reach a handle being consumed); it calls
    /// [`Continuation::log_audio`] first, while `prev` is still guarded, then
    /// [`Continuation::audio_as`] with its answer.
    pub fn audio_unlogged(self, prev: TrackAudioCursor) -> TrackAudioCursor {
        let opening = self.with_index(|index| {
            prev.opening(self.area, self.first, (self.image_sectors, self.image_id), index)
        });
        self.audio_as(prev, opening)
    }

    /// Opens this range's audio from `prev` as `opening` says (from
    /// [`Continuation::log_audio`]), so that the log and the cursor agree; first takes
    /// what `prev` recorded into the disc's index. No log sink runs.
    pub fn audio_as(self, mut prev: TrackAudioCursor, opening: Opening) -> TrackAudioCursor {
        self.absorb(&mut prev.frames.area);
        self.with_index(|index| {
            let mut next = prev.open_as(
                self.area,
                (self.first, self.end),
                (self.image_sectors, self.image_id),
                index,
                (opening, self.policy),
            );
            if opening != Opening::Continues && index.is_some() {
                next.frames.area.keep_for_disc();
            }
            next
        })
    }

    /// `prev` continued into this range (see [`TrackCursor::continue_to_range`]), as
    /// [`Continuation::audio`] does for decoded audio.
    pub fn frames(self, prev: TrackCursor) -> TrackCursor {
        let opening = self.log_frames(&prev);
        self.frames_as(prev, opening)
    }

    /// How this range's frames open from `prev` (see [`Continuation::log_audio`]).
    pub fn log_frames(&self, prev: &TrackCursor) -> Opening {
        let opening = self.with_index(|index| {
            prev.opening(self.area, self.first, self.image_sectors, self.image_id, index)
        });
        self.log_how("frames", opening);
        opening
    }

    /// [`Continuation::frames`] without logging (see [`Continuation::audio_unlogged`]).
    pub fn frames_unlogged(self, prev: TrackCursor) -> TrackCursor {
        let opening = self.with_index(|index| {
            prev.opening(self.area, self.first, self.image_sectors, self.image_id, index)
        });
        self.frames_as(prev, opening)
    }

    /// Opens this range's frames from `prev` as `opening` says (see
    /// [`Continuation::audio_as`]).
    pub fn frames_as(self, mut prev: TrackCursor, opening: Opening) -> TrackCursor {
        self.absorb(&mut prev.area);
        self.with_index(|index| {
            let mut next = prev.open_as(
                self.area,
                (self.first, self.end),
                (self.image_sectors, self.image_id),
                index,
                (opening, self.policy),
            );
            if opening != Opening::Continues && index.is_some() {
                next.area.keep_for_disc();
            }
            next
        })
    }

    /// Whether `index` is of this range's area and image size, as the `_indexed` methods
    /// need.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`] if it is not.
    pub fn check_index(&self, index: &AreaIndex) -> Result<(), Error> {
        index.check_key(self.area, self.image_sectors)
    }

    /// How this range's audio opens from `prev` with the caller's seek `index` in place of
    /// the disc's: logged and returned, for [`Continuation::audio_indexed_as`].
    pub fn log_audio_indexed(&self, prev: &TrackAudioCursor, index: &AreaIndex) -> Opening {
        let index = Some(index).filter(|i| self.check_index(i).is_ok());
        let opening = prev.opening(self.area, self.first, (self.image_sectors, self.image_id), index);
        self.log_how("decoded audio", opening);
        opening
    }

    /// [`Continuation::audio_unlogged`] with the caller's seek `index` (of this range's
    /// area, [`Continuation::check_index`]) in place of the disc's: first takes what
    /// `prev` recorded into it, then continues `prev` or starts from it. An index of
    /// another area is not used.
    pub fn audio_indexed_unlogged(self, prev: TrackAudioCursor, index: &mut AreaIndex) -> TrackAudioCursor {
        let opening = {
            let index = Some(&*index).filter(|i| self.check_index(i).is_ok());
            prev.opening(self.area, self.first, (self.image_sectors, self.image_id), index)
        };
        self.audio_indexed_as(prev, index, opening)
    }

    /// [`Continuation::audio_as`] with the caller's seek `index`.
    pub fn audio_indexed_as(
        self,
        mut prev: TrackAudioCursor,
        index: &mut AreaIndex,
        opening: Opening,
    ) -> TrackAudioCursor {
        self.absorb_into(&mut prev.frames.area, index);
        let index = Some(&*index).filter(|i| self.check_index(i).is_ok());
        prev.open_as(
            self.area,
            (self.first, self.end),
            (self.image_sectors, self.image_id),
            index,
            (opening, self.policy),
        )
    }

    /// How this range's frames open from `prev` with the caller's `index` (see
    /// [`Continuation::log_audio_indexed`]).
    pub fn log_frames_indexed(&self, prev: &TrackCursor, index: &AreaIndex) -> Opening {
        let index = Some(index).filter(|i| self.check_index(i).is_ok());
        let opening = prev.opening(self.area, self.first, self.image_sectors, self.image_id, index);
        self.log_how("frames", opening);
        opening
    }

    /// [`Continuation::frames_unlogged`] with the caller's seek `index` (see
    /// [`Continuation::audio_indexed_unlogged`]).
    pub fn frames_indexed_unlogged(self, prev: TrackCursor, index: &mut AreaIndex) -> TrackCursor {
        let opening = {
            let index = Some(&*index).filter(|i| self.check_index(i).is_ok());
            prev.opening(self.area, self.first, self.image_sectors, self.image_id, index)
        };
        self.frames_indexed_as(prev, index, opening)
    }

    /// [`Continuation::frames_as`] with the caller's seek `index`.
    pub fn frames_indexed_as(
        self,
        mut prev: TrackCursor,
        index: &mut AreaIndex,
        opening: Opening,
    ) -> TrackCursor {
        self.absorb_into(&mut prev.area, index);
        let index = Some(&*index).filter(|i| self.check_index(i).is_ok());
        prev.open_as(
            self.area,
            (self.first, self.end),
            (self.image_sectors, self.image_id),
            index,
            (opening, self.policy),
        )
    }

    /// Logs (at [`Level::Info`]) the opening of this range's frames on a new scan, as
    /// [`Continuation::fresh_frames`] does not.
    pub fn log_fresh_frames(&self) {
        self.log_how("frames", Opening::Fresh);
    }

    /// A fresh cursor of this range's positions (see [`TrackCursor::for_range`]), without
    /// logging (see [`Continuation::log_fresh_frames`]).
    pub fn fresh_frames(self) -> TrackCursor {
        self.seek.read(self.area.slot, |index| {
            let index = index.filter(|i| i.check_key(self.area, self.image_sectors).is_ok());
            let range = (self.first, self.end);
            let mut c = TrackCursor::start_with(self.area, range, self.image_sectors, index, self.policy);
            if index.is_some() {
                c.area.keep_for_disc();
            }
            c
        })
    }
}

/// Logs the opening of a track (`track` is its number) or frame range `[first, end)` of
/// `area`.
pub(crate) fn log_open(
    log: &Log,
    area: &Area,
    track: Option<u32>,
    (first, end): (u32, u32),
    what: &str,
    how: &str,
) {
    let slot = area.slot;
    let c = area.channel_count;
    match track {
        Some(n) => {
            sb_log!(log, Info, "area {slot} track {n}: frames {first}..{end} ({c} channels), {what}, {how}")
        }
        None => sb_log!(log, Info, "area {slot} range: frames {first}..{end} ({c} channels), {what}, {how}"),
    }
}

/// What a timeline position holds (specification 4, 4.6), before DST decoding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameContent {
    /// A plain DSD frame: `C x 4704` bytes in interleaved layout, used unchanged.
    Dsd(Vec<u8>),
    /// A DST frame's bytes, to be decoded.
    Dst(Vec<u8>),
    /// No accepted frame fills the position.
    Missing,
}

impl FrameContent {
    /// The position's **DST equivalent** (specification 4, 4.6.1 item 3): a DST frame that
    /// decodes, with `channels` channels, to exactly the position's content of 4.6. For a
    /// DST frame its stored bytes; for a plain frame the byte 0x00 (a plain DST frame
    /// header, 5.2.1) followed by its bytes; for a missing position 0x00 followed by
    /// `channels x 4704` bytes of 0x69. At most `1 + 6 x 4704` bytes for a plain or missing
    /// position, [`MAX_FRAME_BYTES`] for a DST frame.
    ///
    /// `channels` is the area's channel count (the cursors' `channels()`); it sizes only
    /// the missing fill. [`TrackFrames::dst_equivalent`] and
    /// [`TrackCursor::dst_equivalent`] pass it for you.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`] if `channels` is not 1 to 6.
    pub fn dst_equivalent(&self, channels: u8) -> Result<std::borrow::Cow<'_, [u8]>, Error> {
        if !(1..=MAX_CHANNELS).contains(&channels) {
            return Err(Error::InvalidArgument("channel count must be 1 to 6"));
        }
        Ok(self.equivalent_checked(channels))
    }

    /// [`FrameContent::dst_equivalent`] for a channel count already checked (1 to 6).
    fn equivalent_checked(&self, channels: u8) -> std::borrow::Cow<'_, [u8]> {
        let plain = |bytes: &[u8]| {
            let mut v = Vec::with_capacity(1 + bytes.len());
            v.push(0x00);
            v.extend_from_slice(bytes);
            v
        };
        match self {
            FrameContent::Dst(bytes) => std::borrow::Cow::Borrowed(bytes),
            FrameContent::Dsd(bytes) => std::borrow::Cow::Owned(plain(bytes)),
            FrameContent::Missing => {
                let fill = fill_frame(channels, MISSING_FILL);
                std::borrow::Cow::Owned(plain(&fill))
            }
        }
    }
}

/// One timeline position of a track or frame range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackFrame {
    /// The frame number.
    pub number: u32,
    /// Its content.
    pub content: FrameContent,
}

/// The reading state of one track's timeline positions (specification 4, 4.7),
/// independent of the image it reads from. [`TrackFrames`] is the borrowing iterator.
///
/// A track is one frame range of its area; [`TrackCursor::for_range`] reads any other
/// (the whole area, or tracks with the pauses between them) with the same frame
/// numbering, timeline and contents.
#[derive(Debug)]
pub struct TrackCursor {
    area: AreaCursor,
    /// Which area, and in which image, the area cursor reads: continuing into a track of
    /// another area (or the same area of another image) needs a fresh scan.
    area_key: AreaKey,
    next: u32,
    end: u32,
    channels: u8,
    peeked: Option<AreaFrame>,
    exhausted: bool,
    failed: bool,
    /// Missing positions returned since the cursor was made or continued.
    missing: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AreaKey {
    pub(crate) slot: Slot,
    pub(crate) start: u32,
    pub(crate) end: u32,
    pub(crate) channels: u8,
    pub(crate) image_sectors: u64,
}

impl AreaKey {
    /// The existing audio sectors, as [`audio_sector_range`].
    pub(crate) fn audio_sector_range(&self) -> (u64, u64) {
        let start = u64::from(self.start);
        let end = (u64::from(self.end) + 1).min(self.image_sectors);
        (start, end.max(start))
    }

    pub(crate) fn new(area: &Area, image_sectors: u64) -> Self {
        AreaKey {
            slot: area.slot,
            start: area.audio_start_lsn,
            end: area.audio_end_lsn,
            channels: area.channel_count,
            image_sectors,
        }
    }
}

impl TrackCursor {
    /// A cursor at the start of `track` of `area`, after the checks of 4.8.
    ///
    /// # Errors
    ///
    /// As [`check_track`].
    pub fn new(area: &Area, track: &Track, image_sectors: u64) -> Result<Self, Error> {
        check_track(area, track, image_sectors)?;
        Ok(Self::start(area, track.start_frame, track.end_frame(), image_sectors))
    }

    /// A cursor at frame `first` of `area`, ending before frame `end`, after
    /// [`check_range`]: the positions `first` to `end - 1` in order, exactly as a track
    /// covering them would give.
    ///
    /// # Errors
    ///
    /// As [`check_range`].
    pub fn for_range(area: &Area, first: u32, end: u32, image_sectors: u64) -> Result<Self, Error> {
        check_range(area, first, end, image_sectors)?;
        Ok(Self::start(area, first, end, image_sectors))
    }

    /// As [`TrackCursor::for_range`], starting from `index` instead of the area's first
    /// audio sector (see [`crate::seek`]): at its last checkpoint whose next acceptable
    /// frame number is at most `first`, or reading nothing when the index is complete and
    /// no frame numbered `first` or more is accepted. The positions, their contents, the
    /// missing count, [`TrackCursor::malformed_sectors`] and [`TrackCursor::warnings`] are
    /// exactly those of [`TrackCursor::for_range`]: the malformed sectors counted are
    /// those a scan from the area's first audio sector has processed, the ones before
    /// the checkpoint coming from the index. Only the sectors read and the log lines
    /// differ; sectors replayed to rebuild the frame under assembly are not logged.
    ///
    /// The cursor records the checkpoints it crosses past the index's frontier; take them
    /// with [`AreaIndex::absorb`]. An empty index ([`AreaIndex::new`]) gives a scan from
    /// the first sector that builds one.
    ///
    /// # Errors
    ///
    /// As [`check_range`], then [`Error::InvalidArgument`] if `index` is of another area
    /// or image size.
    pub fn for_range_indexed(
        area: &Area,
        first: u32,
        end: u32,
        image_sectors: u64,
        index: &AreaIndex,
    ) -> Result<Self, Error> {
        check_range(area, first, end, image_sectors)?;
        index.check_key(area, image_sectors)?;
        Ok(Self::start_indexed(area, first, end, image_sectors, index))
    }

    /// A scan of `area` for the range `[first, end)`, already checked, from `index` (of
    /// that area, checked).
    pub(crate) fn start_indexed(
        area: &Area,
        first: u32,
        end: u32,
        image_sectors: u64,
        index: &AreaIndex,
    ) -> Self {
        let mut cursor = TrackCursor {
            area: AreaCursor::resume(index, first),
            ..Self::start(area, first, end, image_sectors)
        }
        .estimated(area);
        if let ResumedFrom::Checkpoint { lsn, replay_from } = cursor.area.resumed {
            // One read from the replay to the range's end: through the seek hint after it,
            // or the range's frames at the area's average density from the checkpoint (whose
            // successor is past `first`, so frame `first` starts within an interval of it),
            // plus a quarter and a few sectors.
            let (s0, s1) = audio_sector_range(area, image_sectors);
            let hinted = index.hint(first, end, area.playtime_frames()).filter(|h| h.3).map(|h| h.2);
            let to = hinted.filter(|&to| to > lsn).unwrap_or_else(|| {
                let frames = u128::from(end - first);
                let span = frames * u128::from(s1 - s0) / u128::from(area.playtime_frames().max(1));
                let span = u64::try_from(span + span / 4 + 4).unwrap_or(u64::MAX);
                lsn.saturating_add(u64::from(index.interval())).saturating_add(span)
            });
            // At most the replay, an interval and 64 sectors of the range (128 KiB): a range
            // to the area's end is not read ahead further than that.
            let most = lsn + u64::from(index.interval()) + 64;
            cursor.area.size_first_read(to.min(most).min(replay_from + crate::seek::MAX_READ_SECTORS));
        }
        cursor
    }

    /// This cursor, expecting its reading to end where the range's end lies in proportion
    /// to the area's play time (see [`Progress`]).
    fn estimated(mut self, area: &Area) -> Self {
        let (start, stop) = audio_sector_range(area, self.area_key.image_sectors);
        let playtime = area.playtime_frames();
        let lsn = if playtime == 0 || self.end >= playtime {
            stop
        } else {
            start + (u128::from(stop - start) * u128::from(self.end) / u128::from(playtime)) as u64
        };
        self.area.set_progress_end(lsn);
        self
    }

    /// The progress of this cursor's reading, readable from other threads (see
    /// [`Progress`]): the sectors read since it was made or continued.
    pub fn progress(&mut self) -> Progress {
        self.area.progress()
    }

    /// As [`TrackCursor::for_range_indexed`] (or [`TrackCursor::for_range`] without an
    /// index), allowing a provisional start (specification 4, 4.2 item 8, sacd-spec
    /// `82eac6c`; see [`SeekPolicy`]) when the exact start would read more than
    /// `options.min_gap_sectors` before the range's estimated position. The cursor chooses on
    /// its first read; [`TrackCursor::speculation`] and [`TrackCursor::resumed_from`] then
    /// say which start it took. A provisional cursor records no checkpoints.
    ///
    /// # Errors
    ///
    /// As [`TrackCursor::for_range_indexed`].
    pub fn for_range_speculative(
        area: &Area,
        first: u32,
        end: u32,
        image_sectors: u64,
        index: Option<&AreaIndex>,
        options: crate::seek::SpeculativeOptions,
    ) -> Result<Self, Error> {
        check_range(area, first, end, image_sectors)?;
        if let Some(index) = index {
            index.check_key(area, image_sectors)?;
        }
        Ok(Self::start_with(area, (first, end), image_sectors, index, SeekPolicy::Speculative(options)))
    }

    /// A cursor of the range, already checked, from `index` (of the area, checked) or the
    /// first sector, provisional where `policy` allows it.
    pub(crate) fn start_with(
        area: &Area,
        (first, end): (u32, u32),
        image_sectors: u64,
        index: Option<&AreaIndex>,
        policy: SeekPolicy,
    ) -> Self {
        let exact = match index {
            Some(index) => Self::start_indexed(area, first, end, image_sectors, index),
            None => Self::start(area, first, end, image_sectors),
        };
        let interval = index.map_or(crate::seek::DEFAULT_INTERVAL, AreaIndex::interval);
        let mut cursor = exact.speculate(area, (first, end), (interval, index), policy).estimated(area);
        cursor.area.record_hints(index.and_then(AreaIndex::hint_recorder));
        cursor
    }

    /// This cursor, or a provisional one in its place when `policy` allows it and the
    /// exact start lies more than the options' gap before the range's estimated position.
    fn speculate(
        self,
        area: &Area,
        (first, end): (u32, u32),
        (interval, provisional): (u32, Option<&AreaIndex>),
        policy: SeekPolicy,
    ) -> Self {
        let SeekPolicy::Speculative(options) = policy else { return self };
        let from = match self.area.resumed {
            ResumedFrom::Checkpoint { lsn, .. } => lsn,
            ResumedFrom::Start => self.area.next_lsn,
            // Reads nothing, or a cursor already chosen: keep it.
            _ => return self,
        };
        let (start, stop) = audio_sector_range(area, self.area_key.image_sectors);
        let playtime = area.playtime_frames();
        if playtime == 0 {
            return self;
        }
        // The range's position in proportion to the play time.
        let estimate = start + (u128::from(stop - start) * u128::from(first) / u128::from(playtime)) as u64;
        if estimate <= from.saturating_add(options.min_gap_sectors) {
            return self;
        }
        // A seek hint near `first` (seek hints, 4.2 item 8 locator samples): one read from it,
        // no search; the read ends through the next hint past the range, or is estimated.
        let hint = provisional
            .and_then(|i| i.hint(first, end, playtime))
            .filter(|h| h.0 > from)
            .map(|(lsn, t, to, _)| (lsn, t, to));
        // An earlier provisional reading's checkpoint around `first`: no search needed.
        if let Some(p) = provisional
            .and_then(|i| i.plan_provisional(first))
            .filter(|&(lsn, _)| lsn > from && hint.is_none())
        {
            return TrackCursor { area: AreaCursor::provisional_at(&self.area, p, first, interval), ..self };
        }
        // Track starts inside the audio sectors, as points to interpolate between (4.2 item
        // 8 lets them choose `L`; they never change what is read from it).
        let knots = area
            .tracks
            .iter()
            .map(|t| (u64::from(t.start_lsn), t.start_frame))
            .filter(|&(l, _)| (start..stop).contains(&l))
            .collect();
        let area_cursor = AreaCursor::speculative(
            self.area,
            (first, self.end),
            (playtime, knots),
            (interval, hint),
            options,
        );
        TrackCursor { area: area_cursor, ..self }
    }

    /// The slot of this cursor's area.
    pub fn slot(&self) -> Slot {
        self.area_key.slot
    }

    /// Whether this cursor's positions are provisional (see [`SeekPolicy`]).
    pub fn speculation(&self) -> Speculation {
        self.area.speculation()
    }

    /// Where its malformed count and warnings begin (see [`AreaCursor::counts_from`]).
    pub fn counts_from(&self) -> Option<u64> {
        self.area.counts_from()
    }

    /// The number of the provisional reading, as events name it ([`crate::SeekEvent`]);
    /// `None` for exact positions.
    pub fn speculation_id(&self) -> Option<u64> {
        self.area.speculation_id()
    }

    /// Where this cursor's scan started (see [`ResumedFrom`]); continued cursors keep it.
    pub fn resumed_from(&self) -> ResumedFrom {
        self.area.resumed_from()
    }

    pub(crate) fn area_mut(&mut self) -> &mut AreaCursor {
        &mut self.area
    }

    /// A fresh scan of `area` for the range `[first, end)`, already checked.
    pub(crate) fn start(area: &Area, first: u32, end: u32, image_sectors: u64) -> Self {
        let cursor = TrackCursor {
            area: AreaCursor::new(area, image_sectors),
            area_key: AreaKey::new(area, image_sectors),
            next: first,
            end,
            channels: area.channel_count,
            peeked: None,
            exhausted: false,
            failed: false,
            missing: 0,
        };
        cursor.estimated(area)
    }

    /// A cursor at the start of `track`, reusing this cursor's reading state when that
    /// gives the same result as a fresh scan, so that reading an area's tracks in order
    /// reads each audio sector once.
    ///
    /// A cursor only ever discards accepted frames numbered below its current position.
    /// The state is therefore reused when `track` is in the same area (same slot, audio
    /// range, channel count and image size) and starts at or after that position, and no
    /// read has failed. Otherwise this is [`TrackCursor::new`]. Either way the frames are
    /// exactly those of a fresh scan (specification 4, 4.2 item 5).
    ///
    /// This method cannot tell two images of the same size apart; a cursor continued
    /// onto another image refuses to read it (see [`AreaCursor::next_frame`]). Use
    /// [`crate::Disc::continue_track_audio`], which compares the images and scans afresh.
    ///
    /// A cursor made from a seek index ([`TrackCursor::for_range_indexed`]) does not keep
    /// the index: continued backwards with this method, it scans from the area's first
    /// sector. To start from the index instead, open a new cursor from it.
    ///
    /// # Errors
    ///
    /// As [`check_track`].
    pub fn continue_to(self, area: &Area, track: &Track, image_sectors: u64) -> Result<Self, Error> {
        check_track(area, track, image_sectors)?;
        Ok(self.continue_in(area, track.start_frame, track.end_frame(), (image_sectors, None), None))
    }

    /// A cursor for the frame range `[first, end)` of `area`, continuing this one's
    /// reading state where that gives the same result, as [`TrackCursor::continue_to`]
    /// does for a track: the state is reused when `first` is at or after this cursor's
    /// position in the same area.
    ///
    /// # Errors
    ///
    /// As [`check_range`].
    pub fn continue_to_range(
        self,
        area: &Area,
        first: u32,
        end: u32,
        image_sectors: u64,
    ) -> Result<Self, Error> {
        check_range(area, first, end, image_sectors)?;
        Ok(self.continue_in(area, first, end, (image_sectors, None), None))
    }

    /// As [`TrackCursor::continue_to_range`] for a range already checked, also starting
    /// afresh when this cursor has read an image other than `image_id`, and from `index`
    /// (of `area`) when that is given and the state is not reused.
    pub(crate) fn continue_in(
        self,
        area: &Area,
        first: u32,
        end: u32,
        (image_sectors, image_id): (u64, Option<u64>),
        index: Option<&AreaIndex>,
    ) -> Self {
        let opening = self.opening(area, first, image_sectors, image_id, index);
        self.open_as(area, (first, end), (image_sectors, image_id), index, (opening, SeekPolicy::Exact))
    }

    /// How [`TrackCursor::continue_in`] opens the range starting at `first`.
    pub(crate) fn opening(
        &self,
        area: &Area,
        first: u32,
        image_sectors: u64,
        image_id: Option<u64>,
        index: Option<&AreaIndex>,
    ) -> Opening {
        if self.reuses(area, first, image_sectors, image_id, index) {
            Opening::Continues
        } else if index.is_some() {
            Opening::FromIndex
        } else {
            Opening::Fresh
        }
    }

    /// The range `[first, end)` opened as `opening` says (from [`TrackCursor::opening`]):
    /// this cursor's state reused, a start from `index`, or a new scan.
    pub(crate) fn open_as(
        self,
        area: &Area,
        (first, end): (u32, u32),
        (image_sectors, image_id): (u64, Option<u64>),
        index: Option<&AreaIndex>,
        (opening, policy): (Opening, SeekPolicy),
    ) -> Self {
        let reusable = self.reusable(area, first, image_sectors, image_id);
        match (opening, index) {
            (Opening::Continues, _) if reusable => {
                let mut next = TrackCursor { next: first, end, missing: 0, ..self };
                next.area.restart_progress();
                next.estimated(area)
            }
            // A range (or track) before this cursor's position, or in another area, scans
            // the area again: specification 4, 4.2 item 6 allows a pass per track or range,
            // and item 7 a start from the seek index (item 8 a provisional one, if allowed).
            (Opening::Continues | Opening::FromIndex, Some(index)) => {
                let mut next = Self::start_with(area, (first, end), image_sectors, Some(index), policy);
                if reusable
                    && next.area.speculation() == Speculation::Exact
                    && self.area.speculation() == Speculation::Exact
                {
                    // Moved ahead of a scan that could have gone on: until it reads, it
                    // reports what that scan reports (a pass from the first sector). Not
                    // from a provisional scan, whose counts begin at its `L` (4.4.5 items 4
                    // and 5).
                    next.area.carry_counts(&self.area);
                }
                next
            }
            _ => Self::start_with(area, (first, end), image_sectors, None, policy),
        }
    }

    /// Whether [`TrackCursor::continue_in`] keeps this cursor's reading state: when it
    /// can ([`TrackCursor::reusable`]), unless `index` starts the range more than two
    /// checkpoints past the sector this cursor reads next.
    fn reuses(
        &self,
        area: &Area,
        first: u32,
        image_sectors: u64,
        image_id: Option<u64>,
        index: Option<&AreaIndex>,
    ) -> bool {
        self.reusable(area, first, image_sectors, image_id)
            && index.is_none_or(|index| {
                let ahead = 2 * u64::from(index.interval());
                index.plan_lsn(first).is_some_and(|lsn| lsn <= self.area.scan_lsn().saturating_add(ahead))
            })
    }

    /// Whether [`TrackCursor::continue_in`] keeps this cursor's reading state.
    fn reusable(&self, area: &Area, first: u32, image_sectors: u64, image_id: Option<u64>) -> bool {
        let other_image = matches!((self.area.image_id, image_id), (Some(a), Some(b)) if a != b);
        !(self.failed
            || other_image
            || self.area_key != AreaKey::new(area, image_sectors)
            || first < self.next)
    }

    /// The area's channel count `C` (1 to 6).
    pub fn channels(&self) -> u8 {
        self.channels
    }

    /// The DST equivalent of a position of this cursor's area (specification 4, 4.6.1 item
    /// 3; see [`FrameContent::dst_equivalent`]), with the area's channel count.
    pub fn dst_equivalent<'c>(&self, content: &'c FrameContent) -> std::borrow::Cow<'c, [u8]> {
        // The channel count passed the checks of 4.8 item 1 when the cursor was made.
        content.equivalent_checked(self.channels)
    }

    /// Positions returned so far that no accepted frame fills ([`FrameContent::Missing`]);
    /// 0 again after [`TrackCursor::continue_to`] or [`TrackCursor::continue_to_range`].
    pub fn missing_frames(&self) -> u64 {
        self.missing
    }

    /// The frame range still to come, `[next, end)`.
    pub fn range(&self) -> (u32, u32) {
        (self.next, self.end)
    }

    /// Number of malformed sectors processed so far by this cursor's scan of its area. The
    /// scan starts at the area's first audio sector, not at the track, and a continued
    /// cursor keeps counting from where the earlier tracks left off.
    /// Track cursors read ahead of the frames they return (a group of
    /// [`crate::dst::lanes`] frames, or with `parallel` up to [`MAX_PARALLEL_BATCH`]
    /// positions), so this may count sectors of frames not yet returned.
    pub fn malformed_sectors(&self) -> u64 {
        self.area.malformed_sectors()
    }

    /// The `audio-sector-malformed` warnings of this cursor's scan of its area so far
    /// (see [`AreaCursor::warnings`]): per area, not per track, and each sector once.
    pub fn warnings(&self) -> &[Warning] {
        self.area.warnings()
    }

    /// The next timeline position, `None` after the track's (or range's) last frame.
    pub fn next_frame<R: Read + Seek>(&mut self, image: &mut Image<R>) -> Option<Result<TrackFrame, Error>> {
        if self.next >= self.end {
            return None;
        }
        if let Err(e) = image.check_cancel() {
            return Some(Err(e));
        }
        let t = self.next;
        // Accepted frames arrive in increasing order: skip those before t. Stops reading
        // as soon as a frame at or after t is seen (specification 4, 4.5 item 4).
        while !self.exhausted && self.peeked.as_ref().is_none_or(|f| f.number < t) {
            match self.area.next_frame(image) {
                Some(Ok(f)) => self.peeked = Some(f),
                // Nothing consumed: position t again next time.
                Some(Err(Error::Cancelled)) => return Some(Err(Error::Cancelled)),
                Some(Err(e)) => {
                    self.next = self.end;
                    self.failed = true;
                    return Some(Err(e));
                }
                None => {
                    self.peeked = None;
                    self.exhausted = true;
                }
            }
        }
        self.next += 1;
        if self.next >= self.end {
            self.area.progress_done();
            self.area.flush_hints();
        }
        if let Some(claim) = &self.area.claim {
            claim.returned(t);
        }
        let content = match self.peeked.take_if(|f| f.number == t) {
            Some(f) => match f.kind {
                FrameKind::Dsd => FrameContent::Dsd(f.data),
                FrameKind::Dst => FrameContent::Dst(f.data),
            },
            None => {
                self.missing += 1;
                FrameContent::Missing
            }
        };
        Some(Ok(TrackFrame { number: t, content }))
    }
}

/// The timeline positions of one track or frame range, in order (specification 4, 4.7),
/// without decoding: each position's frame as stored, a DST frame's bytes (its length is
/// the frame length `FB` of specification 5) or a plain frame's `C x 4704` bytes, or
/// [`FrameContent::Missing`]. Decoding a DST frame's bytes with [`DstDecoder`] gives the
/// position's decoded audio; frame numbers, the timeline, missing positions and malformed
/// sectors are exactly those of the decoded reads ([`TrackAudio`]).
///
/// A frame is at most [`MAX_FRAME_BYTES`] bytes: nothing larger is ever assembled.
// Specification 4, 4.6.1 items 1 and 2 (the stored content of a track or frame range):
// positions are DST, plain or missing as stored, missing positions are counted and
// invalid DST frames are not reported, since validity is known only by decoding.
#[derive(Debug)]
pub struct TrackFrames<'a, R> {
    image: &'a mut Image<R>,
    cursor: TrackCursor,
    /// The disc's seek indexes, which take the checkpoints the cursor records.
    seek: Option<&'a SeekIndexes>,
}

impl<'a, R: Read + Seek> TrackFrames<'a, R> {
    pub(crate) fn with_cursor(
        image: &'a mut Image<R>,
        cursor: TrackCursor,
        seek: Option<&'a SeekIndexes>,
    ) -> Self {
        TrackFrames { image, cursor, seek }
    }

    /// The reading state, to continue into a later track or range with
    /// [`crate::Disc::continue_track_frames`] or [`crate::Disc::continue_range_frames`].
    pub fn into_cursor(self) -> TrackCursor {
        self.cursor
    }

    /// The area's channel count `C` (1 to 6).
    pub fn channels(&self) -> u8 {
        self.cursor.channels()
    }

    /// Positions returned so far that no accepted frame fills (see
    /// [`TrackCursor::missing_frames`]).
    pub fn missing_frames(&self) -> u64 {
        self.cursor.missing_frames()
    }

    /// The DST equivalent of a position of this track or range (specification 4, 4.6.1
    /// item 3; see [`FrameContent::dst_equivalent`]), with the area's channel count.
    pub fn dst_equivalent<'c>(&self, content: &'c FrameContent) -> std::borrow::Cow<'c, [u8]> {
        self.cursor.dst_equivalent(content)
    }

    /// The frame range still to come, `[next, end)`.
    pub fn range(&self) -> (u32, u32) {
        self.cursor.range()
    }

    /// Number of malformed sectors processed so far by this cursor's scan of its area. The
    /// scan starts at the area's first audio sector, not at the track, and a continued
    /// cursor keeps counting from where the earlier tracks left off.
    /// Track cursors read ahead of the frames they return (a group of
    /// [`crate::dst::lanes`] frames, or with `parallel` up to [`MAX_PARALLEL_BATCH`]
    /// positions), so this may count sectors of frames not yet returned.
    pub fn malformed_sectors(&self) -> u64 {
        self.cursor.malformed_sectors()
    }

    /// The `audio-sector-malformed` warnings of this cursor's scan of its area so far
    /// (see [`AreaCursor::warnings`]): per area, not per track, and each sector once.
    pub fn warnings(&self) -> &[Warning] {
        self.cursor.warnings()
    }

    /// Whether the positions are provisional (see [`SeekPolicy`]).
    pub fn speculation(&self) -> Speculation {
        self.cursor.speculation()
    }

    /// Where the malformed count and warnings begin (see [`AreaCursor::counts_from`]).
    pub fn counts_from(&self) -> Option<u64> {
        self.cursor.counts_from()
    }

    /// The number of the provisional reading, as events name it ([`crate::SeekEvent`]);
    /// `None` for exact positions.
    pub fn speculation_id(&self) -> Option<u64> {
        self.cursor.speculation_id()
    }

    /// The progress of the reading, readable from other threads (see [`Progress`]).
    pub fn progress(&mut self) -> Progress {
        self.cursor.progress()
    }
}

impl<R: Read + Seek> Iterator for TrackFrames<'_, R> {
    type Item = Result<TrackFrame, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let frame = self.cursor.next_frame(self.image);
        if let Some(seek) = self.seek {
            seek.absorb(&mut self.cursor.area, self.image.id());
        }
        frame
    }
}

/// How a decoded timeline position was produced (specification 4, 4.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FrameStatus {
    /// A plain DSD frame, passed through unchanged.
    Dsd,
    /// A DST frame, decoded.
    Decoded,
    /// No frame: filled with 0x69.
    Missing,
    /// An invalid DST frame: filled with 0x55.
    Invalid(DstError),
}

/// One decoded timeline position: `C x 4704` bytes in interleaved layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioFrame {
    /// The frame number.
    pub number: u32,
    /// How it was produced.
    pub status: FrameStatus,
    /// The DSD bytes, interleaved (specification 0, 0.6).
    pub data: Vec<u8>,
}

/// Decodes one timeline position of an area of `channels` channels (specification 4,
/// 4.6).
///
/// # Errors
///
/// [`Error::InvalidArgument`] if `channels` is not 1 to 6, or a plain DSD frame is not
/// `channels x 4704` bytes.
pub fn decode_frame(decoder: &mut DstDecoder, frame: TrackFrame, channels: u8) -> Result<AudioFrame, Error> {
    if !(1..=MAX_CHANNELS).contains(&channels) {
        return Err(Error::InvalidArgument("channel count must be 1 to 6"));
    }
    if let FrameContent::Dsd(data) = &frame.content {
        if data.len() != usize::from(channels) * FRAME_BYTES_PER_CHANNEL {
            return Err(Error::InvalidArgument("a plain DSD frame must be channels x 4704 bytes"));
        }
    }
    Ok(decode_checked(decoder, frame, channels))
}

/// [`decode_frame`] for arguments already validated: `channels` is 1 to 6 (checked by
/// [`check_track`]) and plain frames are complete (4.4.2).
fn decode_checked(decoder: &mut DstDecoder, frame: TrackFrame, channels: u8) -> AudioFrame {
    let (status, data) = match frame.content {
        FrameContent::Dsd(data) => (FrameStatus::Dsd, data),
        FrameContent::Missing => (FrameStatus::Missing, fill_frame(channels, MISSING_FILL)),
        FrameContent::Dst(bytes) => {
            let mut out = vec![0u8; usize::from(channels) * FRAME_BYTES_PER_CHANNEL];
            match decoder.decode(&bytes, channels, &mut out) {
                Ok(()) => (FrameStatus::Decoded, out),
                Err(e) => {
                    // The channel count was checked (4.8) and the buffer sized from it, so
                    // only frame rules can fail here.
                    debug_assert_ne!(
                        e,
                        DstError::InvalidArguments,
                        "decoder arguments checked by check_track"
                    );
                    out.fill(INVALID_FILL);
                    (FrameStatus::Invalid(e), out)
                }
            }
        }
    };
    AudioFrame { number: frame.number, status, data }
}

/// [`decode_checked`] for DST frames (`frames` all have `FrameContent::Dst`), together
/// with [`DstDecoder::decode_many`]: the same frames as decoding them one by one.
fn decode_dst_run(
    decoder: &mut DstDecoder,
    frames: Vec<TrackFrame>,
    channels: u8,
    log: &Log,
) -> Vec<AudioFrame> {
    if let (Some(first), Some(last)) = (frames.first(), frames.last()) {
        sb_log!(
            log,
            Trace,
            "decoding DST frames {}..={} on thread {:?}",
            first.number,
            last.number,
            std::thread::current().id()
        );
    }
    let size = usize::from(channels) * FRAME_BYTES_PER_CHANNEL;
    let mut outs = vec![vec![0u8; size]; frames.len()];
    let results = {
        let bytes: Vec<&[u8]> = frames
            .iter()
            .map(|f| match &f.content {
                FrameContent::Dst(b) => b.as_slice(),
                _ => &[],
            })
            .collect();
        let mut slices: Vec<&mut [u8]> = outs.iter_mut().map(Vec::as_mut_slice).collect();
        decoder.decode_many(&bytes, channels, &mut slices)
    };
    frames
        .into_iter()
        .zip(outs)
        .zip(results)
        .map(|((frame, mut out), result)| {
            let status = match result {
                Ok(()) => FrameStatus::Decoded,
                Err(e) => {
                    // The channel count was checked (4.8) and the buffers sized from it, so
                    // only frame rules can fail here.
                    debug_assert_ne!(
                        e,
                        DstError::InvalidArguments,
                        "decoder arguments checked by check_track"
                    );
                    out.fill(INVALID_FILL);
                    FrameStatus::Invalid(e)
                }
            };
            AudioFrame { number: frame.number, status, data: out }
        })
        .collect()
}

/// A position's stored content (specification 4, 4.6.1 item 2) next to its decoded audio
/// ([`TrackAudioCursor::next_frame_with_stored`]): what [`TrackCursor::next_frame`] gives
/// for it, without a second copy of a plain frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stored {
    /// A DST frame's bytes as stored (its decoding is the [`AudioFrame`]'s data).
    Dst(Vec<u8>),
    /// A plain DSD frame: its stored bytes are the [`AudioFrame`]'s data, unchanged
    /// (status [`FrameStatus::Dsd`]).
    Plain,
    /// No accepted frame fills the position.
    Missing,
}

impl Stored {
    /// The position's [`FrameContent`], given its decoded `frame` (which a plain one's
    /// bytes are copied from).
    pub fn content(&self, frame: &AudioFrame) -> FrameContent {
        match self {
            Stored::Dst(bytes) => FrameContent::Dst(bytes.clone()),
            Stored::Plain => FrameContent::Dsd(frame.data.clone()),
            Stored::Missing => FrameContent::Missing,
        }
    }

    /// The stored bytes, given the decoded `frame`: a DST frame's, a plain frame's (the
    /// frame's data), none for a missing position.
    pub fn bytes<'a>(&'a self, frame: &'a AudioFrame) -> &'a [u8] {
        match self {
            Stored::Dst(bytes) => bytes,
            Stored::Plain => &frame.data,
            Stored::Missing => &[],
        }
    }
}

/// The decoding state of one track's audio (specification 4, 4.6 and 4.7), or of any
/// frame range of an area ([`TrackAudioCursor::for_range`]), independent of the image it
/// reads from. [`TrackAudio`] is the borrowing iterator.
///
/// With the `parallel` feature, DST frames are decoded on a rayon thread pool ahead of
/// the caller, while the calling thread demultiplexes and consumes; memory stays bounded
/// (see [`MAX_PARALLEL_BATCH`]). The pool is the current one unless
/// [`TrackAudioCursor::set_decode_on`] says otherwise.
#[derive(Debug)]
pub struct TrackAudioCursor {
    frames: TrackCursor,
    /// Where DST frames are decoded; kept by continuations.
    #[cfg(feature = "parallel")]
    decode_on: DecodeOn,
    /// Decodes on the calling thread: always without `parallel`; with it, the positions
    /// that are not DST frames, and everything on a pool of one thread.
    decoder: DstDecoder,
    /// Positions demultiplexed but not yet returned, in timeline order: they come before
    /// `pending_error`.
    ahead: VecDeque<Pending>,
    /// Frames decoded on the calling thread together with the one returned before them,
    /// not yet returned: they come before `ahead`.
    ready: VecDeque<AudioFrame>,
    /// An error met while demultiplexing, reported after every frame before it.
    pending_error: Option<Error>,
    /// With [`TrackAudioCursor::keep_stored`], the stored content of each position
    /// demultiplexed and not yet returned, in timeline order: one per position of
    /// `ready` and `ahead`, in step with them.
    stored: Option<VecDeque<Stored>>,
    missing: u64,
    invalid: u64,
    /// Lines written about missing frames.
    missing_lines: LineCap,
    /// Lines written about invalid DST frames.
    invalid_lines: LineCap,
}

/// A position demultiplexed ahead of the caller.
#[derive(Debug)]
enum Pending {
    /// Decoded on the calling thread when it is returned.
    Here(TrackFrame),
    /// The `usize`th frame of a group of DST frames being decoded on the pool.
    #[cfg(feature = "parallel")]
    Pool(std::sync::Arc<pool::Job>, usize),
}

impl TrackAudioCursor {
    /// A cursor at the start of `track` of `area`, after the checks of 4.8.
    ///
    /// # Errors
    ///
    /// As [`check_track`].
    pub fn new(area: &Area, track: &Track, image_sectors: u64) -> Result<Self, Error> {
        Ok(Self::from_frames(TrackCursor::new(area, track, image_sectors)?))
    }

    /// A cursor at frame `first` of `area`, ending before frame `end` (see
    /// [`TrackCursor::for_range`]): the whole area is `[0, P)`, with `P` the play time.
    /// Frame numbering, fills, statuses and counts are exactly as for a track.
    ///
    /// # Errors
    ///
    /// As [`check_range`].
    pub fn for_range(area: &Area, first: u32, end: u32, image_sectors: u64) -> Result<Self, Error> {
        // Missing and invalid frames are counted per range (4.7.1 item 6).
        Ok(Self::from_frames(TrackCursor::for_range(area, first, end, image_sectors)?))
    }

    /// As [`TrackAudioCursor::for_range`], starting from `index` (see
    /// [`TrackCursor::for_range_indexed`]): the frames, statuses and counts are exactly
    /// those of [`TrackAudioCursor::for_range`].
    ///
    /// # Errors
    ///
    /// As [`TrackCursor::for_range_indexed`].
    pub fn for_range_indexed(
        area: &Area,
        first: u32,
        end: u32,
        image_sectors: u64,
        index: &AreaIndex,
    ) -> Result<Self, Error> {
        Ok(Self::from_frames(TrackCursor::for_range_indexed(area, first, end, image_sectors, index)?))
    }

    /// Where this cursor's scan started (see [`ResumedFrom`]).
    pub fn resumed_from(&self) -> ResumedFrom {
        self.frames.resumed_from()
    }

    /// As [`TrackAudioCursor::for_range`], allowing a provisional start (see
    /// [`TrackCursor::for_range_speculative`]).
    ///
    /// # Errors
    ///
    /// As [`TrackCursor::for_range_speculative`].
    pub fn for_range_speculative(
        area: &Area,
        first: u32,
        end: u32,
        image_sectors: u64,
        index: Option<&AreaIndex>,
        options: crate::seek::SpeculativeOptions,
    ) -> Result<Self, Error> {
        Ok(Self::from_frames(TrackCursor::for_range_speculative(
            area,
            first,
            end,
            image_sectors,
            index,
            options,
        )?))
    }

    /// The slot of this cursor's area.
    pub fn slot(&self) -> Slot {
        self.frames.slot()
    }

    /// Whether this cursor's positions are provisional (see [`SeekPolicy`]). Its missing
    /// and invalid counts are provisional as well (specification 4, 4.7.1 item 6).
    pub fn speculation(&self) -> Speculation {
        self.frames.speculation()
    }

    /// Where its malformed count and warnings begin (see [`AreaCursor::counts_from`]).
    pub fn counts_from(&self) -> Option<u64> {
        self.frames.counts_from()
    }

    /// The number of the provisional reading, as events name it ([`crate::SeekEvent`]);
    /// `None` for exact positions.
    pub fn speculation_id(&self) -> Option<u64> {
        self.frames.speculation_id()
    }

    /// A cursor at the start of `track`, continuing this one's reading state where that
    /// gives the same result (see [`TrackCursor::continue_to`]). Frames of this cursor
    /// not yet returned are dropped; the counts start again at 0. With `parallel`, DST
    /// frames of this cursor still queued on the pool are then not decoded (see
    /// [`MAX_PARALLEL_BATCH`] for the frames being decoded at that moment).
    ///
    /// # Errors
    ///
    /// As [`check_track`].
    pub fn continue_to(self, area: &Area, track: &Track, image_sectors: u64) -> Result<Self, Error> {
        check_track(area, track, image_sectors)?;
        Ok(self.continue_in(area, track.start_frame, track.end_frame(), (image_sectors, None), None))
    }

    /// A cursor for the frame range `[first, end)`, continuing this one's reading state
    /// where that gives the same result (see [`TrackCursor::continue_to_range`]). Tracks
    /// and ranges may follow each other in any mix. Frames of this cursor not yet
    /// returned are dropped; the counts start again at 0.
    ///
    /// # Errors
    ///
    /// As [`check_range`].
    pub fn continue_to_range(
        self,
        area: &Area,
        first: u32,
        end: u32,
        image_sectors: u64,
    ) -> Result<Self, Error> {
        check_range(area, first, end, image_sectors)?;
        Ok(self.continue_in(area, first, end, (image_sectors, None), None))
    }

    /// As [`TrackAudioCursor::continue_to_range`] for a range already checked (see
    /// [`TrackCursor::continue_in`]).
    pub(crate) fn continue_in(
        self,
        area: &Area,
        first: u32,
        end: u32,
        image: (u64, Option<u64>),
        index: Option<&AreaIndex>,
    ) -> Self {
        let opening = self.opening(area, first, image, index);
        self.open_as(area, (first, end), image, index, (opening, SeekPolicy::Exact))
    }

    /// How [`TrackAudioCursor::continue_in`] opens the range starting at `first`.
    pub(crate) fn opening(
        &self,
        area: &Area,
        first: u32,
        (image_sectors, image_id): (u64, Option<u64>),
        index: Option<&AreaIndex>,
    ) -> Opening {
        match self.pending_error {
            Some(_) if index.is_some() => Opening::FromIndex,
            Some(_) => Opening::Fresh,
            None => self.frames.opening(area, first, image_sectors, image_id, index),
        }
    }

    /// The range opened as `opening` says (see [`TrackCursor::open_as`]).
    pub(crate) fn open_as(
        self,
        area: &Area,
        range: (u32, u32),
        image: (u64, Option<u64>),
        index: Option<&AreaIndex>,
        opening: (Opening, SeekPolicy),
    ) -> Self {
        // Frames decoded or demultiplexed ahead of the caller came from the track cursor
        // already, so it stands after them and they lie inside this range: dropping them
        // loses nothing a later range can use (jobs still on the pool finish unread). An
        // error ends the reuse.
        let mut frames = self.frames;
        if self.pending_error.is_some() {
            frames.failed = true;
        }
        let mut next = Self::from_frames(frames.open_as(area, range, image, index, opening));
        #[cfg(feature = "parallel")]
        {
            next.decode_on = self.decode_on;
        }
        next.stored = self.stored.map(|_| VecDeque::new());
        next
    }

    pub(crate) fn from_frames(frames: TrackCursor) -> Self {
        TrackAudioCursor {
            frames,
            #[cfg(feature = "parallel")]
            decode_on: DecodeOn::default(),
            decoder: DstDecoder::new(),
            ahead: VecDeque::new(),
            ready: VecDeque::new(),
            pending_error: None,
            stored: None,
            missing: 0,
            invalid: 0,
            missing_lines: LineCap::default(),
            invalid_lines: LineCap::default(),
        }
    }

    /// The area's channel count `C` (1 to 6).
    pub fn channels(&self) -> u8 {
        self.frames.channels()
    }

    /// Frames returned so far that were missing (filled with 0x69).
    pub fn missing_frames(&self) -> u64 {
        self.missing
    }

    /// Frames returned so far that were invalid DST frames (filled with 0x55).
    pub fn invalid_dst_frames(&self) -> u64 {
        self.invalid
    }

    /// Number of malformed sectors processed so far by this cursor's scan of its area. The
    /// scan starts at the area's first audio sector, not at the track, and a continued
    /// cursor keeps counting from where the earlier tracks left off.
    /// Track cursors read ahead of the frames they return (a group of
    /// [`crate::dst::lanes`] frames, or with `parallel` up to [`MAX_PARALLEL_BATCH`]
    /// positions), so this may count sectors of frames not yet returned.
    pub fn malformed_sectors(&self) -> u64 {
        self.frames.malformed_sectors()
    }

    /// The `audio-sector-malformed` warnings of this cursor's scan of its area so far
    /// (see [`AreaCursor::warnings`]): per area, not per track, and each sector once.
    pub fn warnings(&self) -> &[Warning] {
        self.frames.warnings()
    }

    /// The progress of this cursor's reading, readable from other threads (see
    /// [`Progress`]): the sectors read since it was made or continued.
    pub fn progress(&mut self) -> Progress {
        self.frames.progress()
    }

    /// Where DST frames are decoded (see [`DecodeOn`]); continuations keep it.
    #[cfg(feature = "parallel")]
    pub fn decode_on(&self) -> &DecodeOn {
        &self.decode_on
    }

    /// Chooses where the DST frames demultiplexed from now on are decoded (see
    /// [`DecodeOn`]); continuations ([`TrackAudioCursor::continue_to`] and the like) keep
    /// the choice. Frames already sent to a pool finish there and are returned in order,
    /// so the frames returned are the same whenever this is called.
    #[cfg(feature = "parallel")]
    pub fn set_decode_on(&mut self, on: DecodeOn) {
        self.decode_on = on;
    }

    /// Most positions held ahead of the caller, the one being returned included: one group
    /// of frames decoded together on the calling thread (see [`MAX_PARALLEL_BATCH`]) or,
    /// with `parallel` on a pool of more than one thread, many.
    fn window(&self, channels: u8) -> usize {
        let group = crate::dst::lanes(channels);
        #[cfg(feature = "parallel")]
        {
            let threads = match &self.decode_on {
                DecodeOn::CurrentPool => rayon::current_num_threads(),
                DecodeOn::Pool(pool) => pool.current_num_threads(),
                DecodeOn::CallingThread => 1,
            };
            match threads {
                0 | 1 => group,
                n => (PARALLEL_FRAMES_PER_THREAD * n).min(MAX_PARALLEL_BATCH),
            }
        }
        #[cfg(not(feature = "parallel"))]
        {
            group
        }
    }

    /// Demultiplexes positions, a whole number of groups of `lanes` of them, when a group
    /// fits in the window, until the track ends or an error is met (kept for after the
    /// positions before it). With `parallel` on a pool of more than one thread, each group
    /// of consecutive DST frames goes to the pool at once.
    ///
    /// The image reader stays on the calling thread: only decoding jobs, which own their
    /// frames, go to the pool. Returns whether it stopped at a cancellation request, which
    /// leaves the positions demultiplexed so far in the window and no error pending.
    fn fill<R: Read + Seek>(&mut self, image: &mut Image<R>) -> bool {
        let channels = self.channels();
        let window = self.window(channels);
        let group = crate::dst::lanes(channels);
        let held = self.ahead.len() + self.ready.len();
        if self.pending_error.is_some() || (held > 0 && held + group > window) {
            return false;
        }
        let mut room = (window - held) / group * group;
        #[cfg(feature = "parallel")]
        let mut batch: Vec<TrackFrame> = Vec::with_capacity(group);
        let mut cancelled = false;
        while self.pending_error.is_none() && room > 0 {
            let next = self.frames.next_frame(image);
            if let (Some(stored), Some(Ok(f))) = (self.stored.as_mut(), &next) {
                stored.push_back(match &f.content {
                    // A copy: the frame itself goes on to its decoding, wherever that runs.
                    FrameContent::Dst(bytes) => Stored::Dst(bytes.clone()),
                    // Returned unchanged as the decoded data: nothing to copy.
                    FrameContent::Dsd(_) => Stored::Plain,
                    FrameContent::Missing => Stored::Missing,
                });
            }
            match next {
                #[cfg(feature = "parallel")]
                Some(Ok(f @ TrackFrame { content: FrameContent::Dst(_), .. })) if window > group => {
                    batch.push(f);
                    if batch.len() == group {
                        self.spawn_batch(&mut batch, channels, image.log());
                    }
                }
                Some(Ok(f)) => {
                    #[cfg(feature = "parallel")]
                    self.spawn_batch(&mut batch, channels, image.log());
                    self.ahead.push_back(Pending::Here(f));
                }
                Some(Err(Error::Cancelled)) => {
                    cancelled = true;
                    break;
                }
                Some(Err(e)) => {
                    #[cfg(feature = "parallel")]
                    self.spawn_batch(&mut batch, channels, image.log());
                    self.pending_error = Some(e);
                }
                None => break,
            }
            room -= 1;
        }
        #[cfg(feature = "parallel")]
        self.spawn_batch(&mut batch, channels, image.log());
        debug_assert!(self.stored_in_step(), "stored content out of step after fill");
        cancelled
    }

    /// Sends the DST frames of `batch` (consecutive positions) to the cursor's pool as one
    /// job, which reports to `log`.
    #[cfg(feature = "parallel")]
    fn spawn_batch(&mut self, batch: &mut Vec<TrackFrame>, channels: u8, log: &Log) {
        if batch.is_empty() {
            return;
        }
        let n = batch.len();
        let pool = match &self.decode_on {
            DecodeOn::Pool(pool) => Some(&**pool),
            DecodeOn::CurrentPool | DecodeOn::CallingThread => None,
        };
        let job = pool::Job::spawn(std::mem::take(batch), channels, log.clone(), pool);
        self.ahead.extend((0..n).map(|i| Pending::Pool(std::sync::Arc::clone(&job), i)));
    }

    /// The next decoded frame, `None` after the track's (or range's) last frame.
    ///
    /// With `parallel`, while the next frame is still being decoded on a pool, a calling
    /// thread that belongs to a pool runs that pool's pending jobs meanwhile; otherwise,
    /// or when there are none, it blocks until the frame is decoded. A panic while decoding
    /// on the pool is resumed on the calling thread.
    ///
    /// While cancellation is requested ([`crate::CancelToken`]) it returns
    /// [`Error::Cancelled`] and keeps every frame for later.
    ///
    /// With [`TrackAudioCursor::keep_stored`], the position's stored content is dropped;
    /// [`TrackAudioCursor::next_frame_with_stored`] returns it too.
    pub fn next_frame<R: Read + Seek>(&mut self, image: &mut Image<R>) -> Option<Result<AudioFrame, Error>> {
        let frame = self.next_decoded(image);
        if let (Some(Ok(_)), Some(stored)) = (&frame, self.stored.as_mut()) {
            let popped = stored.pop_front();
            debug_assert!(popped.is_some() && self.stored_in_step(), "stored content out of step");
        }
        frame
    }

    /// Whether the stored content kept (if any) is one per position held ahead: each
    /// position `fill` takes pushes one, and each position returned pops one.
    fn stored_in_step(&self) -> bool {
        self.stored.as_ref().is_none_or(|s| s.len() == self.ready.len() + self.ahead.len())
    }

    /// Keeps, from now on, each position's stored content next to its decoding, for
    /// [`TrackAudioCursor::next_frame_with_stored`]: from one read of the area's sectors,
    /// both what [`TrackCursor::next_frame`] gives (as stored, DST frames undecoded) and
    /// what [`TrackAudioCursor::next_frame`] gives, decoded where the cursor decodes
    /// ([`TrackAudioCursor::set_decode_on`]) with the same positions held ahead. A DST
    /// frame's bytes are copied (at most [`MAX_FRAME_BYTES`]) before it goes to its
    /// decoding. Continuations keep the choice.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`] once positions are held ahead of the caller (after the
    /// first read, until the cursor is continued): their content was not kept.
    pub fn keep_stored(&mut self) -> Result<(), Error> {
        if self.stored.is_some() {
            return Ok(());
        }
        if !(self.ready.is_empty() && self.ahead.is_empty()) {
            return Err(Error::InvalidArgument("stored content is kept only from before the first read"));
        }
        self.stored = Some(VecDeque::new());
        Ok(())
    }

    /// Whether the cursor keeps stored content ([`TrackAudioCursor::keep_stored`]).
    pub fn keeps_stored(&self) -> bool {
        self.stored.is_some()
    }

    /// The next position, decoded as [`TrackAudioCursor::next_frame`] gives it, with its
    /// stored content as [`TrackCursor::next_frame`] gives it (for the same frame number;
    /// see [`Stored`]: a plain frame's bytes are the decoded data, not copied again).
    /// [`Stored::content`] gives the [`FrameContent`], [`TrackAudioCursor::dst_equivalent`]
    /// its DST equivalent. The two calls may be mixed.
    ///
    /// # Errors
    ///
    /// As [`TrackAudioCursor::next_frame`], and [`Error::InvalidArgument`] if the cursor
    /// does not keep stored content ([`TrackAudioCursor::keep_stored`]).
    pub fn next_frame_with_stored<R: Read + Seek>(
        &mut self,
        image: &mut Image<R>,
    ) -> Option<Result<(AudioFrame, Stored), Error>> {
        if self.stored.is_none() {
            return Some(Err(Error::InvalidArgument("the cursor does not keep stored content")));
        }
        let frame = match self.next_decoded(image)? {
            Ok(f) => f,
            Err(e) => return Some(Err(e)),
        };
        // One content per position returned, in the same order (see `fill`).
        let popped = self.stored.as_mut().and_then(VecDeque::pop_front);
        debug_assert!(popped.is_some() && self.stored_in_step(), "stored content out of step");
        Some(match popped {
            Some(stored) => Ok((frame, stored)),
            // Never reached: a bug, reported rather than filled in.
            None => Err(Error::Io(std::io::Error::other("internal error: stored content out of step"))),
        })
    }

    /// The DST equivalent of a position of this cursor's area (specification 4, 4.6.1
    /// item 3; see [`FrameContent::dst_equivalent`]), with the area's channel count.
    pub fn dst_equivalent<'c>(&self, content: &'c FrameContent) -> std::borrow::Cow<'c, [u8]> {
        self.frames.dst_equivalent(content)
    }

    /// [`TrackAudioCursor::next_frame`], leaving the stored content of the position (if
    /// kept) to the caller.
    fn next_decoded<R: Read + Seek>(&mut self, image: &mut Image<R>) -> Option<Result<AudioFrame, Error>> {
        if let Err(e) = image.check_cancel() {
            return Some(Err(e));
        }
        let mut frame = if let Some(f) = self.ready.pop_front() {
            f
        } else {
            if self.fill(image) {
                return Some(Err(Error::Cancelled));
            }
            match self.ahead.pop_front() {
                Some(Pending::Here(f)) => {
                    let channels = self.channels();
                    if matches!(f.content, FrameContent::Dst(_)) {
                        // This frame and the DST frames waiting behind it, together.
                        let mut run = vec![f];
                        while run.len() < crate::dst::lanes(channels) {
                            match self.ahead.front() {
                                Some(Pending::Here(TrackFrame { content: FrameContent::Dst(_), .. })) => {
                                    if let Some(Pending::Here(g)) = self.ahead.pop_front() {
                                        run.push(g);
                                    }
                                }
                                _ => break,
                            }
                        }
                        let mut decoded =
                            decode_dst_run(&mut self.decoder, run, channels, image.log()).into_iter();
                        let first = decoded.next()?;
                        self.ready.extend(decoded);
                        first
                    } else {
                        decode_checked(&mut self.decoder, f, channels)
                    }
                }
                #[cfg(feature = "parallel")]
                Some(Pending::Pool(job, i)) => job.wait(i),
                None => return self.pending_error.take().map(Err),
            }
        };
        // Logged and counted as frames are returned, so the counts never run ahead of the
        // caller, whatever the window, and frame problems are logged in timeline order.
        let log = image.log();
        let level = match frame.status {
            FrameStatus::Missing | FrameStatus::Invalid(_) => Level::Warn,
            FrameStatus::Dsd | FrameStatus::Decoded => Level::Trace,
        };
        if log.enabled(level) {
            // The frame stays in the cursor while the sink runs: if the sink panics, the
            // next call returns (and logs) it again, and nothing is counted twice.
            let (number, status, slot) = (frame.number, frame.status, self.frames.area.slot);
            self.ready.push_front(frame);
            match status {
                FrameStatus::Missing => {
                    if self.missing_lines.admit(log, level, slot.name(), "missing-frame") {
                        sb_log!(log, Warn, "area {slot} frame {number}: missing, filled with 0x69 (4.6)");
                    }
                }
                FrameStatus::Invalid(e) => {
                    if self.invalid_lines.admit(log, level, slot.name(), "invalid-DST-frame") {
                        sb_log!(
                            log,
                            Warn,
                            "area {slot} frame {number}: invalid DST frame ({e}, rule {} of 5.6), filled with 0x55 \
                             (4.6)",
                            e.rule()
                        );
                    }
                }
                FrameStatus::Dsd | FrameStatus::Decoded => {
                    sb_log!(log, Trace, "area {slot} frame {number}: {status:?}");
                }
            }
            frame = self.ready.pop_front()?;
        }
        match frame.status {
            FrameStatus::Missing => self.missing += 1,
            FrameStatus::Invalid(_) => self.invalid += 1,
            FrameStatus::Dsd | FrameStatus::Decoded => {}
        }
        Some(Ok(frame))
    }
}

/// A cursor whose recorded checkpoints [`AreaIndex::absorb`] takes: [`AreaCursor`],
/// [`TrackCursor`] or [`TrackAudioCursor`]. Sealed: implemented only by this crate.
pub trait SeekCursor: sealed::Sealed {}

pub(crate) mod sealed {
    /// The area cursor inside a [`super::SeekCursor`].
    pub trait Sealed {
        /// The area cursor.
        fn area_cursor(&self) -> &super::AreaCursor;
        /// The area cursor, to take its checkpoints.
        fn area_cursor_mut(&mut self) -> &mut super::AreaCursor;
    }
}

impl SeekCursor for AreaCursor {}
impl SeekCursor for TrackCursor {}
impl SeekCursor for TrackAudioCursor {}

impl sealed::Sealed for AreaCursor {
    fn area_cursor(&self) -> &AreaCursor {
        self
    }

    fn area_cursor_mut(&mut self) -> &mut AreaCursor {
        self
    }
}

impl sealed::Sealed for TrackCursor {
    fn area_cursor(&self) -> &AreaCursor {
        &self.area
    }

    fn area_cursor_mut(&mut self) -> &mut AreaCursor {
        &mut self.area
    }
}

impl sealed::Sealed for TrackAudioCursor {
    fn area_cursor(&self) -> &AreaCursor {
        &self.frames.area
    }

    fn area_cursor_mut(&mut self) -> &mut AreaCursor {
        &mut self.frames.area
    }
}

/// Decoding jobs on rayon's pool, for the `parallel` feature.
#[cfg(feature = "parallel")]
mod pool {
    use std::any::Any;
    use std::cell::RefCell;
    use std::panic::AssertUnwindSafe;
    use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

    use super::{AudioFrame, TrackFrame, decode_dst_run};
    use crate::dst::DstDecoder;
    use crate::log::{Log, sb_log};

    thread_local! {
        /// Each pool thread's decoder, reused from frame to frame.
        static DECODER: RefCell<DstDecoder> = RefCell::new(DstDecoder::new());
    }

    /// A group of consecutive DST frames decoded on the pool; each frame is taken once.
    #[derive(Debug, Default)]
    pub(super) struct Job {
        state: Mutex<State>,
        done: Condvar,
    }

    /// The payload of a panic, until it is resumed.
    type Panic = Option<Box<dyn Any + Send>>;

    #[derive(Debug, Default)]
    struct State {
        /// The decoded frames, or the panic that stopped their decoding (`None` once
        /// resumed on a thread).
        result: Option<Result<Vec<Option<AudioFrame>>, Panic>>,
        /// Whether the owner sleeps on `done`: only then does the job wake it.
        waiting: bool,
    }

    impl Job {
        fn lock(&self) -> MutexGuard<'_, State> {
            // A panic never happens while the lock is held: the state stays consistent.
            self.state.lock().unwrap_or_else(PoisonError::into_inner)
        }

        /// Starts decoding `frames` (DST frames) on `pool`, or the current pool if `None`,
        /// reporting to `log` from the pool's thread.
        pub(super) fn spawn(
            frames: Vec<TrackFrame>,
            channels: u8,
            log: Log,
            pool: Option<&rayon::ThreadPool>,
        ) -> Arc<Job> {
            let job = Arc::new(Job::default());
            let out = Arc::clone(&job);
            let work = move || {
                // Besides `out` here, the only handles are the owner's, one per frame of the
                // group in its window. A count of 1 means it dropped them all (a cursor
                // continued or dropped with frames in flight): nobody will take these
                // frames, so skip decoding them. The count cannot rise again; any new
                // handle to a job breaks this test. Jobs already decoding finish; see
                // MAX_PARALLEL_BATCH.
                if Arc::strong_count(&out) == 1 {
                    return;
                }
                let range = (frames.first().map(|f| f.number), frames.last().map(|f| f.number));
                let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    DECODER.with(|d| match d.try_borrow_mut() {
                        Ok(mut d) => decode_dst_run(&mut d, frames, channels, &log),
                        // Reached only if this thread's decoder is in use further up the stack:
                        // a log sink called while it is borrowed ran other pool jobs (for
                        // example by decoding on this pool itself, which waits in
                        // rayon::yield_now). Stay correct with a fresh decoder.
                        Err(_) => decode_dst_run(&mut DstDecoder::new(), frames, channels, &log),
                    })
                }));
                if result.is_err() {
                    // A sink that panics again here is caught as well, and the job's panic
                    // stays the one resumed.
                    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
                        sb_log!(
                            &log,
                            Error,
                            "panic while decoding DST frames {:?}..={:?} on a pool thread; resumed on the caller",
                            range.0,
                            range.1
                        );
                    }));
                    // Start afresh rather than reuse a decoder a panic left behind.
                    DECODER.with(|d| {
                        if let Ok(mut d) = d.try_borrow_mut() {
                            *d = DstDecoder::new();
                        }
                    });
                }
                let result = result.map(|v| v.into_iter().map(Some).collect()).map_err(Some);
                let mut state = out.lock();
                state.result = Some(result);
                if state.waiting {
                    out.done.notify_one();
                }
            };
            match pool {
                Some(pool) => pool.spawn(work),
                None => rayon::spawn(work),
            }
            job
        }

        /// Frame `i` of the group, decoded: runs other pool jobs while it is not ready, or
        /// blocks when there are none (every job then runs on some thread, and finishes) or
        /// this thread is not one of the pool's.
        pub(super) fn wait(&self, i: usize) -> AudioFrame {
            loop {
                if let Some(f) = Self::take(&mut self.lock(), i) {
                    return f;
                }
                if rayon::yield_now() != Some(rayon::Yield::Executed) {
                    let mut state = self.lock();
                    state.waiting = true;
                    loop {
                        if let Some(f) = Self::take(&mut state, i) {
                            return f;
                        }
                        state = self.done.wait(state).unwrap_or_else(PoisonError::into_inner);
                    }
                }
            }
        }

        /// Frame `i`, if the group is decoded; resumes the panic of a group that failed
        /// (for later frames of it, a new one).
        fn take(state: &mut State, i: usize) -> Option<AudioFrame> {
            match state.result.as_mut()? {
                Ok(frames) => frames.get_mut(i).and_then(Option::take),
                Err(panic) => std::panic::resume_unwind(
                    panic.take().unwrap_or_else(|| Box::new("a group of DST frames panicked while decoding")),
                ),
            }
        }
    }
}

/// The decoded audio of one track or frame range, frame by frame (specification 4, 4.6
/// and 4.7).
#[derive(Debug)]
pub struct TrackAudio<'a, R> {
    image: &'a mut Image<R>,
    cursor: TrackAudioCursor,
    /// The disc's seek indexes, which take the checkpoints the cursor records.
    seek: Option<&'a SeekIndexes>,
}

impl<'a, R: Read + Seek> TrackAudio<'a, R> {
    pub(crate) fn with_cursor(
        image: &'a mut Image<R>,
        cursor: TrackAudioCursor,
        seek: Option<&'a SeekIndexes>,
    ) -> Self {
        TrackAudio { image, cursor, seek }
    }

    /// The decoding state, to continue into a later track or range with
    /// [`crate::Disc::continue_track_audio`] or [`crate::Disc::continue_range_audio`].
    pub fn into_cursor(self) -> TrackAudioCursor {
        self.cursor
    }

    /// The area's channel count `C` (1 to 6).
    pub fn channels(&self) -> u8 {
        self.cursor.channels()
    }

    /// Frames returned so far that were missing (filled with 0x69).
    pub fn missing_frames(&self) -> u64 {
        self.cursor.missing_frames()
    }

    /// Frames returned so far that were invalid DST frames (filled with 0x55).
    pub fn invalid_dst_frames(&self) -> u64 {
        self.cursor.invalid_dst_frames()
    }

    /// Number of malformed sectors processed so far by this cursor's scan of its area. The
    /// scan starts at the area's first audio sector, not at the track, and a continued
    /// cursor keeps counting from where the earlier tracks left off.
    /// Track cursors read ahead of the frames they return (a group of
    /// [`crate::dst::lanes`] frames, or with `parallel` up to [`MAX_PARALLEL_BATCH`]
    /// positions), so this may count sectors of frames not yet returned.
    pub fn malformed_sectors(&self) -> u64 {
        self.cursor.malformed_sectors()
    }

    /// The `audio-sector-malformed` warnings of this cursor's scan of its area so far
    /// (see [`AreaCursor::warnings`]): per area, not per track, and each sector once.
    pub fn warnings(&self) -> &[Warning] {
        self.cursor.warnings()
    }

    /// Whether the positions are provisional (see [`SeekPolicy`]).
    pub fn speculation(&self) -> Speculation {
        self.cursor.speculation()
    }

    /// Where the malformed count and warnings begin (see [`AreaCursor::counts_from`]).
    pub fn counts_from(&self) -> Option<u64> {
        self.cursor.counts_from()
    }

    /// The number of the provisional reading, as events name it ([`crate::SeekEvent`]);
    /// `None` for exact positions.
    pub fn speculation_id(&self) -> Option<u64> {
        self.cursor.speculation_id()
    }

    /// The progress of the reading, readable from other threads (see [`Progress`]).
    pub fn progress(&mut self) -> Progress {
        self.cursor.progress()
    }
}

impl<R: Read + Seek> TrackAudio<'_, R> {
    /// Keeps each position's stored content from now on (see
    /// [`TrackAudioCursor::keep_stored`]), for [`TrackAudio::next_with_stored`].
    ///
    /// # Errors
    ///
    /// As [`TrackAudioCursor::keep_stored`].
    pub fn keep_stored(&mut self) -> Result<(), Error> {
        self.cursor.keep_stored()
    }

    /// The next position decoded, with its stored content (see
    /// [`TrackAudioCursor::next_frame_with_stored`]).
    pub fn next_with_stored(&mut self) -> Option<Result<(AudioFrame, Stored), Error>> {
        let frame = self.cursor.next_frame_with_stored(self.image);
        if let Some(seek) = self.seek {
            seek.absorb(&mut self.cursor.frames.area, self.image.id());
        }
        frame
    }
}

impl<R: Read + Seek> Iterator for TrackAudio<'_, R> {
    type Item = Result<AudioFrame, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let frame = self.cursor.next_frame(self.image);
        if let Some(seek) = self.seek {
            seek.absorb(&mut self.cursor.frames.area, self.image.id());
        }
        frame
    }
}

/// A frame of `channels` channels filled with one byte value.
pub fn fill_frame(channels: u8, value: u8) -> Vec<u8> {
    vec![value; usize::from(channels) * FRAME_BYTES_PER_CHANNEL]
}

/// Copies channel `channel` of an interleaved frame into `out` (4704 bytes).
///
/// # Errors
///
/// [`Error::InvalidArgument`] if the sizes do not match.
pub fn deinterleave(frame: &[u8], channels: u8, channel: u8, out: &mut [u8]) -> Result<(), Error> {
    let c = usize::from(channels);
    if c == 0
        || channel >= channels
        || frame.len() != c * FRAME_BYTES_PER_CHANNEL
        || out.len() != FRAME_BYTES_PER_CHANNEL
    {
        return Err(Error::InvalidArgument("frame, channel or output size mismatch"));
    }
    for (o, chunk) in out.iter_mut().zip(frame.chunks_exact(c)) {
        *o = chunk.get(usize::from(channel)).copied().unwrap_or(0);
    }
    Ok(())
}

/// The track a slot and number name, with its area.
pub(crate) fn find_track(areas: &[Area], slot: Slot, number: u32) -> Result<(&Area, &Track), Error> {
    let area = areas.iter().find(|a| a.slot == slot).ok_or(Error::NoSuchArea(slot))?;
    let track = area.track(number).ok_or(Error::NoSuchTrack { slot, track: number })?;
    Ok((area, track))
}

/// Areas for the unit tests of other modules.
#[cfg(test)]
pub(crate) mod tests_support {
    /// An area of `sectors` malformed audio sectors at LSN 0, one track over all its frames.
    pub(crate) fn area(sectors: u32) -> (crate::Image<std::io::Cursor<Vec<u8>>>, crate::Area, crate::Track) {
        super::tests::malformed_area(sectors, sectors)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn sector(header: u8, packets: &[(bool, u8, u16)], records: &[[u8; 4]], dst: bool) -> [u8; 2048] {
        let mut s = [0u8; 2048];
        s[0] = header;
        let mut at = 1;
        for &(start, ty, len) in packets {
            let w = (u16::from(start) << 15) | (u16::from(ty) << 11) | len;
            s[at..at + 2].copy_from_slice(&w.to_be_bytes());
            at += 2;
        }
        for r in records {
            let n = if dst { 4 } else { 3 };
            s[at..at + n].copy_from_slice(&r[..n]);
            at += n;
        }
        s
    }

    #[test]
    fn dst_frame_across_two_sectors() {
        let mut d = Demuxer::new(2);
        // Sector 1: DST flag, 1 frame record, 1 audio packet (start) of 100 bytes.
        let s1 = sector(1 | (1 << 2) | (1 << 5), &[(true, 2, 100)], &[[0, 0, 5, 2 << 2]], true);
        // Sector 2: 1 audio packet continuing the frame.
        let s2 = sector(1 | (1 << 5), &[(false, 2, 50)], &[], true);
        assert!(!d.push_sector(&s1));
        assert!(!d.push_sector(&s2));
        assert!(d.pop().is_none());
        d.finish();
        let f = d.pop().unwrap();
        assert_eq!((f.number, f.kind, f.data.len()), (5, FrameKind::Dst, 150));
    }

    #[test]
    fn overrun_and_backwards_timecodes() {
        let mut d = Demuxer::new(2);
        let over =
            sector(1 | (1 << 2) | (2 << 5), &[(true, 2, 2000), (false, 2, 100)], &[[0, 0, 1, 1 << 2]], true);
        assert!(d.push_sector(&over));
        // The first packet was appended before the overrun stopped the sector; its
        // remaining count reached 0, so the frame is complete and is delivered at the end.
        d.finish();
        assert_eq!(d.pop().map(|f| f.number), Some(1));
        let mut d = Demuxer::new(2);
        let a = sector(1 | (1 << 2) | (1 << 5), &[(true, 2, 10)], &[[0, 0, 9, 1 << 2]], true);
        let b = sector(1 | (1 << 2) | (1 << 5), &[(true, 2, 10)], &[[0, 0, 3, 1 << 2]], true);
        d.push_sector(&a);
        d.push_sector(&b);
        d.finish();
        assert_eq!(d.pop().map(|f| f.number), Some(9));
        assert!(d.pop().is_none());
    }

    #[test]
    fn start_without_record_is_malformed() {
        let mut d = Demuxer::new(2);
        let s = sector(1 << 5, &[(true, 2, 10)], &[], false);
        assert!(d.push_sector(&s));
        d.finish();
        assert!(d.pop().is_none());
    }

    #[cfg(feature = "parallel")]
    #[test]
    #[cfg_attr(miri, ignore = "starts 67 threads")]
    fn parallel_window_is_capped() {
        let (_, area, track) = malformed_area(1, 1);
        let mut cursor = TrackAudioCursor::new(&area, &track, 1).unwrap();
        let pool = |n| std::sync::Arc::new(rayon::ThreadPoolBuilder::new().num_threads(n).build().unwrap());
        let (wide, narrow, single) = (pool(64), pool(2), pool(1));
        wide.install(|| assert_eq!(cursor.window(2), MAX_PARALLEL_BATCH));
        narrow.install(|| assert_eq!(cursor.window(2), 2 * PARALLEL_FRAMES_PER_THREAD));
        single.install(|| assert_eq!(cursor.window(2), crate::dst::lanes(2)));
        // A pool chosen for the cursor counts whatever pool the caller runs on.
        cursor.set_decode_on(DecodeOn::Pool(std::sync::Arc::clone(&narrow)));
        wide.install(|| assert_eq!(cursor.window(2), 2 * PARALLEL_FRAMES_PER_THREAD));
        assert_eq!(cursor.window(2), 2 * PARALLEL_FRAMES_PER_THREAD);
        cursor.set_decode_on(DecodeOn::Pool(single));
        wide.install(|| assert_eq!(cursor.window(2), crate::dst::lanes(2)));
        cursor.set_decode_on(DecodeOn::CallingThread);
        wide.install(|| assert_eq!(cursor.window(2), crate::dst::lanes(2)));
        // Continuations keep the choice.
        let next = cursor.continue_to(&area, &track, 1).unwrap();
        assert!(matches!(next.decode_on(), DecodeOn::CallingThread));
        // The bound documented on MAX_PARALLEL_BATCH: each position held ahead is at most
        // one frame of DST bytes and one of decoded audio, about 48 MB in all.
        let bound = MAX_PARALLEL_BATCH * (MAX_FRAME_BYTES + 6 * FRAME_BYTES_PER_CHANNEL);
        assert!(bound <= 48_100_000, "{bound}");
    }

    /// An area of `sectors` malformed audio sectors (a packet that overruns each one) at
    /// LSN 0, with one track of `frames` frames, all missing.
    pub(crate) fn malformed_area(
        sectors: u32,
        frames: u32,
    ) -> (Image<std::io::Cursor<Vec<u8>>>, Area, Track) {
        // One packet (k = 1), audio, 2047 bytes: past the end of the sector.
        area_of(sectors, frames, |_, s| s[..3].copy_from_slice(&[1 << 5, 0x17, 0xFF]))
    }

    /// An area of `sectors` well-formed audio sectors at LSN 0, each starting plain frame
    /// `i` with 10 bytes only, so that every frame is discarded as incomplete; one track
    /// of `frames` frames, all missing.
    fn incomplete_frames_area(sectors: u32, frames: u32) -> (Image<std::io::Cursor<Vec<u8>>>, Area, Track) {
        area_of(sectors, frames, |i, s| {
            // k = 1 packet, f = 1 frame record, plain DSD; a start audio packet of 10 bytes;
            // the record's timecode is frame i.
            let (m, sec, f) = ((i / 4500) as u8, (i / 75 % 60) as u8, (i % 75) as u8);
            s[..6].copy_from_slice(&[(1 << 5) | (1 << 2), 0x90, 10, m, sec, f]);
        })
    }

    /// An area of `sectors` sectors at LSN 0, sector `i` written by `fill`, with one track
    /// of `frames` frames.
    fn area_of(
        sectors: u32,
        frames: u32,
        fill: impl Fn(usize, &mut [u8]),
    ) -> (Image<std::io::Cursor<Vec<u8>>>, Area, Track) {
        use crate::toc::{AreaKind, AreaText, Timecode, TrackText, Version};
        let mut bytes = vec![0u8; sectors as usize * SECTOR_SIZE];
        for (i, s) in bytes.chunks_exact_mut(SECTOR_SIZE).enumerate() {
            fill(i, s);
        }
        let image = Image::with_format(std::io::Cursor::new(bytes), crate::SectorFormat::Plain).unwrap();
        let playtime = Timecode {
            minutes: (frames / 4500) as u8,
            seconds: (frames / 75 % 60) as u8,
            frames: (frames % 75) as u8,
        };
        let track = Track {
            number: 1,
            start_lsn: 0,
            length_sectors: sectors,
            start_frame: 0,
            duration_frames: frames,
            start_flags: 0,
            duration_flags: 0,
            isrc: None,
            genre: None,
            text: TrackText::default(),
        };
        let area = Area {
            slot: Slot::TwoChannel,
            kind: AreaKind::TwoChannel,
            version: Version { major: 1, minor: 20 },
            toc_lsn: 0,
            toc_sectors: 0,
            max_byte_rate: 0,
            sample_frequency_code: 4,
            frame_format: 0,
            channel_count: 2,
            area_config: 0,
            max_available_channels: 2,
            area_mute_flags: 0,
            track_attribute: 0,
            playtime,
            track_offset: 0,
            audio_start_lsn: 0,
            audio_end_lsn: sectors - 1,
            text_channel_count: 0,
            locales: Vec::new(),
            text_charset: 0,
            text: AreaText::default(),
            tracks: vec![track.clone()],
        };
        (image, area, track)
    }

    type Lines = std::sync::Arc<std::sync::Mutex<Vec<String>>>;

    fn recorder(level: Level) -> (Log, Lines) {
        let lines = Lines::default();
        let sink = std::sync::Arc::clone(&lines);
        (Log::new(level, move |_, m| sink.lock().unwrap().push(m.to_string())), lines)
    }

    #[test]
    #[cfg_attr(miri, ignore = "decodes thousands of frames")]
    fn problem_lines_are_capped() {
        let n = MAX_PROBLEM_LINES + 100;
        let (mut image, area, track) = malformed_area(n as u32, n as u32);
        let (log, lines) = recorder(Level::Warn);
        image.set_log(log);
        let mut cursor = TrackAudioCursor::new(&area, &track, image.sector_count()).unwrap();
        let mut frames = 0;
        while let Some(f) = cursor.next_frame(&mut image) {
            assert_eq!(f.unwrap().status, FrameStatus::Missing);
            frames += 1;
        }
        assert_eq!((frames, cursor.missing_frames()), (n, n as u64));
        assert_eq!(cursor.malformed_sectors(), n as u64);
        let lines = lines.lock().unwrap();
        let malformed: Vec<_> = lines.iter().filter(|m| m.contains(": audio-sector-malformed: ")).collect();
        let missing = lines.iter().filter(|m| m.contains(": missing, filled")).count();
        assert_eq!((malformed.len(), missing), (MAX_PROBLEM_LINES, MAX_PROBLEM_LINES));
        // The lines are those of the warnings kept, which are capped alike.
        let listed: Vec<_> = cursor
            .warnings()
            .iter()
            .map(|w| {
                format!(
                    "area 2ch audio sector LSN {}: audio-sector-malformed: packet 0 of 1 (2047 bytes at 3) overruns the sector",
                    w.lsn.unwrap()
                )
            })
            .collect();
        assert_eq!(malformed.into_iter().cloned().collect::<Vec<_>>(), listed);
        for what in ["audio-sector-malformed", "missing-frame"] {
            let suppressed =
                format!("area 2ch: further {what} lines suppressed (at most {MAX_PROBLEM_LINES} per cursor)");
            assert_eq!(lines.iter().filter(|m| **m == suppressed).count(), 1, "{what}");
        }

        // Frames the reassembler discards: capped alike, per area cursor.
        let (mut image, area, track) = incomplete_frames_area(n as u32, n as u32);
        let (log, lines) = recorder(Level::Warn);
        image.set_log(log);
        let mut cursor = TrackAudioCursor::new(&area, &track, image.sector_count()).unwrap();
        while let Some(f) = cursor.next_frame(&mut image) {
            assert_eq!(f.unwrap().status, FrameStatus::Missing);
        }
        assert_eq!(cursor.malformed_sectors(), 0);
        let lines = lines.lock().unwrap();
        let discarded: Vec<_> =
            lines.iter().filter(|m| m.contains(": discarded, incomplete (Dsd, 10 bytes")).collect();
        assert_eq!(discarded.len(), MAX_PROBLEM_LINES);
        assert_eq!(
            discarded[0],
            "area 2ch frame 0 (from LSN 0): discarded, incomplete (Dsd, 10 bytes; 4.4.2)"
        );
        let suppressed = format!(
            "area 2ch: further discarded-frame lines suppressed (at most {MAX_PROBLEM_LINES} per cursor)"
        );
        assert_eq!(lines.iter().filter(|m| **m == suppressed).count(), 1);
        // Its first line comes right after the last discarded frame admitted.
        let at = lines.iter().position(|m| *m == suppressed).unwrap();
        assert!(lines[..at].iter().filter(|m| m.contains(": discarded,")).count() == MAX_PROBLEM_LINES);
    }

    #[test]
    fn a_frame_survives_a_sink_that_panics() {
        let (mut image, area, track) = malformed_area(1, 3);
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = std::sync::Arc::clone(&calls);
        image.set_log(Log::new(Level::Warn, move |_, m| {
            if m.to_string().contains("frame 1: missing")
                && counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0
            {
                panic!("sink");
            }
        }));
        let mut cursor = TrackAudioCursor::new(&area, &track, image.sector_count()).unwrap();
        assert_eq!(cursor.next_frame(&mut image).unwrap().unwrap().number, 0);
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cursor.next_frame(&mut image)));
        assert!(caught.is_err());
        assert_eq!(cursor.missing_frames(), 1, "the frame whose line panicked is not counted");
        let numbers: Vec<_> =
            std::iter::from_fn(|| cursor.next_frame(&mut image)).map(|f| f.unwrap().number).collect();
        assert_eq!(numbers, [1, 2]);
        assert_eq!(cursor.missing_frames(), 3);
    }

    #[test]
    fn decode_frame_rejects_bad_arguments() {
        let mut d = DstDecoder::new();
        let missing = || TrackFrame { number: 0, content: FrameContent::Missing };
        for channels in [0, 7, 255] {
            assert!(matches!(decode_frame(&mut d, missing(), channels), Err(Error::InvalidArgument(_))));
        }
        let short = TrackFrame { number: 0, content: FrameContent::Dsd(vec![0; 10]) };
        assert!(matches!(decode_frame(&mut d, short, 2), Err(Error::InvalidArgument(_))));
        let ok = decode_frame(&mut d, missing(), 2).unwrap();
        assert_eq!((ok.status, ok.data.len()), (FrameStatus::Missing, 2 * 4704));
    }

    #[test]
    fn dst_equivalents() {
        let missing = FrameContent::Missing.dst_equivalent(3).unwrap();
        assert_eq!(missing.len(), 1 + 3 * FRAME_BYTES_PER_CHANNEL);
        assert_eq!(missing[0], 0);
        assert!(missing[1..].iter().all(|&b| b == MISSING_FILL));
        let plain = FrameContent::Dsd(vec![7; FRAME_BYTES_PER_CHANNEL]);
        let plain = plain.dst_equivalent(1).unwrap();
        assert_eq!((plain.len(), plain[0], plain[1]), (1 + FRAME_BYTES_PER_CHANNEL, 0, 7));
        let dst = FrameContent::Dst(vec![1, 2, 3]);
        assert_eq!(&*dst.dst_equivalent(6).unwrap(), &[1, 2, 3]);
        for channels in [0, 7, 255] {
            assert!(matches!(FrameContent::Missing.dst_equivalent(channels), Err(Error::InvalidArgument(_))));
        }
    }

    #[test]
    fn deinterleave_channel() {
        let frame: Vec<u8> = (0..2 * 4704).map(|i| (i % 2) as u8).collect();
        let mut out = vec![9u8; 4704];
        deinterleave(&frame, 2, 1, &mut out).unwrap();
        assert!(out.iter().all(|&b| b == 1));
        assert!(deinterleave(&frame, 2, 2, &mut out).is_err());
    }
}
