// SPDX-License-Identifier: MIT OR Apache-2.0

//! Output files: the layout of a DSF or DSDIFF file holding a track, a frame range or a
//! track with pause (specification 8), for writers and for filesystems that serve such
//! files without writing them (issue #131).
//!
//! A [`Layout`] is computed from the TOC alone (8.2.5): it knows the file's exact size and
//! every byte that is not audio before any audio sector is read. Its audio is described,
//! not held: [`Layout::positions_for`] gives the timeline positions that a byte range of
//! the file needs, and [`Layout::fill`] produces that byte range from the positions'
//! content, which the caller supplies through a [`PositionSource`] (from its own cache, for
//! example). [`Layout::read_at`] does both from a [`Disc`].
//!
//! ```no_run
//! # fn main() -> Result<(), scarletbook::Error> {
//! use scarletbook::output::{Format, OutputOptions};
//! use scarletbook::{Disc, Slot};
//!
//! let file = std::fs::File::open("disc.iso")?;
//! let mut disc = Disc::open(std::io::BufReader::new(file))?;
//! let (first, end) = disc.track_range_with_pause(Slot::TwoChannel, 1)?;
//! let layout = disc.output_layout(Slot::TwoChannel, first, end, Format::Dsf, &OutputOptions::default())?;
//! let mut header = vec![0; 4096];
//! let n = layout.read_at(&mut disc, 0, &mut header)?;
//! println!("{} bytes; the first {n} read", layout.size());
//! # Ok(())
//! # }
//! ```
//!
//! Every point where the formats allow more than one valid output (specification 8, 8.6,
//! "writer's choices") is a field of [`OutputOptions`], with a documented default.

mod dsdiff;
mod dsdiff_dst;
mod dsf;

use std::borrow::Cow;
use std::collections::VecDeque;
use std::io::{Read, Seek};
use std::ops::Range;

use crate::audio::{
    self, AudioFrame, FRAME_BYTES_PER_CHANNEL, FrameContent, TrackAudio, TrackFrame, TrackFrames,
};
use crate::disc::Disc;
use crate::dst::DstDecoder;
use crate::error::{Error, Slot};
use crate::toc::{Area, TocDigests};

pub use dsdiff::{LocalChunk, PropertyChunk};
pub use dsdiff_dst::{DstIndexOffset, DstLengths, InvalidDst};
pub use dsf::DsfBitsPerSample;

/// An output file format (specification 8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Format {
    /// DSF (8.2), without a tag.
    Dsf,
    /// DSDIFF with plain DSD sound data (8.4.5).
    DsdiffDsd,
    /// DSDIFF with DST sound data (8.4.6): the positions' DST equivalents, unchanged. Its
    /// layout needs the frame lengths as well as the TOC: see [`Layout::new_dst`].
    DsdiffDst,
}

/// The writer's choices of specification 8, 8.6 ("W" numbers below are its rows), each
/// with its default.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct OutputOptions {
    /// W1, DSF bits per sample (8.2.3). Default [`DsfBitsPerSample::One`]: each DSD byte
    /// with its bits reversed, earliest sample in bit 0.
    pub dsf_bits_per_sample: DsfBitsPerSample,
    /// W2, the DSF channel type of an area whose layout is unspecified (8.2.4 item 2). It
    /// must be a type with the area's channel count: 1 for 1 channel, 2 for 2, 3 for 3,
    /// 4 or 5 for 4, 6 for 5 and 7 for 6. Default `None`: such an area is refused with
    /// [`crate::OutputRefusal::UnspecifiedLayout`]. Areas with a defined layout ignore it.
    pub dsf_unspecified_channel_type: Option<u32>,
    /// W14, the order of the chunks inside the DSDIFF property chunk (8.4.3): each
    /// [`PropertyChunk`] once. Chunks that aren't written are skipped. Default
    /// [`PropertyChunk::TABLE_ORDER`]: `FS  `, `CHNL`, `CMPR`, `ABSS`, `LSCO`.
    pub dsdiff_property_order: [PropertyChunk; 5],
    /// W15, the DSDIFF channel IDs of an area whose layout is unspecified (8.4.4 item 2):
    /// one distinct number from 0 to 999 per channel, written as `C000` to `C999`. Default
    /// `None`: `C000`, `C001`, ... in channel order.
    pub dsdiff_unspecified_channel_numbers: Option<Vec<u16>>,
    /// W15, whether an area whose layout is unspecified gets an `LSCO` chunk with 65 535
    /// ("undefined channel set-up", 8.4.4 item 2). Default `false`: no `LSCO`.
    pub dsdiff_unspecified_loudspeaker_configuration: bool,
    /// W16, whether an area with a defined layout gets an `LSCO` chunk (8.4.3, 8.4.4).
    /// Default `true`.
    pub dsdiff_loudspeaker_configuration: bool,
    /// W17, the frame count `A` of the DSDIFF absolute start time (8.4.8): the `ABSS`
    /// chunk gives the time of frame `A`, and `A + N` must be at most 6 479 999. Default
    /// `None`: no `ABSS`, which means `A = 0`. `Some(first)` makes times equal timeline
    /// positions.
    pub dsdiff_absolute_start: Option<u32>,
    /// W13, the order of the DSDIFF local chunks after `FVER` (8.4.2): each chunk the file
    /// holds once, `PROP` before the sound data; chunks the file doesn't hold are ignored.
    /// Default [`LocalChunk::DEFAULT_ORDER`]: `PROP`, the sound data, `DSTI`.
    pub dsdiff_local_order: Vec<LocalChunk>,
    /// W19, whether each `DSTF` chunk is followed by a `DSTC` chunk with the CRC of the
    /// position's content (8.4.6.1). Producing one needs the position decoded. Default
    /// `false`.
    pub dsdiff_dst_crc: bool,
    /// W20, whether a DST file has a `DSTI` sound index (8.4.6.2), which the DSDIFF document
    /// recommends. Default `true`.
    pub dsdiff_dst_index: bool,
    /// W21, what a `DSTI` entry's offset points at (8.4.6.2 item 3). Default
    /// [`DstIndexOffset::ChunkHeader`], the `DSTF` chunk's first byte; the other reading,
    /// [`DstIndexOffset::FrameData`], is 12 bytes later.
    pub dsdiff_dst_index_offset: DstIndexOffset,
}

impl Default for OutputOptions {
    fn default() -> Self {
        OutputOptions {
            dsf_bits_per_sample: DsfBitsPerSample::One,
            dsf_unspecified_channel_type: None,
            dsdiff_property_order: PropertyChunk::TABLE_ORDER,
            dsdiff_unspecified_channel_numbers: None,
            dsdiff_unspecified_loudspeaker_configuration: false,
            dsdiff_loudspeaker_configuration: true,
            dsdiff_absolute_start: None,
            dsdiff_local_order: LocalChunk::DEFAULT_ORDER.to_vec(),
            dsdiff_dst_crc: false,
            dsdiff_dst_index: true,
            dsdiff_dst_index_offset: DstIndexOffset::ChunkHeader,
        }
    }
}

impl OutputOptions {
    /// The defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// These options with DSF bits per sample `bits` (W1).
    pub fn with_dsf_bits_per_sample(self, bits: DsfBitsPerSample) -> Self {
        OutputOptions { dsf_bits_per_sample: bits, ..self }
    }

    /// These options with DSF channel type `channel_type` for unspecified layouts (W2).
    pub fn with_dsf_unspecified_channel_type(self, channel_type: Option<u32>) -> Self {
        OutputOptions { dsf_unspecified_channel_type: channel_type, ..self }
    }

    /// These options with DSDIFF property chunk order `order` (W14).
    pub fn with_dsdiff_property_order(self, order: [PropertyChunk; 5]) -> Self {
        OutputOptions { dsdiff_property_order: order, ..self }
    }

    /// These options with DSDIFF channel numbers `numbers` for unspecified layouts (W15).
    pub fn with_dsdiff_unspecified_channel_numbers(self, numbers: Option<Vec<u16>>) -> Self {
        OutputOptions { dsdiff_unspecified_channel_numbers: numbers, ..self }
    }

    /// These options with or without `LSCO` 65 535 for unspecified layouts (W15).
    pub fn with_dsdiff_unspecified_loudspeaker_configuration(self, write: bool) -> Self {
        OutputOptions { dsdiff_unspecified_loudspeaker_configuration: write, ..self }
    }

    /// These options with or without `LSCO` for defined layouts (W16).
    pub fn with_dsdiff_loudspeaker_configuration(self, write: bool) -> Self {
        OutputOptions { dsdiff_loudspeaker_configuration: write, ..self }
    }

    /// These options with DSDIFF absolute start frame `frame` (W17).
    pub fn with_dsdiff_absolute_start(self, frame: Option<u32>) -> Self {
        OutputOptions { dsdiff_absolute_start: frame, ..self }
    }

    /// These options with DSDIFF local chunk order `order` (W13).
    pub fn with_dsdiff_local_order(self, order: Vec<LocalChunk>) -> Self {
        OutputOptions { dsdiff_local_order: order, ..self }
    }

    /// These options with or without `DSTC` frame CRCs (W19).
    pub fn with_dsdiff_dst_crc(self, write: bool) -> Self {
        OutputOptions { dsdiff_dst_crc: write, ..self }
    }

    /// These options with or without a `DSTI` sound index (W20).
    pub fn with_dsdiff_dst_index(self, write: bool) -> Self {
        OutputOptions { dsdiff_dst_index: write, ..self }
    }

    /// These options with `DSTI` offsets pointing at `offset` (W21).
    pub fn with_dsdiff_dst_index_offset(self, offset: DstIndexOffset) -> Self {
        OutputOptions { dsdiff_dst_index_offset: offset, ..self }
    }
}

/// What a region of an output file holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RegionKind {
    /// Chunk headers and other bytes before the sound data.
    Header,
    /// The sound data: audio of the range's positions, and the framing and padding the
    /// format puts among it.
    SoundData,
    /// Chunks after the sound data, such as a DST sound index.
    Trailer,
}

/// One contiguous region of an output file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Region {
    /// What it holds.
    pub kind: RegionKind,
    /// Its first byte's offset in the file.
    pub offset: u64,
    /// Its length in bytes.
    pub len: u64,
}

/// The contents behind a [`Region`].
#[derive(Clone, Debug)]
enum Part {
    /// Bytes built with the layout.
    Bytes(Vec<u8>),
    /// DSF sound data (8.2.2).
    Dsf(dsf::DsfData),
    /// DSDIFF plain DSD sound data (8.4.5).
    Dsd(dsdiff::DsdData),
    /// DSDIFF DST sound data (8.4.6).
    Dst(dsdiff_dst::DstData),
}

/// The content of timeline positions, which [`Layout::fill`] reads.
pub trait PositionSource {
    /// The content of position `t` (specification 4, 4.6): `C x 4704` bytes in interleaved
    /// layout. [`Layout::fill`] asks only for positions of [`Layout::positions_for`] of the
    /// bytes it fills, mostly in increasing order; it may ask for a position again.
    ///
    /// # Errors
    ///
    /// Any error is returned by [`Layout::fill`] unchanged.
    fn content(&mut self, t: u32) -> Result<Cow<'_, [u8]>, Error>;

    /// The DST equivalent of position `t` (specification 4, 4.6.1 item 3), as the file
    /// holds it: for a [`Format::DsdiffDst`] layout whose lengths are
    /// [`InvalidDst::Replaced`], an invalid DST frame's is the 0x55 fill's (4.6.1 item 5).
    /// Its length must be the one [`DstLengths`] gave. [`Layout::fill`] asks for it only
    /// for DST layouts ([`Layout::input`]).
    ///
    /// # Errors
    ///
    /// Any error is returned by [`Layout::fill`] unchanged. The default gives
    /// [`Error::InvalidArgument`]: a source of content only.
    fn dst_equivalent(&mut self, t: u32) -> Result<Cow<'_, [u8]>, Error> {
        let _ = t;
        Err(Error::InvalidArgument("this source gives no DST equivalents"))
    }
}

/// What [`Layout::fill`] asks a [`PositionSource`] for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Input {
    /// Content (specification 4, 4.6) only.
    Content,
    /// DST equivalents only.
    DstEquivalents,
    /// DST equivalents, and content for the frame CRCs.
    DstEquivalentsAndContent,
}

/// The layout of one output file: one format, one frame range `[first, end)` of one area
/// (specification 8, 8.1 item 2).
#[derive(Clone, Debug)]
pub struct Layout {
    format: Format,
    slot: Slot,
    first: u32,
    end: u32,
    channels: u8,
    size: u64,
    regions: Vec<Region>,
    parts: Vec<Part>,
    /// For DST layouts, what invalid DST frames are given as.
    invalid: Option<InvalidDst>,
    crc: bool,
    /// The image's TOC digests, for a layout made from a [`Disc`]: [`Layout::read_at`]
    /// refuses a disc of another image.
    image: Option<TocDigests>,
}

impl Layout {
    /// The layout of the frame range `[first, end)` of `area` as a `format` file.
    /// `image_sectors` is the image's logical sector count, which bounds the area's
    /// capacity; [`Disc::output_layout`] passes it for you.
    ///
    /// # Errors
    ///
    /// The errors of [`audio::check_range`], which apply first (8.1 item 2);
    /// [`Error::OutputRefused`] when the format can't hold the area with these options;
    /// [`Error::InvalidArgument`] when an option is invalid for the area.
    pub fn new(
        area: &Area,
        image_sectors: u64,
        first: u32,
        end: u32,
        format: Format,
        options: &OutputOptions,
    ) -> Result<Layout, Error> {
        audio::check_range(area, first, end, image_sectors)?;
        let mut layout = Layout {
            format,
            slot: area.slot,
            first,
            end,
            channels: area.channel_count,
            size: 0,
            regions: Vec::new(),
            parts: Vec::new(),
            invalid: None,
            crc: false,
            image: None,
        };
        match format {
            Format::Dsf => dsf::build(&mut layout, area, options)?,
            Format::DsdiffDsd => dsdiff::build_dsd(&mut layout, area, options)?,
            Format::DsdiffDst => {
                return Err(Error::InvalidArgument("a DST layout needs the frame lengths: Layout::new_dst"));
            }
        }
        Ok(layout)
    }

    /// The layout of a [`Format::DsdiffDst`] file holding the range of `lengths` of `area`
    /// (specification 8, 8.4.6): every size and the sound index follow from the lengths
    /// (8.4.6 "Sizes").
    ///
    /// # Errors
    ///
    /// As [`Layout::new`], and [`Error::InvalidArgument`] if `lengths` are of another area
    /// or range.
    pub fn new_dst(
        area: &Area,
        image_sectors: u64,
        lengths: &DstLengths,
        options: &OutputOptions,
    ) -> Result<Layout, Error> {
        let (first, end) = lengths.range();
        audio::check_range(area, first, end, image_sectors)?;
        let mut layout = Layout {
            format: Format::DsdiffDst,
            slot: area.slot,
            first,
            end,
            channels: area.channel_count,
            size: 0,
            regions: Vec::new(),
            parts: Vec::new(),
            invalid: Some(lengths.invalid()),
            crc: options.dsdiff_dst_crc,
            image: None,
        };
        let data = dsdiff_dst::DstData::new(&layout, lengths, options)?;
        dsdiff::assemble(&mut layout, area, options, dsdiff::Sound::Dst(data))?;
        Ok(layout)
    }

    /// Appends a region; regions are pushed in file order.
    fn push(&mut self, kind: RegionKind, len: u64, part: Part) {
        self.regions.push(Region { kind, offset: self.size, len });
        self.parts.push(part);
        self.size += len;
    }

    /// The file's size in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// The format.
    pub fn format(&self) -> Format {
        self.format
    }

    /// The area's slot.
    pub fn slot(&self) -> Slot {
        self.slot
    }

    /// The frame range `[first, end)` the file holds.
    pub fn range(&self) -> (u32, u32) {
        (self.first, self.end)
    }

    /// The area's channel count `C`, 1 to 6.
    pub fn channels(&self) -> u8 {
        self.channels
    }

    /// What [`Layout::fill`] asks its source for.
    pub fn input(&self) -> Input {
        match (self.invalid, self.crc) {
            (None, _) => Input::Content,
            (Some(_), false) => Input::DstEquivalents,
            (Some(_), true) => Input::DstEquivalentsAndContent,
        }
    }

    /// The file's regions, in order, covering `[0, size)`.
    pub fn regions(&self) -> &[Region] {
        &self.regions
    }

    /// The memory the layout holds, in bytes (approximate, without allocator overhead).
    pub fn memory(&self) -> usize {
        let parts: usize = self
            .parts
            .iter()
            .map(|p| match p {
                Part::Bytes(b) => b.len(),
                Part::Dst(d) => d.memory(),
                Part::Dsf(_) | Part::Dsd(_) => 0,
            })
            .sum();
        size_of::<Self>() + parts + self.regions.len() * (size_of::<Region>() + size_of::<Part>())
    }

    /// The timeline positions that [`Layout::fill`] may read to produce the file's bytes
    /// `range` (clipped to the file), as a frame range within [`Layout::range`]; empty when
    /// those bytes need no audio.
    pub fn positions_for(&self, range: Range<u64>) -> Range<u32> {
        let mut lo = u32::MAX;
        let mut hi = 0;
        for (region, part) in self.regions.iter().zip(&self.parts) {
            let start = range.start.max(region.offset);
            let end = range.end.min(region.offset + region.len);
            if start >= end {
                continue;
            }
            let p = match part {
                Part::Bytes(_) => continue,
                Part::Dsf(d) => d.positions(start - region.offset..end - region.offset),
                Part::Dsd(d) => d.positions(start - region.offset..end - region.offset),
                Part::Dst(d) => d.positions(start - region.offset..end - region.offset),
            };
            if !p.is_empty() {
                lo = lo.min(p.start);
                hi = hi.max(p.end);
            }
        }
        if lo >= hi { self.first..self.first } else { self.first + lo..self.first + hi }
    }

    /// Fills `out` with the file's bytes from `offset`, reading the content of positions
    /// from `source`, and returns how many bytes it filled: `out.len()`, or fewer at the
    /// end of the file (0 from the end on).
    ///
    /// # Errors
    ///
    /// The errors of `source`, and [`Error::InvalidArgument`] if it gives content of the
    /// wrong size. `out` is then partly filled.
    pub fn fill(&self, offset: u64, out: &mut [u8], source: &mut dyn PositionSource) -> Result<usize, Error> {
        let len =
            usize::try_from(self.size.saturating_sub(offset)).map_or(out.len(), |rest| rest.min(out.len()));
        let out = &mut out[..len];
        let mut source = Checked { inner: source, len: usize::from(self.channels) * FRAME_BYTES_PER_CHANNEL };
        let mut done = 0;
        for (region, part) in self.regions.iter().zip(&self.parts) {
            if done == len {
                break;
            }
            let at = offset + done as u64;
            if at >= region.offset + region.len {
                continue;
            }
            let rel = at - region.offset;
            let n = usize::try_from(region.len - rel).map_or(len - done, |rest| rest.min(len - done));
            let dst = &mut out[done..done + n];
            match part {
                Part::Bytes(b) => {
                    let rel = rel as usize; // below `b.len()`
                    dst.copy_from_slice(&b[rel..rel + n]);
                }
                Part::Dsf(d) => d.fill(rel, dst, self.first, &mut source)?,
                Part::Dsd(d) => d.fill(rel, dst, self.first, &mut source)?,
                Part::Dst(d) => d.fill(rel, dst, self.first, &mut source)?,
            }
            done += n;
        }
        Ok(len)
    }

    /// Reads the file's bytes from `offset` into `out` as [`Layout::fill`] does, taking the
    /// positions' content from `disc` (decoded, with its seek index when one is enabled).
    /// `disc` must be a disc of the image the layout was made from: for a layout of
    /// [`Disc::output_layout`] or [`Disc::output_layout_dst`], that is checked against the
    /// digests of the TOC sectors (as [`Disc::share`] checks a reader), at no cost.
    ///
    /// # Errors
    ///
    /// The errors of [`Disc::range_audio`], and [`Error::InvalidArgument`] if `disc` is of
    /// another image, has no such area, or one of another channel count.
    pub fn read_at<R: Read + Seek>(
        &self,
        disc: &mut Disc<R>,
        offset: u64,
        out: &mut [u8],
    ) -> Result<usize, Error> {
        if self.image.as_ref().is_some_and(|d| d != disc.toc_digests()) {
            return Err(Error::InvalidArgument("the layout is of another image"));
        }
        let area = disc.toc().area(self.slot).ok_or(Error::NoSuchArea(self.slot))?;
        if area.channel_count != self.channels {
            return Err(Error::InvalidArgument("the layout is of another area"));
        }
        let end = offset.saturating_add(out.len() as u64).min(self.size);
        let positions = self.positions_for(offset..end);
        if positions.is_empty() {
            return self.fill(offset, out, &mut NoPositions);
        }
        match self.invalid {
            None => {
                let frames = disc.range_audio(self.slot, positions.start, positions.end)?;
                self.fill(offset, out, &mut Window { frames, window: VecDeque::new() })
            }
            Some(invalid) => {
                let frames = disc.range_frames(self.slot, positions.start, positions.end)?;
                let mut source = StoredWindow {
                    frames,
                    window: VecDeque::new(),
                    decoder: DstDecoder::new(),
                    channels: self.channels,
                    invalid,
                };
                self.fill(offset, out, &mut source)
            }
        }
    }
}

impl<R: Read + Seek> Disc<R> {
    /// The [`Layout`] of the frame range `[first, end)` of the area from `slot` as a
    /// `format` file (specification 8). A track's range is
    /// `(start_frame, start_frame + duration_frames)`; a track with pause's is
    /// [`Disc::track_range_with_pause`]'s.
    ///
    /// # Errors
    ///
    /// [`Error::NoSuchArea`], then those of [`Layout::new`].
    pub fn output_layout(
        &self,
        slot: Slot,
        first: u32,
        end: u32,
        format: Format,
        options: &OutputOptions,
    ) -> Result<Layout, Error> {
        let area = self.toc().area(slot).ok_or(Error::NoSuchArea(slot))?;
        let mut layout = Layout::new(area, self.image_sectors(), first, end, format, options)?;
        layout.image = Some(self.toc_digests().clone());
        Ok(layout)
    }

    /// The [`Format::DsdiffDst`] [`Layout`] of the range of `lengths` of their area (see
    /// [`Layout::new_dst`]).
    ///
    /// # Errors
    ///
    /// [`Error::NoSuchArea`], then those of [`Layout::new_dst`].
    pub fn output_layout_dst(&self, lengths: &DstLengths, options: &OutputOptions) -> Result<Layout, Error> {
        let area = self.toc().area(lengths.slot()).ok_or(Error::NoSuchArea(lengths.slot()))?;
        let mut layout = Layout::new_dst(area, self.image_sectors(), lengths, options)?;
        layout.image = Some(self.toc_digests().clone());
        Ok(layout)
    }
}

/// A source whose content sizes are checked.
struct Checked<'a> {
    inner: &'a mut dyn PositionSource,
    len: usize,
}

impl Checked<'_> {
    fn content(&mut self, t: u32) -> Result<Cow<'_, [u8]>, Error> {
        let len = self.len;
        let c = self.inner.content(t)?;
        if c.len() != len {
            return Err(Error::InvalidArgument("position content must be C x 4704 bytes"));
        }
        Ok(c)
    }

    fn dst_equivalent(&mut self, t: u32) -> Result<Cow<'_, [u8]>, Error> {
        self.inner.dst_equivalent(t)
    }
}

/// The source of a read that needs no positions.
struct NoPositions;

impl PositionSource for NoPositions {
    fn content(&mut self, _t: u32) -> Result<Cow<'_, [u8]>, Error> {
        Err(Error::InvalidArgument("no position is needed"))
    }
}

/// Positions kept behind the latest one asked for: a DSF row reads at most 2 positions
/// per channel, from the row's first position again for each channel.
const WINDOW: u32 = 4;

/// Decoded positions of one range, read in order, with the last few kept.
struct Window<'a, R> {
    frames: TrackAudio<'a, R>,
    window: VecDeque<AudioFrame>,
}

impl<R: Read + Seek> PositionSource for Window<'_, R> {
    fn content(&mut self, t: u32) -> Result<Cow<'_, [u8]>, Error> {
        while self.window.back().is_none_or(|f| f.number < t) {
            let frame = self.frames.next().ok_or(Error::InvalidArgument("position past the range"))??;
            self.window.push_back(frame);
        }
        while self.window.front().is_some_and(|f| f.number.saturating_add(WINDOW) < t) {
            self.window.pop_front();
        }
        let front = self.window.front().map_or(t, |f| f.number);
        let i = t.checked_sub(front).ok_or(Error::InvalidArgument("position no longer held"))?;
        let frame = self.window.get(i as usize).ok_or(Error::InvalidArgument("position not held"))?;
        Ok(Cow::Borrowed(&frame.data))
    }
}

/// A stored position, with its content once decoded.
struct Stored {
    frame: TrackFrame,
    decoded: Option<AudioFrame>,
}

/// Stored positions of one range, read in order, with the last few kept, decoded only
/// when content is asked for or invalid frames are replaced.
struct StoredWindow<'a, R> {
    frames: TrackFrames<'a, R>,
    window: VecDeque<Stored>,
    decoder: DstDecoder,
    channels: u8,
    invalid: InvalidDst,
}

impl<R: Read + Seek> StoredWindow<'_, R> {
    fn get(&mut self, t: u32, decode: bool) -> Result<&Stored, Error> {
        while self.window.back().is_none_or(|f| f.frame.number < t) {
            let frame = self.frames.next().ok_or(Error::InvalidArgument("position past the range"))??;
            self.window.push_back(Stored { frame, decoded: None });
        }
        while self.window.front().is_some_and(|f| f.frame.number.saturating_add(WINDOW) < t) {
            self.window.pop_front();
        }
        let front = self.window.front().map_or(t, |f| f.frame.number);
        let i = t.checked_sub(front).ok_or(Error::InvalidArgument("position no longer held"))?;
        let held = self.window.get_mut(i as usize).ok_or(Error::InvalidArgument("position not held"))?;
        if decode && held.decoded.is_none() {
            held.decoded = Some(audio::decode_frame(&mut self.decoder, held.frame.clone(), self.channels)?);
        }
        Ok(held)
    }
}

impl<R: Read + Seek> PositionSource for StoredWindow<'_, R> {
    fn content(&mut self, t: u32) -> Result<Cow<'_, [u8]>, Error> {
        let held = self.get(t, true)?;
        Ok(Cow::Borrowed(held.decoded.as_ref().map_or(&[][..], |d| &d.data)))
    }

    fn dst_equivalent(&mut self, t: u32) -> Result<Cow<'_, [u8]>, Error> {
        let channels = self.channels;
        let replace = self.invalid == InvalidDst::Replaced;
        let held = self.get(t, replace)?;
        let invalid = matches!(held.frame.content, FrameContent::Dst(_))
            && held.decoded.as_ref().is_some_and(|d| matches!(d.status, audio::FrameStatus::Invalid(_)));
        if invalid {
            return Ok(Cow::Owned(dsdiff_dst::replaced(channels)));
        }
        held.frame.content.dst_equivalent(channels)
    }
}
