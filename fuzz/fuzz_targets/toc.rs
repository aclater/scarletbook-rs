// SPDX-License-Identifier: MIT OR Apache-2.0

//! TOC parsing: the input is the image from LSN 510 on, after 510 empty sectors.
//!
//! An input that opens is also opened through the C interface, and its whole TOC read with
//! `sb_disc_toc` (#132): `scarletbook_ffi::fuzz_disc_toc`, behind that crate's `fuzzing`
//! feature, which holds the unsafe calls.

#![no_main]

#[path = "../../crates/scarletbook/tests/harness/mod.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    harness::toc(data);
    scarletbook_ffi::fuzz_disc_toc(510 * 2048, data);
});
