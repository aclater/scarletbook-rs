// SPDX-License-Identifier: MIT OR Apache-2.0

//! DSDIFF (specification 8, 8.4).

use std::ops::Range;

use super::{Checked, Layout, OutputOptions, Part, RegionKind};
use crate::audio::FRAME_BYTES_PER_CHANNEL;
use crate::error::Error;
use crate::toc::{Area, LoudspeakerLayout};

/// The sample rate written (8.1 item 4, 8.4.3).
const SAMPLE_RATE: u32 = 2_822_400;

/// Frames per hour, the hours unit of a DSDIFF time (8.4.7 item 1).
const FRAMES_PER_HOUR: u64 = 270_000;

/// The largest `A + N` an absolute start time allows (8.4.8): every time stays below 24 hours.
pub(super) const MAX_TIME_FRAMES: u64 = 6_479_999;

/// A chunk of the property chunk (specification 8, 8.4.3), for
/// [`OutputOptions::dsdiff_property_order`] (writer's choice W14).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PropertyChunk {
    /// `FS  `, the sample rate.
    SampleRate,
    /// `CHNL`, the channels.
    Channels,
    /// `CMPR`, the compression type.
    CompressionType,
    /// `ABSS`, the absolute start time (written when [`OutputOptions::dsdiff_absolute_start`]
    /// is set).
    AbsoluteStartTime,
    /// `LSCO`, the loudspeaker configuration (written as W15 and W16 say).
    LoudspeakerConfiguration,
}

impl PropertyChunk {
    /// The order of the table of 8.4.3, the default.
    pub const TABLE_ORDER: [PropertyChunk; 5] = [
        PropertyChunk::SampleRate,
        PropertyChunk::Channels,
        PropertyChunk::CompressionType,
        PropertyChunk::AbsoluteStartTime,
        PropertyChunk::LoudspeakerConfiguration,
    ];
}

/// Appends a chunk: its ID, its data size `D` (u64, most significant byte first), its data,
/// and a pad byte 0x00 when `D` is odd (8.4.1 item 1).
pub(super) fn chunk(out: &mut Vec<u8>, id: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(data.len() as u64).to_be_bytes());
    out.extend_from_slice(data);
    if data.len() % 2 == 1 {
        out.push(0);
    }
}

/// The space a chunk of data size `d` occupies (8.4.1 item 1).
pub(super) fn occupied(d: u64) -> u64 {
    12 + d + d % 2
}

/// A DSDIFF time of frame `g`: hours, minutes, seconds, samples (8.4.7 item 1).
pub(super) fn time(g: u64) -> (u16, u8, u8, u32) {
    // `g` is at most `MAX_TIME_FRAMES`, so the hours are below 24.
    ((g / FRAMES_PER_HOUR) as u16, (g / 4500 % 60) as u8, (g / 75 % 60) as u8, (g % 75 * 37_632) as u32)
}

/// The channel IDs and `LSCO` value of the area (8.4.4), or of an unspecified layout from
/// the options (W15).
fn channels(area: &Area, options: &OutputOptions) -> Result<(Vec<[u8; 4]>, Option<u16>), Error> {
    let lsco = |v| options.dsdiff_loudspeaker_configuration.then_some(v);
    Ok(match area.loudspeaker_layout() {
        LoudspeakerLayout::Stereo => (vec![*b"SLFT", *b"SRGT"], lsco(0)),
        LoudspeakerLayout::ThreeTwo => (vec![*b"MLFT", *b"MRGT", *b"C   ", *b"LS  ", *b"RS  "], lsco(3)),
        LoudspeakerLayout::FivePointOne => {
            (vec![*b"MLFT", *b"MRGT", *b"C   ", *b"LFE ", *b"LS  ", *b"RS  "], lsco(4))
        }
        LoudspeakerLayout::Unspecified { channels, .. } => {
            let numbers: Vec<u16> = match &options.dsdiff_unspecified_channel_numbers {
                Some(n) => n.clone(),
                None => (0..u16::from(channels)).collect(),
            };
            if numbers.len() != usize::from(channels) {
                return Err(Error::InvalidArgument("give one DSDIFF channel number per channel"));
            }
            let mut seen = [false; 1000];
            let mut ids = Vec::with_capacity(numbers.len());
            for n in numbers {
                let slot = seen
                    .get_mut(usize::from(n))
                    .ok_or(Error::InvalidArgument("DSDIFF channel numbers are 0 to 999"))?;
                if std::mem::replace(slot, true) {
                    return Err(Error::InvalidArgument("DSDIFF channel numbers must be distinct"));
                }
                let d = |p: u16| b'0' + (n / p % 10) as u8;
                ids.push([b'C', d(100), d(10), d(1)]);
            }
            (ids, options.dsdiff_unspecified_loudspeaker_configuration.then_some(65_535))
        }
    })
}

/// The compression type of plain DSD (8.4.3).
const CMPR_DSD: (&[u8; 4], &[u8]) = (b"DSD ", b"not compressed");

/// Builds `FVER` and `PROP` (8.4.2, 8.4.3) for compression `cmpr`.
fn property(area: &Area, options: &OutputOptions, n: u64, cmpr: (&[u8; 4], &[u8])) -> Result<Vec<u8>, Error> {
    let (ids, lsco) = channels(area, options)?;
    let order = options.dsdiff_property_order;
    let mut sorted = order;
    sorted.sort_by_key(|c| PropertyChunk::TABLE_ORDER.iter().position(|t| t == c));
    if sorted != PropertyChunk::TABLE_ORDER {
        return Err(Error::InvalidArgument("the property order must name each property chunk once"));
    }
    if let Some(a) = options.dsdiff_absolute_start {
        if u64::from(a) + n > MAX_TIME_FRAMES {
            return Err(Error::InvalidArgument(
                "the absolute start time plus the range must stay below 24 hours",
            ));
        }
    }
    let mut prop = b"SND ".to_vec();
    for c in order {
        match c {
            PropertyChunk::SampleRate => chunk(&mut prop, b"FS  ", &SAMPLE_RATE.to_be_bytes()),
            PropertyChunk::Channels => {
                let mut d = (ids.len() as u16).to_be_bytes().to_vec();
                ids.iter().for_each(|id| d.extend_from_slice(id));
                chunk(&mut prop, b"CHNL", &d);
            }
            PropertyChunk::CompressionType => {
                let mut d = cmpr.0.to_vec();
                d.push(cmpr.1.len() as u8);
                d.extend_from_slice(cmpr.1);
                chunk(&mut prop, b"CMPR", &d);
            }
            PropertyChunk::AbsoluteStartTime => {
                if let Some(a) = options.dsdiff_absolute_start {
                    let (h, m, s, samples) = time(u64::from(a));
                    let mut d = h.to_be_bytes().to_vec();
                    d.extend_from_slice(&[m, s]);
                    d.extend_from_slice(&samples.to_be_bytes());
                    chunk(&mut prop, b"ABSS", &d);
                }
            }
            PropertyChunk::LoudspeakerConfiguration => {
                if let Some(v) = lsco {
                    chunk(&mut prop, b"LSCO", &v.to_be_bytes());
                }
            }
        }
    }
    let mut out = Vec::new();
    chunk(&mut out, b"FVER", &0x0105_0000u32.to_be_bytes());
    chunk(&mut out, b"PROP", &prop);
    Ok(out)
}

/// Builds the regions of a plain DSD DSDIFF file (8.4.2, 8.4.5).
pub(super) fn build_dsd(layout: &mut Layout, area: &Area, options: &OutputOptions) -> Result<(), Error> {
    let positions = u64::from(layout.end - layout.first);
    let local = property(area, options, positions, CMPR_DSD)?;
    let d = positions * u64::from(layout.channels) * FRAME_BYTES_PER_CHANNEL as u64;
    // `FRM8` data: the ID `DSD `, then the local chunks (8.4.2 item 4); `d` is even (8.4.5).
    let frm8 = 4 + local.len() as u64 + occupied(d);
    let mut h = Vec::with_capacity(16 + local.len() + 12);
    h.extend_from_slice(b"FRM8");
    h.extend_from_slice(&frm8.to_be_bytes());
    h.extend_from_slice(b"DSD ");
    h.extend_from_slice(&local);
    h.extend_from_slice(b"DSD ");
    h.extend_from_slice(&d.to_be_bytes());
    layout.push(RegionKind::Header, h.len() as u64, Part::Bytes(h));
    layout.push(RegionKind::SoundData, d, Part::Dsd(DsdData { channels: layout.channels }));
    Ok(())
}

/// Plain DSD sound data (8.4.5): the positions' content, in order, unchanged.
#[derive(Clone, Debug)]
pub(super) struct DsdData {
    channels: u8,
}

impl DsdData {
    fn frame(&self) -> u64 {
        u64::from(self.channels) * FRAME_BYTES_PER_CHANNEL as u64
    }

    /// The positions, relative to the range's first, that sound data bytes `range` read.
    pub(super) fn positions(&self, range: Range<u64>) -> Range<u32> {
        // Below `N`, a u32.
        (range.start / self.frame()) as u32..((range.end - 1) / self.frame() + 1) as u32
    }

    /// Fills `out` with sound data bytes from `offset`: byte `o mod 4704C` of the content of
    /// position `o / 4704C`.
    pub(super) fn fill(
        &self,
        offset: u64,
        out: &mut [u8],
        first: u32,
        source: &mut Checked<'_>,
    ) -> Result<(), Error> {
        let frame = self.frame();
        let mut done = 0;
        while done < out.len() {
            let o = offset + done as u64;
            let within = (o % frame) as usize;
            let content = source.content(first + (o / frame) as u32)?;
            let n = (content.len() - within).min(out.len() - done);
            out[done..done + n].copy_from_slice(&content[within..within + n]);
            done += n;
        }
        Ok(())
    }
}
