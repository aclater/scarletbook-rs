// SPDX-License-Identifier: MIT OR Apache-2.0

//! Property tests of the DST decoder, including a round trip through an encoder written
//! from annex 5.A of specification 5.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod harness;

use proptest::prelude::*;
use scarletbook::dst::{self, DstDecoder, DstError, FL, MAX_LANES, NB};

fn config() -> ProptestConfig {
    if cfg!(miri) {
        ProptestConfig { cases: 2, failure_persistence: None, ..ProptestConfig::default() }
    } else {
        // PROPTEST_CASES, if set, overrides the default number of cases.
        let cases = std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(48);
        ProptestConfig { cases, ..ProptestConfig::default() }
    }
}

#[derive(Default)]
struct BitWriter {
    bits: Vec<u8>,
}

impl BitWriter {
    fn put(&mut self, v: u32, n: u32) {
        for i in (0..n).rev() {
            self.bits.push(((v >> i) & 1) as u8);
        }
    }

    fn bytes(&self) -> Vec<u8> {
        self.bits.chunks(8).map(|c| c.iter().enumerate().fold(0u8, |b, (i, &x)| b | (x << (7 - i)))).collect()
    }
}

/// The arithmetic encoder of annex 5.A.6; `lo` holds Lo's bits, most significant first.
struct ArithEncoder {
    lo: Vec<u8>,
    a: u32,
}

impl ArithEncoder {
    fn new() -> Self {
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

    fn encode(&mut self, e: u32, p: u32) {
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
    fn finish(mut self) -> Vec<u8> {
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

/// Encodes per-channel DSD (each `FL` bytes) with one filter and one ptable for every
/// channel, the simplest coded frame of annex 5.A.1.
fn encode(signal: &[Vec<u8>], coefs: &[i32], ptable: &[u32], half: &[bool]) -> Vec<u8> {
    let nch = signal.len();
    // A one-entry ptable sends only its length; its entry is 128 (specification 5, 5.3.6).
    let ptable: &[u32] = if ptable.len() == 1 { &[128] } else { ptable };
    let mut w = BitWriter::default();
    w.put(1, 1); // DST_coded
    w.put(1, 1); // same_segmentation
    w.put(1, 1); // same_segm_all_ch
    w.put(1, 1); // end_of_channel_segm
    w.put(1, 1); // same_mapping
    w.put(1, 1); // same_map_all_ch (one segment: no table indexes)
    for &h in half {
        w.put(u32::from(h), 1);
    }
    let order = coefs.len();
    w.put(order as u32 - 1, 7);
    w.put(0, 1); // coefficients verbatim
    for &a in coefs {
        w.put((a as u32) & 0x1FF, 9);
    }
    w.put(ptable.len() as u32 - 1, 6);
    if ptable.len() > 1 {
        w.put(0, 1);
        for &p in ptable {
            w.put(p - 1, 7);
        }
    }
    let mut enc = ArithEncoder::new();
    let px = u32::from(((coefs[0] & 0x7F) as u8).reverse_bits() >> 1) + 1;
    enc.encode(0, px);
    let mut history = vec![0xAAAA_AAAA_AAAA_AAAA_AAAA_AAAA_AAAA_AAAAu128; nch];
    for n in 0..NB {
        for c in 0..nch {
            let h = history[c];
            let s: i32 = coefs.iter().enumerate().map(|(i, &a)| if (h >> i) & 1 == 1 { a } else { -a }).sum();
            let s = if s == 32768 { -32768 } else { s };
            let sign = u32::from(s < 0);
            let p = if half[c] && n < order {
                128
            } else {
                ptable[((s.unsigned_abs() / 8) as usize).min(ptable.len() - 1)]
            };
            let y = u32::from((signal[c][n / 8] >> (7 - n % 8)) & 1);
            enc.encode(y ^ sign, p);
            history[c] = (h << 1) | u128::from(y);
        }
    }
    for b in enc.finish() {
        w.put(u32::from(b), 1);
    }
    w.bytes()
}

fn interleave(signal: &[Vec<u8>]) -> Vec<u8> {
    let nch = signal.len();
    (0..FL * nch).map(|i| signal[i % nch][i / nch]).collect()
}

fn signal_strategy(nch: usize) -> impl Strategy<Value = Vec<Vec<u8>>> {
    // A seed per channel: either random bytes or a slowly varying pattern, which the
    // filter predicts well.
    proptest::collection::vec((any::<u64>(), any::<bool>()), nch).prop_map(|seeds| {
        seeds
            .into_iter()
            .map(|(seed, smooth)| {
                let mut x = seed | 1;
                (0..FL)
                    .map(|i| {
                        x ^= x << 13;
                        x ^= x >> 7;
                        x ^= x << 17;
                        if smooth { [0xAA, 0x55, 0x69, 0x96][i / 600 % 4] } else { x as u8 }
                    })
                    .collect()
            })
            .collect()
    })
}

proptest! {
    #![proptest_config(config())]

    /// encode then decode returns the signal, bit for bit.
    #[test]
    #[cfg_attr(miri, ignore = "decodes full frames")]
    fn encoder_round_trip(
        (signal, half) in (1usize..=3).prop_flat_map(|n| (signal_strategy(n), proptest::collection::vec(any::<bool>(), n))),
        coefs in proptest::collection::vec(-256i32..=255, 1..=128),
        ptable in proptest::collection::vec(1u32..=128, 1..=64),
    ) {
        let frame = encode(&signal, &coefs, &ptable, &half);
        let mut out = vec![0u8; FL * signal.len()];
        dst::decode(&frame, signal.len() as u8, &mut out).unwrap();
        prop_assert!(out == interleave(&signal), "decoded audio differs");
        // Appending one zero byte is still accepted (specification 5, 5.8 item 2).
        let mut longer = frame.clone();
        longer.push(0);
        prop_assert_eq!(dst::decode(&longer, signal.len() as u8, &mut out), Ok(()));
    }

    /// Arbitrary frames: never a panic, and decoding is deterministic.
    #[test]
    #[cfg_attr(miri, ignore = "decodes full frames")]
    fn arbitrary_frames(data in proptest::collection::vec(any::<u8>(), 0..4096)) {
        harness::dst(&data);
    }

    /// Plain frames copy their payload and zero-fill the rest.
    #[test]
    fn plain_frames(nch in 1u8..=6, payload in proptest::collection::vec(any::<u8>(), 1..64)) {
        let mut frame = vec![0x40];
        frame.extend_from_slice(&payload);
        let mut out = vec![0xFFu8; FL * usize::from(nch)];
        dst::decode(&frame, nch, &mut out).unwrap();
        prop_assert_eq!(&out[..payload.len()], &payload[..]);
        prop_assert!(out[payload.len()..].iter().all(|&b| b == 0));
    }
}

/// A frame for the lockstep tests, of one of several kinds: an encoder frame (random
/// signal, filter of one of the two `orders`, ptable and half-probability flags), a plain
/// frame, a truncated or damaged encoder frame, or random bytes.
fn mixed_frame(nch: usize, kind: u8, seed: u64, orders: (usize, usize)) -> Vec<u8> {
    let mut x = seed | 1;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    match kind % 6 {
        0..=2 => {
            let signal: Vec<Vec<u8>> = (0..nch)
                .map(|c| {
                    let smooth = (seed >> c) & 1 == 1;
                    (0..FL)
                        .map(|i| if smooth { [0xAA, 0x55, 0x69, 0x96][i / 600 % 4] } else { next() as u8 })
                        .collect()
                })
                .collect();
            let order = if kind % 6 == 0 { orders.0 } else { orders.1 };
            let coefs: Vec<i32> = (0..order).map(|_| (next() % 512) as i32 - 256).collect();
            let ptable: Vec<u32> =
                (0..1 + (next() % 64) as usize).map(|_| 1 + (next() % 128) as u32).collect();
            let half: Vec<bool> = (0..nch).map(|_| next() % 3 == 0).collect();
            encode(&signal, &coefs, &ptable, &half)
        }
        3 => {
            let mut f = vec![0x40];
            f.extend((0..1 + next() % 64).map(|_| next() as u8));
            f
        }
        4 => {
            // A damaged or truncated encoder frame.
            let signal: Vec<Vec<u8>> = (0..nch).map(|_| (0..FL).map(|_| next() as u8).collect()).collect();
            let coefs: Vec<i32> = (0..orders.0).map(|_| (next() % 512) as i32 - 256).collect();
            let mut f = encode(&signal, &coefs, &[7, 90, 128], &vec![false; nch]);
            match next() % 3 {
                0 => f.truncate((next() as usize) % f.len()),
                1 => {
                    let i = (next() as usize) % f.len().min(64);
                    f[i] ^= 1 << (next() % 8);
                }
                _ => f.truncate(f.len() - 1 - (next() as usize) % 4),
            }
            f
        }
        _ => (0..(next() % 3000) as usize).map(|_| next() as u8).collect(),
    }
}

/// `decode_many` against independent `decode` calls: the same results, and the same
/// output for every frame that decodes.
fn assert_lockstep(frames: &[Vec<u8>], nch: u8) -> Result<(), TestCaseError> {
    let refs: Vec<&[u8]> = frames.iter().map(Vec::as_slice).collect();
    let mut singles = DstDecoder::new();
    let want: Vec<(Result<(), DstError>, Vec<u8>)> = refs
        .iter()
        .map(|f| {
            let mut out = vec![0u8; FL * usize::from(nch)];
            (singles.decode(f, nch, &mut out), out)
        })
        .collect();
    let mut bufs = vec![vec![0u8; FL * usize::from(nch)]; refs.len()];
    let mut outs: Vec<&mut [u8]> = bufs.iter_mut().map(Vec::as_mut_slice).collect();
    let mut dec = DstDecoder::new();
    let results = dec.decode_many(&refs, nch, &mut outs);
    prop_assert_eq!(results.len(), refs.len());
    for (i, (r, (wr, wout))) in results.iter().zip(&want).enumerate() {
        prop_assert_eq!(r, wr, "frame {} result", i);
        if wr.is_ok() {
            prop_assert!(&bufs[i] == wout, "frame {} output (channels {})", i, nch);
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(config())]

    /// Frames decoded together equal frames decoded one by one, for every channel count and
    /// every number of frames up to twice `MAX_LANES` (more than one group), mixing encoder
    /// frames of different orders (so different table counts), plain, damaged and random frames.
    #[test]
    #[cfg_attr(miri, ignore = "decodes full frames")]
    fn decode_many_equals_decode(
        nch in 1u8..=6,
        specs in proptest::collection::vec((any::<u8>(), any::<u64>()), 1..=2 * MAX_LANES),
        orders in (1usize..=128, 1usize..=128),
    ) {
        let frames: Vec<Vec<u8>> =
            specs.iter().map(|&(kind, seed)| mixed_frame(usize::from(nch), kind, seed, orders)).collect();
        for k in 1..=frames.len().min(2 * MAX_LANES) {
            assert_lockstep(&frames[..k], nch)?;
        }
    }
}

/// A shorter output buffer list than frames is an error for the frames without one, and
/// the others still decode.
#[test]
fn decode_many_without_enough_outputs() {
    let frame = vec![0x40, 1, 2, 3];
    let mut dec = DstDecoder::new();
    let mut a = vec![0u8; FL];
    let mut outs: Vec<&mut [u8]> = vec![a.as_mut_slice()];
    let results = dec.decode_many(&[&frame, &frame], 1, &mut outs);
    assert_eq!(results, vec![Ok(()), Err(DstError::InvalidArguments)]);
    assert_eq!(&a[..3], &[1, 2, 3]);
}

/// Found by the round trip in CI: a one-entry ptable whose generated value was not 128.
#[test]
#[cfg_attr(miri, ignore = "decodes full frames")]
fn one_entry_ptable_round_trip() {
    let signal = vec![(0..FL).map(|i| (i * 37 % 251) as u8).collect::<Vec<u8>>()];
    let frame = encode(&signal, &[0], &[1], &[false]);
    let mut out = vec![0u8; FL];
    dst::decode(&frame, 1, &mut out).unwrap();
    assert!(out == interleave(&signal));
}

#[test]
fn header_overrun_is_invalid() {
    let mut out = vec![0u8; FL];
    // DST_coded, then zeros: the channel's first segment needs a 13-bit resolution that
    // runs past the 16 bits of the frame.
    assert_eq!(dst::decode(&[0x80, 0x00], 1, &mut out), Err(DstError::HeaderTruncated));
    // A coded frame whose first code bit is 1 (one segment, one filter of order 1, one
    // one-entry ptable): the header is 1+1+1+1+1+1+1 + 7+1+9 + 6 = 30 bits.
    let mut w = 0u64;
    let mut n = 0;
    let mut put = |v: u64, bits: u32| {
        w = (w << bits) | v;
        n += bits;
    };
    put(0b11_1111, 6); // DST_coded, same_segmentation, same_segm_all_ch, end, same_mapping, same_map_all_ch
    put(0, 1); // half_prob
    put(0, 7); // order 1
    put(0, 1); // verbatim
    put(5, 9); // a_0 = 5
    put(0, 6); // ptable length 1
    put(1, 1); // cb[0] = 1
    put(0, 1);
    let frame = (w << (64 - n)).to_be_bytes();
    assert_eq!(dst::decode(&frame[..4], 1, &mut out), Err(DstError::ArithmeticCode));
}
