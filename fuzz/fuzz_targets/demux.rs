// SPDX-License-Identifier: MIT OR Apache-2.0

//! Frame demultiplexing: byte 0 is the channel count, the rest are audio sectors.

#![no_main]

#[path = "../../crates/scarletbook/tests/harness/mod.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| harness::demux(data));
