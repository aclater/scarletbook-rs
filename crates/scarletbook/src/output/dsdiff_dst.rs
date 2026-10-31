// SPDX-License-Identifier: MIT OR Apache-2.0

//! DSDIFF with DST sound data (specification 8, 8.4.6): the frame lengths a layout needs,
//! the `DSTF` and `DSTC` chunks, the frame CRC and the sound index.

use std::io::{Read, Seek};
use std::ops::Range;

use super::{Checked, Layout, OutputOptions};
use crate::audio::{
    self, FRAME_BYTES_PER_CHANNEL, FrameContent, FrameStatus, INVALID_FILL, MAX_FRAME_BYTES, TrackFrame,
};
use crate::disc::Disc;
use crate::dst::DstDecoder;
use crate::error::{Error, Slot};
use crate::toc::Area;

/// What a DST file holds for an invalid DST frame (specification 8, 8.4.6; W18).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum InvalidDst {
    /// The frame's stored bytes, as for every other DST frame. Lengths need no decoding.
    Stored,
    /// The byte 0x00 followed by `C x 4704` bytes of 0x55 (specification 4, 4.6.1 item 5),
    /// which decoders outside these specifications decode to the same content. Finding
    /// the invalid frames needs decoding every DST frame.
    Replaced,
}

/// What a DSTI entry's offset points at (specification 8, 8.4.6.2 item 3; W21). The DSDIFF document
/// is ambiguous; the two readings differ by 12 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DstIndexOffset {
    /// The file position of the `DSTF` chunk's first header byte.
    ChunkHeader,
    /// The file position of the frame's first byte, 12 bytes after the chunk header.
    FrameData,
}

/// The lengths of the DST equivalents of a frame range (specification 8, 8.4.6 "Sizes"):
/// what a DST layout needs besides the TOC.
///
/// A DST frame's length is known only once it is reassembled (specification 4, 4.4), so
/// [`DstLengths::scan`] reads the range's sectors, without decoding unless invalid frames
/// are [`InvalidDst::Replaced`]. The lengths can be saved ([`DstLengths::lengths`]) and
/// loaded again ([`DstLengths::from_lengths`]) with the image's identity, as a seek index
/// is. One length takes 4 bytes; a range has at most 7 positions per audio sector
/// (specification 4, 4.8 item 3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DstLengths {
    slot: Slot,
    first: u32,
    end: u32,
    channels: u8,
    invalid: InvalidDst,
    lengths: Vec<u32>,
}

/// The length of a plain or missing position's DST equivalent, and of a replaced invalid
/// frame's.
fn plain_len(channels: u8) -> u32 {
    // At most 1 + 6 x 4704.
    1 + u32::from(channels) * FRAME_BYTES_PER_CHANNEL as u32
}

impl DstLengths {
    /// Reads the lengths of the DST equivalents of the frame range `[first, end)` of the
    /// area from `slot`: the stored length of a DST frame, `1 + C x 4704` for plain and
    /// missing positions, and for invalid DST frames when `invalid` is
    /// [`InvalidDst::Replaced`] (which decodes every DST frame).
    ///
    /// # Errors
    ///
    /// Those of [`Disc::range_frames`] and of reading the range.
    pub fn scan<R: Read + Seek>(
        disc: &mut Disc<R>,
        slot: Slot,
        first: u32,
        end: u32,
        invalid: InvalidDst,
    ) -> Result<DstLengths, Error> {
        let channels = disc.toc().area(slot).ok_or(Error::NoSuchArea(slot))?.channel_count;
        let mut lengths = Vec::new();
        let mut decoder = DstDecoder::new();
        for frame in disc.range_frames(slot, first, end)? {
            let frame = frame?;
            let len = match &frame.content {
                FrameContent::Dst(bytes) => {
                    if invalid == InvalidDst::Replaced && is_invalid(&mut decoder, &frame, channels)? {
                        plain_len(channels)
                    } else {
                        // At most `MAX_FRAME_BYTES`.
                        bytes.len() as u32
                    }
                }
                FrameContent::Dsd(_) | FrameContent::Missing => plain_len(channels),
            };
            lengths.push(len);
        }
        Ok(DstLengths { slot, first, end, channels, invalid, lengths })
    }

    /// Lengths saved from [`DstLengths::scan`] of the same range of `area`, in position
    /// order. They are checked for their count (one per position) and their bounds (1 to
    /// 65 536); that they are the image's is the caller's to know.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`] if they fail those checks or the area's channel count
    /// is not 1 to 6.
    pub fn from_lengths(
        area: &Area,
        first: u32,
        end: u32,
        invalid: InvalidDst,
        lengths: Vec<u32>,
    ) -> Result<DstLengths, Error> {
        audio::check_channels(area)?;
        if first > end || lengths.len() as u64 != u64::from(end - first) {
            return Err(Error::InvalidArgument("give one length per position of the range"));
        }
        if lengths.iter().any(|&l| l == 0 || l as usize > MAX_FRAME_BYTES) {
            return Err(Error::InvalidArgument("a DST equivalent is 1 to 65 536 bytes"));
        }
        Ok(DstLengths { slot: area.slot, first, end, channels: area.channel_count, invalid, lengths })
    }

    /// The lengths, one per position from the range's first.
    pub fn lengths(&self) -> &[u32] {
        &self.lengths
    }

    /// The area's slot.
    pub fn slot(&self) -> Slot {
        self.slot
    }

    /// The frame range `[first, end)`.
    pub fn range(&self) -> (u32, u32) {
        (self.first, self.end)
    }

    /// What invalid DST frames are given as.
    pub fn invalid(&self) -> InvalidDst {
        self.invalid
    }
}

/// Whether a DST frame is invalid (specification 5, 5.6), by decoding it.
pub(super) fn is_invalid(decoder: &mut DstDecoder, frame: &TrackFrame, channels: u8) -> Result<bool, Error> {
    Ok(matches!(audio::decode_frame(decoder, frame.clone(), channels)?.status, FrameStatus::Invalid(_)))
}

/// The DST sound data of a layout: the `DSTF` (and `DSTC`) chunks after `FRTE`.
#[derive(Clone, Debug)]
pub(super) struct DstData {
    lengths: Vec<u32>,
    /// `starts[k]`: where position `k`'s unit (`DSTF`, then `DSTC`) begins, from the first
    /// unit; `N + 1` entries.
    starts: Vec<u64>,
}

/// Bytes of a `DSTC` chunk (8.4.6.1).
const DSTC: u64 = 16;

impl DstData {
    /// The sound data of `lengths` for `layout`'s range.
    pub(super) fn new(
        layout: &Layout,
        lengths: &DstLengths,
        options: &OutputOptions,
    ) -> Result<DstData, Error> {
        if (lengths.slot, lengths.first, lengths.end, lengths.channels)
            != (layout.slot, layout.first, layout.end, layout.channels)
        {
            return Err(Error::InvalidArgument("the DST lengths are of another range"));
        }
        let crc = options.dsdiff_dst_crc;
        let mut starts = Vec::with_capacity(lengths.lengths.len() + 1);
        let mut at = 0u64;
        starts.push(at);
        for &l in &lengths.lengths {
            at += super::dsdiff::occupied(u64::from(l)) + if crc { DSTC } else { 0 };
            starts.push(at);
        }
        Ok(DstData { lengths: lengths.lengths.clone(), starts })
    }

    /// The sound data chunk's header and its `FRTE` chunk (8.4.6 items 1).
    pub(super) fn head(&self) -> Vec<u8> {
        let mut h = Vec::with_capacity(30);
        super::dsdiff::chunk_head(&mut h, b"DST ", self.data_size());
        let mut frte = (self.lengths.len() as u32).to_be_bytes().to_vec();
        frte.extend_from_slice(&75u16.to_be_bytes());
        super::dsdiff::chunk(&mut h, b"FRTE", &frte);
        h
    }

    /// The `DST ` chunk's data size: `FRTE` and the units.
    pub(super) fn data_size(&self) -> u64 {
        18 + self.units()
    }

    /// The memory it holds, in bytes.
    pub(super) fn memory(&self) -> usize {
        self.lengths.len() * size_of::<u32>() + self.starts.len() * size_of::<u64>()
    }

    fn units(&self) -> u64 {
        self.starts.last().copied().unwrap_or(0)
    }

    /// The `DSTI` entries (8.4.6.2), with the first unit at file offset `base`.
    pub(super) fn index(&self, base: u64, offset: DstIndexOffset) -> Vec<u8> {
        let shift = match offset {
            DstIndexOffset::ChunkHeader => 0,
            DstIndexOffset::FrameData => 12,
        };
        let mut v = Vec::with_capacity(12 * self.lengths.len());
        for (start, &len) in self.starts.iter().zip(&self.lengths) {
            v.extend_from_slice(&(base + start + shift).to_be_bytes());
            v.extend_from_slice(&len.to_be_bytes());
        }
        v
    }

    /// The unit holding sound data byte `o` (below the units' size).
    fn unit(&self, o: u64) -> usize {
        self.starts.partition_point(|&s| s <= o) - 1
    }

    /// The positions, relative to the range's first, that sound data bytes `range` read.
    pub(super) fn positions(&self, range: Range<u64>) -> Range<u32> {
        // Below `N`, a u32.
        self.unit(range.start) as u32..self.unit(range.end - 1) as u32 + 1
    }

    /// Fills `out` with sound data bytes from `offset`: for each position, a `DSTF` chunk
    /// of its DST equivalent with its pad byte, then, with CRCs, a `DSTC` chunk of its
    /// content's CRC (8.4.6, 8.4.6.1).
    pub(super) fn fill(
        &self,
        offset: u64,
        out: &mut [u8],
        first: u32,
        source: &mut Checked<'_>,
    ) -> Result<(), Error> {
        let mut done = 0;
        while done < out.len() {
            let o = offset + done as u64;
            let k = self.unit(o);
            let t = first + k as u32;
            let len = u64::from(self.lengths[k]);
            let mut unit = Vec::with_capacity(28);
            let mut within = o - self.starts[k];
            let rest = (self.starts[k + 1] - o).min((out.len() - done) as u64) as usize;
            let dst = &mut out[done..done + rest];
            let mut at = 0;
            while at < rest {
                if within < 12 {
                    super::dsdiff::chunk_head(&mut unit, b"DSTF", len);
                    let n = (12 - within as usize).min(rest - at);
                    dst[at..at + n].copy_from_slice(&unit[within as usize..within as usize + n]);
                    unit.clear();
                    at += n;
                    within += n as u64;
                } else if within < 12 + len {
                    let eq = source.dst_equivalent(t)?;
                    if eq.len() as u64 != len {
                        return Err(Error::InvalidArgument(
                            "a DST equivalent's length differs from the DST lengths",
                        ));
                    }
                    let from = (within - 12) as usize;
                    let n = (eq.len() - from).min(rest - at);
                    dst[at..at + n].copy_from_slice(&eq[from..from + n]);
                    at += n;
                    within += n as u64;
                } else if within < 12 + len + len % 2 {
                    dst[at] = 0;
                    at += 1;
                    within += 1;
                } else {
                    let from = (within - 12 - len - len % 2) as usize;
                    super::dsdiff::chunk_head(&mut unit, b"DSTC", 4);
                    unit.extend_from_slice(&crc(&source.content(t)?).to_be_bytes());
                    let n = (unit.len() - from).min(rest - at);
                    dst[at..at + n].copy_from_slice(&unit[from..from + n]);
                    unit.clear();
                    at += n;
                    within += n as u64;
                }
            }
            done += rest;
        }
        Ok(())
    }
}

/// The frame CRC of 8.4.6.1: the remainder of the content, most significant bit first,
/// followed by 32 zero bits, divided by `x^32 + x^31 + x^4 + 1`; no initial value, no final
/// exclusive-or, no reflection.
pub(super) fn crc(content: &[u8]) -> u32 {
    content.iter().fold(0u32, |r, &b| (r << 8) ^ CRC_TABLE[usize::from((r >> 24) as u8 ^ b)])
}

/// The polynomial without its `x^32` term.
const POLY: u32 = 0x8000_0011;

/// The remainder of each byte value followed by 32 zero bits.
const CRC_TABLE: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut r = (i as u32) << 24;
        let mut bit = 0;
        while bit < 8 {
            r = if r & 0x8000_0000 != 0 { (r << 1) ^ POLY } else { r << 1 };
            bit += 1;
        }
        t[i] = r;
        i += 1;
    }
    t
};

/// The DST equivalent of an invalid DST frame replaced (W18): 0x00, then `C x 4704` bytes
/// of 0x55 (specification 4, 4.6.1 item 5).
pub(super) fn replaced(channels: u8) -> Vec<u8> {
    let mut v = vec![INVALID_FILL; 1 + usize::from(channels) * FRAME_BYTES_PER_CHANNEL];
    v[0] = 0;
    v
}

#[cfg(test)]
mod tests {
    use super::crc;

    /// The test vectors of specification 8, 8.4.6.1.
    #[test]
    fn crc_matches_the_vectors_of_8_4_6_1() {
        assert_eq!(crc(&[0x69; 4704]), 0x2159_B5D9);
        assert_eq!(crc(&[0x69; 9408]), 0x2B3A_B9F8);
        assert_eq!(crc(&[0x55; 9408]), 0x380C_A058);
    }
}
