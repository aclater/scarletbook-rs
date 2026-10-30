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
    tables: Vec<[i16; 256]>,
    /// Per ptable, its probability for each value of |Spred| / 8.
    luts: Vec<[u8; LUT_LEN]>,
    /// The frame's code bits, realigned.
    code: Vec<u8>,
}

impl Default for DstDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl DstDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        DstDecoder { tables: Vec::new(), luts: Vec::new(), code: Vec::new() }
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
        if fs > 0 && Bits::bit_at(frame, header) == 1 {
            return Err(DstError::ArithmeticCode);
        }
        self.build_tables(&filters);
        self.build_luts(&ptables);
        let mut code = CodeBits::new(&mut self.code, frame, header + 1, fs.saturating_sub(1));

        // Decoder initialisation (5.5.7): C is cb[1] .. cb[12].
        let c = code.take12();
        if c == 4095 {
            return Err(DstError::CodeOutOfRange);
        }
        let mut arith = Arith { a: 4095 << 52, x: u64::from(c) << 52, code };

        // Coded X bit (5.5.8), decoded and discarded.
        let a0 = filters.first().map_or(0, |f| f.coefs[0]);
        let px = u32::from(((a0 & 0x7F) as u8).reverse_bits() >> 1) + 1;
        arith.refill();
        let _ = arith.decode(px);

        // Per-channel state (5.4, 5.5.2).
        let mut state = Channels { history: [0xAAAA_AAAA_AAAA_AAAA_AAAA_AAAA_AAAA_AAAAu128; MAX_CHANNELS] };
        let mut nh = [0usize; MAX_CHANNELS];
        for ch in 0..nch {
            if half[ch] {
                nh[ch] = filters[usize::from(fmap[ch][0])].order;
            }
        }
        let mut fidx = [0usize; MAX_CHANNELS];
        let mut pidx = [0usize; MAX_CHANNELS];
        let segment_end = |starts: &[u32], idx: usize| starts.get(idx + 1).map_or(NB, |&b| b as usize);

        // Decode in chunks of bits within which, for every channel, the filter, the ptable
        // and the half-probability state are fixed.
        let mut n = 0usize;
        while n < NB {
            let mut end = NB;
            let mut tabs: [&[[i16; 256]]; MAX_CHANNELS] = [&[]; MAX_CHANNELS];
            let mut luts: [&[u8; LUT_LEN]; MAX_CHANNELS] = [&HALF_LUT; MAX_CHANNELS];
            for ch in 0..nch {
                end = end.min(segment_end(&fseg.starts[ch], fidx[ch]));
                end = end.min(segment_end(&pseg.starts[ch], pidx[ch]));
                if n < nh[ch] {
                    end = end.min(nh[ch]);
                } else {
                    luts[ch] = &self.luts[usize::from(pmap[ch][pidx[ch]])];
                }
                let f = usize::from(fmap[ch][fidx[ch]]);
                let used = filters[f].order.div_ceil(8);
                tabs[ch] = &self.tables[f * HISTORY_BYTES..f * HISTORY_BYTES + used];
            }
            let chunk = Chunk { start: n, end, luts: &luts };
            // The common filter orders (every channel using the same number of history
            // bytes) get loops of constant length; others use the general loop.
            macro_rules! dispatch {
                ($n:literal) => {
                    match tabs[0].len() {
                        4 if chunk.run_fixed::<$n, 4>(&tabs, &mut state, &mut arith, out) => {}
                        8 if chunk.run_fixed::<$n, 8>(&tabs, &mut state, &mut arith, out) => {}
                        16 if chunk.run_fixed::<$n, 16>(&tabs, &mut state, &mut arith, out) => {}
                        _ => chunk.run::<$n, _>(&tabs.map(Var::new), &mut state, &mut arith, out),
                    }
                };
            }
            match nch {
                1 => dispatch!(1),
                2 => dispatch!(2),
                3 => dispatch!(3),
                4 => dispatch!(4),
                5 => dispatch!(5),
                _ => dispatch!(6),
            }
            n = end;
            for ch in 0..nch {
                if segment_end(&fseg.starts[ch], fidx[ch]) == n {
                    fidx[ch] += 1;
                }
                if segment_end(&pseg.starts[ch], pidx[ch]) == n {
                    pidx[ch] += 1;
                }
            }
        }
        let code = arith.code;

        // End of frame (5.5.7): `ptr` is the index of the next code bit.
        let ptr = 1 + code.consumed();
        if fs > ptr + 7 {
            return Err(DstError::UnreadCodeBits);
        }
        Ok(())
    }

    fn build_tables(&mut self, filters: &[Filter]) {
        // Only the tables a filter uses are written and read; the others keep whatever
        // they hold.
        self.tables.resize(filters.len() * HISTORY_BYTES, [0; 256]);
        for (f, filter) in filters.iter().enumerate() {
            let used = filter.order.div_ceil(8);
            for k in 0..used {
                let table = &mut self.tables[f * HISTORY_BYTES + k];
                let coefs = &filter.coefs[8 * k..8 * k + 8];
                // History bit 8k + j is bit j of the byte: +a_j for 1, -a_j for 0. Entry 0
                // is -(sum of a_j); setting bit j adds 2 a_j. |entry| <= 8 x 256 fits i16.
                let mut sums = [0i32; 256];
                sums[0] = -coefs.iter().sum::<i32>();
                for byte in 1..256usize {
                    let j = byte.trailing_zeros() as usize;
                    sums[byte] = sums[byte & (byte - 1)] + 2 * coefs[j];
                }
                for (slot, &v) in table.iter_mut().zip(sums.iter()) {
                    *slot = v as i16;
                }
            }
        }
    }

    fn build_luts(&mut self, ptables: &[Vec<u32>]) {
        // In place; entries past 4096 are never read and keep whatever they hold.
        self.luts.resize(ptables.len(), [0; LUT_LEN]);
        for (lut, p) in self.luts.iter_mut().zip(ptables) {
            let used = &mut lut[..=4096];
            // Entries are 1 to 128 (5.3.6); a ptable has at least one.
            used.fill(p.last().copied().unwrap_or(128) as u8);
            for (slot, &v) in used.iter_mut().zip(p) {
                *slot = v as u8;
            }
        }
    }
}

/// The arithmetic decoder of 5.5.7.
///
/// A and C are kept x 2^52: A as `a`, and C as the top 12 bits of the 64-bit `x`; below it are the code bits that
/// follow (from `code.pos` on), of which [`Arith::refill`] makes at least 52 valid. A
/// symbol takes at most 8 bits, so up to 6 symbols can be decoded per refill: shifting
/// `x` left by the renormalisation count then both doubles C and appends the next bits.
#[derive(Clone, Copy)]
struct Arith<'a> {
    a: u64,
    x: u64,
    code: CodeBits<'a>,
}

impl Arith<'_> {
    /// Fills the 52 bits below C with the code bits from `code.pos` on. Bits already there
    /// are the same code bits, and those shifted in are 0, so an OR does it.
    #[inline(always)]
    fn refill(&mut self) {
        self.x |= self.code.peek() >> 12;
    }

    /// Decodes one symbol with probability `p` (1 to 128) and renormalises by the whole
    /// shift count at once (A < 2048 doubles until bit 11 is set). At most 6 calls per
    /// [`Arith::refill`].
    #[inline(always)]
    fn decode(&mut self, p: u32) -> u32 {
        // A is 2048 to 4095 here (4095 at the start, then renormalised), so Q is 8 to 15
        // and Q x p is at most A / 2 + 128 < A: nothing below wraps, and the wrapping
        // forms only spare the hot loop the overflow checks. Everything is x 2^52, like C.
        let a = self.a;
        let q = ((a >> 8) & (0xF << 52)) | ((a >> 7) & (1 << 52));
        let w0 = q.wrapping_mul(u64::from(p));
        let w1 = a.wrapping_sub(w0);
        let zero = self.x >= w1;
        let x = if zero { self.x - w1 } else { self.x };
        let a = if zero { w0 } else { w1 };
        // A is 8 to 4095 here (w0 >= 8, w1 >= 128), so the shift is 0 to 8; C stays
        // below A, so nothing leaves the top of x.
        let k = a.leading_zeros();
        self.a = a.wrapping_shl(k);
        self.x = x.wrapping_shl(k);
        self.code.skip(k);
        u32::from(!zero)
    }
}

/// Per-channel history.
struct Channels {
    history: [u128; MAX_CHANNELS],
}

/// The prediction of 5.5.3 from a channel's history, held as two 64-bit halves (bit i of
/// the history is bit i of `lo` for i < 64, bit i - 64 of `hi` otherwise): one lookup per
/// history byte in the filter's tables of partial sums.
trait Predict: Copy {
    /// Whether the prediction can read history bits 64 to 127 (`hi`).
    const HI: bool;
    fn predict(self, lo: u64, hi: u64) -> i32;
}

/// Exactly `U` tables (history bytes): a loop of constant length.
#[derive(Clone, Copy)]
struct Fixed<'t, const U: usize>(&'t [[i16; 256]; U]);

impl<const U: usize> Predict for Fixed<'_, U> {
    const HI: bool = U > 8;
    #[inline(always)]
    fn predict(self, lo: u64, hi: u64) -> i32 {
        // At most 16 terms of at most 2048 in magnitude: the sum cannot wrap, and the
        // wrapping form lets the loops be unrolled without overflow checks.
        let mut s = 0i32;
        for k in 0..U.min(8) {
            s = s.wrapping_add(i32::from(self.0[k][((lo >> (8 * k)) & 0xFF) as usize]));
        }
        for k in 8..U {
            s = s.wrapping_add(i32::from(self.0[k][((hi >> (8 * (k - 8))) & 0xFF) as usize]));
        }
        s
    }
}

/// Any number of tables, split into those of history bytes 0 to 7 and 8 to 15.
#[derive(Clone, Copy)]
struct Var<'t> {
    lo: &'t [[i16; 256]],
    hi: &'t [[i16; 256]],
}

impl<'t> Var<'t> {
    fn new(tables: &'t [[i16; 256]]) -> Self {
        let (lo, hi) = tables.split_at(tables.len().min(8));
        Var { lo, hi }
    }
}

impl Predict for Var<'_> {
    const HI: bool = true;
    #[inline(always)]
    fn predict(self, lo: u64, hi: u64) -> i32 {
        // As for `Fixed`: the sum cannot wrap.
        let mut s = 0i32;
        for (k, table) in self.lo.iter().enumerate() {
            s = s.wrapping_add(i32::from(table[((lo >> (8 * k)) & 0xFF) as usize]));
        }
        for (k, table) in self.hi.iter().enumerate() {
            s = s.wrapping_add(i32::from(table[((hi >> (8 * k)) & 0xFF) as usize]));
        }
        s
    }
}

/// Bits `start..end` of every channel, with a fixed ptable per channel.
struct Chunk<'t> {
    start: usize,
    end: usize,
    luts: &'t [&'t [u8; LUT_LEN]; MAX_CHANNELS],
}

impl Chunk<'_> {
    /// [`Chunk::run`] with exactly `U` tables for each of the `NCH` channels; false (and
    /// nothing decoded) if a channel has another number.
    #[inline(always)]
    fn run_fixed<const NCH: usize, const U: usize>(
        &self,
        tabs: &[&[[i16; 256]]; MAX_CHANNELS],
        st: &mut Channels,
        arith: &mut Arith<'_>,
        out: &mut [u8],
    ) -> bool {
        let fixed = tabs.map(|t| t.first_chunk::<U>().filter(|_| t.len() == U).map(Fixed));
        let Some(first) = fixed[0] else { return false };
        if fixed[..NCH].iter().any(Option::is_none) {
            return false;
        }
        // Channels from NCH on are not decoded.
        self.run::<NCH, _>(&fixed.map(|f| f.unwrap_or(first)), st, arith, out);
        true
    }

    /// Decodes the chunk for `NCH` (1 to 6) channels, in the symbol order of 5.5.1, with
    /// the history of each channel and the decoder state in locals for the whole chunk.
    #[inline(never)]
    fn run<const NCH: usize, P: Predict>(
        &self,
        preds: &[P; MAX_CHANNELS],
        st: &mut Channels,
        arith: &mut Arith<'_>,
        out: &mut [u8],
    ) {
        let mut ar = *arith;
        let mut lo = [0u64; NCH];
        let mut hi = [0u64; NCH];
        for ch in 0..NCH {
            lo[ch] = st.history[ch] as u64;
            hi[ch] = (st.history[ch] >> 64) as u64;
        }
        if P::HI {
            self.steps::<NCH, true, P>(self.start..self.end, preds, &mut lo, &mut hi, &mut ar, out);
        } else {
            // The prediction reads only history bits 0 to 63, so bits 64 to 127 are not
            // shifted along every bit: at the end of the chunk they are what bits 0 to 63
            // were 64 bits earlier (or, in a shorter chunk, partly the earlier bits).
            let split = self.end.saturating_sub(64).max(self.start);
            self.steps::<NCH, false, P>(self.start..split, preds, &mut lo, &mut hi, &mut ar, out);
            let mid = lo;
            self.steps::<NCH, false, P>(split..self.end, preds, &mut lo, &mut hi, &mut ar, out);
            // 0 to 64 bits since `mid`; when there were 64, `hi` is shifted out entirely.
            let n = (self.end - split) as u32;
            for ch in 0..NCH {
                let h = (u128::from(hi[ch]) << 64) | u128::from(mid[ch]);
                hi[ch] = ((h << n) >> 64) as u64;
            }
        }
        for ch in 0..NCH {
            st.history[ch] = (u128::from(hi[ch]) << 64) | u128::from(lo[ch]);
        }
        *arith = ar;
    }

    /// Decodes bits `range` of every channel; with `HI` false, `hi` is left as it is.
    #[inline(always)]
    fn steps<const NCH: usize, const HI: bool, P: Predict>(
        &self,
        range: core::ops::Range<usize>,
        preds: &[P; MAX_CHANNELS],
        lo: &mut [u64; NCH],
        hi: &mut [u64; NCH],
        ar: &mut Arith<'_>,
        out: &mut [u8],
    ) {
        // A refill waits for the position after the previous symbol, so it is done as
        // rarely as allowed: before every 6 / NCH bits (at most 6 symbols).
        let per_refill = 6 / NCH;
        let mut i = range.start;
        while i + per_refill <= range.end {
            ar.refill();
            for j in 0..per_refill {
                self.step::<NCH, HI, P>(i + j, preds, lo, hi, ar, out);
            }
            i += per_refill;
        }
        while i < range.end {
            ar.refill();
            self.step::<NCH, HI, P>(i, preds, lo, hi, ar, out);
            i += 1;
        }
    }

    /// Decodes bit `i` of every channel, without refilling.
    #[inline(always)]
    fn step<const NCH: usize, const HI: bool, P: Predict>(
        &self,
        i: usize,
        preds: &[P; MAX_CHANNELS],
        lo: &mut [u64; NCH],
        hi: &mut [u64; NCH],
        ar: &mut Arith<'_>,
        out: &mut [u8],
    ) {
        for ch in 0..NCH {
            let (l, h) = (lo[ch], hi[ch]);
            // The sum is within -32768 to 32768, and 5.5.3 maps 32768 to -32768,
            // which is what the conversion to i16 does.
            let spred = preds[ch].predict(l, h) as i16;
            let sign = u32::from(spred < 0);
            // Probability (5.5.4): |Spred| / 8 is at most 4096.
            let p = u32::from(self.luts[ch][usize::from(spred.unsigned_abs() >> 3)]);
            let y = sign ^ ar.decode(p);
            if HI {
                hi[ch] = (h << 1) | (l >> 63);
            }
            lo[ch] = (l << 1) | u64::from(y);
        }
        // The last 8 bits of the history are the output byte (5.4) once a whole byte
        // of the frame has been decoded.
        if i % 8 == 7 {
            let base = (i / 8) * NCH;
            if let Some(dst) = out.get_mut(base..base + NCH) {
                for (d, &l) in dst.iter_mut().zip(lo.iter()) {
                    *d = l as u8;
                }
            }
        }
    }
}

/// Probability lookup tables have one entry per value of |Spred| / 8 (0 to 4096), and
/// are padded to 8192 so that any `u16 >> 3` indexes them without a check.
const LUT_LEN: usize = 8192;

/// The probability table of the half-probability region (5.5.5): always 128.
static HALF_LUT: [u8; LUT_LEN] = [128; LUT_LEN];

/// The code bits cb[1], cb[2], ... of a frame, read MSB first; bits at or after the end
/// of the code read as 0 (5.3.7 item 3). Each read is one unaligned 8-byte load from a
/// realigned copy padded with zero bytes.
#[derive(Clone, Copy)]
struct CodeBits<'a> {
    bytes: &'a [u8],
    pos: usize,
}

/// Zero bytes after the realigned code, so that every 8-byte load inside the code exists.
const CODE_PADDING: usize = 8;

impl<'a> CodeBits<'a> {
    /// Copies the `len` code bits starting at bit `start` of `frame` into `buf`,
    /// realigned so that they start at bit 7 of byte 0, with every bit after them 0.
    fn new(buf: &'a mut Vec<u8>, frame: &[u8], start: usize, len: usize) -> Self {
        buf.clear();
        let shift = (start % 8) as u32;
        let first = start / 8;
        let nbytes = len.div_ceil(8);
        let src = frame.get(first..).unwrap_or(&[]);
        let realign = |hi: u8, lo: u8| (((u16::from(hi) << 8 | u16::from(lo)) << shift) >> 8) as u8;
        buf.extend(src.windows(2).take(nbytes).map(|w| realign(w[0], w[1])));
        // The last byte of the frame, if it is needed, has no byte after it.
        if buf.len() < nbytes {
            if let Some(&b) = src.get(buf.len()) {
                buf.push(realign(b, 0));
            }
        }
        buf.resize(nbytes, 0);
        if len % 8 != 0 {
            if let Some(last) = buf.last_mut() {
                *last &= 0xFF << (8 - len % 8);
            }
        }
        buf.extend_from_slice(&[0; CODE_PADDING]);
        CodeBits { bytes: buf, pos: 0 }
    }

    /// The next 57 or more bits, from the most significant bit on, without taking them.
    #[inline(always)]
    fn peek(&self) -> u64 {
        let byte = self.pos / 8;
        // Past the padded copy every bit is 0 (the code can be read beyond its end).
        let word = self.bytes.get(byte..byte + 8).map_or(0, |w| {
            let mut b = [0u8; 8];
            b.copy_from_slice(w);
            u64::from_be_bytes(b)
        });
        word << (self.pos % 8)
    }

    /// Takes `k` bits.
    #[inline(always)]
    fn skip(&mut self, k: u32) {
        // At most 8 bits per symbol, a few million per frame: no wrap.
        self.pos = self.pos.wrapping_add(k as usize);
    }

    /// The next 12 bits as an integer.
    fn take12(&mut self) -> u32 {
        let v = (self.peek() >> 52) as u32;
        self.skip(12);
        v
    }

    /// How many code bits (after cb[0]) have been taken.
    fn consumed(&self) -> usize {
        self.pos
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
    fn history_without_hi() {
        // A chunk whose filters read only history bits 0 to 63 rebuilds bits 64 to 127 at
        // its end; check everything it leaves against the general loop, for chunks
        // shorter than, as long as and longer than 64 bits.
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let code: Vec<u8> = (0..512).map(|_| rnd() as u8).collect();
        let tables: Vec<[i16; 256]> =
            (0..4).map(|_| core::array::from_fn(|_| (rnd() % 2001) as i16 - 1000)).collect();
        let lut: [u8; LUT_LEN] = core::array::from_fn(|j| (j % 128 + 1) as u8);
        let luts = [&lut; MAX_CHANNELS];
        let tabs: [&[[i16; 256]]; MAX_CHANNELS] = [&tables; MAX_CHANNELS];
        for (start, end) in [(0, 0), (8, 9), (3, 50), (16, 80), (5, 69), (0, 200)] {
            let history = core::array::from_fn(|_| u128::from(rnd()) << 64 | u128::from(rnd()));
            let chunk = Chunk { start, end, luts: &luts };
            let mut results = Vec::new();
            for general in [false, true] {
                let mut buf = Vec::new();
                let code = CodeBits::new(&mut buf, &code, 0, code.len() * 8);
                let mut ar = Arith { a: 4095 << 52, x: 0, code };
                let mut st = Channels { history };
                let mut out = vec![0u8; 2 * FL];
                if general {
                    chunk.run::<2, _>(&tabs.map(Var::new), &mut st, &mut ar, &mut out);
                } else {
                    assert!(chunk.run_fixed::<2, 4>(&tabs, &mut st, &mut ar, &mut out));
                }
                results.push((st.history, out, ar.a, ar.x, ar.code.pos));
            }
            assert!(results[0] == results[1], "chunk {start}..{end}");
        }
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
