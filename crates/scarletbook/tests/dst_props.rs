// SPDX-License-Identifier: MIT OR Apache-2.0

//! Property tests of the DST decoder, including a round trip through an encoder written
//! from annex 5.A of specification 5.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/dst_enc.rs"]
mod dst_enc;
mod harness;

use dst_enc::{encode_simple as encode, interleave};
use proptest::prelude::*;
use scarletbook::dst::{self, DstError, FL};

fn config() -> ProptestConfig {
    if cfg!(miri) {
        ProptestConfig { cases: 2, failure_persistence: None, ..ProptestConfig::default() }
    } else {
        // PROPTEST_CASES, if set, overrides the default number of cases.
        let cases = std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(48);
        ProptestConfig { cases, ..ProptestConfig::default() }
    }
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

/// The generated stress set (as used by the benchmarks) decodes to its signal, in every
/// layout: segmented, per-channel and coded filters and ptables.
#[test]
#[cfg_attr(miri, ignore = "decodes full frames")]
fn stress_set_round_trip() {
    let mut dec = scarletbook::DstDecoder::new();
    for f in dst_enc::stress_set(1) {
        let mut out = vec![0u8; FL * usize::from(f.channels)];
        dec.decode(&f.frame, f.channels, &mut out).unwrap();
        assert!(out == f.audio, "{} channels, {}: decoded audio differs", f.channels, f.layout);
        eprintln!(
            "{} ch {}: {} bytes ({:.1}%)",
            f.channels,
            f.layout,
            f.frame.len(),
            100.0 * f.frame.len() as f64 / out.len() as f64
        );
    }
}
