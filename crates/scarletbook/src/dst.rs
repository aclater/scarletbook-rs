// SPDX-License-Identifier: MIT OR Apache-2.0

//! DST frame decoding (specification 5).
//!
//! A frame is decoded independently of every other frame. Each rule of 5.6 that makes a
//! frame invalid is reported as a [`DstError`]; on SACD an invalid frame is replaced by
//! 0x55 bytes (specification 4, 4.6), which [`crate::audio::TrackAudio`] does.

use crate::audio::FRAME_BYTES_PER_CHANNEL;

/// Bits (samples) per channel per frame, `NB` (specification 5, 5.1.2).
pub const NB: usize = 37_632;

/// Bytes per channel per frame, `FL`.
pub const FL: usize = FRAME_BYTES_PER_CHANNEL;

/// Largest channel count, `Nch`.
pub const MAX_CHANNELS: usize = 6;

const MAX_ORDER: usize = 128;
const MAX_PTABLE_LEN: usize = 64;
const HISTORY_BYTES: usize = MAX_ORDER / 8;

/// Why a DST frame is invalid: the rules of specification 5, 5.6, plus caller errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum DstError {
    /// Rule 1: `FB <= 1`.
    #[error("frame-too-short")]
    FrameTooShort,
    /// Rule 2: a plain frame with non-zero stuffing bits.
    #[error("stuffing")]
    Stuffing,
    /// Rule 3: the header needs more bits than the frame has.
    #[error("header-truncated")]
    HeaderTruncated,
    /// Rule 4: segment resolution 0 or too large.
    #[error("resolution")]
    Resolution,
    /// Rule 5: an explicit segment too short or too long.
    #[error("segment-length")]
    SegmentLength,
    /// Rule 6: too many explicit segment lengths.
    #[error("too-many-segments")]
    TooManySegments,
    /// Rule 7: a table index greater than the running count.
    #[error("table-number")]
    TableNumber,
    /// Rule 8: more than `2 x Nch` filters or ptables.
    #[error("too-many-tables")]
    TooManyTables,
    /// Rule 9: a shared mapping with unequal segment counts.
    #[error("channel-mapping")]
    ChannelMapping,
    /// Rule 10: a copied mapping with different filter and ptable segment counts.
    #[error("segment-count-mismatch")]
    SegmentCountMismatch,
    /// Rule 11: coefficient coding method 3, or its order not below the filter order.
    #[error("coefficient-coding")]
    CoefficientCoding,
    /// Rule 12: a reconstructed coefficient outside -256 to 255.
    #[error("coefficient-range")]
    CoefficientRange,
    /// Rule 13: ptable coding method 3, or its order not below the table length.
    #[error("ptable-coding")]
    PtableCoding,
    /// Rule 14: a reconstructed ptable entry outside 1 to 128.
    #[error("ptable-range")]
    PtableRange,
    /// Rule 15: the first code bit is 1.
    #[error("arithmetic-code")]
    ArithmeticCode,
    /// Rule 16: more than 7 code bits left unread.
    #[error("unread-code-bits")]
    UnreadCodeBits,
    /// Rule 17: the initial code value is 4095.
    #[error("code-out-of-range")]
    CodeOutOfRange,
    /// Not a frame rule: the channel count is not 1 to 6 or the output has the wrong size.
    #[error("invalid decoder arguments")]
    InvalidArguments,
}

impl DstError {
    /// The rule number of specification 5, 5.6 (0 for [`DstError::InvalidArguments`]).
    pub fn rule(self) -> u8 {
        match self {
            DstError::FrameTooShort => 1,
            DstError::Stuffing => 2,
            DstError::HeaderTruncated => 3,
            DstError::Resolution => 4,
            DstError::SegmentLength => 5,
            DstError::TooManySegments => 6,
            DstError::TableNumber => 7,
            DstError::TooManyTables => 8,
            DstError::ChannelMapping => 9,
            DstError::SegmentCountMismatch => 10,
            DstError::CoefficientCoding => 11,
            DstError::CoefficientRange => 12,
            DstError::PtableCoding => 13,
            DstError::PtableRange => 14,
            DstError::ArithmeticCode => 15,
            DstError::UnreadCodeBits => 16,
            DstError::CodeOutOfRange => 17,
            DstError::InvalidArguments => 0,
        }
    }
}

/// `bitlen(x)`: the smallest y with 2^y > x (specification 5, 5.1.3).
fn bitlen(x: u32) -> u32 {
    32 - x.leading_zeros()
}

/// Reads bits MSB first (specification 5, 5.1.4). Reads past the end fail with
/// `HeaderTruncated`; only the header is read through it.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    len: usize,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Bits { data, pos: 0, len: data.len().saturating_mul(8) }
    }

    fn bit_at(data: &[u8], i: usize) -> u32 {
        data.get(i / 8).map_or(0, |b| u32::from((b >> (7 - i % 8)) & 1))
    }

    fn bit(&mut self) -> Result<u32, DstError> {
        if self.pos >= self.len {
            return Err(DstError::HeaderTruncated);
        }
        let b = Self::bit_at(self.data, self.pos);
        self.pos += 1;
        Ok(b)
    }

    fn read(&mut self, n: u32) -> Result<u32, DstError> {
        let n = n as usize;
        if self.len - self.pos < n {
            return Err(DstError::HeaderTruncated);
        }
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | Self::bit_at(self.data, self.pos);
            self.pos += 1;
        }
        Ok(v)
    }

    /// `tcimsbf(9)`.
    fn read_tc9(&mut self) -> Result<i32, DstError> {
        let v = self.read(9)? as i32;
        Ok(if v >= 256 { v - 512 } else { v })
    }

    /// A Rice-coded signed integer (specification 5, 5.3.8).
    fn rice(&mut self, m: u32) -> Result<i64, DstError> {
        let mut q: i64 = 0;
        while self.bit()? == 0 {
            q += 1;
        }
        let lsb = i64::from(self.read(m)?);
        let u = (q << m) + lsb;
        if u == 0 {
            return Ok(0);
        }
        Ok(if self.bit()? == 1 { -u } else { u })
    }
}

/// One segmentation: per channel, the start bit of each segment.
#[derive(Clone, Debug, Default)]
struct Segmentation {
    starts: [Vec<u32>; MAX_CHANNELS],
}

impl Segmentation {
    fn count(&self, c: usize) -> usize {
        self.starts.get(c).map_or(0, Vec::len)
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Filter,
    Ptable,
}

impl Kind {
    fn lmin_bytes(self) -> u32 {
        match self {
            Kind::Filter => 128,
            Kind::Ptable => 4,
        }
    }

    fn max_explicit(self, shared: bool) -> usize {
        match (self, shared) {
            (Kind::Filter, true) => 4,
            (Kind::Filter, false) => 3,
            (Kind::Ptable, _) => 7,
        }
    }
}

/// `table_segmentation(kind)` (specification 5, 5.3.2).
fn read_segmentation(bits: &mut Bits<'_>, kind: Kind, nch: usize) -> Result<Segmentation, DstError> {
    let shared = bits.bit()? == 1;
    let lmin = kind.lmin_bytes();
    let rmax = FL as u32 - lmin;
    let mut resolution: Option<u32> = None;
    let mut seg = Segmentation::default();
    let read_channels = if shared { 1 } else { nch };
    for c in 0..read_channels {
        let mut rem = rmax;
        let mut starts = vec![0u32];
        let mut pos = 0u32;
        loop {
            if bits.bit()? == 1 {
                break;
            }
            if starts.len() > kind.max_explicit(shared) {
                return Err(DstError::TooManySegments);
            }
            let r = match resolution {
                Some(r) => r,
                None => {
                    let r = bits.read(bitlen(rmax))?;
                    if r == 0 || r > rmax {
                        return Err(DstError::Resolution);
                    }
                    resolution = Some(r);
                    r
                }
            };
            let l = bits.read(bitlen(rem / r))?;
            // r <= 4700 and l < 2^13: no overflow.
            let bytes = r * l;
            if bytes < lmin || bytes > rem {
                return Err(DstError::SegmentLength);
            }
            rem -= bytes;
            pos += 8 * bytes;
            starts.push(pos);
        }
        if let Some(slot) = seg.starts.get_mut(c) {
            *slot = starts;
        }
    }
    if shared {
        let first = seg.starts[0].clone();
        for c in 1..nch {
            seg.starts[c].clone_from(&first);
        }
    }
    Ok(seg)
}

/// `table_mapping` (specification 5, 5.3.3): per channel and segment, a table number.
fn read_mapping(
    bits: &mut Bits<'_>,
    seg: &Segmentation,
    nch: usize,
) -> Result<(Vec<Vec<u8>>, usize), DstError> {
    let shared = bits.bit()? == 1;
    let mut t: u32 = 1;
    let mut index = |bits: &mut Bits<'_>| -> Result<u8, DstError> {
        let v = bits.read(bitlen(t))?;
        if v > t {
            return Err(DstError::TableNumber);
        }
        if v == t {
            t += 1;
        }
        // t is at most the total number of segments (48).
        Ok(v as u8)
    };
    let mut map: Vec<Vec<u8>> = Vec::with_capacity(nch);
    if shared {
        let mut first = vec![0u8];
        for _ in 1..seg.count(0) {
            first.push(index(bits)?);
        }
        for c in 0..nch {
            if seg.count(c) != seg.count(0) {
                return Err(DstError::ChannelMapping);
            }
            map.push(first.clone());
        }
    } else {
        for c in 0..nch {
            let mut m = Vec::with_capacity(seg.count(c));
            for j in 0..seg.count(c) {
                m.push(if c == 0 && j == 0 { 0 } else { index(bits)? });
            }
            map.push(m);
        }
    }
    let count = t as usize;
    if count > 2 * nch {
        return Err(DstError::TooManyTables);
    }
    Ok((map, count))
}

const COEF_PRED: [[i64; 3]; 3] = [[-8, 0, 0], [-16, 8, 0], [-9, -5, 6]];
const PTABLE_PRED: [[i64; 3]; 3] = [[-8, 0, 0], [-16, 8, 0], [-24, 24, -8]];

/// `floor((x + 4) / 8)`.
fn round8(x: i64) -> i64 {
    (x + 4).div_euclid(8)
}

/// A decoded filter: its prediction order and coefficients (zero beyond the order).
#[derive(Clone, Debug)]
struct Filter {
    order: usize,
    coefs: [i32; MAX_ORDER],
}

/// Decodes DST frames, reusing its buffers between frames.
#[derive(Debug)]
pub struct DstDecoder {
    /// Per filter, `HISTORY_BYTES` tables of 256 partial sums.
    tables: Vec<[i32; 256]>,
}

impl Default for DstDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl DstDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        DstDecoder { tables: Vec::new() }
    }

    /// Decodes one frame of `channels` channels into `out` (`channels x 4704` bytes, in
    /// interleaved layout).
    ///
    /// On error `out` holds unspecified bytes.
    ///
    /// # Errors
    ///
    /// The rule of specification 5, 5.6 that makes the frame invalid, or
    /// [`DstError::InvalidArguments`].
    pub fn decode(&mut self, frame: &[u8], channels: u8, out: &mut [u8]) -> Result<(), DstError> {
        let nch = usize::from(channels);
        if !(1..=MAX_CHANNELS).contains(&nch) || out.len() != nch * FL {
            return Err(DstError::InvalidArguments);
        }
        if frame.len() <= 1 {
            return Err(DstError::FrameTooShort);
        }
        if frame[0] & 0x80 == 0 {
            return plain(frame, out);
        }
        let mut bits = Bits::new(frame);
        bits.pos = 1;

        // Segmentation and mapping (5.3.1 to 5.3.3).
        let same_segmentation = bits.bit()? == 1;
        let fseg = read_segmentation(&mut bits, Kind::Filter, nch)?;
        let pseg =
            if same_segmentation { fseg.clone() } else { read_segmentation(&mut bits, Kind::Ptable, nch)? };
        let same_mapping = bits.bit()? == 1;
        let (fmap, nf) = read_mapping(&mut bits, &fseg, nch)?;
        let (pmap, np) = if same_mapping {
            for c in 0..nch {
                if pseg.count(c) != fseg.count(c) {
                    return Err(DstError::SegmentCountMismatch);
                }
            }
            (fmap.clone(), nf)
        } else {
            read_mapping(&mut bits, &pseg, nch)?
        };

        // Half probability (5.3.4).
        let mut half = [false; MAX_CHANNELS];
        for h in half.iter_mut().take(nch) {
            *h = bits.bit()? == 1;
        }

        // Filters (5.3.5).
        let mut filters = Vec::with_capacity(nf);
        for _ in 0..nf {
            filters.push(read_filter(&mut bits)?);
        }

        // Ptables (5.3.6).
        let mut ptables: Vec<Vec<u32>> = Vec::with_capacity(np);
        for _ in 0..np {
            ptables.push(read_ptable(&mut bits)?);
        }

        // Arithmetic code (5.3.7).
        let header = bits.pos;
        let total = frame.len().saturating_mul(8);
        let fs = total - header;
        let cb = |i: usize| -> u32 { if i < fs { Bits::bit_at(frame, header + i) } else { 0 } };
        if fs > 0 && cb(0) == 1 {
            return Err(DstError::ArithmeticCode);
        }

        self.build_tables(&filters);

        // Decoder initialisation (5.5.7).
        let mut a: u32 = 4095;
        let mut c: u32 = 0;
        for i in 1..=12 {
            c = (c << 1) | cb(i);
        }
        if c == 4095 {
            return Err(DstError::CodeOutOfRange);
        }
        let mut ptr: usize = 13;
        let mut decode = |p: u32| -> u32 {
            let q = (a >> 8) | ((a >> 7) & 1);
            let w0 = q * p;
            let w1 = a - w0;
            let e = if c >= w1 {
                c -= w1;
                a = w0;
                0
            } else {
                a = w1;
                1
            };
            while a < 2048 {
                a <<= 1;
                c = (c << 1) | cb(ptr);
                ptr += 1;
            }
            e
        };

        // Coded X bit (5.5.8), decoded and discarded.
        let a0 = filters.first().map_or(0, |f| f.coefs[0]);
        let px = u32::from(((a0 & 0x7F) as u8).reverse_bits() >> 1) + 1;
        let _ = decode(px);

        // Per-channel state.
        let mut history = [0xAAAA_AAAA_AAAA_AAAA_AAAA_AAAA_AAAA_AAAAu128; MAX_CHANNELS];
        let mut nh = [0usize; MAX_CHANNELS];
        for ch in 0..nch {
            if half[ch] {
                let f0 = usize::from(fmap[ch][0]);
                nh[ch] = filters[f0].order;
            }
        }
        let mut fidx = [0usize; MAX_CHANNELS];
        let mut pidx = [0usize; MAX_CHANNELS];
        let mut acc = [0u8; MAX_CHANNELS];
        let tables = &self.tables;

        for n in 0..NB {
            let n32 = n as u32;
            for ch in 0..nch {
                // Advance to the segments containing bit n (5.4).
                let fs_ = &fseg.starts[ch];
                while fidx[ch] + 1 < fs_.len() && fs_[fidx[ch] + 1] <= n32 {
                    fidx[ch] += 1;
                }
                let ps = &pseg.starts[ch];
                while pidx[ch] + 1 < ps.len() && ps[pidx[ch] + 1] <= n32 {
                    pidx[ch] += 1;
                }
                let f = usize::from(fmap[ch][fidx[ch]]);
                let filter = &filters[f];
                // Prediction (5.5.3): the sum over the history, one table per history byte.
                let h = history[ch];
                let base = f * HISTORY_BYTES;
                let used = filter.order.div_ceil(8);
                let mut s: i32 = 0;
                for (k, table) in tables[base..base + used].iter().enumerate() {
                    s += table[((h >> (8 * k)) & 0xFF) as usize];
                }
                let spred = if s == 32768 { -32768 } else { s };
                let sign = u32::from(spred < 0);
                // Probability (5.5.4).
                let p = if n < nh[ch] {
                    128
                } else {
                    let table = &ptables[usize::from(pmap[ch][pidx[ch]])];
                    let j = ((spred.unsigned_abs() / 8) as usize).min(table.len() - 1);
                    table[j]
                };
                let y = sign ^ decode(p);
                history[ch] = (h << 1) | u128::from(y);
                acc[ch] = (acc[ch] << 1) | y as u8;
            }
            if n % 8 == 7 {
                let base = (n / 8) * nch;
                out[base..base + nch].copy_from_slice(&acc[..nch]);
            }
        }

        // End of frame (5.5.7).
        if fs > ptr + 7 {
            return Err(DstError::UnreadCodeBits);
        }
        Ok(())
    }

    fn build_tables(&mut self, filters: &[Filter]) {
        self.tables.clear();
        self.tables.resize(filters.len() * HISTORY_BYTES, [0; 256]);
        for (f, filter) in filters.iter().enumerate() {
            let used = filter.order.div_ceil(8);
            for k in 0..used {
                let table = &mut self.tables[f * HISTORY_BYTES + k];
                let coefs = &filter.coefs[8 * k..8 * k + 8];
                for (byte, slot) in table.iter_mut().enumerate() {
                    // History bit 8k + j is bit j of this byte: +a for 1, -a for 0.
                    *slot = coefs
                        .iter()
                        .enumerate()
                        .map(|(j, &a)| if (byte >> j) & 1 == 1 { a } else { -a })
                        .sum();
                }
            }
        }
    }
}

/// A plain frame (`DST_coded` = 0, specification 5, 5.2.1 item 2).
fn plain(frame: &[u8], out: &mut [u8]) -> Result<(), DstError> {
    if frame[0] & 0x3F != 0 {
        return Err(DstError::Stuffing);
    }
    let payload = &frame[1..];
    let n = payload.len().min(out.len());
    out[..n].copy_from_slice(&payload[..n]);
    out[n..].fill(0);
    Ok(())
}

fn read_filter(bits: &mut Bits<'_>) -> Result<Filter, DstError> {
    let order = bits.read(7)? as usize + 1;
    let mut values = [0i64; MAX_ORDER];
    if bits.bit()? == 0 {
        for v in values.iter_mut().take(order) {
            *v = i64::from(bits.read_tc9()?);
        }
    } else {
        let method = bits.read(2)? as usize;
        if method == 3 {
            return Err(DstError::CoefficientCoding);
        }
        let k = method + 1;
        if k >= order {
            return Err(DstError::CoefficientCoding);
        }
        for v in values.iter_mut().take(k) {
            *v = i64::from(bits.read_tc9()?);
        }
        let m = bits.read(3)?;
        for i in k..order {
            let r = bits.rice(m)?;
            let x: i64 = (0..k).map(|t| COEF_PRED[method][t] * values[i - 1 - t]).sum();
            let a = r - round8(x);
            if !(-256..=255).contains(&a) {
                return Err(DstError::CoefficientRange);
            }
            values[i] = a;
        }
    }
    let mut coefs = [0i32; MAX_ORDER];
    for (c, &v) in coefs.iter_mut().zip(values.iter()) {
        // Every value is within -256 to 255.
        *c = v as i32;
    }
    Ok(Filter { order, coefs })
}

fn read_ptable(bits: &mut Bits<'_>) -> Result<Vec<u32>, DstError> {
    let len = bits.read(6)? as usize + 1;
    if len == 1 {
        return Ok(vec![128]);
    }
    let mut values = [0i64; MAX_PTABLE_LEN];
    if bits.bit()? == 0 {
        for v in values.iter_mut().take(len) {
            *v = i64::from(bits.read(7)?) + 1;
        }
    } else {
        let method = bits.read(2)? as usize;
        if method == 3 {
            return Err(DstError::PtableCoding);
        }
        let k = method + 1;
        if k >= len {
            return Err(DstError::PtableCoding);
        }
        for v in values.iter_mut().take(k) {
            *v = i64::from(bits.read(7)?) + 1;
        }
        let m = bits.read(3)?;
        for i in k..len {
            let r = bits.rice(m)?;
            let x: i64 = (0..k).map(|t| PTABLE_PRED[method][t] * values[i - 1 - t]).sum();
            let p = r - round8(x);
            if !(1..=128).contains(&p) {
                return Err(DstError::PtableRange);
            }
            values[i] = p;
        }
    }
    // Every value is within 1 to 128.
    Ok(values[..len].iter().map(|&v| v as u32).collect())
}

/// Decodes one frame with a fresh decoder. See [`DstDecoder::decode`].
///
/// # Errors
///
/// As [`DstDecoder::decode`].
pub fn decode(frame: &[u8], channels: u8, out: &mut [u8]) -> Result<(), DstError> {
    DstDecoder::new().decode(frame, channels, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitlen_examples() {
        assert_eq!([0, 1, 2, 3, 4, 7, 4576].map(bitlen), [0, 1, 2, 2, 3, 3, 13]);
    }

    #[test]
    fn round8_floors() {
        assert_eq!(round8(-5), -1);
        assert_eq!(round8(-4), 0);
    }

    #[test]
    fn plain_frames() {
        let mut out = vec![1u8; 2 * FL];
        decode(&[0x40, 7, 8], 2, &mut out).unwrap();
        assert_eq!(&out[..3], &[7, 8, 0]);
        assert!(out[2..].iter().all(|&b| b == 0));
        assert_eq!(decode(&[0x01, 7], 2, &mut out), Err(DstError::Stuffing));
        assert_eq!(decode(&[0x00], 2, &mut out), Err(DstError::FrameTooShort));
        assert_eq!(decode(&[0x00, 1], 7, &mut out), Err(DstError::InvalidArguments));
    }

    #[test]
    fn rice_codes() {
        // m = 2: q = 1 ("01"), lsb "10" = 2, u = 6, sign "1": -6.
        let data = [0b0110_1000];
        let mut b = Bits::new(&data);
        assert_eq!(b.rice(2), Ok(-6));
        // u = 0 reads no sign bit.
        let data = [0b1000_0000];
        let mut b = Bits::new(&data);
        assert_eq!(b.rice(0), Ok(0));
        assert_eq!(b.pos, 1);
        let mut b = Bits::new(&[0u8]);
        assert_eq!(b.rice(0), Err(DstError::HeaderTruncated));
    }
}
