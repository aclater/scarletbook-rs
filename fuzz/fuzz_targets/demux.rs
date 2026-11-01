// SPDX-License-Identifier: MIT OR Apache-2.0

//! Frame demultiplexing: byte 0 is the channel count, the rest are audio sectors.
//!
//! For one input in 4096 (`harness::pcm_case`), the same bytes are also converted to PCM,
//! in Rust (in `harness::demux`) and through the C interface's `sb_pcm_converter_new` and
//! `sb_pcm_convert_run` (#130): `scarletbook_ffi::fuzz_pcm_convert_run`, behind that
//! crate's `fuzzing` feature, which holds the unsafe calls.

#![no_main]

#[path = "../../crates/scarletbook/tests/harness/mod.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    if let Some(c) = harness::demux_case(data) {
        // An invalid channel byte as it is (refused); a valid one as the frames' count.
        let channels = if (1..=6).contains(&c.channels_raw) { c.channels } else { c.channels_raw };
        scarletbook_ffi::fuzz_pcm_convert_run(
            c.rate_hz,
            c.cutoff,
            u32::from(channels),
            c.gain_db,
            c.before,
            c.frames,
            c.after,
        );
    }
});
