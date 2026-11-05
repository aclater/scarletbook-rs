// SPDX-License-Identifier: MIT OR Apache-2.0

//! DSF (specification 8, 8.2), and byte ranges built from a chain (8.2.7, sacd-spec
//! e069569).

use std::ops::Range;

use super::{Checked, Layout, OutputOptions, Part, RegionKind};
use crate::audio::{self, FRAME_BYTES_PER_CHANNEL};
use crate::error::{Error, OutputRefusal, Slot};
use crate::toc::{Area, LoudspeakerLayout, Toc};

/// Block size per channel (8.2.1, 8.2.2 item 1).
const BLOCK: u64 = 4096;

/// Bytes before the sound data: the `DSD `, `fmt ` and `data` chunk headers (8.2.1).
const HEADER: usize = 92;

/// Samples per DSD byte.
const SAMPLES_PER_BYTE: u64 = 8;

/// The sample rate written (8.1 item 4).
const SAMPLE_RATE: u32 = 2_822_400;

/// Bytes of a row of the most channels DSF has (8.2.2 item 2, 8.2.4 item 2).
const MAX_ROW: usize = 6 * BLOCK as usize;

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
    /// `p`, the first byte of each channel's byte timeline that the file holds (8.2.7 item
    /// 2): `4704 a` for a range of positions.
    start: u64,
    /// `S`, DSD bytes per channel.
    stream: u64,
    /// `p - 4704 t0`, where `t0` is the first position the file reads
    /// ([`Layout::positions_for`]): 0 for a range of positions, and for a byte range the
    /// bytes of `t0` before `p` (below 4704).
    skip: u64,
    reverse: bool,
}

impl DsfData {
    /// The byte range `[p, q)` of each channel's byte timeline that the file holds.
    pub(super) fn bytes(&self) -> (u64, u64) {
        (self.start, self.start + self.stream)
    }
}

/// One file of a chain of ranges of positions (specification 8, 8.2.7 item 3; sacd-spec
/// e069569), as [`plan_dsf_chain`] gives it: range `[a_i, b_i)`, the chain `[a_1, b_K)`
/// it belongs to after any split, and the byte range `[p_i, p_{i+1})` the file holds.
///
/// Its layout is [`Layout::new`] of [`DsfChainFile::range`] as a [`super::Format::Dsf`]
/// file, with [`OutputOptions::with_dsf_chain`] of [`DsfChainFile::chain`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct DsfChainFile {
    /// `[a_i, b_i)`, the range of positions, which gives the file's source track (8.2.7
    /// item 6).
    pub range: (u32, u32),
    /// `[a_1, b_K)`: the first frame of the chain's first range and the end of its last.
    pub chain: (u32, u32),
    /// `[p_i, p_{i+1})`, the bytes of each channel's byte timeline the file holds (8.2.7
    /// items 1 to 3): `S = p_{i+1} - p_i`.
    pub bytes: (u64, u64),
}

impl DsfChainFile {
    /// Whether the file is the last of its chain: the only one whose stream may end with
    /// the pad of 8.2.2 (8.2.7 item 4).
    pub fn is_last(&self) -> bool {
        self.range.1 == self.chain.1
    }
}

/// `p_i` of a range starting at frame `a` in a chain starting at frame `a1` (8.2.7 item
/// 3): `4704 a1 + 4096 floor(4704 (a - a1) / 4096)`, which is `4704 a1` for `a = a1`.
fn chain_start(a1: u32, a: u32) -> u64 {
    let per = FRAME_BYTES_PER_CHANNEL as u64;
    per * u64::from(a1) + BLOCK * (per * u64::from(a - a1) / BLOCK)
}

/// `[p_i, p_{i+1})` of the range `[a, b)` in the chain `[a1, end)` (8.2.7 item 3), with
/// `a1 <= a < b <= end`: the next range's `p` when `b < end`, and `4704 end` for the last.
pub(super) fn chain_bytes(a1: u32, end: u32, a: u32, b: u32) -> (u64, u64) {
    let q = if b == end { FRAME_BYTES_PER_CHANNEL as u64 * u64::from(end) } else { chain_start(a1, b) };
    (chain_start(a1, a), q)
}

/// Plans the DSF byte ranges of a chain (specification 8, 8.2.7 item 3; sacd-spec
/// e069569, as clarified at fa558a8): `bounds` are `a_1, a_2, ..., a_K, b_K`, so range `i` is
/// `[bounds[i - 1], bounds[i])` and each ends where the next starts. The tracks with pause
/// of consecutive tracks (4.7.1 item 5) give such bounds when they meet end to end.
///
/// Every range is checked by 8.1 item 2 ([`audio::check_range`]) before any file is
/// planned, as 8.2.7 item 3 requires, because a file's end depends on whether the range
/// after it stays in the chain. The result has one entry per range, in order: the range's
/// file, or the error of the checks that skip or refuse it, the same error
/// [`Layout::new`] gives for it. Each skipped or refused range splits the chain, an empty
/// range skipped with [`Error::TrackEmpty`] included even though its neighbours meet: the
/// ranges before it and after it form separate chains, each with its own
/// [`DsfChainFile::chain`], and the first file after it starts at `4704 a`.
///
/// Which files are written is the caller's choice (8.2.7 item 7): a file not written does
/// not split the chain, and each file written holds the bytes planned here for the whole
/// chain. Another chain, one split elsewhere, is another plan, and gives other files.
///
/// Everything here follows from the TOC: no audio is read (8.2.7 item 5).
///
/// # Errors
///
/// [`Error::NoSuchArea`]; [`Error::InvalidArgument`] for fewer than two bounds.
pub fn plan_dsf_chain(
    toc: &Toc,
    slot: Slot,
    bounds: &[u32],
) -> Result<Vec<Result<DsfChainFile, Error>>, Error> {
    let area = toc.area(slot).ok_or(Error::NoSuchArea(slot))?;
    if bounds.len() < 2 {
        return Err(Error::InvalidArgument("a chain needs at least one range: two bounds"));
    }
    // The error of each range's checks, if any.
    let mut errors: Vec<Option<Error>> =
        bounds.windows(2).map(|w| audio::check_range(area, w[0], w[1], toc.sectors).err()).collect();
    let mut files = Vec::with_capacity(errors.len());
    let mut i = 0;
    while i < errors.len() {
        // The sub-chain of ranges i..j that pass their checks.
        let j = (i..errors.len()).find(|&j| errors[j].is_some()).unwrap_or(errors.len());
        let chain = (bounds[i], bounds[j]);
        for k in i..j {
            let range = (bounds[k], bounds[k + 1]);
            let bytes = chain_bytes(chain.0, chain.1, range.0, range.1);
            files.push(Ok(DsfChainFile { range, chain, bytes }));
        }
        // A skipped or refused range keeps its error and ends the sub-chain.
        if let Some(e) = errors.get_mut(j).and_then(Option::take) {
            files.push(Err(e));
        }
        i = j + 1;
    }
    Ok(files)
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

/// Builds the regions of a DSF file with tag `tag` (8.2.1, 8.2.5, 8.2.6).
pub(super) fn build(
    layout: &mut Layout,
    area: &Area,
    options: &OutputOptions,
    tag: Option<Vec<u8>>,
) -> Result<(), Error> {
    let channel_type = channel_type(area, options)?;
    let c = u64::from(layout.channels);
    let per = FRAME_BYTES_PER_CHANNEL as u64;
    // A range of positions holds [4704 a, 4704 b); a file of a chain, its byte range
    // (8.2.7 items 2 and 3), which starts at most 4095 bytes before 4704 a, inside the
    // position before a, and so it reads from that position (8.2.7 item 4).
    let (start, end) = match layout.chain {
        Some((a1, chain_end)) => chain_bytes(a1, chain_end, layout.first, layout.end),
        None => (per * u64::from(layout.first), per * u64::from(layout.end)),
    };
    // At least `first - 1`, which is in the chain because a_1 <= first - 1 when p < 4704 a.
    layout.base = (start / per) as u32;
    let skip = start - per * u64::from(layout.base);
    let stream = end - start;
    let blocks = stream.div_ceil(BLOCK);
    let n = BLOCK * blocks * c;
    // Without a tag, m = 0 and the metadata pointer is 0 (8.2.6 item 2).
    let m = tag.as_ref().map_or(0, |t| t.len() as u64);
    let total = HEADER as u64 + n + m;
    let pointer = if m > 0 { HEADER as u64 + n } else { 0 };
    let bits = match options.dsf_bits_per_sample {
        DsfBitsPerSample::One => 1,
        DsfBitsPerSample::Eight => 8,
    };

    let mut h = Vec::with_capacity(HEADER);
    h.extend_from_slice(b"DSD ");
    h.extend_from_slice(&28u64.to_le_bytes());
    h.extend_from_slice(&total.to_le_bytes());
    h.extend_from_slice(&pointer.to_le_bytes());
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
    let data = DsfData { channels: layout.channels, start, stream, skip, reverse: bits == 1 };
    layout.push(RegionKind::SoundData, n, Part::Dsf(data));
    if let Some(tag) = tag.filter(|t| !t.is_empty()) {
        layout.push(RegionKind::Trailer, tag.len() as u64, Part::Bytes(tag));
    }
    Ok(())
}

impl DsfData {
    /// Bytes of one row: block `j` of every channel (8.2.2 item 2).
    fn row(&self) -> u64 {
        BLOCK * u64::from(self.channels)
    }

    /// The positions, relative to the first the file reads, that sound data bytes `range`
    /// read: every stream byte of the rows they touch, below `S`.
    pub(super) fn positions(&self, range: Range<u64>) -> Range<u32> {
        let row = self.row();
        let lo = range.start / row * BLOCK;
        let hi = ((range.end - 1) / row + 1).saturating_mul(BLOCK).min(self.stream);
        if lo >= hi {
            return 0..0;
        }
        let per = FRAME_BYTES_PER_CHANNEL as u64;
        // Below `N + 1`, a u32.
        ((lo + self.skip) / per) as u32..((hi - 1 + self.skip) / per + 1) as u32
    }

    /// Fills `out` with sound data bytes from `offset` (8.2.2): data byte `o` is in row
    /// `j = o / 4096C`, channel `c = (o mod 4096C) / 4096`, at stream byte
    /// `k = 4096j + o mod 4096`; it is stream byte `k` of channel `c` as 8.2.3 stores it, or
    /// the pad 0x00 when `k >= S`. Stream byte `k` is byte `p + k` of the byte timeline
    /// (8.2.7 item 2): byte `skip + k` from the start of position `first`.
    ///
    /// Every row is filled whole, a position at a time, every channel from one pass over its
    /// content ([`DsfData::fill_row`]): in place, or, for a row `out` only touches, in a
    /// scratch row and copied from there.
    #[inline(always)]
    pub(super) fn fill(
        &self,
        offset: u64,
        out: &mut [u8],
        first: u32,
        source: &mut Checked<'_>,
    ) -> Result<(), Error> {
        let row = self.row() as usize;
        let mut scratch = [0; MAX_ROW];
        let mut done = 0;
        while done < out.len() {
            let o = offset + done as u64;
            let j = o / row as u64;
            // Below `row`.
            let within = (o % row as u64) as usize;
            let n = (row - within).min(out.len() - done);
            let dst = &mut out[done..done + n];
            if n == row {
                self.fill_row(j, dst, first, source)?;
            } else {
                // At most 6 channels (`channel_type`), so `row <= MAX_ROW`.
                self.fill_row(j, &mut scratch[..row], first, source)?;
                dst.copy_from_slice(&scratch[within..within + n]);
            }
            done += n;
        }
        Ok(())
    }

    /// Fills row `j` whole (8.2.2 item 2): `out` is its `4096 C` bytes.
    #[inline(always)]
    fn fill_row(&self, j: u64, out: &mut [u8], first: u32, source: &mut Checked<'_>) -> Result<(), Error> {
        let per = FRAME_BYTES_PER_CHANNEL as u64;
        let c = usize::from(self.channels);
        let mut k = j * BLOCK;
        let end = (k + BLOCK).min(self.stream);
        let mut at = 0;
        while k < end {
            let timeline = k + self.skip;
            let within = (timeline % per) as usize;
            // At most 4096.
            let n = (FRAME_BYTES_PER_CHANNEL - within).min((end - k) as usize);
            // Below the positions read, so `first + t` is within them.
            let t = first + (timeline / per) as u32;
            let content = source.content(t)?;
            split(&content[within * c..(within + n) * c], out, at, n, c, self.reverse);
            at += n;
            k += n as u64;
        }
        if at < BLOCK as usize {
            for block in out.chunks_exact_mut(BLOCK as usize) {
                block[at..].fill(0);
            }
        }
        Ok(())
    }
}

/// Separates `n` interleaved samples of `c` channels, `src`, into the blocks of a row:
/// channel `i` to bytes `[at, at + n)` of `row`'s block `i`, bits reversed if `reverse`
/// (8.2.3).
#[inline(always)]
fn split(src: &[u8], row: &mut [u8], at: usize, n: usize, c: usize, reverse: bool) {
    let mut blocks = row.chunks_exact_mut(BLOCK as usize).map(|b| &mut b[at..at + n]);
    let mut next = || blocks.next().unwrap_or_default();
    // One loop per channel count over all its channels at once, the bit order outside the
    // loop: the compiler vectorises each in x86-64-v3 builds. Generic x86-64 builds
    // vectorise none of the formulations tried (this, a pass per channel, u16 or u64
    // words), and this was the fastest of them there too.
    macro_rules! split {
        ($c:literal, $pat:pat, $zip:expr, $($o:ident $i:literal),+) => {{
            $(let $o = next().iter_mut();)+
            if reverse {
                for (f, $pat) in src.chunks_exact($c).zip($zip) {
                    $(*$o = f[$i].reverse_bits();)+
                }
            } else {
                for (f, $pat) in src.chunks_exact($c).zip($zip) {
                    $(*$o = f[$i];)+
                }
            }
        }};
    }
    match c {
        1 => split!(1, a, a, a 0),
        2 => split!(2, (a, b), a.zip(b), a 0, b 1),
        3 => split!(3, ((a, b), d), a.zip(b).zip(d), a 0, b 1, d 2),
        4 => split!(4, (((a, b), d), e), a.zip(b).zip(d).zip(e), a 0, b 1, d 2, e 3),
        5 => split5(src, [next(), next(), next(), next(), next()], reverse),
        6 => {
            split!(6, (((((a, b), d), e), g), h), a.zip(b).zip(d).zip(e).zip(g).zip(h), a 0, b 1, d 2, e 3, g 4, h 5)
        }
        // An area has 1 to 6 channels (specification 4, 4.8).
        _ => {}
    }
}

/// [`split`] for 5 channels: four frames at a time, as five little-endian `u32` words
/// in and one word per channel out. The compiler turns a loop over 5-byte frames into
/// one byte insert per byte (in x86-64-v3 builds too), but it vectorises loads of
/// 5-word groups and the masks and shifts below (#202).
#[inline(always)]
fn split5(src: &[u8], out: [&mut [u8]; 5], reverse: bool) {
    /// Byte `i` of a word.
    const B: [u32; 4] = [0xFF, 0xFF00, 0xFF_0000, 0xFF00_0000];
    let [a, b, d, e, g] = out;
    let whole = a.len() / 4 * 4;
    let (src, src_rest) = src.split_at(whole * 5);
    let (a, a_rest) = a.split_at_mut(whole);
    let (b, b_rest) = b.split_at_mut(whole);
    let (d, d_rest) = d.split_at_mut(whole);
    let (e, e_rest) = e.split_at_mut(whole);
    let (g, g_rest) = g.split_at_mut(whole);
    // Whole-word loads, which the vectoriser sees as such (not four byte loads).
    let word = |s: &[u8], k: usize| {
        s.get(4 * k..4 * k + 4).and_then(|w| w.try_into().ok()).map_or(0, u32::from_le_bytes)
    };
    let words = a
        .chunks_exact_mut(4)
        .zip(b.chunks_exact_mut(4))
        .zip(d.chunks_exact_mut(4))
        .zip(e.chunks_exact_mut(4))
        .zip(g.chunks_exact_mut(4));
    // Byte `t` of channel `c`'s word is byte `5t + c` of the 20, which is byte
    // `(5t + c) mod 4` of word `(5t + c) / 4`.
    macro_rules! words {
        ($f:expr) => {
            for (s, ((((a, b), d), e), g)) in src.chunks_exact(20).zip(words) {
                let [w0, w1, w2, w3, w4] = [word(s, 0), word(s, 1), word(s, 2), word(s, 3), word(s, 4)];
                let f = $f;
                a.copy_from_slice(&f((w0 & B[0]) | (w1 & B[1]) | (w2 & B[2]) | (w3 & B[3])).to_le_bytes());
                b.copy_from_slice(
                    &f(((w0 & B[1]) | (w1 & B[2]) | (w2 & B[3])) >> 8 | w4 << 24).to_le_bytes(),
                );
                d.copy_from_slice(
                    &f(((w0 & B[2]) | (w1 & B[3])) >> 16 | ((w3 & B[0]) | (w4 & B[1])) << 16).to_le_bytes(),
                );
                e.copy_from_slice(
                    &f(w0 >> 24 | ((w2 & B[0]) | (w3 & B[1]) | (w4 & B[2])) << 8).to_le_bytes(),
                );
                g.copy_from_slice(&f((w1 & B[0]) | (w2 & B[1]) | (w3 & B[2]) | (w4 & B[3])).to_le_bytes());
            }
        };
    }
    if reverse {
        // The bits of each byte reversed (8.2.3), not those of the word.
        words!(|w: u32| w.reverse_bits().swap_bytes());
    } else {
        words!(|w: u32| w);
    }
    // The last 0 to 3 frames.
    let rest =
        a_rest.iter_mut().zip(b_rest.iter_mut()).zip(d_rest.iter_mut()).zip(e_rest.iter_mut()).zip(g_rest);
    for (f, ((((a, b), d), e), g)) in src_rest.chunks_exact(5).zip(rest) {
        for (o, &v) in [a, b, d, e, g].into_iter().zip(f) {
            *o = if reverse { v.reverse_bits() } else { v };
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::borrow::Cow;

    use super::super::{Checked, PositionSource};
    use super::{BLOCK, DsfData};
    use crate::audio::FRAME_BYTES_PER_CHANNEL;
    use crate::error::Error;

    #[test]
    fn reversal_matches_the_example_of_8_2_3() {
        assert_eq!([0x00, 0x01, 0x02, 0x03, 0x04].map(u8::reverse_bits), [0x00, 0x80, 0x40, 0xC0, 0x20]);
    }

    /// Positions of `C x 4704` bytes made from their number and channel count, each made
    /// once (the fills ask for a position again for every block).
    struct Made(u8, Vec<(u32, Vec<u8>)>);

    impl PositionSource for Made {
        fn content(&mut self, t: u32) -> Result<Cow<'_, [u8]>, Error> {
            let i = match self.1.iter().position(|(u, _)| *u == t) {
                Some(i) => i,
                None => {
                    let len = usize::from(self.0) * FRAME_BYTES_PER_CHANNEL;
                    let content =
                        (0..len).map(|i| (i as u32 * 131 + t * 7919 + i as u32 / 255) as u8).collect();
                    self.1.push((t, content));
                    self.1.len() - 1
                }
            };
            Ok(Cow::Borrowed(&self.1[i].1))
        }
    }

    /// Fills `out` from `offset` a block at a time, straight from [`DsfData::fill`]'s
    /// definition of each data byte: what the rows it fills are compared with.
    fn fill_blocks(
        data: &DsfData,
        offset: u64,
        out: &mut [u8],
        first: u32,
        source: &mut Checked<'_>,
    ) -> Result<(), Error> {
        let row = data.row();
        let c = usize::from(data.channels);
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
                if k >= data.stream {
                    block[at..].fill(0);
                    break;
                }
                let timeline = k + data.skip;
                let within = (timeline % per) as usize;
                let n = (run - at)
                    .min(FRAME_BYTES_PER_CHANNEL - within)
                    .min(usize::try_from(data.stream - k).unwrap_or(usize::MAX));
                // Below the positions read, so `first + t` is within them.
                let t = first + (timeline / per) as u32;
                let content = source.content(t)?;
                let src = content[within * c + channel..].iter().step_by(c);
                for (dst, &b) in block[at..at + n].iter_mut().zip(src) {
                    *dst = if data.reverse { b.reverse_bits() } else { b };
                }
                at += n;
                k += n as u64;
            }
            done += run;
        }
        Ok(())
    }

    /// Whole rows (filled a position at a time) are the bytes that filling a block at a
    /// time gives, for every channel count, both bit orders, streams that start inside a
    /// position (8.2.7) and streams that end inside a row. Under Miri: 1, 2 and 6 channels
    /// and two short streams, one starting inside a position and ending inside a row, one
    /// starting a byte before a block ends and ending at a row's end.
    #[test]
    fn rows_equal_blocks() {
        let per = FRAME_BYTES_PER_CHANNEL as u64;
        let (channels, streams): (&[u8], &[(u64, u64)]) = if cfg!(miri) {
            (&[1, 2, 6], &[(1000, per + 17), (4095, 2 * BLOCK)])
        } else {
            (
                &[1, 2, 3, 4, 5, 6],
                &[(0, 5 * per), (1000, 4 * per + 17), (4095, 3 * BLOCK), (8, 9 * BLOCK + 8)],
            )
        };
        for &channels in channels {
            for reverse in [false, true] {
                for &(skip, stream) in streams {
                    let data = DsfData { channels, start: skip, stream, skip, reverse };
                    let size = (BLOCK * stream.div_ceil(BLOCK) * u64::from(channels)) as usize;
                    let mut source = Made(channels, Vec::new());
                    let len = usize::from(channels) * FRAME_BYTES_PER_CHANNEL;
                    let (mut rows, mut blocks) = (vec![0xAA; size], vec![0x55; size]);
                    data.fill(0, &mut rows, 3, &mut Checked { inner: &mut source, len }).unwrap();
                    fill_blocks(&data, 0, &mut blocks, 3, &mut Checked { inner: &mut source, len }).unwrap();
                    assert!(
                        rows == blocks,
                        "{channels} channels, reverse {reverse}, skip {skip}, S {stream}"
                    );
                    // Reads that start and end inside rows.
                    for (at, n) in
                        [(1, size - 2), (4097, 3 * 4096 * usize::from(channels)), (size - 5000, 5000)]
                    {
                        let n = n.min(size - at);
                        let mut part = vec![0; n];
                        data.fill(at as u64, &mut part, 3, &mut Checked { inner: &mut source, len }).unwrap();
                        assert!(part == blocks[at..at + n], "{channels} channels: {at}+{n}");
                    }
                }
            }
        }
    }
}
