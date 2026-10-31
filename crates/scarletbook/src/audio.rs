// SPDX-License-Identifier: MIT OR Apache-2.0

//! Audio sector multiplexing (specification 4): frame reassembly, the timeline and tracks.

use std::collections::VecDeque;
use std::io::{Read, Seek};

use crate::dst::{DstDecoder, DstError};
use crate::error::{Error, Slot, Warning, WarningCode};
use crate::image::{Image, ReadOutcome, SECTOR_SIZE};
use crate::log::{Level, Log, sb_log};
use crate::seek::{AreaIndex, Assembling, Plan, Recorder, ResumedFrom, digest};
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
/// the frame under assembly); DST frames of 40 % of their decoded size need about 6.8 MB
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
        }
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
            Plan::At { lsn, checkpoint, held } => {
                // Specification 4, 4.4.5 item 4: a late reader MAY report the malformed
                // sectors an earlier pass found; this one reports them all, as a scan from
                // the first sector would.
                cursor.warnings = index.malformed_below(lsn).map(warn).collect();
                let replay_from = match held {
                    Some((h, bytes)) => {
                        cursor.demux.assembling = Some(Assembly {
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
                cursor.demux.next_acceptable = checkpoint.next_acceptable;
                cursor.demux.malformed = checkpoint.malformed_before;
                if replay_from < lsn {
                    // Lemma B1 of #68: from the first start packet the replay processes, its
                    // frame under assembly is the scan's; the frames it delivers are all
                    // numbered below the checkpoint's `a`, and are dropped.
                    cursor.replay = Some(Replay {
                        until: lsn,
                        next_acceptable: checkpoint.next_acceptable,
                        malformed: checkpoint.malformed_before,
                    });
                }
                cursor.next_lsn = replay_from;
                cursor.verify = Some((lsn, checkpoint.digest));
                cursor.recorder = index.recorder(lsn);
                cursor.resumed = ResumedFrom::Checkpoint { lsn, replay_from };
            }
        }
        cursor
    }

    /// Starts afresh at the area's first audio sector: the seek index this cursor started
    /// from does not hold sector `lsn` as it is in the image (a stale index, or one of
    /// another image of the same size). Nothing was returned from it yet: the sectors
    /// processed so far only rebuilt the frame under assembly.
    fn discard_index(&mut self, log: &Log, lsn: u64) {
        let (image_id, stop) = (self.image_id, self.stop_lsn);
        *self = Self::with_key(self.key);
        self.image_id = image_id;
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

    /// Number of malformed sectors processed so far by this cursor's scan of its area. The
    /// scan starts at the area's first audio sector, not at the track, and a continued
    /// cursor keeps counting from where the earlier tracks left off.
    /// Track cursors read ahead of the frames they return (a group of
    /// [`crate::dst::lanes`] frames, or with `parallel` up to [`MAX_PARALLEL_BATCH`]
    /// positions), so this may count sectors of frames not yet returned.
    pub fn malformed_sectors(&self) -> u64 {
        // A cursor started from a seek index holds the counts of the sectors before its
        // checkpoint, which a scan from the first sector reports only once it has read.
        if self.image_id.is_none() { 0 } else { self.demux.malformed_sectors() }
    }

    /// The `audio-sector-malformed` warnings of the sectors processed so far, with slot
    /// and LSN, in processing order (specification 4, 4.4.5). At most
    /// [`MAX_MALFORMED_WARNINGS`] are kept; [`AreaCursor::malformed_sectors`] counts them
    /// all. Like that count, they may include sectors of frames not yet returned.
    pub fn warnings(&self) -> &[Warning] {
        if self.image_id.is_none() { &[] } else { &self.warnings }
    }

    fn sector<R: Read + Seek>(&mut self, image: &mut Image<R>, lsn: u64) -> Result<[u8; SECTOR_SIZE], Error> {
        if !(self.buf_lsn..self.buf_lsn + self.buf_sectors).contains(&lsn) {
            let n = READ_CHUNK.min(self.end_lsn - lsn);
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
    /// The cursor binds to the first image it reads; with any other image it returns
    /// [`Error::InvalidArgument`] (and stays usable with its own image).
    pub fn next_frame<R: Read + Seek>(&mut self, image: &mut Image<R>) -> Option<Result<AreaFrame, Error>> {
        match self.image_id {
            None => {
                self.image_id = Some(image.id());
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
                            self.demux.ready.clear();
                            self.demux.next_acceptable = r.next_acceptable;
                            self.demux.malformed = r.malformed;
                            self.replay = None;
                        }
                        if let Some((at, want)) = self.verify.filter(|&(at, _)| at == lsn) {
                            self.verify = None;
                            if digest(&s) != want {
                                self.discard_index(image.log(), at);
                                continue;
                            }
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
                    Err(e) => {
                        self.finished = true;
                        return Some(Err(e));
                    }
                }
            } else {
                self.demux.finish_at(self.demux.at(image.log(), self.end_lsn));
                self.finished = true;
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
}

impl<'a> Continuation<'a> {
    pub(crate) fn new(
        area: &'a Area,
        (first, end): (u32, u32),
        image_sectors: u64,
        image_id: Option<u64>,
        log: &'a Log,
        track: Option<u32>,
    ) -> Self {
        Continuation { area, first, end, image_sectors, image_id, log, track }
    }

    /// Logs the opening, continuing the scan of a cursor or not.
    fn log_open(&self, what: &str, continues: impl FnOnce() -> bool) {
        if self.log.enabled(Level::Info) {
            let how = if continues() { "continuing the earlier scan" } else { "new scan of the area" };
            log_open(self.log, self.area, self.track, (self.first, self.end), what, how);
        }
    }

    /// The frame range, `[first, end)`.
    pub fn range(&self) -> (u32, u32) {
        (self.first, self.end)
    }

    /// `prev` continued into this range (see [`TrackAudioCursor::continue_to_range`]),
    /// or a fresh cursor when its state cannot be reused. Logs the opening first
    /// ([`Continuation::log_audio`]).
    pub fn audio(self, prev: TrackAudioCursor) -> TrackAudioCursor {
        self.log_audio(&prev);
        self.audio_unlogged(prev)
    }

    /// Logs (at [`Level::Info`]) the opening of this range's audio from `prev`: whether
    /// its scan continues. [`Continuation::audio`] does this itself.
    pub fn log_audio(&self, prev: &TrackAudioCursor) {
        self.log_open("decoded audio", || {
            prev.continues(self.area, self.first, self.image_sectors, self.image_id)
        });
    }

    /// [`Continuation::audio`] without logging: no log sink runs during the call. For a
    /// caller that must not run one while it hands `prev` over (the C interface, whose
    /// callbacks could otherwise reach a handle being consumed); it calls
    /// [`Continuation::log_audio`] first, while `prev` is still guarded.
    pub fn audio_unlogged(self, prev: TrackAudioCursor) -> TrackAudioCursor {
        prev.continue_in(self.area, self.first, self.end, self.image_sectors, self.image_id)
    }

    /// `prev` continued into this range (see [`TrackCursor::continue_to_range`]), or a
    /// fresh cursor when its state cannot be reused. Logs the opening first
    /// ([`Continuation::log_frames`]).
    pub fn frames(self, prev: TrackCursor) -> TrackCursor {
        self.log_frames(&prev);
        self.frames_unlogged(prev)
    }

    /// Logs (at [`Level::Info`]) the opening of this range's frames from `prev`.
    pub fn log_frames(&self, prev: &TrackCursor) {
        self.log_open("frames", || prev.reusable(self.area, self.first, self.image_sectors, self.image_id));
    }

    /// [`Continuation::frames`] without logging (see [`Continuation::audio_unlogged`]).
    pub fn frames_unlogged(self, prev: TrackCursor) -> TrackCursor {
        prev.continue_in(self.area, self.first, self.end, self.image_sectors, self.image_id)
    }

    /// Logs (at [`Level::Info`]) the opening of this range's frames on a new scan, as
    /// [`Continuation::fresh_frames`] does not.
    pub fn log_fresh_frames(&self) {
        self.log_open("frames", || false);
    }

    /// A fresh cursor of this range's positions (see [`TrackCursor::for_range`]), without
    /// logging (see [`Continuation::log_fresh_frames`]).
    pub fn fresh_frames(self) -> TrackCursor {
        TrackCursor::start(self.area, self.first, self.end, self.image_sectors)
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
        TrackCursor { area: AreaCursor::resume(index, first), ..Self::start(area, first, end, image_sectors) }
    }

    /// Where this cursor's scan started (see [`ResumedFrom`]); continued cursors keep it.
    pub fn resumed_from(&self) -> ResumedFrom {
        self.area.resumed_from()
    }

    /// A fresh scan of `area` for the range `[first, end)`, already checked.
    fn start(area: &Area, first: u32, end: u32, image_sectors: u64) -> Self {
        TrackCursor {
            area: AreaCursor::new(area, image_sectors),
            area_key: AreaKey::new(area, image_sectors),
            next: first,
            end,
            channels: area.channel_count,
            peeked: None,
            exhausted: false,
            failed: false,
            missing: 0,
        }
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
        Ok(self.continue_in(area, track.start_frame, track.end_frame(), image_sectors, None))
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
        Ok(self.continue_in(area, first, end, image_sectors, None))
    }

    /// As [`TrackCursor::continue_to_range`] for a range already checked, also starting
    /// afresh when this cursor has read an image other than `image_id`.
    pub(crate) fn continue_in(
        self,
        area: &Area,
        first: u32,
        end: u32,
        image_sectors: u64,
        image_id: Option<u64>,
    ) -> Self {
        if !self.reusable(area, first, image_sectors, image_id) {
            // A range (or track) before this cursor's position, or in another area, scans
            // the area again: specification 4, 4.2 item 6 allows a pass per track or range.
            return Self::start(area, first, end, image_sectors);
        }
        TrackCursor { next: first, end, missing: 0, ..self }
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
        let t = self.next;
        // Accepted frames arrive in increasing order: skip those before t. Stops reading
        // as soon as a frame at or after t is seen (specification 4, 4.5 item 4).
        while !self.exhausted && self.peeked.as_ref().is_none_or(|f| f.number < t) {
            match self.area.next_frame(image) {
                Some(Ok(f)) => self.peeked = Some(f),
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
}

impl<'a, R: Read + Seek> TrackFrames<'a, R> {
    pub(crate) fn with_cursor(image: &'a mut Image<R>, cursor: TrackCursor) -> Self {
        TrackFrames { image, cursor }
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
}

impl<R: Read + Seek> Iterator for TrackFrames<'_, R> {
    type Item = Result<TrackFrame, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        self.cursor.next_frame(self.image)
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
        Ok(self.continue_in(area, track.start_frame, track.end_frame(), image_sectors, None))
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
        Ok(self.continue_in(area, first, end, image_sectors, None))
    }

    /// As [`TrackAudioCursor::continue_to_range`] for a range already checked (see
    /// [`TrackCursor::continue_in`]).
    pub(crate) fn continue_in(
        self,
        area: &Area,
        first: u32,
        end: u32,
        image_sectors: u64,
        image_id: Option<u64>,
    ) -> Self {
        // Frames decoded or demultiplexed ahead of the caller came from the track cursor
        // already, so it stands after them and they lie inside this range: dropping them
        // loses nothing a later range can use (jobs still on the pool finish unread). An
        // error ends the reuse.
        let mut frames = self.frames;
        if self.pending_error.is_some() {
            frames.failed = true;
        }
        #[allow(unused_mut, reason = "set only with the parallel feature")]
        let mut next = Self::from_frames(frames.continue_in(area, first, end, image_sectors, image_id));
        #[cfg(feature = "parallel")]
        {
            next.decode_on = self.decode_on;
        }
        next
    }

    /// Whether [`TrackAudioCursor::continue_in`] keeps this cursor's reading state (for the
    /// log).
    pub(crate) fn continues(
        &self,
        area: &Area,
        first: u32,
        image_sectors: u64,
        image_id: Option<u64>,
    ) -> bool {
        self.pending_error.is_none() && self.frames.reusable(area, first, image_sectors, image_id)
    }

    fn from_frames(frames: TrackCursor) -> Self {
        TrackAudioCursor {
            frames,
            #[cfg(feature = "parallel")]
            decode_on: DecodeOn::default(),
            decoder: DstDecoder::new(),
            ahead: VecDeque::new(),
            ready: VecDeque::new(),
            pending_error: None,
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
    /// frames, go to the pool.
    fn fill<R: Read + Seek>(&mut self, image: &mut Image<R>) {
        let channels = self.channels();
        let window = self.window(channels);
        let group = crate::dst::lanes(channels);
        let held = self.ahead.len() + self.ready.len();
        if self.pending_error.is_some() || (held > 0 && held + group > window) {
            return;
        }
        let mut room = (window - held) / group * group;
        #[cfg(feature = "parallel")]
        let mut batch: Vec<TrackFrame> = Vec::with_capacity(group);
        while self.pending_error.is_none() && room > 0 {
            match self.frames.next_frame(image) {
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
    pub fn next_frame<R: Read + Seek>(&mut self, image: &mut Image<R>) -> Option<Result<AudioFrame, Error>> {
        let mut frame = if let Some(f) = self.ready.pop_front() {
            f
        } else {
            self.fill(image);
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
    }
}

impl SeekCursor for AreaCursor {}
impl SeekCursor for TrackCursor {}
impl SeekCursor for TrackAudioCursor {}

impl sealed::Sealed for AreaCursor {
    fn area_cursor(&self) -> &AreaCursor {
        self
    }
}

impl sealed::Sealed for TrackCursor {
    fn area_cursor(&self) -> &AreaCursor {
        &self.area
    }
}

impl sealed::Sealed for TrackAudioCursor {
    fn area_cursor(&self) -> &AreaCursor {
        &self.frames.area
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
}

impl<'a, R: Read + Seek> TrackAudio<'a, R> {
    pub(crate) fn with_cursor(image: &'a mut Image<R>, cursor: TrackAudioCursor) -> Self {
        TrackAudio { image, cursor }
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
}

impl<R: Read + Seek> Iterator for TrackAudio<'_, R> {
    type Item = Result<AudioFrame, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        self.cursor.next_frame(self.image)
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
