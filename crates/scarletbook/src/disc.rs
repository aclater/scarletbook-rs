// SPDX-License-Identifier: MIT OR Apache-2.0

//! A disc: an image with its parsed TOC.

use std::io::{Read, Seek};
use std::sync::Arc;

use crate::audio::{
    self, AreaCursor, AreaFrames, Continuation, TrackAudio, TrackAudioCursor, TrackCursor, TrackFrames,
};
use crate::control::CancelToken;
use crate::error::{Error, Slot, Warning};
use crate::image::{Image, SectorFormat};
use crate::log::{Log, sb_log};
use crate::read_at::{ReadAt, ReadAtReader};
use crate::seek::{AreaIndex, IndexOptions, SeekIndexes, SeekPolicy, Speculation};
use crate::toc::{self, Timecode, Toc, TocDigests, TocStored};

/// An opened disc image with its table of contents.
///
/// Opening reads only the master TOC and the area TOCs. Audio is read on demand.
///
/// # Several readers
///
/// A disc reads through one reader with one position, so its reads take `&mut self`. For
/// reads of one image on several threads at once, give each thread a disc of its own made
/// from this one: [`Disc::share`] with another reader of the same image, or `clone` for a
/// disc read through a [`ReadAtReader`] (no I/O). They share the parsed TOC, warnings,
/// sector format and log, and the image identifier ([`Image::id`]), so a cursor made or
/// read through one may be read through any other: they read the same bytes.
///
/// ```no_run
/// # fn main() -> Result<(), scarletbook::Error> {
/// use std::sync::Arc;
/// use scarletbook::{Disc, ReadAtReader, Slot};
///
/// let file = Arc::new(std::fs::File::open("disc.iso")?);
/// let disc = Disc::open(ReadAtReader::new(file))?;
/// let count = disc.toc().area(Slot::TwoChannel).map_or(0, |a| a.tracks.len()) as u32;
/// std::thread::scope(|s| {
///     for n in 1..=count {
///         let mut disc = disc.clone();
///         s.spawn(move || -> Result<(), scarletbook::Error> {
///             let cursor = disc.track_audio_cursor(Slot::TwoChannel, n)?;
///             for frame in disc.read_track_audio(cursor) {
///                 let _frame = frame?;
///             }
///             Ok(())
///         });
///     }
/// });
/// # Ok(())
/// # }
/// ```
///
/// # Seeking
///
/// With a seek index enabled for an area ([`Disc::enable_seek_index`] or
/// [`Disc::set_seek_index`]; off by default), every track and range cursor of that area
/// this disc, or any disc made from it by [`Disc::share`] or `clone`, makes or continues
/// starts from the index (one index per area, which they all use and grow, from any
/// thread) (see [`crate::seek`]), and the
/// borrowing iterators ([`TrackAudio`], [`TrackFrames`]) and continuations hand the
/// checkpoints their cursors record back to it as they read. The frames, statuses, counts
/// and warnings are exactly those without an index; once the index covers a range, opening
/// it reads at most a few dozen sectors before the range's own, wherever it lies. A
/// continuation starts from the index instead of the area's first sector when the earlier
/// cursor's state cannot be reused, and also when the index starts the range more than two
/// checkpoints past that cursor's scan (until it first reads, a cursor moved ahead so
/// reports the malformed sectors and warnings of the scan it was moved from, as a reused
/// one would). The stored frame list and [`Disc::area_frames`] always scan from the area's
/// first sector.
///
/// ```no_run
/// # fn main() -> Result<(), scarletbook::Error> {
/// use scarletbook::{AreaIndex, Disc, IndexOptions, Slot};
/// let mut disc = Disc::open(std::io::BufReader::new(std::fs::File::open("disc.iso")?))?;
/// disc.enable_seek_index(Slot::TwoChannel, IndexOptions::new())?;
/// // Built by reads as they go, or at once in steps: one pass over the area.
/// while !disc.extend_seek_index(Slot::TwoChannel, 64 * 1024)? {}
/// for frame in disc.range_audio(Slot::TwoChannel, 200_000, 200_075)? {
///     let _frame = frame?;
/// }
/// // Kept for the next time: checked again when loaded, and as it is used.
/// let saved = disc.seek_index(Slot::TwoChannel).map(|i| i.to_bytes()).unwrap_or_default();
/// let area = disc.toc().area(Slot::TwoChannel).ok_or(scarletbook::Error::NoSuchArea(Slot::TwoChannel))?;
/// let index = AreaIndex::from_bytes(area, disc.toc().sectors, &saved, scarletbook::seek::DEFAULT_INLINE_BUDGET)?;
/// disc.set_seek_index(Slot::TwoChannel, index)?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct Disc<R> {
    image: Image<R>,
    shared: Arc<DiscShared>,
    /// The seek policy of each slot, this handle's own ([`Disc::set_seek_policy`]).
    policy: [SeekPolicy; 2],
}

/// What the discs made from one by [`Disc::share`] and `clone` have in common: parsed
/// once, read by all of them, from any thread.
///
/// State of the image that every one of its discs should see (for example per-area seek
/// indexes, #68) belongs here, behind its own lock, rather than in [`Disc`].
#[derive(Debug)]
struct DiscShared {
    toc: Toc,
    warnings: Vec<Warning>,
    /// The digests of the TOC sectors the parse used, against which [`Disc::share`]
    /// checks another reader.
    digests: TocDigests,
    /// TOC fields as stored that the TOC gives only as interpreted.
    stored: TocStored,
    /// The seek index of each slot, if enabled: one for every disc of the image, so that
    /// they all use and grow the same index.
    seek: SeekIndexes,
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
        let (toc, warnings, digests, stored) = toc::read_toc(&mut image)?;
        Ok(Disc {
            image,
            shared: Arc::new(DiscShared { toc, warnings, digests, stored, seek: SeekIndexes::default() }),
            policy: [SeekPolicy::Exact; 2],
        })
    }

    /// A disc of the same image read through `reader`, with a position of its own, sharing
    /// this disc's TOC, warnings, sector format, image identifier and log (see [Several
    /// readers](Disc#several-readers)). Cursors of either disc may be read through the
    /// other. [`Disc::set_log`] on one does not change the other's.
    ///
    /// `reader` is checked: it must have the image's length, and the master TOC sector
    /// (LSN 510) and the area TOC copies the parse used must hold the same bytes (by a
    /// 64-bit digest). This catches another file given by mistake; it cannot prove that the
    /// audio sectors are the same, which stays the caller's responsibility.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`] if `reader` is of another image, [`Error::Io`] if it
    /// fails.
    pub fn share<R2: Read + Seek>(&self, reader: R2) -> Result<Disc<R2>, Error> {
        self.share_with_log(reader, self.log().clone())
    }

    /// [`Disc::share`], with the new disc reporting to `log` (see [`crate::log`]), the
    /// check of `reader` included.
    ///
    /// # Errors
    ///
    /// As [`Disc::share`].
    pub fn share_with_log<R2: Read + Seek>(&self, reader: R2, log: Log) -> Result<Disc<R2>, Error> {
        let mut image = self.image.share(reader, log)?;
        if TocDigests::of(&mut image, &self.shared.toc)?.as_ref() != Some(&self.shared.digests) {
            sb_log!(image.log(), Info, "shared reader refused: its TOC sectors differ from the image's");
            return Err(Error::InvalidArgument("shared reader of another image"));
        }
        sb_log!(image.log(), Debug, "shared reader checked: same length and TOC sectors");
        Ok(Disc { image, shared: Arc::clone(&self.shared), policy: self.policy })
    }

    /// The table of contents.
    pub fn toc(&self) -> &Toc {
        &self.shared.toc
    }

    /// The digests of the TOC sectors the parse used, which tell images apart.
    #[cfg(feature = "output")]
    pub(crate) fn toc_digests(&self) -> &TocDigests {
        &self.shared.digests
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

    /// The cancellation request of this disc's reads, to cancel them from another thread
    /// (see [`CancelToken`]). Every disc of the image ([`Disc::share`], `clone`) has the
    /// same: cancelling stops the reads of all of them and their cursors.
    pub fn cancel_token(&self) -> CancelToken {
        self.image.cancel_token()
    }

    /// The start and duration timecodes of track `number` (1-based) of the area from `slot`
    /// as stored in the area TOC's `SACDTRL2` (specification 3, 3.7.2), byte for byte:
    /// before the clamping of 3.9.2 and whatever their range, so that a stored timecode
    /// such as 255:255:255 is given as stored.
    /// [`crate::Track::start_frame`] and [`crate::Track::duration_frames`] are their frame
    /// numbers after clamping. The area's play time as stored is [`crate::Area::playtime`].
    ///
    /// # Errors
    ///
    /// [`Error::NoSuchArea`] or [`Error::NoSuchTrack`].
    pub fn track_timecodes_stored(&self, slot: Slot, number: u32) -> Result<(Timecode, Timecode), Error> {
        self.find_track(slot, number)?;
        self.shared.stored.track_timecodes(slot, number).ok_or(Error::NoSuchTrack { slot, track: number })
    }

    /// The warnings emitted while reading the TOC (specification 7, 7.7.3).
    pub fn warnings(&self) -> &[Warning] {
        &self.shared.warnings
    }

    /// Keeps a seek index of the area from `slot`, empty to begin with, and makes and
    /// continues that area's cursors from it (see [Seeking](Self#seeking)); an index
    /// already kept is replaced. Every disc of the image ([`Disc::share`], `clone`) keeps
    /// the same index: this, [`Disc::set_seek_index`] and [`Disc::take_seek_index`] change
    /// it for all of them.
    ///
    /// # Errors
    ///
    /// [`Error::NoSuchArea`], or [`Error::InvalidArgument`] for invalid options.
    pub fn enable_seek_index(&mut self, slot: Slot, options: IndexOptions) -> Result<(), Error> {
        let area = self.shared.toc.area(slot).ok_or(Error::NoSuchArea(slot))?;
        let mut index = AreaIndex::new(area, self.image.sector_count(), options)?;
        index.bind_fingerprint(crate::seek::toc_fingerprint(&self.shared.toc, slot))?;
        sb_log!(self.log(), Debug, "area {slot}: seek index enabled, interval {} sectors", options.interval);
        self.shared.seek.write(slot, |kept| *kept = Some(index));
        Ok(())
    }

    /// Keeps `index` (for example one loaded with [`AreaIndex::from_bytes`]) as the seek
    /// index of the area from `slot`, as [`Disc::enable_seek_index`] does an empty one. It
    /// is trusted to be of this disc's image (see [One index per
    /// image](AreaIndex#one-index-per-image)): whatever image it was built from in this
    /// process, it takes this disc's from then on.
    ///
    /// An index a disc kept before (saved with [`AreaIndex::to_bytes`] from version 2 of
    /// its format on) holds a fingerprint of that disc's TOC (the master TOC, the area TOC
    /// and the image's sector count; see [`AreaIndex#seek-hints`]): an index of another disc
    /// of the same layout is refused. One without (made with [`AreaIndex::new`], or saved
    /// in version 1) takes this disc's. Saved confirmations and exact checkpoints are then
    /// trusted for this image whenever the TOC fingerprint and key match, so a re-rip with
    /// different audio bytes would get confirmed or exact readings that differ from its own:
    /// key saved indexes by the image file's identity (path, size and modification time, or a
    /// content hash) as well.
    ///
    /// # Errors
    ///
    /// [`Error::NoSuchArea`], or [`Error::InvalidArgument`] if `index` is of another area
    /// or image size, or of a disc with another TOC.
    pub fn set_seek_index(&mut self, slot: Slot, mut index: AreaIndex) -> Result<(), Error> {
        let area = self.shared.toc.area(slot).ok_or(Error::NoSuchArea(slot))?;
        index.check_key(area, self.image.sector_count())?;
        index.bind_fingerprint(crate::seek::toc_fingerprint(&self.shared.toc, slot))?;
        // Trusted to be of this image, as a loaded one is: it is this image's from its
        // first absorb on.
        index.forget_image();
        sb_log!(
            self.log(),
            Debug,
            "area {slot}: seek index set, frontier LSN {}{}",
            index.frontier(),
            if index.is_complete() { ", complete" } else { "" }
        );
        self.shared.seek.write(slot, |kept| *kept = Some(index));
        Ok(())
    }

    /// Sets how this disc handle starts ranges of the area from `slot` that its seek index
    /// (if any) cannot reach cheaply: [`SeekPolicy::Exact`] (the default) or
    /// [`SeekPolicy::Speculative`], which allows provisional reading (specification 4, 4.2
    /// item 8, sacd-spec `82eac6c`; see [`SeekPolicy`] for what it guarantees). The policy is
    /// this handle's: discs made from it by [`Disc::share`] and `clone` start with a copy,
    /// and each may change its own. It applies to the cursors this handle makes and
    /// continues from then on.
    ///
    /// # Errors
    ///
    /// [`Error::NoSuchArea`].
    pub fn set_seek_policy(&mut self, slot: Slot, policy: SeekPolicy) -> Result<(), Error> {
        self.shared.toc.area(slot).ok_or(Error::NoSuchArea(slot))?;
        sb_log!(self.log(), Debug, "area {slot}: seek policy {policy:?}");
        self.policy[slot_index(slot)] = policy;
        Ok(())
    }

    /// The seek policy of the area from `slot` for this handle.
    pub fn seek_policy(&self, slot: Slot) -> SeekPolicy {
        self.policy[slot_index(slot)]
    }

    /// Stops keeping a seek index of the area from `slot`, and returns it.
    pub fn take_seek_index(&mut self, slot: Slot) -> Option<AreaIndex> {
        self.shared.seek.write(slot, Option::take)
    }

    /// A copy of the seek index of the area from `slot`, if one is kept: to save it
    /// ([`AreaIndex::to_bytes`]) or to merge it with another.
    pub fn seek_index(&self, slot: Slot) -> Option<AreaIndex> {
        self.shared.seek.read(slot, |kept| kept.cloned())
    }

    /// Reads the area from `slot` on from its seek index's frontier, about `max_sectors`
    /// sectors, recording checkpoints (see [`AreaIndex::extend`]): whether the index is
    /// now complete. Reading a whole area this way builds the index for all its ranges at
    /// the cost of one pass. It also checks the provisional readings the index now reaches
    /// ([`AreaIndex::verify`]): their verdicts go to [`Disc::take_seek_events`] and to the
    /// cursors that made them.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`] if no seek index is kept for `slot` or it does not match
    /// the image, [`Error::Io`] if a read fails.
    pub fn extend_seek_index(&mut self, slot: Slot, max_sectors: u64) -> Result<bool, Error> {
        // A copy is extended, then merged back: the index stays usable meanwhile (by discs
        // that share it) and no lock is held while the image is read.
        let Some(mut index) = self.seek_index(slot) else {
            return Err(Error::InvalidArgument("no seek index kept for the area"));
        };
        let result = index.extend(&mut self.image, max_sectors);
        self.shared.seek.push_events(slot, index.take_events());
        let merged = self.shared.seek.write(slot, |kept| match kept {
            Some(kept) => kept.merge(&index).map(|()| kept.is_complete()),
            None => Ok(false),
        });
        match (result, merged) {
            (Err(e), _) | (Ok(_), Err(e)) => Err(e),
            (Ok(_), Ok(complete)) => Ok(complete),
        }
    }

    /// Builds the seek index of the area from `slot` by about `max_sectors` more sectors
    /// and checks the provisional readings it then reaches (specification 4, 4.2 item 8):
    /// [`Disc::extend_seek_index`], named for what a caller of [`SeekPolicy::Speculative`]
    /// runs it for, typically in a loop on a clone of the disc in another thread. Returns
    /// whether the index is complete; every provisional reading of the area has its verdict
    /// then.
    ///
    /// # Errors
    ///
    /// As [`Disc::extend_seek_index`].
    pub fn verify_speculation(&mut self, slot: Slot, max_sectors: u64) -> Result<bool, Error> {
        self.extend_seek_index(slot, max_sectors)
    }

    /// Takes the verdicts on provisional readings of the area from `slot` reached since the
    /// last call, by this disc or any disc made from it by [`Disc::share`] or `clone`
    /// (they share the queue): [`Speculation::Confirmed`], [`Speculation::Refuted`] (the
    /// positions below its `exact_from` are possibly different from the specification's:
    /// drop or re-read what was served from them; an exact read of them is now cheap, from
    /// the index) or [`Speculation::Unverified`]. A provisional reading is checked only
    /// when the disc keeps a seek index of its area ([`Disc::enable_seek_index`]).
    pub fn take_seek_events(&self, slot: Slot) -> Vec<crate::seek::SeekEvent> {
        self.shared.seek.take_events(slot)
    }

    /// Takes into the seek index of its area the checkpoints `cursor` recorded, for a
    /// cursor read without this disc's iterators (with [`TrackAudioCursor::next_frame`]
    /// on [`Disc::image_mut`]); the iterators and continuations do this themselves. Only
    /// cursors made from the index by a disc of this image (this one, or one made by
    /// [`Disc::share`] or `clone`) that read this image are taken. Nothing happens for
    /// others: cursors made from a caller's index or without one, and cursors whose
    /// checkpoints do not join the index. A cursor of another image (of the same size and
    /// layout) stops recording instead, so that its checkpoints never reach this index.
    pub fn absorb_seek_checkpoints(&self, cursor: &mut impl crate::audio::SeekCursor) {
        self.shared.seek.absorb(crate::audio::sealed::Sealed::area_cursor_mut(cursor), self.image.id());
    }

    /// The accepted frames of the area from `slot`, read from its audio sectors
    /// (specification 4, 4.2 to 4.5), whatever the area's channel count. For the stored
    /// frame list of 4.6.1, which refuses areas without audio, use
    /// [`Disc::stored_frame_list`].
    ///
    /// # Errors
    ///
    /// [`Error::NoSuchArea`] if the TOC has no area from that slot.
    pub fn area_frames(&mut self, slot: Slot) -> Result<AreaFrames<'_, R>, Error> {
        let area = self.shared.toc.area(slot).ok_or(Error::NoSuchArea(slot))?;
        Ok(AreaFrames::new(&mut self.image, area))
    }

    /// The **stored frame list** of the area from `slot` (specification 4, 4.6.1 item 2):
    /// every accepted frame of the area in frame number order, frames numbered at or past
    /// the play time included, each with its number, kind and bytes as stored. Every audio
    /// sector is processed, with no early stop; positions not in the list are missing.
    /// There are no DST equivalents for the list (4.6.1 item 4: only for tracks and ranges).
    ///
    /// These are the frames of [`Disc::area_frames`], offered only for an area whose
    /// channel count is 1 to 6 (4.6.1 item 1).
    ///
    /// # Errors
    ///
    /// [`Error::NoSuchArea`] or [`Error::ChannelCountUnsupported`].
    pub fn stored_frame_list(&mut self, slot: Slot) -> Result<AreaFrames<'_, R>, Error> {
        let cursor = self.stored_frame_list_cursor(slot)?;
        Ok(AreaFrames::with_cursor(&mut self.image, cursor))
    }

    /// The reading state of [`Disc::stored_frame_list`], for callers that cannot hold the
    /// borrow; advance it with [`AreaCursor::next_frame`], passing [`Disc::image_mut`].
    ///
    /// # Errors
    ///
    /// As [`Disc::stored_frame_list`].
    pub fn stored_frame_list_cursor(&self, slot: Slot) -> Result<AreaCursor, Error> {
        let area = self.shared.toc.area(slot).ok_or(Error::NoSuchArea(slot))?;
        audio::check_channels(area)
            .inspect_err(|e| sb_log!(self.log(), Info, "area {slot} stored frame list: {e}"))?;
        sb_log!(
            self.log(),
            Info,
            "area {slot} stored frame list ({} channels), new scan of the area",
            area.channel_count
        );
        Ok(AreaCursor::new(area, self.image.sector_count()))
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
        audio::find_track(&self.shared.toc.areas, slot, number)
            .inspect_err(|e| sb_log!(self.log(), Info, "area {slot} track {number}: {e}"))
    }

    /// A cursor of the range `[first, end)` of `area`, already checked: from the area's
    /// seek index if one is kept, else a new scan; logs the opening of `what` (track
    /// `track`, if one).
    fn frames_cursor(
        &self,
        area: &crate::Area,
        (first, end): (u32, u32),
        track: Option<u32>,
        what: &str,
    ) -> TrackCursor {
        let n = self.image.sector_count();
        let policy = self.seek_policy(area.slot);
        let (cursor, how) = self.shared.seek.read(area.slot, |index| {
            let mut cursor = TrackCursor::start_with(area, (first, end), n, index, policy);
            if index.is_some() {
                cursor.area_mut().keep_for_disc();
            }
            let how = match (cursor.speculation(), index) {
                (Speculation::Exact, Some(_)) => "from the seek index",
                (Speculation::Exact, None) => "new scan of the area",
                // Chosen on the first read, logged then.
                _ => "provisional start allowed (4.2 item 8)",
            };
            (cursor, how)
        });
        // Logged after the lock is released.
        audio::log_open(self.log(), area, track, (first, end), what, how);
        cursor
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
        let cursor = self.track_frames_cursor(slot, number)?;
        Ok(TrackFrames::with_cursor(&mut self.image, cursor, Some(&self.shared.seek)))
    }

    /// The reading state of [`Disc::track_frames`], for callers that cannot hold the
    /// borrow; read it with [`Disc::read_frames`].
    ///
    /// # Errors
    ///
    /// As [`Disc::check_track`].
    pub fn track_frames_cursor(&self, slot: Slot, number: u32) -> Result<TrackCursor, Error> {
        self.check_track(slot, number)?;
        let (area, track) = audio::find_track(&self.shared.toc.areas, slot, number)?;
        Ok(self.frames_cursor(area, (track.start_frame, track.end_frame()), Some(number), "frames"))
    }

    /// The timeline positions of the frame range `[first, end)` of the area from `slot`,
    /// without decoding: for each position, its frame number and the frame as stored (DST
    /// bytes, plain DSD bytes) or nothing, exactly the frames that [`Disc::range_audio`]
    /// decodes. The whole area is `[0, P)` with `P` the area's play time.
    ///
    /// # Errors
    ///
    /// As [`Disc::check_range`].
    pub fn range_frames(&mut self, slot: Slot, first: u32, end: u32) -> Result<TrackFrames<'_, R>, Error> {
        let cursor = self.range_frames_cursor(slot, first, end)?;
        Ok(TrackFrames::with_cursor(&mut self.image, cursor, Some(&self.shared.seek)))
    }

    /// The reading state of [`Disc::range_frames`], for callers that cannot hold the
    /// borrow; read it with [`Disc::read_frames`].
    ///
    /// # Errors
    ///
    /// As [`Disc::check_range`].
    pub fn range_frames_cursor(&self, slot: Slot, first: u32, end: u32) -> Result<TrackCursor, Error> {
        self.check_range(slot, first, end)?;
        let area = self.shared.toc.area(slot).ok_or(Error::NoSuchArea(slot))?;
        Ok(self.frames_cursor(area, (first, end), None, "frames"))
    }

    /// The reading state of track `number`'s positions, continuing `prev` (the state of an
    /// earlier track or range) without rescanning the area when the result is the same,
    /// as [`Disc::continue_track_audio`] does for decoded audio.
    ///
    /// # Errors
    ///
    /// As [`Disc::check_track`].
    pub fn continue_track_frames(
        &self,
        prev: TrackCursor,
        slot: Slot,
        number: u32,
    ) -> Result<TrackCursor, Error> {
        Ok(self.track_continuation(slot, number)?.frames(prev))
    }

    /// The reading state of the positions of the frame range `[first, end)`, continuing
    /// `prev` as [`Disc::continue_range_audio`] does for decoded audio.
    ///
    /// # Errors
    ///
    /// As [`Disc::check_range`].
    pub fn continue_range_frames(
        &self,
        prev: TrackCursor,
        slot: Slot,
        first: u32,
        end: u32,
    ) -> Result<TrackCursor, Error> {
        Ok(self.range_continuation(slot, first, end)?.frames(prev))
    }

    /// The borrowing iterator over a track (or range) reading state of this disc.
    pub fn read_frames(&mut self, cursor: TrackCursor) -> TrackFrames<'_, R> {
        TrackFrames::with_cursor(&mut self.image, cursor, Some(&self.shared.seek))
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
        Ok(TrackAudio::with_cursor(&mut self.image, cursor, Some(&self.shared.seek)))
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
        self.shared
            .toc
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
        Ok(TrackAudio::with_cursor(&mut self.image, cursor, Some(&self.shared.seek)))
    }

    /// The decoding state of a frame range (see [`Disc::range_audio`]), for callers that
    /// cannot hold the borrow; read it with [`Disc::read_track_audio`].
    ///
    /// # Errors
    ///
    /// As [`Disc::check_range`].
    pub fn range_audio_cursor(&self, slot: Slot, first: u32, end: u32) -> Result<TrackAudioCursor, Error> {
        self.check_range(slot, first, end)?;
        let area = self.shared.toc.area(slot).ok_or(Error::NoSuchArea(slot))?;
        Ok(TrackAudioCursor::from_frames(self.frames_cursor(area, (first, end), None, "decoded audio")))
    }

    /// [`Disc::range_audio_cursor`] from the caller's seek `index` (see
    /// [`TrackAudioCursor::for_range_indexed`]) rather than the disc's: for callers that
    /// keep their indexes themselves (the C interface). Take what the cursor records with
    /// [`AreaIndex::absorb`].
    ///
    /// # Errors
    ///
    /// As [`Disc::check_range`], then [`Error::InvalidArgument`] if `index` is of another
    /// area or image size.
    pub fn range_audio_cursor_indexed(
        &self,
        slot: Slot,
        first: u32,
        end: u32,
        index: &AreaIndex,
    ) -> Result<TrackAudioCursor, Error> {
        Ok(TrackAudioCursor::from_frames(self.indexed_cursor(slot, (first, end), index, "decoded audio")?))
    }

    /// [`Disc::range_frames_cursor`] from the caller's seek `index` (see
    /// [`Disc::range_audio_cursor_indexed`]).
    ///
    /// # Errors
    ///
    /// As [`Disc::range_audio_cursor_indexed`].
    pub fn range_frames_cursor_indexed(
        &self,
        slot: Slot,
        first: u32,
        end: u32,
        index: &AreaIndex,
    ) -> Result<TrackCursor, Error> {
        self.indexed_cursor(slot, (first, end), index, "frames")
    }

    fn indexed_cursor(
        &self,
        slot: Slot,
        (first, end): (u32, u32),
        index: &AreaIndex,
        what: &str,
    ) -> Result<TrackCursor, Error> {
        self.check_range(slot, first, end)?;
        let area = self.shared.toc.area(slot).ok_or(Error::NoSuchArea(slot))?;
        let n = self.image.sector_count();
        index
            .check_key(area, n)
            .inspect_err(|e| sb_log!(self.log(), Info, "area {slot} range: frames {first}..{end}: {e}"))?;
        audio::log_open(self.log(), area, None, (first, end), what, "from the seek index");
        Ok(TrackCursor::start_with(area, (first, end), n, Some(index), self.seek_policy(slot)))
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
        let (area, track) = audio::find_track(&self.shared.toc.areas, slot, number)?;
        let range = (track.start_frame, track.end_frame());
        Ok(TrackAudioCursor::from_frames(self.frames_cursor(area, range, Some(number), "decoded audio")))
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
        let (area, track) = audio::find_track(&self.shared.toc.areas, slot, number)?;
        let n = self.image.sector_count();
        let range = (track.start_frame, track.end_frame());
        // A cursor from another disc is never continued, whatever its layout.
        Ok(Continuation::new(
            area,
            range,
            (n, Some(self.image.id())),
            self.log(),
            Some(number),
            (&self.shared.seek, self.seek_policy(area.slot)),
        ))
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
        let area = self.shared.toc.area(slot).ok_or(Error::NoSuchArea(slot))?;
        let n = self.image.sector_count();
        // A cursor from another disc is never continued, whatever its layout.
        Ok(Continuation::new(
            area,
            (first, end),
            (n, Some(self.image.id())),
            self.log(),
            None,
            (&self.shared.seek, self.seek_policy(area.slot)),
        ))
    }

    /// Track `number`'s track with pause (specification 4, 4.7.1 item 5): see
    /// [`audio::track_range_with_pause`].
    /// Pass it to [`Disc::range_audio`].
    ///
    /// # Errors
    ///
    /// [`Error::NoSuchArea`] or [`Error::NoSuchTrack`].
    pub fn track_range_with_pause(&self, slot: Slot, number: u32) -> Result<(u32, u32), Error> {
        let (area, _) = self.find_track(slot, number)?;
        audio::track_range_with_pause(area, number)
            .ok_or(Error::NoSuchTrack { slot, track: number })
            .inspect_err(|e| sb_log!(self.log(), Info, "area {slot} track {number}: {e}"))
    }

    /// The borrowing iterator over a track (or range) decoding state of this disc.
    pub fn read_track_audio(&mut self, cursor: TrackAudioCursor) -> TrackAudio<'_, R> {
        TrackAudio::with_cursor(&mut self.image, cursor, Some(&self.shared.seek))
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

/// A disc of the same image through another reader of the same source (see [Several
/// readers](Disc#several-readers)): no I/O and no check, the source being the same.
impl<A: ReadAt + Clone> Clone for Disc<ReadAtReader<A>> {
    fn clone(&self) -> Self {
        Disc { image: self.image.clone(), shared: Arc::clone(&self.shared), policy: self.policy }
    }
}

fn slot_index(slot: Slot) -> usize {
    match slot {
        Slot::TwoChannel => 0,
        Slot::Multichannel => 1,
    }
}
