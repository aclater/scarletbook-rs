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

/// Eight prediction terms, one for each bit position of the byte being decoded.
type Row = [i16; 8];

/// For one complete history byte and each of its 256 values, its share of the prediction at
/// each bit position of the byte being decoded.
type ByteTable = [Row; 256];

/// The tables of a filter's history bytes beyond its order, and of channels not decoded.
static ZERO_TABLES: [ByteTable; HISTORY_BYTES] = [[[0; 8]; 256]; HISTORY_BYTES];

/// The initial history (5.5.2) as complete bytes (`h[8d + 7]` the most significant bit of
/// byte d, `h[8d]` the least): `h[i]` is 1 for odd i.
const HISTORY_FILL: u8 = 0xAA;

/// Bytes of history read for one byte of every channel, at most: [`HISTORY_BYTES`] bytes
/// of [`MAX_CHANNELS`] channels, interleaved.
const WINDOW: usize = HISTORY_BYTES * MAX_CHANNELS;

/// Most frames [`DstDecoder::decode_many`] decodes in lockstep; it decodes more in groups
/// of this many.
pub const MAX_LANES: usize = 4;

/// How many frames of `channels` channels to pass to [`DstDecoder::decode_many`] at once
/// for the most frames per second on one thread (1 if `channels` is not 1 to 6).
///
/// Chosen by measurement for each build (see the commit that added it): the decoder is
/// bound by the serial chain of the arithmetic decoder, which more frames overlap until
/// the 16 general registers of x86-64 run out and the state spills. Builds with
/// AVX-512 (`avx512cd`) have more registers, but the compiler pairs the steps of
/// two frames into vector operations there, which is much slower for 6 channels, so 3
/// frames are used.
///
/// The 1-, 3- and 4-channel choices were measured on generated frames; their output is
/// checked by the corpus's 1-, 3- and 4-channel DST cases (sacd-spec#15).
pub fn lanes(channels: u8) -> usize {
    let avx512 = cfg!(target_feature = "avx512cd");
    match channels {
        1 => {
            if avx512 {
                3
            } else {
                4
            }
        }
        3 if !avx512 => 3,
        6 if avx512 => 3,
        2..=6 => 2,
        _ => 1,
    }
}

/// The buffers of one frame being decoded, reused from frame to frame.
#[derive(Debug, Default)]
struct Lane {
    /// Per filter, `HISTORY_BYTES` byte tables (see [`Lane::build_tables`]), zero beyond
    /// the filter's order.
    tables: Vec<ByteTable>,
    /// Per filter, how many of its tables may be non-zero.
    filled: Vec<usize>,
    /// Per filter, the share of the bits already decoded in the current byte, indexed by
    /// `2^r | bits` for `r` bits.
    partial: Vec<[i16; 256]>,
    /// Per ptable, its probability for each value of |Spred| / 8.
    luts: Vec<[u8; LUT_LEN]>,
    /// The frame's code bits, realigned.
    code: Vec<u8>,
    /// The history and output of every channel, interleaved: `HISTORY_BYTES` bytes of
    /// initial history per channel, then the output frame, then `WINDOW` spare bytes.
    hist: Vec<u8>,
}

/// Decodes DST frames, reusing its buffers between frames.
#[derive(Debug, Default)]
pub struct DstDecoder {
    /// One per frame decoded at once.
    lanes: Vec<Lane>,
}

impl DstDecoder {
    /// A new decoder.
    pub fn new() -> Self {
        DstDecoder { lanes: Vec::new() }
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
        let mut result = [Ok(())];
        self.decode_group(&[frame], channels, &mut [out], &mut result);
        result[0]
    }

    /// Decodes `frames[i]` into `outs[i]` for every `i`, as [`DstDecoder::decode`] would
    /// one by one, with the same results, but up to [`MAX_LANES`] frames at a time in
    /// lockstep on the calling thread: frames are independent (specification 5, 5.1.1), so
    /// the processor overlaps their arithmetic decoders, whose steps otherwise wait for each
    /// other. [`lanes`] is the best number of frames to pass at once.
    ///
    /// The result has one entry per frame; a frame without an output buffer (when `outs` is
    /// shorter) is [`DstError::InvalidArguments`].
    pub fn decode_many(
        &mut self,
        frames: &[&[u8]],
        channels: u8,
        outs: &mut [&mut [u8]],
    ) -> Vec<Result<(), DstError>> {
        let mut results = vec![Err(DstError::InvalidArguments); frames.len()];
        let n = frames.len().min(outs.len());
        let groups = frames[..n].chunks(MAX_LANES).zip(outs[..n].chunks_mut(MAX_LANES));
        for ((frames, outs), results) in groups.zip(results.chunks_mut(MAX_LANES)) {
            self.decode_group(frames, channels, outs, results);
        }
        results
    }

    /// Decodes at most [`MAX_LANES`] frames in lockstep: every frame's header first, then
    /// the code byte by byte of every frame in turn. A frame whose header is invalid, or
    /// that is plain, drops out at once; one whose next chunk holds the end of a
    /// half-probability region decodes that chunk alone.
    fn decode_group(
        &mut self,
        frames: &[&[u8]],
        channels: u8,
        outs: &mut [&mut [u8]],
        results: &mut [Result<(), DstError>],
    ) {
        if self.lanes.len() < frames.len() {
            self.lanes.resize_with(frames.len(), Lane::default);
        }
        let nch = usize::from(channels);
        let mut live: Vec<Live<'_, '_>> = Vec::with_capacity(frames.len());
        let lanes = self.lanes.iter_mut().zip(frames).zip(outs.iter_mut());
        for ((i, ((lane, frame), out)), result) in lanes.enumerate().zip(results.iter_mut()) {
            match Coded::start(lane, frame, nch, out) {
                Ok(Some(coded)) => live.push(Live { index: i, coded, out }),
                Ok(None) => *result = Ok(()),
                Err(e) => *result = Err(e),
            }
        }
        let mut spans: Vec<Span<'_>> = Vec::with_capacity(live.len());
        loop {
            // Frames at their end leave; a frame whose next chunk is in a half-probability
            // region decodes it alone (it lies in the first 16 bytes).
            let mut i = 0;
            spans.clear();
            while let Some(lv) = live.get_mut(i) {
                if lv.coded.n >= NB {
                    let lv = live.swap_remove(i);
                    results[lv.index] = lv.coded.finish(lv.out);
                    continue;
                }
                let span = lv.coded.span();
                if span.half {
                    lv.coded.run_general(&span);
                } else {
                    spans.push(span);
                    i += 1;
                }
            }
            if live.is_empty() {
                break;
            }
            // Then every frame for as many bytes as all of them keep their tables, with as
            // many history bytes (4, 8 or 16) as the longest filter needs.
            let bytes = live.iter().zip(&spans).map(|(lv, s)| (s.end - lv.coded.n) / 8).min().unwrap_or(0);
            let used = spans.iter().map(|s| s.most).max().unwrap_or(0);
            let mut runs: Vec<LaneRun<'_, '_>> =
                live.iter_mut().zip(&spans).map(|(lv, s)| lv.coded.lane_run(s)).collect();
            macro_rules! by_tables {
                ($k:literal, $n:literal) => {
                    match used {
                        0..=4 => run::<$k, $n, 4>(&mut runs, bytes),
                        5..=8 => run::<$k, $n, 8>(&mut runs, bytes),
                        _ => run::<$k, $n, 16>(&mut runs, bytes),
                    }
                };
            }
            macro_rules! by_channels {
                ($k:literal) => {
                    match nch {
                        1 => by_tables!($k, 1),
                        2 => by_tables!($k, 2),
                        3 => by_tables!($k, 3),
                        4 => by_tables!($k, 4),
                        5 => by_tables!($k, 5),
                        _ => by_tables!($k, 6),
                    }
                };
            }
            match runs.len() {
                1 => by_channels!(1),
                2 => by_channels!(2),
                3 => by_channels!(3),
                _ => by_channels!(4),
            }
            drop(runs);
            for lv in &mut live {
                lv.coded.advance(lv.coded.n + 8 * bytes);
            }
        }
    }
}

impl Lane {
    /// Splits the prediction of 5.5.3 by history byte. At bit `r` (0 to 7) of a byte, the
    /// history is the `r` bits already decoded in that byte (bit 0 of `partial`'s index
    /// the newest), then the complete bytes before it: bit `b` (0 the newest) of complete
    /// byte `d` (0 the newest) is history bit `8d + r + b`. Table `d` of a filter holds, for
    /// each byte value, its share at each of the 8 values of `r`.
    fn build_tables(&mut self, filters: &[Filter]) {
        // Tables beyond a filter's order are zero; only those a previous frame filled are
        // cleared again.
        self.tables.resize(filters.len() * HISTORY_BYTES, [[0; 8]; 256]);
        self.filled.resize(filters.len(), 0);
        self.partial.resize(filters.len(), [0; 256]);
        for (f, filter) in filters.iter().enumerate() {
            // Coefficients beyond the order (and beyond 127) are 0; every |a_i| <= 256.
            let a = |i: usize| filter.coefs.get(i).map_or(0, |&v| v as i16);
            let used = filter.order.div_ceil(8);
            for d in 0..used {
                let table = &mut self.tables[f * HISTORY_BYTES + d];
                // A history bit of 1 contributes +a_i and 0 contributes -a_i: entry 0 is
                // minus the sum over the byte's bits, and setting bit b adds 2 a_i. Every
                // entry is at most 8 x 256 in magnitude.
                let mut first = [0i16; 8];
                let mut cols = [[0i16; 8]; 8];
                for (r, f) in first.iter_mut().enumerate() {
                    for (b, col) in cols.iter_mut().enumerate() {
                        let v = a(8 * d + r + b);
                        *f -= v;
                        col[r] = 2 * v;
                    }
                }
                table[0] = first;
                for byte in 1..256usize {
                    let col = &cols[byte.trailing_zeros() as usize];
                    let prev = table[byte & (byte - 1)];
                    table[byte] = core::array::from_fn(|r| prev[r].wrapping_add(col[r]));
                }
            }
            for d in used..self.filled[f] {
                self.tables[f * HISTORY_BYTES + d] = [[0; 8]; 256];
            }
            self.filled[f] = used;
            // The newest r bits of the current byte, at most 7 terms.
            let partial = &mut self.partial[f];
            for r in 0..8 {
                for bits in 0..1usize << r {
                    partial[(1 << r) | bits] =
                        (0..r).map(|i| if (bits >> i) & 1 == 1 { a(i) } else { -a(i) }).sum();
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

/// A coded frame of [`DstDecoder::decode_group`] still being decoded, and its output.
struct Live<'b, 'o> {
    /// Its position in the group.
    index: usize,
    coded: Coded<'b>,
    out: &'o mut [u8],
}

/// A coded frame whose header has been read, decoded up to bit `n` of every channel.
struct Coded<'b> {
    nch: usize,
    fseg: Segmentation,
    pseg: Segmentation,
    fmap: Vec<Vec<u8>>,
    pmap: Vec<Vec<u8>>,
    /// Each filter's prediction order.
    orders: Vec<usize>,
    /// Per channel, the half-probability length `NH` (5.4).
    nh: [usize; MAX_CHANNELS],
    tables: &'b [ByteTable],
    partial: &'b [[i16; 256]],
    luts: &'b [[u8; LUT_LEN]],
    arith: Arith<'b>,
    /// See [`Lane::hist`].
    hist: &'b mut [u8],
    /// Per channel, the current filter and ptable segment.
    fidx: [usize; MAX_CHANNELS],
    pidx: [usize; MAX_CHANNELS],
    n: usize,
    /// The number of code bits, `fs`.
    fs: usize,
}

/// Bytes `n / 8 .. end / 8` of every channel of a frame (`n` its position), with a fixed
/// filter and ptable per channel.
#[derive(Clone, Copy)]
struct Span<'t> {
    end: usize,
    /// Whether the span holds bits of some channel's half-probability region.
    half: bool,
    /// Per channel, the history bytes its filter uses, and the most of any channel.
    used: [usize; MAX_CHANNELS],
    most: usize,
    tabs: [&'t [ByteTable; HISTORY_BYTES]; MAX_CHANNELS],
    partial: [&'t [i16; 256]; MAX_CHANNELS],
    luts: [&'t [u8; LUT_LEN]; MAX_CHANNELS],
}

impl<'b> Coded<'b> {
    /// Reads the header of `frame` (`nch` channels) and prepares `lane` for its code, up
    /// to the coded X bit. A plain frame is copied to `out` at once (`Ok(None)`).
    fn start(lane: &'b mut Lane, frame: &[u8], nch: usize, out: &mut [u8]) -> Result<Option<Self>, DstError> {
        if !(1..=MAX_CHANNELS).contains(&nch) || out.len() != nch * FL {
            return Err(DstError::InvalidArguments);
        }
        if frame.len() <= 1 {
            return Err(DstError::FrameTooShort);
        }
        if frame[0] & 0x80 == 0 {
            return plain(frame, out).map(|()| None);
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
        lane.build_tables(&filters);
        lane.build_luts(&ptables);
        // Initial history (5.5.2).
        lane.hist.resize((HISTORY_BYTES + FL) * nch + WINDOW, 0);
        lane.hist[..HISTORY_BYTES * nch].fill(HISTORY_FILL);
        let Lane { tables, partial, luts, code, hist, .. } = lane;
        let mut code = CodeBits::new(code, frame, header + 1, fs.saturating_sub(1));

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

        // Half-probability lengths (5.4).
        let mut nh = [0usize; MAX_CHANNELS];
        for ch in 0..nch {
            if half[ch] {
                nh[ch] = filters[usize::from(fmap[ch][0])].order;
            }
        }
        Ok(Some(Coded {
            nch,
            fseg,
            pseg,
            fmap,
            pmap,
            orders: filters.iter().map(|f| f.order).collect(),
            nh,
            tables,
            partial,
            luts,
            arith,
            hist,
            fidx: [0; MAX_CHANNELS],
            pidx: [0; MAX_CHANNELS],
            n: 0,
            fs,
        }))
    }

    /// The next chunk: whole bytes within which, for every channel, the filter and the
    /// ptable are fixed (segment boundaries are multiples of 8), ending at the end of a
    /// half-probability region if it holds one.
    fn span(&self) -> Span<'b> {
        let segment_end = |starts: &[u32], idx: usize| starts.get(idx + 1).map_or(NB, |&b| b as usize);
        let mut span = Span {
            end: NB,
            half: false,
            used: [0; MAX_CHANNELS],
            most: 0,
            tabs: [&ZERO_TABLES; MAX_CHANNELS],
            partial: [&[0; 256]; MAX_CHANNELS],
            luts: [&NO_LUT; MAX_CHANNELS],
        };
        for ch in 0..self.nch {
            span.end = span.end.min(segment_end(&self.fseg.starts[ch], self.fidx[ch]));
            span.end = span.end.min(segment_end(&self.pseg.starts[ch], self.pidx[ch]));
            if self.n < self.nh[ch] {
                span.half = true;
                span.end = span.end.min(self.nh[ch].next_multiple_of(8));
            }
            span.luts[ch] = &self.luts[usize::from(self.pmap[ch][self.pidx[ch]])];
            let f = usize::from(self.fmap[ch][self.fidx[ch]]);
            span.used[ch] = self.orders[f].div_ceil(8);
            span.most = span.most.max(span.used[ch]);
            let tabs = self.tables.get(f * HISTORY_BYTES..(f + 1) * HISTORY_BYTES);
            span.tabs[ch] = tabs.and_then(|t| t.try_into().ok()).unwrap_or(&ZERO_TABLES);
            span.partial[ch] = &self.partial[f];
        }
        span
    }

    /// Moves to bit `n`, at most the end of the current span, and to the next segments of
    /// the channels that end there.
    fn advance(&mut self, n: usize) {
        let segment_end = |starts: &[u32], idx: usize| starts.get(idx + 1).map_or(NB, |&b| b as usize);
        self.n = n;
        for ch in 0..self.nch {
            if segment_end(&self.fseg.starts[ch], self.fidx[ch]) == n {
                self.fidx[ch] += 1;
            }
            if segment_end(&self.pseg.starts[ch], self.pidx[ch]) == n {
                self.pidx[ch] += 1;
            }
        }
    }

    /// Decodes `span` (the next chunk) through the general loop.
    fn run_general(&mut self, span: &Span<'_>) {
        run_general(self.n, span, self.nch, &self.nh, &mut self.arith, self.hist);
        self.advance(span.end);
    }

    /// The state [`run`] decodes `span` (the next chunk) with.
    fn lane_run<'r>(&'r mut self, span: &Span<'b>) -> LaneRun<'r, 'b> {
        LaneRun {
            start: self.n / 8,
            tabs: span.tabs,
            partial: span.partial,
            luts: span.luts,
            arith: &mut self.arith,
            hist: self.hist,
        }
    }

    /// The end-of-frame check of 5.5.7, after the last symbol, and the output.
    fn finish(self, out: &mut [u8]) -> Result<(), DstError> {
        // `ptr` is the index of the next code bit.
        let ptr = 1 + self.arith.code.consumed();
        if self.fs > ptr + 7 {
            return Err(DstError::UnreadCodeBits);
        }
        let first = HISTORY_BYTES * self.nch;
        if let Some(audio) = self.hist.get(first..first + out.len()) {
            out.copy_from_slice(audio);
        }
        Ok(())
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

/// Decodes one symbol for [`run_general`]: the prediction from the shares of the
/// complete bytes (`base`) and of the bits already decoded in the byte
/// (`partial[2^r | bits]`), the probability (128 in the half-probability region, 5.5.5),
/// and the output bit.
#[inline(always)]
fn symbol(
    ar: &mut Arith<'_>,
    base: i16,
    partial: &[i16; 256],
    idx: u32,
    lut: &[u8; LUT_LEN],
    half: bool,
) -> u32 {
    // The wrapping sum is S reduced as 5.5.3 does (32768 becomes -32768).
    let spred = base.wrapping_add(partial[(idx & 0xFF) as usize]);
    let sign = u32::from(spred < 0);
    // Probability (5.5.4): |Spred| / 8 is at most 4096.
    let p = if half { 128 } else { u32::from(lut[usize::from(spred.unsigned_abs() >> 3)]) };
    sign ^ ar.decode(p)
}

/// The probability (5.5.4) and sign indicator (5.5.3) of a prediction outside the
/// half-probability region.
#[inline(always)]
fn probability(spred: i16, lut: &[u8; LUT_LEN]) -> (u32, u32) {
    // |Spred| / 8 is at most 4096.
    (u32::from(lut[usize::from(spred.unsigned_abs() >> 3)]), u32::from(spred < 0))
}

/// The `WINDOW` bytes of [`Lane::hist`] from the history of byte `m` of `nch` channels on:
/// complete byte `d` (0 the newest) of channel c is at `(HISTORY_BYTES - 1 - d) x nch + c`.
#[inline(always)]
fn window(hist: &[u8], m: usize, nch: usize) -> &[u8; WINDOW] {
    // `hist` always holds them (it has `WINDOW` spare bytes after the output).
    hist.get(m * nch..m * nch + WINDOW).and_then(|w| w.try_into().ok()).unwrap_or(&[HISTORY_FILL; WINDOW])
}

/// The sum of the rows of tables `FROM..U` of channel `ch` (of `NCH`) for the byte
/// `FROM` after the one whose history is `w`: table d takes complete byte `d - FROM`.
#[inline(always)]
fn rows<const NCH: usize, const U: usize, const FROM: usize>(
    tables: &[ByteTable; HISTORY_BYTES],
    w: &[u8; WINDOW],
    ch: usize,
) -> Row {
    let mut sum = [0i16; 8];
    // A loop over constant bounds, which is unrolled with each byte's position known.
    #[allow(clippy::needless_range_loop)]
    for d in FROM..U.min(HISTORY_BYTES) {
        let byte = w[(HISTORY_BYTES - 1 + FROM - d) * NCH + ch];
        let row = &tables[d][usize::from(byte)];
        for r in 0..8 {
            // 16 rows of at most 2048 and a partial share: the sum can wrap only as 5.5.3
            // reduces S.
            sum[r] = sum[r].wrapping_add(row[r]);
        }
    }
    sum
}

/// Most symbols per bit position (frames x channels) for which [`run`] unrolls a whole
/// byte; beyond, the loop body would outgrow the instruction caches.
const UNROLL_BYTE: usize = 10;

/// A row on the stack of [`run`], aligned so that no vector access of it splits a cache line.
#[derive(Clone, Copy)]
#[repr(align(16))]
struct Aligned([i16; 8]);

/// One frame's part in [`run`]: its chunk from byte `start` on and its decoding state.
struct LaneRun<'r, 'b> {
    start: usize,
    tabs: [&'b [ByteTable; HISTORY_BYTES]; MAX_CHANNELS],
    partial: [&'b [i16; 256]; MAX_CHANNELS],
    luts: [&'b [u8; LUT_LEN]; MAX_CHANNELS],
    arith: &'r mut Arith<'b>,
    hist: &'r mut [u8],
}

/// Decodes `bytes` bytes of every channel of `K` frames (exactly `K` lanes) of `NCH`
/// channels (1 to 6), each from its own start, outside any half-probability region, with
/// the first `U` history bytes (the tables beyond a filter's order are zero). Each
/// frame's symbols are in the order of 5.5.1; the frames take turns symbol by symbol, so
/// that their arithmetic decoders, which are independent, overlap.
#[inline(never)]
#[allow(clippy::needless_range_loop)]
fn run<const K: usize, const NCH: usize, const U: usize>(lanes: &mut [LaneRun<'_, '_>], bytes: usize) {
    let Ok(lanes) = <&mut [LaneRun<'_, '_>; K]>::try_from(lanes) else {
        return;
    };
    let mut ar: [Arith<'_>; K] = core::array::from_fn(|l| *lanes[l].arith);
    let tabs: [[&[ByteTable; HISTORY_BYTES]; NCH]; K] =
        core::array::from_fn(|l| core::array::from_fn(|ch| lanes[l].tabs[ch]));
    let partial: [[&[i16; 256]; NCH]; K] =
        core::array::from_fn(|l| core::array::from_fn(|ch| lanes[l].partial[ch]));
    let luts: [[&[u8; LUT_LEN]; NCH]; K] =
        core::array::from_fn(|l| core::array::from_fn(|ch| lanes[l].luts[ch]));
    // Per frame and channel, the shares of the complete bytes at each bit of the current
    // byte.
    let mut base = [[Aligned([0i16; 8]); NCH]; K];
    for l in 0..K {
        let w = window(lanes[l].hist, lanes[l].start, NCH);
        for ch in 0..NCH {
            base[l][ch] = Aligned(rows::<NCH, U, 0>(tabs[l][ch], w, ch));
        }
    }
    for i in 0..bytes {
        // The shares at the next byte of every complete byte but the newest (which is
        // the current byte).
        let mut rest = [[Aligned([0i16; 8]); NCH]; K];
        for l in 0..K {
            let w = window(lanes[l].hist, lanes[l].start + i, NCH);
            for ch in 0..NCH {
                rest[l][ch] = Aligned(rows::<NCH, U, 1>(tabs[l][ch], w, ch));
            }
        }
        let mut cur = [[0u32; NCH]; K];
        // A refill waits for the position after the previous symbol, so it is done as
        // rarely as allowed: before every 6 symbols, counted from the byte's first.
        macro_rules! symbol {
            ($r:literal, $ch:literal, $l:literal) => {
                if $ch < NCH && $l < K {
                    if ($r * NCH + $ch) % 6 == 0 {
                        ar[$l].refill();
                    }
                    let idx = ((1 << $r) | cur[$l][$ch]) as usize & 0xFF;
                    let spred = base[$l][$ch].0[$r].wrapping_add(partial[$l][$ch][idx]);
                    let (p, sign) = probability(spred, luts[$l][$ch]);
                    let y = sign ^ ar[$l].decode(p);
                    cur[$l][$ch] = (cur[$l][$ch] << 1) | y;
                }
            };
        }
        // Frames and channels unrolled by hand, so that their state stays in registers.
        macro_rules! lanes {
            ($r:literal, $ch:literal) => {
                symbol!($r, $ch, 0);
                symbol!($r, $ch, 1);
                symbol!($r, $ch, 2);
                symbol!($r, $ch, 3);
            };
        }
        macro_rules! bit {
            ($r:literal) => {
                lanes!($r, 0);
                lanes!($r, 1);
                lanes!($r, 2);
                lanes!($r, 3);
                lanes!($r, 4);
                lanes!($r, 5);
            };
        }
        // The same with the bit position not unrolled (a smaller loop body for many
        // symbols per byte), refilling before every channel 0: at most 6 symbols apart.
        macro_rules! symbol_at {
            ($r:ident, $ch:literal, $l:literal) => {
                if $ch < NCH && $l < K {
                    if $ch == 0 {
                        ar[$l].refill();
                    }
                    let idx = ((1 << $r) | cur[$l][$ch]) as usize & 0xFF;
                    let spred = base[$l][$ch].0[$r].wrapping_add(partial[$l][$ch][idx]);
                    let (p, sign) = probability(spred, luts[$l][$ch]);
                    let y = sign ^ ar[$l].decode(p);
                    cur[$l][$ch] = (cur[$l][$ch] << 1) | y;
                }
            };
        }
        macro_rules! lanes_at {
            ($r:ident, $ch:literal) => {
                symbol_at!($r, $ch, 0);
                symbol_at!($r, $ch, 1);
                symbol_at!($r, $ch, 2);
                symbol_at!($r, $ch, 3);
            };
        }
        if K * NCH <= UNROLL_BYTE {
            bit!(0);
            bit!(1);
            bit!(2);
            bit!(3);
            bit!(4);
            bit!(5);
            bit!(6);
            bit!(7);
        } else {
            for r in 0..8 {
                lanes_at!(r, 0);
                lanes_at!(r, 1);
                lanes_at!(r, 2);
                lanes_at!(r, 3);
                lanes_at!(r, 4);
                lanes_at!(r, 5);
            }
        }
        for l in 0..K {
            // The byte's 8 bits are the output byte (5.5.9), and the newest history byte.
            let at = (HISTORY_BYTES + lanes[l].start + i) * NCH;
            if let Some(dst) = lanes[l].hist.get_mut(at..at + NCH) {
                for (d, &c) in dst.iter_mut().zip(cur[l].iter()) {
                    *d = c as u8;
                }
            }
            for ch in 0..NCH {
                let row = &tabs[l][ch][0][(cur[l][ch] & 0xFF) as usize];
                base[l][ch] = Aligned(core::array::from_fn(|r| rest[l][ch].0[r].wrapping_add(row[r])));
            }
        }
    }
    for l in 0..K {
        *lanes[l].arith = ar[l];
    }
}

/// Decodes bytes `start / 8 .. span.end / 8` of `nch` channels, with probability 128 for
/// bits `n < nh[c]` of channel c (5.5.5).
#[allow(clippy::needless_range_loop)]
fn run_general(
    start: usize,
    span: &Span<'_>,
    nch: usize,
    nh: &[usize; MAX_CHANNELS],
    arith: &mut Arith<'_>,
    hist: &mut [u8],
) {
    for m in start / 8..span.end / 8 {
        let mut base = [[0i16; 8]; MAX_CHANNELS];
        for ch in 0..nch {
            for d in 0..span.used[ch] {
                let byte = hist.get((HISTORY_BYTES + m - 1 - d) * nch + ch).copied().unwrap_or(0);
                let row = &span.tabs[ch][d][usize::from(byte)];
                for r in 0..8 {
                    // As in `rows`.
                    base[ch][r] = base[ch][r].wrapping_add(row[r]);
                }
            }
        }
        let mut cur = [0u32; MAX_CHANNELS];
        for r in 0..8u32 {
            for ch in 0..nch {
                arith.refill();
                let half = 8 * m + (r as usize) < nh[ch];
                let idx = (1 << r) | cur[ch];
                let y = symbol(arith, base[ch][r as usize], span.partial[ch], idx, span.luts[ch], half);
                cur[ch] = (cur[ch] << 1) | y;
            }
        }
        let at = (HISTORY_BYTES + m) * nch;
        if let Some(dst) = hist.get_mut(at..at + nch) {
            for (d, &c) in dst.iter_mut().zip(cur.iter()) {
                *d = c as u8;
            }
        }
    }
}

/// Probability lookup tables have one entry per value of |Spred| / 8 (0 to 4096), and
/// are padded to 8192 so that any `u16 >> 3` indexes them without a check.
const LUT_LEN: usize = 8192;

/// The probability table of the channels that are not decoded.
static NO_LUT: [u8; LUT_LEN] = [128; LUT_LEN];

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

    // One test per frame count, so that Miri can run them in parallel jobs: this check is
    // most of Miri's time on the crate (#76).
    #[test]
    fn fast_loop_matches_general_1_frame() {
        fast_loop_matches_general(1);
    }

    #[test]
    fn fast_loop_matches_general_2_frames() {
        fast_loop_matches_general(2);
    }

    #[test]
    fn fast_loop_matches_general_3_frames() {
        fast_loop_matches_general(3);
    }

    #[test]
    fn fast_loop_matches_general_4_frames() {
        fast_loop_matches_general(4);
    }

    /// The lockstep loop of `k` frames and constant channel and table counts against the
    /// general loop run on each frame alone, with channels using different numbers of tables
    /// (zero beyond), frames at different positions with different histories and code, for
    /// every channel count and table count.
    fn fast_loop_matches_general(k: usize) {
        assert!((1..=MAX_LANES).contains(&k));
        let mut seed = 0x9E37_79B9_7F4A_7C15u64 ^ k as u64;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let code: Vec<u8> = (0..1024).map(|_| rnd() as u8).collect();
        let mut random: Vec<ByteTable> = Vec::new();
        for _ in 0..3 {
            random.push(core::array::from_fn(|_| core::array::from_fn(|_| (rnd() % 4097) as i16 - 2048)));
        }
        // For every number of tables used, 1 to 16: that many tables, then zero tables.
        let tables: Vec<[ByteTable; HISTORY_BYTES]> = (0..=HISTORY_BYTES)
            .map(|used| core::array::from_fn(|d| if d < used { random[d % 3] } else { [[0; 8]; 256] }))
            .collect();
        let partials: Vec<[i16; 256]> =
            (0..2).map(|_| core::array::from_fn(|_| (rnd() % 3585) as i16 - 1792)).collect();
        let luts: Vec<[u8; LUT_LEN]> =
            (0..2).map(|k| core::array::from_fn(|j| ((j * (k + 1) + k) % 128 + 1) as u8)).collect();
        let chunks: &[(usize, usize)] =
            if cfg!(miri) { &[(0, 0), (8, 24)] } else { &[(0, 0), (8, 16), (24, 96)] };
        for nch in 1..=MAX_CHANNELS {
            for u in [4, 8, 16] {
                for &(start, end) in chunks {
                    // Frame l starts 64 l bits later; channel c of frame l uses u - (c + l)
                    // tables (at least 1).
                    let spans: Vec<Span<'_>> = (0..k)
                        .map(|l| {
                            let used = core::array::from_fn(|c| u - (c + l).min(u - 1));
                            Span {
                                end: end + 64 * l,
                                half: false,
                                used,
                                most: u,
                                tabs: core::array::from_fn(|c| &tables[used[c]]),
                                partial: core::array::from_fn(|c| &partials[(c + l) % 2]),
                                luts: core::array::from_fn(|c| &luts[(c + 2 * l) % 3 % 2]),
                            }
                        })
                        .collect();
                    let starts: Vec<usize> = (0..k).map(|l| start + 64 * l).collect();
                    let len = (HISTORY_BYTES + FL) * nch + WINDOW;
                    let hists: Vec<Vec<u8>> =
                        (0..k).map(|_| (0..len).map(|_| rnd() as u8).collect()).collect();
                    let mut results = Vec::new();
                    for general in [false, true] {
                        let mut bufs: Vec<Vec<u8>> = vec![Vec::new(); k];
                        let mut ars: Vec<Arith<'_>> = bufs
                            .iter_mut()
                            .enumerate()
                            .map(|(l, buf)| {
                                let from = &code[128 * l..];
                                Arith {
                                    a: 4095 << 52,
                                    x: 0,
                                    code: CodeBits::new(buf, from, 0, from.len() * 8),
                                }
                            })
                            .collect();
                        let mut hists = hists.clone();
                        if general {
                            for l in 0..k {
                                run_general(
                                    starts[l],
                                    &spans[l],
                                    nch,
                                    &[0; MAX_CHANNELS],
                                    &mut ars[l],
                                    &mut hists[l],
                                );
                            }
                        } else {
                            let bytes = (end - start) / 8;
                            let mut runs: Vec<LaneRun<'_, '_>> = ars
                                .iter_mut()
                                .zip(hists.iter_mut())
                                .enumerate()
                                .map(|(l, (arith, hist))| LaneRun {
                                    start: starts[l] / 8,
                                    tabs: spans[l].tabs,
                                    partial: spans[l].partial,
                                    luts: spans[l].luts,
                                    arith,
                                    hist,
                                })
                                .collect();
                            macro_rules! by_tables {
                                ($k:literal, $n:literal) => {
                                    match u {
                                        4 => run::<$k, $n, 4>(&mut runs, bytes),
                                        8 => run::<$k, $n, 8>(&mut runs, bytes),
                                        _ => run::<$k, $n, 16>(&mut runs, bytes),
                                    }
                                };
                            }
                            macro_rules! by_channels {
                                ($k:literal) => {
                                    match nch {
                                        1 => by_tables!($k, 1),
                                        2 => by_tables!($k, 2),
                                        3 => by_tables!($k, 3),
                                        4 => by_tables!($k, 4),
                                        5 => by_tables!($k, 5),
                                        _ => by_tables!($k, 6),
                                    }
                                };
                            }
                            match k {
                                1 => by_channels!(1),
                                2 => by_channels!(2),
                                3 => by_channels!(3),
                                _ => by_channels!(4),
                            }
                        }
                        // The loops refill at different times: compare the code bits after C
                        // as far as a refill makes them valid.
                        let state: Vec<_> = ars
                            .iter_mut()
                            .map(|ar| {
                                ar.refill();
                                (ar.a, ar.x, ar.code.pos)
                            })
                            .collect();
                        results.push((state, hists));
                    }
                    assert!(
                        results[0] == results[1],
                        "{k} frames, {nch} channels, {u} tables, chunk {start}..{end}"
                    );
                }
            }
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
