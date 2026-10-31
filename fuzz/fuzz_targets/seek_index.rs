// SPDX-License-Identifier: MIT OR Apache-2.0

//! Loading a saved seek index from untrusted bytes, then reading through it.

#![no_main]

#[path = "../../crates/scarletbook/tests/harness/mod.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| harness::seek_index(data));
