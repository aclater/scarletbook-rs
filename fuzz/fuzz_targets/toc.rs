// SPDX-License-Identifier: MIT OR Apache-2.0

//! TOC parsing: the input is the image from LSN 510 on, after 510 empty sectors.

#![no_main]

#[path = "../../crates/scarletbook/tests/harness/mod.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| harness::toc(data));
