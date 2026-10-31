// SPDX-License-Identifier: MIT OR Apache-2.0

//! A disc: an image with its parsed TOC.

use std::io::{Read, Seek};

use crate::audio::{self, AreaFrames, Continuation, TrackAudio, TrackAudioCursor, TrackCursor, TrackFrames};
use crate::error::{Error, Slot, Warning};
use crate::image::{Image, SectorFormat};
use crate::log::{Log, sb_log};
use crate::toc::{self, Toc};

/// An opened disc image with its table of contents.
///
/// Opening reads only the master TOC and the area TOCs. Audio is read on demand.
#[derive(Debug)]
pub struct Disc<R> {
    image: Image<R>,
    toc: Toc,
    warnings: Vec<Warning>,
}

impl<R: Read + Seek> Disc<R> {
    /// Opens a disc image, detecting its sector format.
    ///
    /// # Errors
    ///
    /// One of the rejections of specification 7, 7.7.1, or [`Error::Io`].
    pub fn open(reader: R) -> Result<Self, Error> {
        Self::from_image(Image::open(reader)?)
    }

    /// Opens a disc image, detecting its sector format, and reports to `log` from the
    /// detection on (see [`crate::log`]).
    ///
    /// # Errors
    ///
    /// As [`Disc::open`].
    pub fn open_with_log(reader: R, log: Log) -> Result<Self, Error> {
        Self::from_image(Image::open_with_log(reader, log)?)
    }

    /// Opens a disc image in a sector format chosen by the caller.
    ///
    /// # Errors
    ///
    /// As [`Disc::open`].
    pub fn open_with_format(reader: R, format: SectorFormat) -> Result<Self, Error> {
        Self::from_image(Image::with_format(reader, format)?)
    }

    /// Opens a disc image in a sector format chosen by the caller, and reports to `log`
    /// from the start (see [`crate::log`]).
    ///
    /// # Errors
    ///
    /// As [`Disc::open`].
    pub fn open_with_format_and_log(reader: R, format: SectorFormat, log: Log) -> Result<Self, Error> {
        Self::from_image(Image::with_format_and_log(reader, format, log)?)
    }

    /// Parses the TOC of an image, reporting to the image's log ([`Image::set_log`]).
    ///
    /// # Errors
    ///
    /// As [`Disc::open`].
    pub fn from_image(mut image: Image<R>) -> Result<Self, Error> {
        let (toc, warnings) = toc::read_toc(&mut image)?;
        Ok(Disc { image, toc, warnings })
    }

    /// The table of contents.
    pub fn toc(&self) -> &Toc {
        &self.toc
    }

    /// The log this disc reports to.
    pub fn log(&self) -> &Log {
        self.image.log()
    }

    /// Replaces the log this disc reports to, from the next call on. Audio cursors report
    /// to the log of the disc they read through; decoding jobs already started on the pool
    /// (`parallel` feature) keep the log they started with.
    pub fn set_log(&mut self, log: Log) {
        self.image.set_log(log);
    }

    /// The warnings emitted while reading the TOC (specification 7, 7.7.3).
    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    /// The accepted frames of the area from `slot`, read from its audio sectors
    /// (specification 4, 4.2 to 4.5).
    ///
    /// # Errors
    ///
    /// [`Error::NoSuchArea`] if the TOC has no area from that slot.
    pub fn area_frames(&mut self, slot: Slot) -> Result<AreaFrames<'_, R>, Error> {
        let area = self.toc.area(slot).ok_or(Error::NoSuchArea(slot))?;
        Ok(AreaFrames::new(&mut self.image, area))
    }

    /// Whether track `number` of the area from `slot` produces audio (specification 4,
    /// 4.8).
    ///
    /// # Errors
    ///
    /// The per-track outcomes [`Error::ChannelCountUnsupported`], [`Error::TrackEmpty`]
    /// and [`Error::TrackExceedsCapacity`], or [`Error::NoSuchArea`] /
    /// [`Error::NoSuchTrack`].
    pub fn check_track(&self, slot: Slot, number: u32) -> Result<(), Error> {
        let (area, track) = self.find_track(slot, number)?;
        audio::check_track(area, track, self.image.sector_count())
            .inspect_err(|e| sb_log!(self.log(), Info, "area {slot} track {number}: no audio: {e}"))
    }

    /// The track a slot and number name, with its area; logs a track that does not exist.
    fn find_track(&self, slot: Slot, number: u32) -> Result<(&crate::Area, &crate::Track), Error> {
        audio::find_track(&self.toc.areas, slot, number)
            .inspect_err(|e| sb_log!(self.log(), Info, "area {slot} track {number}: {e}"))
    }

    /// Logs the opening of a track's frames or audio, on a new scan of its area.
    fn log_track(&self, slot: Slot, number: u32, what: &str) {
        if let Ok((area, track)) = audio::find_track(&self.toc.areas, slot, number) {
            let range = (track.start_frame, track.end_frame());
            audio::log_open(self.log(), area, Some(number), range, what, "new scan of the area");
        }
    }

    /// The timeline positions of track `number` (1-based) of the area from `slot`, each
    /// holding a plain DSD frame, a DST frame or nothing (specification 4, 4.6 and 4.7).
    ///
    /// Plain DSD frames are passed through unchanged; see [`Disc::track_audio`] for
    /// decoded audio.
    ///
    /// # Errors
    ///
    /// As [`Disc::check_track`].
    pub fn track_frames(&mut self, slot: Slot, number: u32) -> Result<TrackFrames<'_, R>, Error> {
        self.check_track(slot, number)?;
        self.log_track(slot, number, "frames");
        let (area, track) = audio::find_track(&self.toc.areas, slot, number)?;
        let cursor = TrackCursor::new(area, track, self.image.sector_count())?;
        Ok(TrackFrames::with_cursor(&mut self.image, cursor))
    }

    /// The decoded audio of track `number` (1-based) of the area from `slot`: every
    /// timeline position as `C x 4704` interleaved DSD bytes, DST frames decoded and
    /// missing or invalid frames filled (specification 4, 4.6).
    ///
    /// # Errors
    ///
    /// As [`Disc::check_track`].
    pub fn track_audio(&mut self, slot: Slot, number: u32) -> Result<TrackAudio<'_, R>, Error> {
        let cursor = self.track_audio_cursor(slot, number)?;
        Ok(TrackAudio::with_cursor(&mut self.image, cursor))
    }

    /// Whether the frame range `[first, end)` of the area from `slot` produces audio: it
    /// must lie in the area's play range, `first <= end <= P` with `P` the area's play
    /// time ([`crate::Area::playtime_frames`]), and pass the checks of specification 4,
    /// 4.8 as a track would (4.7.1; see [`audio::check_range`]).
    ///
    /// # Errors
    ///
    /// [`Error::ChannelCountUnsupported`], [`Error::FrameRangeInvalid`],
    /// [`Error::TrackEmpty`] and [`Error::TrackExceedsCapacity`], or [`Error::NoSuchArea`].
    pub fn check_range(&self, slot: Slot, first: u32, end: u32) -> Result<(), Error> {
        self.toc
            .area(slot)
            .ok_or(Error::NoSuchArea(slot))
            .and_then(|area| audio::check_range(area, first, end, self.image.sector_count()))
            .inspect_err(|e| {
                sb_log!(self.log(), Info, "area {slot} range: frames {first}..{end}: no audio: {e}")
            })
    }

    /// The decoded audio of the frame range `[first, end)` of the area from `slot`: every
    /// timeline position from `first` to `end - 1`, exactly as [`Disc::track_audio`] gives
    /// a track's. The whole area, pauses included, is `[0, P)` with `P` the area's play
    /// time. For a track with pause (specification 4, 4.7.1 item 5: track 1 from frame 0,
    /// every other track from its own start; every track to the larger of its own end and
    /// the next-numbered track's start, the last to the play time; used as they are when
    /// tracks overlap or are out of order), use the range from
    /// [`Disc::track_range_with_pause`].
    ///
    /// # Errors
    ///
    /// As [`Disc::check_range`].
    pub fn range_audio(&mut self, slot: Slot, first: u32, end: u32) -> Result<TrackAudio<'_, R>, Error> {
        let cursor = self.range_audio_cursor(slot, first, end)?;
        Ok(TrackAudio::with_cursor(&mut self.image, cursor))
    }

    /// The decoding state of a frame range (see [`Disc::range_audio`]), for callers that
    /// cannot hold the borrow; read it with [`Disc::read_track_audio`].
    ///
    /// # Errors
    ///
    /// As [`Disc::check_range`].
    pub fn range_audio_cursor(&self, slot: Slot, first: u32, end: u32) -> Result<TrackAudioCursor, Error> {
        self.check_range(slot, first, end)?;
        let area = self.toc.area(slot).ok_or(Error::NoSuchArea(slot))?;
        audio::log_open(self.log(), area, None, (first, end), "decoded audio", "new scan of the area");
        TrackAudioCursor::for_range(area, first, end, self.image.sector_count())
    }

    /// The decoding state of the frame range `[first, end)` of the area from `slot`,
    /// continuing `prev` (a cursor of an earlier track or range) without rescanning the
    /// area when the result is the same, as [`Disc::continue_track_audio`] does for
    /// tracks. A range that fails [`Disc::check_range`] consumes `prev` without reuse.
    ///
    /// # Errors
    ///
    /// As [`Disc::check_range`].
    pub fn continue_range_audio(
        &self,
        prev: TrackAudioCursor,
        slot: Slot,
        first: u32,
        end: u32,
    ) -> Result<TrackAudioCursor, Error> {
        Ok(self.range_continuation(slot, first, end)?.audio(prev))
    }

    /// The decoding state of a track, for callers that cannot hold the borrow that
    /// [`Disc::track_audio`] needs (for example across an FFI boundary). Advance it with
    /// [`TrackAudioCursor::next_frame`], passing this disc's [`Disc::image_mut`].
    ///
    /// # Errors
    ///
    /// As [`Disc::check_track`].
    pub fn track_audio_cursor(&self, slot: Slot, number: u32) -> Result<TrackAudioCursor, Error> {
        self.check_track(slot, number)?;
        self.log_track(slot, number, "decoded audio");
        let (area, track) = audio::find_track(&self.toc.areas, slot, number)?;
        TrackAudioCursor::new(area, track, self.image.sector_count())
    }

    /// The decoding state of track `number` of the area from `slot`, continuing `prev`
    /// (a cursor of an earlier track) without rescanning the area when the result is
    /// the same. Reading an area's tracks in increasing order this way reads each audio
    /// sector once. See [`TrackAudioCursor::continue_to`]. A track that fails
    /// [`Disc::check_track`] consumes `prev` without reuse, so check first to keep it.
    ///
    /// ```no_run
    /// # fn main() -> Result<(), scarletbook::Error> {
    /// # let mut disc = scarletbook::Disc::open(std::io::Cursor::new(Vec::new()))?;
    /// use scarletbook::Slot;
    /// let count = disc.toc().area(Slot::TwoChannel).map_or(0, |a| a.tracks.len()) as u32;
    /// let mut cursor = None;
    /// for n in 1..=count {
    ///     if disc.check_track(Slot::TwoChannel, n).is_err() {
    ///         continue; // a track without audio; keep the cursor for the next one
    ///     }
    ///     let next = match cursor.take() {
    ///         Some(prev) => disc.continue_track_audio(prev, Slot::TwoChannel, n)?,
    ///         None => disc.track_audio_cursor(Slot::TwoChannel, n)?,
    ///     };
    ///     let mut audio = disc.read_track_audio(next);
    ///     for frame in audio.by_ref() {
    ///         let _frame = frame?;
    ///     }
    ///     cursor = Some(audio.into_cursor());
    /// }
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// As [`Disc::check_track`].
    pub fn continue_track_audio(
        &self,
        prev: TrackAudioCursor,
        slot: Slot,
        number: u32,
    ) -> Result<TrackAudioCursor, Error> {
        Ok(self.track_continuation(slot, number)?.audio(prev))
    }

    /// Track `number` of the area from `slot`, checked ([`Disc::check_track`]), for a
    /// cursor to continue into without failing: [`Continuation::audio`] does what
    /// [`Disc::continue_track_audio`] does once the checks have passed. A caller that must
    /// not give up `prev` unless the continuation succeeds checks first, then hands it
    /// over.
    ///
    /// # Errors
    ///
    /// As [`Disc::check_track`].
    pub fn track_continuation(&self, slot: Slot, number: u32) -> Result<Continuation<'_>, Error> {
        self.check_track(slot, number)?;
        let (area, track) = audio::find_track(&self.toc.areas, slot, number)?;
        let n = self.image.sector_count();
        let range = (track.start_frame, track.end_frame());
        // A cursor from another disc is never continued, whatever its layout.
        Ok(Continuation::new(area, range, n, Some(self.image.id()), self.log(), Some(number)))
    }

    /// The frame range `[first, end)` of the area from `slot`, checked
    /// ([`Disc::check_range`]), for a cursor to continue into without failing (see
    /// [`Disc::track_continuation`]).
    ///
    /// # Errors
    ///
    /// As [`Disc::check_range`].
    pub fn range_continuation(&self, slot: Slot, first: u32, end: u32) -> Result<Continuation<'_>, Error> {
        self.check_range(slot, first, end)?;
        let area = self.toc.area(slot).ok_or(Error::NoSuchArea(slot))?;
        let n = self.image.sector_count();
        // A cursor from another disc is never continued, whatever its layout.
        Ok(Continuation::new(area, (first, end), n, Some(self.image.id()), self.log(), None))
    }

    /// Track `number`'s track with pause (specification 4, 4.7.1 item 5): see
    /// [`audio::track_range_with_pause`].
    /// Pass it to [`Disc::range_audio`].
    ///
    /// # Errors
    ///
    /// [`Error::NoSuchArea`] or [`Error::NoSuchTrack`].
    pub fn track_range_with_pause(&self, slot: Slot, number: u32) -> Result<(u32, u32), Error> {
        let (area, _) = audio::find_track(&self.toc.areas, slot, number)?;
        audio::track_range_with_pause(area, number).ok_or(Error::NoSuchTrack { slot, track: number })
    }

    /// The borrowing iterator over a track (or range) decoding state of this disc.
    pub fn read_track_audio(&mut self, cursor: TrackAudioCursor) -> TrackAudio<'_, R> {
        TrackAudio::with_cursor(&mut self.image, cursor)
    }

    /// The underlying image.
    pub fn image_mut(&mut self) -> &mut Image<R> {
        &mut self.image
    }

    /// Consumes the disc and returns the reader.
    pub fn into_inner(self) -> R {
        self.image.into_inner()
    }
}
