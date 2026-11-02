// SPDX-License-Identifier: MIT OR Apache-2.0

//! What an opened disc keeps once its TOC is read (#228): a raw image (2064 or 2054 bytes
//! per stored sector) keeps no read buffer, so many discs opened only for their TOCs cost
//! no more resident memory than plain ones. (That the first audio read allocates it again
//! and reads what it read before is checked in `src/disc.rs`.)

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::io::Cursor;

use common::{S, Spec};
use scarletbook::{Disc, SectorFormat};

/// `plain` stored with `format`'s sector size, the user data at its offset.
fn stored(plain: &[u8], format: SectorFormat) -> Vec<u8> {
    let (p, d) = (format.stored_size() as usize, format.data_offset() as usize);
    let mut out = vec![0u8; plain.len() / S * p];
    for (i, sector) in plain.chunks_exact(S).enumerate() {
        out[i * p + d..i * p + d + S].copy_from_slice(sector);
    }
    out
}

/// Resident set size in bytes.
fn rss() -> u64 {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap();
    let pages: u64 = statm.split_whitespace().nth(1).unwrap().parse().unwrap();
    pages * 4096
}

/// `N` discs of plain images opened, then `N` of raw ones, all kept, as a file system
/// keeps one per listed image: the raw ones add no more resident memory than the plain
/// ones. Before #228 each kept the read buffer of its TOC reads, 20 KiB on this image
/// (10 master TOC sectors of 2064 bytes) and about 80 KiB on real discs.
#[test]
#[cfg_attr(miri, ignore = "reads /proc")]
fn raw_discs_keep_no_read_buffer() {
    const N: usize = 271;
    let plain = Spec::simple().build();
    let mut kept = Vec::with_capacity(4 * N);
    let mut per_disc = Vec::new();
    for format in [SectorFormat::Plain, SectorFormat::Raw2064, SectorFormat::Raw2054] {
        let bytes: &'static [u8] = stored(&plain, format).leak();
        let before = rss();
        for _ in 0..N {
            kept.push(Disc::open(Cursor::new(bytes)).unwrap());
        }
        let grown = rss().saturating_sub(before) / N as u64;
        eprintln!("{format:?}: {grown} bytes resident per disc after its TOC");
        per_disc.push(grown);
    }
    for (format, grown) in [SectorFormat::Raw2064, SectorFormat::Raw2054].iter().zip(&per_disc[1..]) {
        assert!(*grown <= per_disc[0] + 2048, "{format:?}: {grown} bytes per disc, plain {}", per_disc[0]);
    }
}
