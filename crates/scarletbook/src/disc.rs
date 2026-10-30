// SPDX-License-Identifier: MIT OR Apache-2.0

//! A disc: an image with its parsed TOC.

use std::io::{Read, Seek};

use crate::audio::{self, AreaFrames, TrackAudio, TrackAudioCursor, TrackCursor, TrackFrames};
use crate::error::{Error, Slot, Warning};
use crate::image::{Image, SectorFormat};
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

    /// Opens a disc image in a sector format chosen by the caller.
    ///
    /// # Errors
    ///
    /// As [`Disc::open`].
    pub fn open_with_format(reader: R, format: SectorFormat) -> Result<Self, Error> {
        Self::from_image(Image::with_format(reader, format)?)
    }

    /// Parses the TOC of an image.
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
        let (area, track) = audio::find_track(&self.toc.areas, slot, number)?;
        audio::check_track(area, track, self.image.sector_count())
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
    /// must lie on the area's timeline, `first < end <= P` with `P` the area's play time
    /// ([`crate::Area::playtime_frames`]), and pass the per-track checks of specification
    /// 4, 4.8 (see [`audio::check_range`]).
    ///
    /// # Errors
    ///
    /// [`Error::ChannelCountUnsupported`], [`Error::FrameRangeInvalid`] and
    /// [`Error::TrackExceedsCapacity`], or [`Error::NoSuchArea`].
    pub fn check_range(&self, slot: Slot, first: u32, end: u32) -> Result<(), Error> {
        let area = self.toc.area(slot).ok_or(Error::NoSuchArea(slot))?;
        audio::check_range(area, first, end, self.image.sector_count())
    }

    /// The decoded audio of the frame range `[first, end)` of the area from `slot`: every
    /// timeline position from `first` to `end - 1`, exactly as [`Disc::track_audio`] gives
    /// a track's. The whole area, pauses included, is `[0, P)` with `P` the area's play
    /// time; a track with the pause after it is its start to the next track's start.
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
        let area = self.toc.area(slot).ok_or(Error::NoSuchArea(slot))?;
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
        let area = self.toc.area(slot).ok_or(Error::NoSuchArea(slot))?;
        let n = self.image.sector_count();
        audio::check_range(area, first, end, n)?;
        // A cursor from another disc is never continued, whatever its layout.
        Ok(prev.continue_in(area, first, end, n, Some(self.image.id())))
    }

    /// The decoding state of a track, for callers that cannot hold the borrow that
    /// [`Disc::track_audio`] needs (for example across an FFI boundary). Advance it with
    /// [`TrackAudioCursor::next_frame`], passing this disc's [`Disc::image_mut`].
    ///
    /// # Errors
    ///
    /// As [`Disc::check_track`].
    pub fn track_audio_cursor(&self, slot: Slot, number: u32) -> Result<TrackAudioCursor, Error> {
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
        let (area, track) = audio::find_track(&self.toc.areas, slot, number)?;
        let n = self.image.sector_count();
        audio::check_track(area, track, n)?;
        // A cursor from another disc is never continued, whatever its layout.
        Ok(prev.continue_in(area, track.start_frame, track.end_frame(), n, Some(self.image.id())))
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
