// SPDX-License-Identifier: MIT OR Apache-2.0

//! Audio sector multiplexing (specification 4): frame reassembly, the timeline and tracks.

use std::collections::VecDeque;
use std::io::{Read, Seek};

use crate::dst::{DstDecoder, DstError};
use crate::error::{Error, Slot, Warning, WarningCode};
use crate::image::{Image, ReadOutcome, SECTOR_SIZE};
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

/// Most timeline positions a cursor of the `parallel` feature holds ahead of its caller.
///
/// With `parallel`, a track's positions are demultiplexed on the calling thread up to
/// [`PARALLEL_FRAMES_PER_THREAD`] per thread of the current rayon pool ahead of the frame
/// returned last, at most this many (none ahead on a pool of one thread). Each DST frame
/// becomes one job on the pool as soon as it is demultiplexed, so that the pool decodes
/// while the caller consumes the frames before it; every call tops the window up again.
/// Plain DSD and missing positions are filled in on the calling thread when they are
/// returned. Frames are returned in timeline order whatever order the jobs finish in.
///
/// Each position held ahead is at most one DST frame's bytes (at most 65 536,
/// [`MAX_FRAME_BYTES`]) and, once decoded, its audio (at most 6 x 4704 bytes): a cursor
/// holds at most `256 x (65 536 + 6 x 4704)` bytes, about 24 MB, whatever the host (plus
/// the frame under assembly); DST frames of 40 % of their decoded size need about 3.4 MB
/// at 2 channels and 10 MB at 6. The cap is a constant so that the memory held does not
/// grow with the host's thread count; 256 positions still keep 8 frames per thread in
/// flight at 32 threads.
pub const MAX_PARALLEL_BATCH: usize = 256;

/// Positions held ahead per pool thread with the `parallel` feature (see
/// [`MAX_PARALLEL_BATCH`]).
pub const PARALLEL_FRAMES_PER_THREAD: usize = 16;

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
}

impl Demuxer {
    /// A reassembler for an area of `channels` channels (the area TOC's channel count).
    pub fn new(channels: u8) -> Self {
        Demuxer { channels, assembling: None, next_acceptable: 0, ready: VecDeque::new(), malformed: 0 }
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
        let header = sector[0];
        let dst = header & 1 != 0;
        let f = usize::from((header >> 2) & 7);
        let k = usize::from(header >> 5);
        let record = if dst { 4 } else { 3 };
        // H <= 1 + 14 + 28 = 43: every record lies inside the sector.
        let h = 1 + 2 * k + f * record;
        let byte = |i: usize| sector.get(i).copied().unwrap_or(0);
        let mut malformed = false;
        let mut x = 0usize;
        let mut pos = h;
        for p in 0..k {
            let w = u16::from_be_bytes([byte(1 + 2 * p), byte(2 + 2 * p)]);
            let start = w & 0x8000 != 0;
            let packet_type = (w >> 11) & 7;
            let len = usize::from(w & 0x07FF);
            let Some(payload) = sector.get(pos..pos + len) else {
                malformed = true;
                break;
            };
            if packet_type == 2 {
                if start {
                    self.deliver_if_complete();
                    if x < f {
                        let r = 1 + 2 * k + x * record;
                        let number =
                            (u32::from(byte(r)) * 60 + u32::from(byte(r + 1))) * 75 + u32::from(byte(r + 2));
                        let remaining = if dst { i32::from((byte(r + 3) >> 2) & 0x1F) } else { 0 };
                        let kind = if dst { FrameKind::Dst } else { FrameKind::Dsd };
                        self.assembling = Some(Assembly { number, kind, data: Vec::new(), remaining });
                        x += 1;
                    } else {
                        malformed = true;
                    }
                }
                if let Some(a) = &mut self.assembling {
                    if a.data.len() + len > MAX_FRAME_BYTES {
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
        self.deliver_if_complete();
    }

    fn deliver_if_complete(&mut self) {
        let Some(a) = self.assembling.take() else { return };
        let complete = !a.data.is_empty()
            && match a.kind {
                FrameKind::Dst => a.remaining == 0,
                FrameKind::Dsd => a.data.len() == usize::from(self.channels) * FRAME_BYTES_PER_CHANNEL,
            };
        if complete && a.number >= self.next_acceptable {
            // Frame numbers are at most 1 166 880.
            self.next_acceptable = a.number + 1;
            self.ready.push_back(AreaFrame { number: a.number, kind: a.kind, data: a.data });
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
}

/// Sectors read per call to the reader.
const READ_CHUNK: u64 = 32;

impl AreaCursor {
    /// A cursor at the start of `area`'s audio, in an image of `image_sectors` sectors.
    pub fn new(area: &Area, image_sectors: u64) -> Self {
        let (start, end) = audio_sector_range(area, image_sectors);
        AreaCursor {
            demux: Demuxer::new(area.channel_count),
            image_id: None,
            slot: area.slot,
            warnings: Vec::new(),
            next_lsn: start,
            end_lsn: end,
            buf: Vec::new(),
            buf_lsn: 0,
            buf_sectors: 0,
            finished: false,
        }
    }

    /// Number of malformed sectors processed so far by this cursor's scan of its area. The
    /// scan starts at the area's first audio sector, not at the track, and a continued
    /// cursor keeps counting from where the earlier tracks left off.
    pub fn malformed_sectors(&self) -> u64 {
        self.demux.malformed_sectors()
    }

    /// The `audio-sector-malformed` warnings of the sectors processed so far, with slot
    /// and LSN, in processing order (specification 4, 4.4.5). At most
    /// [`MAX_MALFORMED_WARNINGS`] are kept; [`AreaCursor::malformed_sectors`] counts them
    /// all.
    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    fn sector<R: Read + Seek>(&mut self, image: &mut Image<R>, lsn: u64) -> Result<[u8; SECTOR_SIZE], Error> {
        if !(self.buf_lsn..self.buf_lsn + self.buf_sectors).contains(&lsn) {
            let n = READ_CHUNK.min(self.end_lsn - lsn);
            self.buf.resize(n as usize * SECTOR_SIZE, 0);
            // For the image the cursor was made for, the range lies below its sector
            // count; another image may be shorter, which is reported, not assumed.
            if image.read_into(lsn, &mut self.buf)? == ReadOutcome::OutOfRange {
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
            None => self.image_id = Some(image.id()),
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
            if self.next_lsn < self.end_lsn {
                let lsn = self.next_lsn;
                self.next_lsn += 1;
                match self.sector(image, lsn) {
                    Ok(s) => {
                        if self.demux.push_sector(&s) && self.warnings.len() < MAX_MALFORMED_WARNINGS {
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
                self.demux.finish();
                self.finished = true;
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

    /// Number of malformed sectors processed so far by this cursor's scan of its area. The
    /// scan starts at the area's first audio sector, not at the track, and a continued
    /// cursor keeps counting from where the earlier tracks left off.
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
    if !(1..=MAX_CHANNELS).contains(&area.channel_count) {
        return Err(Error::ChannelCountUnsupported { channels: area.channel_count });
    }
    if track.duration_frames == 0 {
        return Err(Error::TrackEmpty);
    }
    let (start, end) = audio_sector_range(area, image_sectors);
    let capacity = (end - start).saturating_mul(FRAMES_PER_SECTOR);
    let end_frame = track.end_frame();
    if u64::from(end_frame) > capacity {
        return Err(Error::TrackExceedsCapacity { end_frame, capacity });
    }
    Ok(())
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

/// One timeline position of a track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackFrame {
    /// The frame number.
    pub number: u32,
    /// Its content.
    pub content: FrameContent,
}

/// The reading state of one track's timeline positions (specification 4, 4.7),
/// independent of the image it reads from. [`TrackFrames`] is the borrowing iterator.
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AreaKey {
    slot: Slot,
    start: u32,
    end: u32,
    channels: u8,
    image_sectors: u64,
}

impl AreaKey {
    fn new(area: &Area, image_sectors: u64) -> Self {
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
        Ok(TrackCursor {
            area: AreaCursor::new(area, image_sectors),
            area_key: AreaKey::new(area, image_sectors),
            next: track.start_frame,
            end: track.end_frame(),
            channels: area.channel_count,
            peeked: None,
            exhausted: false,
            failed: false,
        })
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
    /// # Errors
    ///
    /// As [`check_track`].
    pub fn continue_to(self, area: &Area, track: &Track, image_sectors: u64) -> Result<Self, Error> {
        self.continue_in(area, track, image_sectors, None)
    }

    /// As [`TrackCursor::continue_to`], also starting afresh when this cursor has read
    /// an image other than `image_id`.
    pub(crate) fn continue_in(
        self,
        area: &Area,
        track: &Track,
        image_sectors: u64,
        image_id: Option<u64>,
    ) -> Result<Self, Error> {
        check_track(area, track, image_sectors)?;
        let other_image = matches!((self.area.image_id, image_id), (Some(a), Some(b)) if a != b);
        if self.failed
            || other_image
            || self.area_key != AreaKey::new(area, image_sectors)
            || track.start_frame < self.next
        {
            return Self::new(area, track, image_sectors);
        }
        Ok(TrackCursor { next: track.start_frame, end: track.end_frame(), ..self })
    }

    /// The area's channel count `C` (1 to 6).
    pub fn channels(&self) -> u8 {
        self.channels
    }

    /// The frame range still to come, `[next, end)`.
    pub fn range(&self) -> (u32, u32) {
        (self.next, self.end)
    }

    /// Number of malformed sectors processed so far by this cursor's scan of its area. The
    /// scan starts at the area's first audio sector, not at the track, and a continued
    /// cursor keeps counting from where the earlier tracks left off.
    pub fn malformed_sectors(&self) -> u64 {
        self.area.malformed_sectors()
    }

    /// The `audio-sector-malformed` warnings of this cursor's scan of its area so far
    /// (see [`AreaCursor::warnings`]): per area, not per track, and each sector once.
    pub fn warnings(&self) -> &[Warning] {
        self.area.warnings()
    }

    /// The next timeline position, `None` after the track's last frame.
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
            None => FrameContent::Missing,
        };
        Some(Ok(TrackFrame { number: t, content }))
    }
}

/// The timeline positions of one track, in order (specification 4, 4.7).
#[derive(Debug)]
pub struct TrackFrames<'a, R> {
    image: &'a mut Image<R>,
    cursor: TrackCursor,
}

impl<'a, R: Read + Seek> TrackFrames<'a, R> {
    pub(crate) fn new(image: &'a mut Image<R>, area: &Area, track: &Track) -> Result<Self, Error> {
        let cursor = TrackCursor::new(area, track, image.sector_count())?;
        Ok(TrackFrames { image, cursor })
    }

    /// The area's channel count `C` (1 to 6).
    pub fn channels(&self) -> u8 {
        self.cursor.channels()
    }

    /// The frame range still to come, `[next, end)`.
    pub fn range(&self) -> (u32, u32) {
        self.cursor.range()
    }

    /// Number of malformed sectors processed so far by this cursor's scan of its area. The
    /// scan starts at the area's first audio sector, not at the track, and a continued
    /// cursor keeps counting from where the earlier tracks left off.
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

/// The decoding state of one track's audio (specification 4, 4.6 and 4.7), independent
/// of the image it reads from. [`TrackAudio`] is the borrowing iterator.
///
/// With the `parallel` feature, DST frames are decoded on rayon's thread pool ahead of
/// the caller, while the calling thread demultiplexes and consumes; memory stays bounded
/// (see [`MAX_PARALLEL_BATCH`]).
#[derive(Debug)]
pub struct TrackAudioCursor {
    frames: TrackCursor,
    /// Decodes on the calling thread: always without `parallel`; with it, the positions
    /// that are not DST frames, and everything on a pool of one thread.
    decoder: DstDecoder,
    /// Positions demultiplexed but not yet returned, in timeline order: they come before
    /// `pending_error`.
    ahead: VecDeque<Pending>,
    /// An error met while demultiplexing, reported after every frame before it.
    pending_error: Option<Error>,
    missing: u64,
    invalid: u64,
}

/// A position demultiplexed ahead of the caller.
#[derive(Debug)]
enum Pending {
    /// Decoded on the calling thread when it is returned.
    Here(TrackFrame),
    /// A DST frame being decoded on the pool.
    #[cfg(feature = "parallel")]
    Pool(std::sync::Arc<pool::Job>),
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

    /// A cursor at the start of `track`, continuing this one's reading state where that
    /// gives the same result (see [`TrackCursor::continue_to`]). Frames of this cursor
    /// not yet returned are dropped; the counts start again at 0.
    ///
    /// # Errors
    ///
    /// As [`check_track`].
    pub fn continue_to(self, area: &Area, track: &Track, image_sectors: u64) -> Result<Self, Error> {
        self.continue_in(area, track, image_sectors, None)
    }

    pub(crate) fn continue_in(
        self,
        area: &Area,
        track: &Track,
        image_sectors: u64,
        image_id: Option<u64>,
    ) -> Result<Self, Error> {
        // Frames decoded or demultiplexed ahead of the caller came from the track cursor
        // already, so it stands after them and they lie inside this track: dropping them
        // loses nothing a later track can use (jobs still on the pool finish unread). An
        // error ends the reuse.
        let mut frames = self.frames;
        if self.pending_error.is_some() {
            frames.failed = true;
        }
        Ok(Self::from_frames(frames.continue_in(area, track, image_sectors, image_id)?))
    }

    fn from_frames(frames: TrackCursor) -> Self {
        TrackAudioCursor {
            frames,
            decoder: DstDecoder::new(),
            ahead: VecDeque::new(),
            pending_error: None,
            missing: 0,
            invalid: 0,
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
    pub fn malformed_sectors(&self) -> u64 {
        self.frames.malformed_sectors()
    }

    /// The `audio-sector-malformed` warnings of this cursor's scan of its area so far
    /// (see [`AreaCursor::warnings`]): per area, not per track, and each sector once.
    pub fn warnings(&self) -> &[Warning] {
        self.frames.warnings()
    }

    /// Most positions held ahead of the caller, the one being returned included.
    fn window() -> usize {
        #[cfg(feature = "parallel")]
        {
            // One thread decodes everything on the calling thread, frame by frame as
            // without `parallel`: reading ahead would only cost cache.
            match rayon::current_num_threads() {
                1 => 1,
                n => (PARALLEL_FRAMES_PER_THREAD * n).min(MAX_PARALLEL_BATCH),
            }
        }
        #[cfg(not(feature = "parallel"))]
        {
            1
        }
    }

    /// Demultiplexes positions until the window is full, the track ends or an error is
    /// met (kept for after the positions before it). With `parallel` on a pool of more
    /// than one thread, each DST frame goes to the pool at once.
    ///
    /// The image reader stays on the calling thread: only decoding jobs, which own their
    /// frames, go to the pool.
    fn fill<R: Read + Seek>(&mut self, image: &mut Image<R>) {
        let window = Self::window();
        #[cfg(feature = "parallel")]
        let channels = self.channels();
        while self.pending_error.is_none() && self.ahead.len() < window {
            match self.frames.next_frame(image) {
                #[cfg(feature = "parallel")]
                Some(Ok(TrackFrame { number, content: FrameContent::Dst(bytes) })) if window > 1 => {
                    let frame = TrackFrame { number, content: FrameContent::Dst(bytes) };
                    self.ahead.push_back(Pending::Pool(pool::Job::spawn(frame, channels)));
                }
                Some(Ok(f)) => self.ahead.push_back(Pending::Here(f)),
                Some(Err(e)) => self.pending_error = Some(e),
                None => break,
            }
        }
    }

    /// The next decoded frame, `None` after the track's last frame.
    ///
    /// With `parallel`, while the next frame is still being decoded on the pool, a
    /// calling thread that belongs to the pool runs pending pool jobs meanwhile; otherwise,
    /// or when there are none, it blocks until the frame is decoded. A panic while decoding
    /// on the pool is resumed on the calling thread.
    pub fn next_frame<R: Read + Seek>(&mut self, image: &mut Image<R>) -> Option<Result<AudioFrame, Error>> {
        self.fill(image);
        let frame = match self.ahead.pop_front() {
            Some(Pending::Here(f)) => {
                let channels = self.channels();
                decode_checked(&mut self.decoder, f, channels)
            }
            #[cfg(feature = "parallel")]
            Some(Pending::Pool(job)) => job.wait(),
            None => return self.pending_error.take().map(Err),
        };
        // Counted as frames are returned, so the counts never run ahead of the caller,
        // whatever the window.
        match frame.status {
            FrameStatus::Missing => self.missing += 1,
            FrameStatus::Invalid(_) => self.invalid += 1,
            FrameStatus::Dsd | FrameStatus::Decoded => {}
        }
        Some(Ok(frame))
    }
}

/// Decoding jobs on rayon's pool, for the `parallel` feature.
#[cfg(feature = "parallel")]
mod pool {
    use std::any::Any;
    use std::cell::RefCell;
    use std::panic::AssertUnwindSafe;
    use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

    use super::{AudioFrame, TrackFrame, decode_checked};
    use crate::dst::DstDecoder;

    thread_local! {
        /// Each pool thread's decoder, reused from frame to frame.
        static DECODER: RefCell<DstDecoder> = RefCell::new(DstDecoder::new());
    }

    /// One frame decoded on the pool; its result is taken once.
    #[derive(Debug, Default)]
    pub(super) struct Job {
        state: Mutex<State>,
        done: Condvar,
    }

    #[derive(Debug, Default)]
    struct State {
        /// The decoded frame, or the panic that stopped its decoding.
        result: Option<Result<AudioFrame, Box<dyn Any + Send>>>,
        /// Whether the owner sleeps on `done`: only then does the job wake it.
        waiting: bool,
    }

    impl Job {
        fn lock(&self) -> MutexGuard<'_, State> {
            // A panic never happens while the lock is held: the state stays consistent.
            self.state.lock().unwrap_or_else(PoisonError::into_inner)
        }

        /// Starts decoding `frame` on the current pool.
        pub(super) fn spawn(frame: TrackFrame, channels: u8) -> Arc<Job> {
            let job = Arc::new(Job::default());
            let out = Arc::clone(&job);
            rayon::spawn(move || {
                let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    DECODER.with(|d| match d.try_borrow_mut() {
                        Ok(mut d) => decode_checked(&mut d, frame, channels),
                        // Not reached (decoding never yields to other jobs); stay correct.
                        Err(_) => decode_checked(&mut DstDecoder::new(), frame, channels),
                    })
                }));
                if result.is_err() {
                    // Start afresh rather than reuse a decoder a panic left behind.
                    DECODER.with(|d| {
                        if let Ok(mut d) = d.try_borrow_mut() {
                            *d = DstDecoder::new();
                        }
                    });
                }
                let mut state = out.lock();
                state.result = Some(result);
                if state.waiting {
                    out.done.notify_one();
                }
            });
            job
        }

        /// The decoded frame: runs other pool jobs while it is not ready, or blocks when
        /// there are none (every job then runs on some thread, and finishes) or this
        /// thread is not one of the pool's.
        pub(super) fn wait(&self) -> AudioFrame {
            let result = loop {
                if let Some(r) = self.lock().result.take() {
                    break r;
                }
                if rayon::yield_now() != Some(rayon::Yield::Executed) {
                    let mut state = self.lock();
                    state.waiting = true;
                    break loop {
                        if let Some(r) = state.result.take() {
                            break r;
                        }
                        state = self.done.wait(state).unwrap_or_else(PoisonError::into_inner);
                    };
                }
            };
            result.unwrap_or_else(|panic| std::panic::resume_unwind(panic))
        }
    }
}

/// The decoded audio of one track, frame by frame (specification 4, 4.6 and 4.7).
#[derive(Debug)]
pub struct TrackAudio<'a, R> {
    image: &'a mut Image<R>,
    cursor: TrackAudioCursor,
}

impl<'a, R: Read + Seek> TrackAudio<'a, R> {
    pub(crate) fn new(image: &'a mut Image<R>, area: &Area, track: &Track) -> Result<Self, Error> {
        let cursor = TrackAudioCursor::new(area, track, image.sector_count())?;
        Ok(TrackAudio { image, cursor })
    }

    pub(crate) fn with_cursor(image: &'a mut Image<R>, cursor: TrackAudioCursor) -> Self {
        TrackAudio { image, cursor }
    }

    /// The decoding state, to continue into a later track with
    /// [`crate::Disc::continue_track_audio`].
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

#[cfg(test)]
mod tests {
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
    fn parallel_window_is_capped() {
        let wide = rayon::ThreadPoolBuilder::new().num_threads(64).build().unwrap();
        wide.install(|| assert_eq!(TrackAudioCursor::window(), MAX_PARALLEL_BATCH));
        let narrow = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
        narrow.install(|| assert_eq!(TrackAudioCursor::window(), 2 * PARALLEL_FRAMES_PER_THREAD));
        let single = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
        single.install(|| assert_eq!(TrackAudioCursor::window(), 1));
        // The bound documented on MAX_PARALLEL_BATCH: each position held ahead is at most
        // one frame of DST bytes and one of decoded audio, about 24 MB in all.
        let bound = MAX_PARALLEL_BATCH * (MAX_FRAME_BYTES + 6 * FRAME_BYTES_PER_CHANNEL);
        assert!(bound <= 24_100_000, "{bound}");
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
    fn deinterleave_channel() {
        let frame: Vec<u8> = (0..2 * 4704).map(|i| (i % 2) as u8).collect();
        let mut out = vec![9u8; 4704];
        deinterleave(&frame, 2, 1, &mut out).unwrap();
        assert!(out.iter().all(|&b| b == 1));
        assert!(deinterleave(&frame, 2, 2, &mut out).is_err());
    }
}
