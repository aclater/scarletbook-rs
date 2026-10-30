// SPDX-License-Identifier: MIT OR Apache-2.0

//! A DST encoder written from annex 5.A of specification 5, and a generator of realistic
//! coded frames for benchmarks.
//!
//! Shared by the property tests (`tests/dst_props.rs`) and the benchmarks
//! (`benches/dst_frames.rs`), which include this file with `#[path]`.
//!
//! The realistic frames ([`stress_set`]) are built from general signal-processing
//! techniques, not from any other encoder: a third-order error-feedback sigma-delta
//! modulator turns sines plus low-passed noise into 1-bit audio, a least-squares predictor
//! (autocorrelation method, Levinson-Durbin recursion) is fitted to each filter's segments,
//! its coefficients are scaled to the 9-bit range, and each ptable holds the measured rate
//! of mispredicted bits per |Spred| / 8 bin (annex 5.A.4 item 6). Everything is seeded, so
//! the frames are identical on every run.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use scarletbook::dst::{FL, NB};

/// Bits of the frame, one byte per bit.
#[derive(Default)]
pub struct BitWriter {
    bits: Vec<u8>,
}

impl BitWriter {
    pub fn put(&mut self, v: u32, n: u32) {
        for i in (0..n).rev() {
            self.bits.push(((v >> i) & 1) as u8);
        }
    }

    pub fn len(&self) -> usize {
        self.bits.len()
    }

    pub fn bytes(&self) -> Vec<u8> {
        self.bits.chunks(8).map(|c| c.iter().enumerate().fold(0u8, |b, (i, &x)| b | (x << (7 - i)))).collect()
    }
}

/// bitlen of 5.1.3: the smallest y with 2^y > x.
pub fn bitlen(x: u32) -> u32 {
    32 - x.leading_zeros()
}

/// The arithmetic encoder of annex 5.A.6; `lo` holds Lo's bits, most significant first.
pub struct ArithEncoder {
    lo: Vec<u8>,
    a: u32,
}

impl Default for ArithEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl ArithEncoder {
    pub fn new() -> Self {
        ArithEncoder { lo: vec![0; 12], a: 4095 }
    }

    /// Adds `v << shift` to Lo, propagating the carry towards the most significant bit.
    fn add(&mut self, v: u32, shift: usize) {
        let mut carry = 0u32;
        let n = self.lo.len();
        let mut i = 0;
        while carry != 0 || (i < 32 && v >> i != 0) {
            let idx = (n - 1).checked_sub(shift + i).expect("carry reached cb[0]");
            let bit = if i < 32 { (v >> i) & 1 } else { 0 };
            let sum = u32::from(self.lo[idx]) + bit + carry;
            self.lo[idx] = (sum & 1) as u8;
            carry = sum >> 1;
            i += 1;
        }
    }

    pub fn encode(&mut self, e: u32, p: u32) {
        let q = (self.a >> 8) | ((self.a >> 7) & 1);
        let w0 = q * p;
        let w1 = self.a - w0;
        if e == 1 {
            self.a = w1;
        } else {
            self.add(w1, 0);
            self.a = w0;
        }
        while self.a < 2048 {
            self.a <<= 1;
            self.lo.push(0);
        }
    }

    /// The codeword: cb[0] = 0, then the bits of floor(Mv / 2^11).
    pub fn finish(mut self) -> Vec<u8> {
        let n = self.lo.len();
        if self.lo[n - 11..].iter().any(|&b| b != 0) {
            self.lo[n - 11..].fill(0);
            self.add(1, 11);
        }
        let mut out = vec![0u8];
        out.extend_from_slice(&self.lo[..n - 11]);
        out
    }
}

/// The two kinds of segmentation of 5.3.2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegKind {
    Filter,
    Ptable,
}

impl SegKind {
    /// Lmin / 8, in bytes.
    fn lmin_bytes(self) -> u32 {
        match self {
            SegKind::Filter => 128,
            SegKind::Ptable => 4,
        }
    }

    /// Most explicit lengths: (shared, per channel).
    fn max_lengths(self) -> (usize, usize) {
        match self {
            SegKind::Filter => (4, 3),
            SegKind::Ptable => (7, 7),
        }
    }
}

/// One segmentation (5.3.2): the resolution `R` in bytes and, per channel, the explicit
/// segment lengths in units of `R` (the last segment of each channel is implicit).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segmentation {
    pub resolution: u32,
    pub lengths: Vec<Vec<u32>>,
}

impl Segmentation {
    /// One segment per channel.
    pub fn single(nch: usize) -> Self {
        Segmentation { resolution: 1, lengths: vec![Vec::new(); nch] }
    }

    /// `parts` equal segments for every channel (`parts` divides 4704).
    pub fn equal(nch: usize, parts: u32) -> Self {
        assert_eq!(FL as u32 % parts, 0);
        Segmentation { resolution: FL as u32 / parts, lengths: vec![vec![1; parts as usize - 1]; nch] }
    }

    /// Number of segments of channel `c`.
    pub fn count(&self, c: usize) -> usize {
        self.lengths[c].len() + 1
    }

    /// The first bit of each segment of channel `c`, then `NB`.
    pub fn bounds(&self, c: usize) -> Vec<usize> {
        let mut b = vec![0];
        for &l in &self.lengths[c] {
            b.push(b.last().unwrap() + 8 * (self.resolution * l) as usize);
        }
        b.push(NB);
        b
    }

    fn write(&self, w: &mut BitWriter, kind: SegKind) {
        let shared = self.lengths.iter().all(|l| l == &self.lengths[0]);
        w.put(u32::from(shared), 1);
        let (max_shared, max_per_channel) = kind.max_lengths();
        let limit = if shared { max_shared } else { max_per_channel };
        let channels = if shared { 1 } else { self.lengths.len() };
        let lmin = kind.lmin_bytes();
        let r = self.resolution;
        let mut r_sent = false;
        for lengths in &self.lengths[..channels] {
            assert!(lengths.len() <= limit, "too many explicit lengths");
            let mut rem = FL as u32 - lmin;
            for &l in lengths {
                assert!(r * l >= lmin && r * l <= rem, "segment length out of range");
                w.put(0, 1);
                if !r_sent {
                    w.put(r, bitlen(FL as u32 - lmin));
                    r_sent = true;
                }
                w.put(l, bitlen(rem / r));
                rem -= r * l;
            }
            w.put(1, 1);
        }
    }
}

/// Writes one table_mapping (5.3.3); returns the number of tables.
fn write_mapping(w: &mut BitWriter, map: &[Vec<usize>]) -> usize {
    assert_eq!(map[0][0], 0, "segment 0 of channel 0 has table 0");
    let same = map.iter().all(|m| m == &map[0]);
    w.put(u32::from(same), 1);
    let mut t = 1usize;
    let mut emit = |v: usize| {
        assert!(v <= t, "tables must be numbered in order of first use");
        w.put(v as u32, bitlen(t as u32));
        if v == t {
            t += 1;
        }
    };
    if same {
        map[0][1..].iter().for_each(|&v| emit(v));
    } else {
        for (c, m) in map.iter().enumerate() {
            for (j, &v) in m.iter().enumerate() {
                if c != 0 || j != 0 {
                    emit(v);
                }
            }
        }
    }
    assert!(t <= 2 * map.len(), "at most 2 x Nch tables");
    t
}

/// Bits of a Rice code of `r` with parameter `m` (annex 5.A.4 item 5).
fn rice_len(r: i32, m: u32) -> usize {
    let u = r.unsigned_abs();
    (u >> m) as usize + 1 + m as usize + usize::from(u != 0)
}

fn put_rice(w: &mut BitWriter, r: i32, m: u32) {
    let u = r.unsigned_abs();
    for _ in 0..u >> m {
        w.put(0, 1);
    }
    w.put(1, 1);
    w.put(u & ((1 << m) - 1), m);
    if u != 0 {
        w.put(u32::from(r < 0), 1);
    }
}

/// Residuals of `values` for a predictive method with predictor `b` (5.3.5, 5.3.6):
/// r_i = v_i + floor((x_i + 4) / 8).
fn residuals(values: &[i32], b: &[i32]) -> Vec<i32> {
    (b.len()..values.len())
        .map(|i| {
            let x: i32 = b.iter().enumerate().map(|(t, &bt)| bt * values[i - 1 - t]).sum();
            values[i] + (x + 4).div_euclid(8)
        })
        .collect()
}

/// Picks the shortest of the coded methods: (method, Rice parameter, bits), or `None` when
/// no method applies.
fn best_coding(
    values: &[i32],
    methods: &[&[i32]; 3],
    verbatim_bits: u32,
    max_m: u32,
) -> Option<(usize, u32)> {
    let mut best: Option<(usize, u32, usize)> = None;
    for (k, b) in methods.iter().enumerate() {
        if b.len() >= values.len() {
            continue;
        }
        let r = residuals(values, b);
        for m in 0..=max_m {
            let bits =
                2 + verbatim_bits as usize * b.len() + 3 + r.iter().map(|&x| rice_len(x, m)).sum::<usize>();
            if best.is_none_or(|(_, _, bb)| bits < bb) {
                best = Some((k, m, bits));
            }
        }
    }
    best.filter(|&(_, _, bits)| bits < 1 + values.len() * verbatim_bits as usize).map(|(k, m, _)| (k, m))
}

const COEF_METHODS: [&[i32]; 3] = [&[-8], &[-16, 8], &[-9, -5, 6]];
const PTABLE_METHODS: [&[i32]; 3] = [&[-8], &[-16, 8], &[-24, 24, -8]];

fn write_filter(w: &mut BitWriter, coefs: &[i32], coded: bool) {
    w.put(coefs.len() as u32 - 1, 7);
    match best_coding(coefs, &COEF_METHODS, 9, 7).filter(|_| coded) {
        None => {
            w.put(0, 1);
            for &a in coefs {
                w.put((a as u32) & 0x1FF, 9);
            }
        }
        Some((k, m)) => {
            w.put(1, 1);
            w.put(k as u32, 2);
            for &a in &coefs[..=k] {
                w.put((a as u32) & 0x1FF, 9);
            }
            w.put(m, 3);
            for r in residuals(coefs, COEF_METHODS[k]) {
                put_rice(w, r, m);
            }
        }
    }
}

fn write_ptable(w: &mut BitWriter, ptable: &[u32], coded: bool) {
    w.put(ptable.len() as u32 - 1, 6);
    if ptable.len() == 1 {
        assert_eq!(ptable[0], 128, "a one-entry ptable is 128");
        return;
    }
    let values: Vec<i32> = ptable.iter().map(|&p| p as i32).collect();
    match best_coding(&values, &PTABLE_METHODS, 7, 7).filter(|_| coded) {
        None => {
            w.put(0, 1);
            for &p in ptable {
                w.put(p - 1, 7);
            }
        }
        Some((k, m)) => {
            w.put(1, 1);
            w.put(k as u32, 2);
            for &p in &ptable[..=k] {
                w.put(p - 1, 7);
            }
            w.put(m, 3);
            for r in residuals(&values, PTABLE_METHODS[k]) {
                put_rice(w, r, m);
            }
        }
    }
}

/// The initial prediction history of 5.5.2.
pub const INITIAL_HISTORY: u128 = 0xAAAA_AAAA_AAAA_AAAA_AAAA_AAAA_AAAA_AAAA;

/// Spred of 5.5.3 for a history (bit i = h[i]).
pub fn predict(h: u128, coefs: &[i32]) -> i32 {
    let s: i32 = coefs.iter().enumerate().map(|(i, &a)| if (h >> i) & 1 == 1 { a } else { -a }).sum();
    if s == 32768 { -32768 } else { s }
}

/// Bit `n` of a channel's DSD bytes.
pub fn bit(signal: &[u8], n: usize) -> u32 {
    u32::from((signal[n / 8] >> (7 - n % 8)) & 1)
}

/// Everything a coded frame states.
#[derive(Clone, Debug)]
pub struct FrameSpec {
    pub filter_seg: Segmentation,
    /// `None`: `same_segmentation` = 1.
    pub ptable_seg: Option<Segmentation>,
    /// Filter number of each filter segment, per channel.
    pub filter_map: Vec<Vec<usize>>,
    /// `None`: `same_mapping` = 1.
    pub ptable_map: Option<Vec<Vec<usize>>>,
    pub half: Vec<bool>,
    pub filters: Vec<Vec<i32>>,
    pub ptables: Vec<Vec<u32>>,
    /// Entropy-code filters and ptables (Rice) where that is shorter.
    pub coded_tables: bool,
}

impl FrameSpec {
    fn ptable_seg(&self) -> &Segmentation {
        self.ptable_seg.as_ref().unwrap_or(&self.filter_seg)
    }

    fn ptable_map(&self) -> &[Vec<usize>] {
        self.ptable_map.as_deref().unwrap_or(&self.filter_map)
    }

    /// Per bit of channel `c`, the table number of `seg`/`map`.
    fn per_bit(seg: &Segmentation, map: &[Vec<usize>], c: usize) -> Vec<u8> {
        let b = seg.bounds(c);
        let mut out = vec![0u8; NB];
        for j in 0..seg.count(c) {
            out[b[j]..b[j + 1]].fill(map[c][j] as u8);
        }
        out
    }
}

/// Encodes per-channel DSD (each `FL` bytes) as a coded frame (annex 5.A).
pub fn encode(signal: &[Vec<u8>], spec: &FrameSpec) -> Vec<u8> {
    let nch = signal.len();
    let mut w = BitWriter::default();
    w.put(1, 1); // DST_coded
    w.put(u32::from(spec.ptable_seg.is_none()), 1);
    spec.filter_seg.write(&mut w, SegKind::Filter);
    if let Some(p) = &spec.ptable_seg {
        p.write(&mut w, SegKind::Ptable);
    }
    w.put(u32::from(spec.ptable_map.is_none()), 1);
    let nf = write_mapping(&mut w, &spec.filter_map);
    let np = match &spec.ptable_map {
        Some(m) => write_mapping(&mut w, m),
        None => {
            for c in 0..nch {
                assert_eq!(
                    spec.ptable_seg().count(c),
                    spec.filter_seg.count(c),
                    "same_mapping needs equal counts"
                );
            }
            nf
        }
    };
    assert_eq!((nf, np), (spec.filters.len(), spec.ptables.len()));
    for &h in &spec.half {
        w.put(u32::from(h), 1);
    }
    for f in &spec.filters {
        write_filter(&mut w, f, spec.coded_tables);
    }
    for p in &spec.ptables {
        write_ptable(&mut w, p, spec.coded_tables);
    }

    let fmap: Vec<Vec<u8>> =
        (0..nch).map(|c| FrameSpec::per_bit(&spec.filter_seg, &spec.filter_map, c)).collect();
    let pmap: Vec<Vec<u8>> =
        (0..nch).map(|c| FrameSpec::per_bit(spec.ptable_seg(), spec.ptable_map(), c)).collect();
    let nh: Vec<usize> = (0..nch)
        .map(|c| if spec.half[c] { spec.filters[usize::from(fmap[c][0])].len() } else { 0 })
        .collect();

    let mut enc = ArithEncoder::new();
    let px = u32::from(((spec.filters[0][0] & 0x7F) as u8).reverse_bits() >> 1) + 1;
    enc.encode(0, px);
    let mut history = vec![INITIAL_HISTORY; nch];
    for n in 0..NB {
        for c in 0..nch {
            let h = history[c];
            let s = predict(h, &spec.filters[usize::from(fmap[c][n])]);
            let sign = u32::from(s < 0);
            let p = if n < nh[c] {
                128
            } else {
                let t = &spec.ptables[usize::from(pmap[c][n])];
                t[((s.unsigned_abs() / 8) as usize).min(t.len() - 1)]
            };
            let y = bit(&signal[c], n);
            enc.encode(y ^ sign, p);
            history[c] = (h << 1) | u128::from(y);
        }
    }
    for b in enc.finish() {
        w.put(u32::from(b), 1);
    }
    w.bytes()
}

/// The simplest coded frame of annex 5.A.1: one filter and one ptable for every channel.
pub fn encode_simple(signal: &[Vec<u8>], coefs: &[i32], ptable: &[u32], half: &[bool]) -> Vec<u8> {
    let nch = signal.len();
    // A one-entry ptable sends only its length; its entry is 128 (specification 5, 5.3.6).
    let ptable: &[u32] = if ptable.len() == 1 { &[128] } else { ptable };
    let spec = FrameSpec {
        filter_seg: Segmentation::single(nch),
        ptable_seg: None,
        filter_map: vec![vec![0]; nch],
        ptable_map: None,
        half: half.to_vec(),
        filters: vec![coefs.to_vec()],
        ptables: vec![ptable.to_vec()],
        coded_tables: false,
    };
    encode(signal, &spec)
}

/// Per-channel DSD in the interleaved layout of 5.5.9.
pub fn interleave(signal: &[Vec<u8>]) -> Vec<u8> {
    let nch = signal.len();
    (0..FL * nch).map(|i| signal[i % nch][i / nch]).collect()
}

/// A xorshift64 generator.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// The 1-bit sample rate, 64 x 44 100 Hz.
const FS: f64 = 2_822_400.0;

/// `frames` consecutive frames of `nch` channels of 1-bit audio: per channel, two or three
/// sines and low-passed noise, through a third-order error-feedback sigma-delta modulator
/// with noise transfer function (1 - z^-1)^3 / (1 - 0.75 z^-1)^3 (its gain at most 1.5).
/// Returns `[frame][channel]` of `FL` bytes each.
pub fn sdm_frames(seed: u64, nch: usize, frames: usize) -> Vec<Vec<Vec<u8>>> {
    let mut rng = Rng::new(seed);
    let mut out = vec![vec![vec![0u8; FL]; nch]; frames];
    for c in 0..nch {
        let tones: Vec<(f64, f64, f64)> = (0..2 + rng.next_u64() % 2)
            .map(|_| {
                let f = 60.0 * (2.0f64).powf(rng.unit() * 7.0); // 60 Hz to 7.7 kHz
                (2.0 * std::f64::consts::PI * f / FS, 0.08 + 0.12 * rng.unit(), rng.unit() * 6.3)
            })
            .collect();
        // One-pole low-pass of white noise, cut-off 2 to 12 kHz.
        let fc = 2000.0 + 10_000.0 * rng.unit();
        let lp = (-2.0 * std::f64::consts::PI * fc / FS).exp();
        let noise_gain = 0.05 * (1.0 - lp) / (1.0 - lp * lp).sqrt() * 8.0;
        let mut noise = 0.0f64;
        // Error-feedback state: past errors e and past shaped errors g.
        let (n1, n2, n3) = (-3.0, 3.0, -1.0);
        let (d1, d2, d3) = (-2.25, 1.6875, -0.421_875);
        let (mut e1, mut e2, mut e3, mut g1, mut g2, mut g3) = (0.0f64, 0.0, 0.0, 0.0, 0.0, 0.0);
        for (k, frame) in out.iter_mut().enumerate() {
            let bytes = &mut frame[c];
            for n in 0..NB {
                let t = (k * NB + n) as f64;
                noise = lp * noise + (1.0 - lp) * (rng.unit() * 2.0 - 1.0);
                let x: f64 =
                    tones.iter().map(|&(w, a, ph)| a * (w * t + ph).sin()).sum::<f64>() + noise_gain * noise;
                let v = x + n1 * e1 + n2 * e2 + n3 * e3 - d1 * g1 - d2 * g2 - d3 * g3;
                let y = if v >= 0.0 { 1.0 } else { -1.0 };
                let e = y - v;
                let g = e + (v - x);
                (e3, e2, e1) = (e2, e1, e);
                (g3, g2, g1) = (g2, g1, g);
                if y > 0.0 {
                    bytes[n / 8] |= 0x80 >> (n % 8);
                }
            }
        }
    }
    out
}

/// A predictor fitted by least squares: x_n ~ sum a_i x_(n-1-i), x = +-1, over the bit
/// ranges `ranges` of `signal`, by the autocorrelation method.
fn fit(signal: &[u8], ranges: &[(usize, usize)], order: usize) -> Vec<f64> {
    let x = |n: usize| if bit(signal, n) == 1 { 1.0f64 } else { -1.0 };
    let mut r = vec![0.0f64; order + 1];
    for &(lo, hi) in ranges {
        let xs: Vec<f64> = (lo..hi).map(x).collect();
        for (k, rk) in r.iter_mut().enumerate() {
            *rk += xs[k..].iter().zip(&xs).map(|(a, b)| a * b).sum::<f64>();
        }
    }
    r[0] *= 1.0 + 1e-4; // a little regularisation
    // Levinson-Durbin.
    let mut a = vec![0.0f64; order];
    let mut err = r[0];
    for i in 0..order {
        let acc: f64 = r[i + 1] - (0..i).map(|j| a[j] * r[i - j]).sum::<f64>();
        let k = acc / err;
        let prev = a.clone();
        a[i] = k;
        for j in 0..i {
            a[j] = prev[j] - k * prev[i - 1 - j];
        }
        err *= 1.0 - k * k;
        if err <= 0.0 {
            break;
        }
    }
    a
}

/// Estimated code bits of the symbols for a scaled real-valued prediction: per bin of
/// |Spred| / 8, the binary entropy of the misprediction rate.
fn estimated_bits(pred: &[(f64, bool)], scale: f64) -> f64 {
    let mut tot = [0u32; 64];
    let mut mis = [0u32; 64];
    for &(p, y) in pred {
        let s = p * scale;
        let j = ((s.abs() / 8.0) as usize).min(63);
        tot[j] += 1;
        mis[j] += u32::from((s >= 0.0) != y);
    }
    tot.iter()
        .zip(&mis)
        .filter(|(t, _)| **t > 0)
        .map(|(&t, &m)| {
            let q = (f64::from(m) / f64::from(t)).clamp(1.0 / 256.0, 0.5);
            -f64::from(t) * (q * q.log2() + (1.0 - q) * (1.0 - q).log2())
        })
        .sum()
}

/// Integer coefficients for `a`: the scale (at most 255 / max |a_i|) that minimises the
/// estimated code length over `ranges`.
fn quantise(signal: &[u8], ranges: &[(usize, usize)], a: &[f64]) -> Vec<i32> {
    let order = a.len();
    let mut pred = Vec::new();
    for &(lo, hi) in ranges {
        for n in lo.max(order)..hi {
            let p: f64 =
                a.iter().enumerate().map(|(i, &ai)| if bit(signal, n - 1 - i) == 1 { ai } else { -ai }).sum();
            pred.push((p, bit(signal, n) == 1));
        }
    }
    let max = a.iter().fold(1e-9f64, |m, &v| m.max(v.abs()));
    let top = 255.0 / max;
    let scale = (0..8)
        .map(|k| top / f64::from(1u32 << k))
        .min_by(|&s, &t| estimated_bits(&pred, s).total_cmp(&estimated_bits(&pred, t)))
        .unwrap();
    a.iter().map(|&v| (v * scale).round().clamp(-256.0, 255.0) as i32).collect()
}

/// How a generated frame is laid out.
#[derive(Clone, Debug)]
pub struct Design {
    /// Prediction order of each filter of a channel; channel c's filter segment j uses
    /// its filter `j % orders.len()`.
    pub orders: Vec<usize>,
    pub filter_seg: Segmentation,
    /// Ptables per channel; channel c's ptable segment j uses its ptable `j % ptables`.
    pub ptables: usize,
    /// `None`: the filter segmentation (and, if `ptables == orders.len()`, its mapping).
    pub ptable_seg: Option<Segmentation>,
    pub half: bool,
    pub coded_tables: bool,
}

fn per_channel_map(seg: &Segmentation, per: usize) -> Vec<Vec<usize>> {
    (0..seg.lengths.len()).map(|c| (0..seg.count(c)).map(|j| c * per + j % per).collect()).collect()
}

/// Fits filters and ptables to `signal` for the layout `d`.
pub fn design(signal: &[Vec<u8>], d: &Design) -> FrameSpec {
    let nch = signal.len();
    let nf = d.orders.len();
    let filter_map = per_channel_map(&d.filter_seg, nf);
    let mut filters = Vec::new();
    for (c, sig) in signal.iter().enumerate() {
        let b = d.filter_seg.bounds(c);
        for (k, &order) in d.orders.iter().enumerate() {
            let ranges: Vec<(usize, usize)> =
                (0..d.filter_seg.count(c)).filter(|j| j % nf == k).map(|j| (b[j], b[j + 1])).collect();
            filters.push(quantise(sig, &ranges, &fit(sig, &ranges, order)));
        }
    }
    let ptable_map = if d.ptable_seg.is_none() && d.ptables == nf {
        None
    } else {
        Some(per_channel_map(d.ptable_seg.as_ref().unwrap_or(&d.filter_seg), d.ptables))
    };
    let mut spec = FrameSpec {
        filter_seg: d.filter_seg.clone(),
        ptable_seg: d.ptable_seg.clone(),
        filter_map,
        ptable_map,
        half: (0..nch).map(|c| d.half && c % 2 == 0).collect(),
        filters,
        ptables: Vec::new(),
        coded_tables: d.coded_tables,
    };

    // Misprediction counts per ptable and bin, with the decoder's own prediction.
    let np = nch * d.ptables;
    let mut tot = vec![[0u32; 64]; np];
    let mut mis = vec![[0u32; 64]; np];
    for (c, sig) in signal.iter().enumerate() {
        let fm = FrameSpec::per_bit(&spec.filter_seg, &spec.filter_map, c);
        let pm = FrameSpec::per_bit(spec.ptable_seg(), spec.ptable_map(), c);
        let nh = if spec.half[c] { spec.filters[usize::from(fm[0])].len() } else { 0 };
        let mut h = INITIAL_HISTORY;
        for n in 0..NB {
            let s = predict(h, &spec.filters[usize::from(fm[n])]);
            let y = bit(sig, n);
            if n >= nh {
                let (q, j) = (usize::from(pm[n]), ((s.unsigned_abs() / 8) as usize).min(63));
                tot[q][j] += 1;
                mis[q][j] += u32::from(y == u32::from(s < 0));
            }
            h = (h << 1) | u128::from(y);
        }
    }
    spec.ptables = tot
        .iter()
        .zip(&mis)
        .map(|(t, m)| {
            let len = t.iter().rposition(|&x| x > 0).map_or(1, |i| i + 1);
            if len == 1 {
                return vec![128];
            }
            // Bins beyond the table's length fold into its last entry.
            let (mut t, mut m) = (*t, *m);
            (t[len - 1], m[len - 1]) = (t[len - 1..].iter().sum(), m[len - 1..].iter().sum());
            (0..len)
                .map(|j| {
                    if t[j] == 0 {
                        64
                    } else {
                        ((256.0 * f64::from(m[j]) / f64::from(t[j])).round() as u32).clamp(1, 128)
                    }
                })
                .collect()
        })
        .collect();
    spec
}

/// One frame of the stress set.
#[derive(Clone, Debug)]
pub struct StressFrame {
    /// Channels.
    pub channels: u8,
    /// Layout name: `o128`, `o32` or `segmented`.
    pub layout: &'static str,
    /// The coded frame.
    pub frame: Vec<u8>,
    /// The audio it decodes to, interleaved.
    pub audio: Vec<u8>,
}

/// The layouts of the stress set, for `nch` channels.
pub fn stress_layouts(nch: usize) -> Vec<(&'static str, Design)> {
    // Segmented: per-channel filter segmentations (two filters per channel, of order 128
    // and 32, alternating over 3 or 4 segments) and a separate shared ptable segmentation
    // of 8 segments with two ptables per channel; filters and ptables Rice coded.
    let r = FL as u32 / 8;
    let filter_seg = Segmentation {
        resolution: r,
        lengths: (0..nch).map(|c| if c % 2 == 0 { vec![2, 2, 2] } else { vec![3, 2] }).collect(),
    };
    vec![
        (
            "o128",
            Design {
                orders: vec![128],
                filter_seg: Segmentation::single(nch),
                ptables: 1,
                ptable_seg: None,
                half: true,
                coded_tables: true,
            },
        ),
        (
            "o32",
            Design {
                orders: vec![32],
                filter_seg: Segmentation::single(nch),
                ptables: 1,
                ptable_seg: None,
                half: true,
                coded_tables: true,
            },
        ),
        (
            "segmented",
            Design {
                orders: vec![128, 32],
                filter_seg,
                ptables: 2,
                ptable_seg: Some(Segmentation::equal(nch, 8)),
                half: false,
                coded_tables: true,
            },
        ),
    ]
}

/// The stress set: for 2 and 6 channels, `frames` consecutive frames of one seeded signal
/// in each layout of [`stress_layouts`]. Identical on every run.
pub fn stress_set(frames: usize) -> Vec<StressFrame> {
    let mut out = Vec::new();
    for nch in [2usize, 6] {
        let signal = sdm_frames(0x5CA4_1E7B ^ nch as u64, nch, frames);
        for (layout, d) in stress_layouts(nch) {
            for s in &signal {
                let frame = encode(s, &design(s, &d));
                assert!(frame.len() < FL * nch, "stress frame does not compress");
                out.push(StressFrame { channels: nch as u8, layout, frame, audio: interleave(s) });
            }
        }
    }
    out
}
