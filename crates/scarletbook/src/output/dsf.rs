// SPDX-License-Identifier: MIT OR Apache-2.0

//! DSF (specification 8, 8.2).

use std::ops::Range;

use super::{Checked, Layout, OutputOptions, Part, RegionKind};
use crate::audio::FRAME_BYTES_PER_CHANNEL;
use crate::error::{Error, OutputRefusal};
use crate::toc::{Area, LoudspeakerLayout};

/// Block size per channel (8.2.1, 8.2.2 item 1).
const BLOCK: u64 = 4096;

/// Bytes before the sound data: the `DSD `, `fmt ` and `data` chunk headers (8.2.1).
const HEADER: usize = 92;

/// Samples per DSD byte.
const SAMPLES_PER_BYTE: u64 = 8;

/// The sample rate written (8.1 item 4).
const SAMPLE_RATE: u32 = 2_822_400;

/// DSF bits per sample (specification 8, 8.2.3; writer's choice W1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DsfBitsPerSample {
    /// 1: each DSD byte stored with its bits reversed, the earliest sample in bit 0.
    One,
    /// 8: each DSD byte stored unchanged, the earliest sample in bit 7.
    Eight,
}

/// The sound data of a DSF file (8.2.2).
#[derive(Clone, Debug)]
pub(super) struct DsfData {
    channels: u8,
    /// `S`, DSD bytes per channel.
    stream: u64,
    reverse: bool,
}

/// The channel type of a layout (8.2.4), or of an unspecified one from the options.
fn channel_type(area: &Area, options: &OutputOptions) -> Result<u32, Error> {
    match area.loudspeaker_layout() {
        LoudspeakerLayout::Stereo => Ok(2),
        LoudspeakerLayout::ThreeTwo => Ok(6),
        LoudspeakerLayout::FivePointOne => Ok(7),
        LoudspeakerLayout::Unspecified { channels, config } => {
            let t = options
                .dsf_unspecified_channel_type
                .ok_or(Error::OutputRefused(OutputRefusal::UnspecifiedLayout { channels, config }))?;
            // The channel count of each type [DSF] defines (8.2.4 item 2).
            let count = match t {
                1 => 1,
                2 => 2,
                3 => 3,
                4 | 5 => 4,
                6 => 5,
                7 => 6,
                _ => return Err(Error::InvalidArgument("DSF channel types are 1 to 7")),
            };
            if count != channels {
                return Err(Error::InvalidArgument(
                    "the DSF channel type must have the area's channel count",
                ));
            }
            Ok(t)
        }
    }
}

/// Builds the regions of a DSF file without a tag (8.2.1, 8.2.5).
pub(super) fn build(layout: &mut Layout, area: &Area, options: &OutputOptions) -> Result<(), Error> {
    let channel_type = channel_type(area, options)?;
    let c = u64::from(layout.channels);
    let positions = u64::from(layout.end - layout.first);
    let stream = positions * FRAME_BYTES_PER_CHANNEL as u64;
    let blocks = stream.div_ceil(BLOCK);
    let n = BLOCK * blocks * c;
    // No tag: m = 0 and the metadata pointer is 0 (8.2.6 item 2).
    let total = HEADER as u64 + n;
    let bits = match options.dsf_bits_per_sample {
        DsfBitsPerSample::One => 1,
        DsfBitsPerSample::Eight => 8,
    };

    let mut h = Vec::with_capacity(HEADER);
    h.extend_from_slice(b"DSD ");
    h.extend_from_slice(&28u64.to_le_bytes());
    h.extend_from_slice(&total.to_le_bytes());
    h.extend_from_slice(&0u64.to_le_bytes());
    h.extend_from_slice(b"fmt ");
    h.extend_from_slice(&52u64.to_le_bytes());
    h.extend_from_slice(&1u32.to_le_bytes());
    h.extend_from_slice(&0u32.to_le_bytes());
    h.extend_from_slice(&channel_type.to_le_bytes());
    h.extend_from_slice(&u32::from(layout.channels).to_le_bytes());
    h.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    h.extend_from_slice(&(bits as u32).to_le_bytes());
    h.extend_from_slice(&(stream * SAMPLES_PER_BYTE).to_le_bytes());
    h.extend_from_slice(&(BLOCK as u32).to_le_bytes());
    h.extend_from_slice(&0u32.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&(n + 12).to_le_bytes());
    debug_assert_eq!(h.len(), HEADER);

    layout.push(RegionKind::Header, HEADER as u64, Part::Bytes(h));
    let data = DsfData { channels: layout.channels, stream, reverse: bits == 1 };
    layout.push(RegionKind::SoundData, n, Part::Dsf(data));
    Ok(())
}

/// Each byte with its bits reversed: bit `i` becomes bit `7 - i` (8.2.3).
const REVERSED: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        t[i] = (i as u8).reverse_bits();
        i += 1;
    }
    t
};

impl DsfData {
    /// Bytes of one row: block `j` of every channel (8.2.2 item 2).
    fn row(&self) -> u64 {
        BLOCK * u64::from(self.channels)
    }

    /// The positions, relative to the range's first, that sound data bytes `range` read:
    /// every stream byte of the rows they touch, below `S`.
    pub(super) fn positions(&self, range: Range<u64>) -> Range<u32> {
        let row = self.row();
        let lo = range.start / row * BLOCK;
        let hi = ((range.end - 1) / row + 1).saturating_mul(BLOCK).min(self.stream);
        if lo >= hi {
            return 0..0;
        }
        let per = FRAME_BYTES_PER_CHANNEL as u64;
        // Below `N`, a u32.
        (lo / per) as u32..((hi - 1) / per + 1) as u32
    }

    /// Fills `out` with sound data bytes from `offset` (8.2.2): data byte `o` is in row
    /// `j = o / 4096C`, channel `c = (o mod 4096C) / 4096`, at stream byte
    /// `k = 4096j + o mod 4096`; it is stream byte `k` of channel `c` as 8.2.3 stores it, or
    /// the pad 0x00 when `k >= S`.
    pub(super) fn fill(
        &self,
        offset: u64,
        out: &mut [u8],
        first: u32,
        source: &mut Checked<'_>,
    ) -> Result<(), Error> {
        let row = self.row();
        let c = usize::from(self.channels);
        let per = FRAME_BYTES_PER_CHANNEL as u64;
        let mut done = 0;
        while done < out.len() {
            let o = offset + done as u64;
            let channel = ((o % row) / BLOCK) as usize;
            let mut k = o / row * BLOCK + o % BLOCK;
            // The rest of this block.
            let run =
                usize::try_from(BLOCK - o % BLOCK).map_or(out.len() - done, |r| r.min(out.len() - done));
            let block = &mut out[done..done + run];
            let mut at = 0;
            while at < run {
                if k >= self.stream {
                    block[at..].fill(0);
                    break;
                }
                let within = (k % per) as usize;
                let n = (run - at)
                    .min(FRAME_BYTES_PER_CHANNEL - within)
                    .min(usize::try_from(self.stream - k).unwrap_or(usize::MAX));
                // Below `N`, so `first + t` is within the range.
                let t = first + (k / per) as u32;
                let content = source.content(t)?;
                let src = content[within * c + channel..].iter().step_by(c);
                for (dst, &b) in block[at..at + n].iter_mut().zip(src) {
                    *dst = if self.reverse { REVERSED[usize::from(b)] } else { b };
                }
                at += n;
                k += n as u64;
            }
            done += run;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::REVERSED;

    #[test]
    fn reversal_matches_the_example_of_8_2_3() {
        assert_eq!(
            [0x00, 0x01, 0x02, 0x03, 0x04].map(|b: u8| REVERSED[usize::from(b)]),
            [0x00, 0x80, 0x40, 0xC0, 0x20]
        );
    }
}
