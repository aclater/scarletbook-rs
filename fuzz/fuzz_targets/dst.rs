// SPDX-License-Identifier: MIT OR Apache-2.0

//! Single-frame DST decoding: byte 0 picks the channel count, the rest is the frame.

#![no_main]

#[path = "../../crates/scarletbook/tests/harness/mod.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| harness::dst(data));
