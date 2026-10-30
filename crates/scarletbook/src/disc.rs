// SPDX-License-Identifier: MIT OR Apache-2.0

//! A disc: an image with its parsed TOC.

use std::io::{Read, Seek};

use crate::audio::{self, AreaFrames, TrackAudio, TrackAudioCursor, TrackFrames};
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
        TrackFrames::new(&mut self.image, area, track)
    }

    /// The decoded audio of track `number` (1-based) of the area from `slot`: every
    /// timeline position as `C x 4704` interleaved DSD bytes, DST frames decoded and
    /// missing or invalid frames filled (specification 4, 4.6).
    ///
    /// # Errors
    ///
    /// As [`Disc::check_track`].
    pub fn track_audio(&mut self, slot: Slot, number: u32) -> Result<TrackAudio<'_, R>, Error> {
        let (area, track) = audio::find_track(&self.toc.areas, slot, number)?;
        TrackAudio::new(&mut self.image, area, track)
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

    /// The underlying image.
    pub fn image_mut(&mut self) -> &mut Image<R> {
        &mut self.image
    }

    /// Consumes the disc and returns the reader.
    pub fn into_inner(self) -> R {
        self.image.into_inner()
    }
}
