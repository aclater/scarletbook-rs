// SPDX-License-Identifier: MIT OR Apache-2.0

//! Hostile audio sector streams for the seek tests (issue #68).

#![allow(dead_code, clippy::unwrap_used)]

use proptest::prelude::*;

use super::{PFrame, pack};

pub const S: usize = 2048;
pub const HUGE: u32 = 1_166_880;

pub fn tc(n: u32) -> [u8; 3] {
    [(n / 4500) as u8, (n / 75 % 60) as u8, (n % 75) as u8]
}

/// A DST sector starting one frame (numbered `number`, sector count `count`) with a
/// packet of `len` bytes, then, if `more`, a second start packet without a record.
pub fn start_sector(number: u32, count: u8, len: u16, more: bool, seed: u8) -> [u8; S] {
    let mut s = [0u8; S];
    let k = if more { 2 } else { 1 };
    s[0] = 1 | (1 << 2) | (k << 5);
    s[1..3].copy_from_slice(&((1u16 << 15) | (2 << 11) | len).to_be_bytes());
    let mut at = 3;
    if more {
        s[3..5].copy_from_slice(&((1u16 << 15) | (2 << 11) | 3).to_be_bytes());
        at = 5;
    }
    s[at..at + 3].copy_from_slice(&tc(number));
    s[at + 3] = (count & 0x1F) << 2;
    for (i, b) in s[at + 4..].iter_mut().enumerate().take(usize::from(len)) {
        *b = (i as u8).wrapping_mul(13) ^ seed;
    }
    s
}

/// A DST sector of `k` continuation packets of `len` bytes each (zero-length allowed).
pub fn continuation_sector(k: u8, len: u16) -> [u8; S] {
    let mut s = [0u8; S];
    s[0] = 1 | (k << 5);
    for p in 0..usize::from(k) {
        s[1 + 2 * p..3 + 2 * p].copy_from_slice(&((2u16 << 11) | len).to_be_bytes());
    }
    s
}

/// A sector whose single packet runs past its end (specification 4, 4.4.3 item 1).
pub fn overrun_sector() -> [u8; S] {
    let mut s = [0u8; S];
    s[0] = 1 << 5;
    s[1..3].copy_from_slice(&((2u16 << 11) | 2047).to_be_bytes());
    s
}

/// Hostile pieces of a sector stream: frames with backwards, repeated and huge timecodes,
/// random sector counts, overruns, garbage, and runs of continuation sectors long enough
/// to hold a frame under assembly across many checkpoints.
pub fn piece() -> impl Strategy<Value = Vec<[u8; S]>> {
    prop_oneof![
        4 => (0u32..120, 1u8..4, 1u16..1500, any::<bool>(), any::<u8>())
            .prop_map(|(n, c, l, m, seed)| vec![start_sector(n, c, l, m, seed)]),
        1 => (prop_oneof![Just(HUGE), 100u32..HUGE], any::<u8>())
            .prop_map(|(n, seed)| vec![start_sector(n, 1, 1, false, seed)]),
        3 => (0u8..4, 0u16..400).prop_map(|(k, l)| vec![continuation_sector(k, l)]),
        1 => (33usize..80, 0u8..3).prop_map(|(n, k)| vec![continuation_sector(k, 0); n]),
        // A complete DST frame spread over many sectors without packets.
        2 => (0u32..120, 1usize..100, 1u16..900, any::<u8>()).prop_map(|(number, gap, len, seed)| {
            let mut v = vec![start_sector(number, 3, len, false, seed)];
            v.extend(vec![continuation_sector(0, 0); gap]);
            v.extend([continuation_sector(1, len), continuation_sector(1, 1)]);
            v
        }),
        1 => Just(vec![overrun_sector()]),
        1 => proptest::collection::vec(any::<u8>(), S).prop_map(|v| vec![<[u8; S]>::try_from(v).unwrap()]),
        2 => proptest::collection::vec((0u32..4, 1usize..5000), 1..6).prop_map(|v| {
            let mut n = 0;
            let fs: Vec<PFrame> = v
                .into_iter()
                .map(|(gap, len)| {
                    n += gap;
                    let f = PFrame { number: n, dst: true, data: vec![n as u8; len] };
                    n += 1;
                    f
                })
                .collect();
            pack(&fs)
        }),
    ]
}

pub fn stream() -> impl Strategy<Value = Vec<[u8; S]>> {
    proptest::collection::vec(piece(), 1..if cfg!(miri) { 4 } else { 40 })
        .prop_map(|v| v.into_iter().flatten().collect())
}
