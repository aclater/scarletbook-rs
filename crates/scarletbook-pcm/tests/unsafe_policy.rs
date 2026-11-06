// SPDX-License-Identifier: MIT OR Apache-2.0

//! The crate's exception to "`unsafe` lives only in scarletbook-ffi" (CONTRIBUTING) stays
//! one module: `src/dispatch.rs`, which only calls `#[target_feature]` builds of the safe
//! functions in `kernel`: the filter kernels and the loops around them. The crate denies
//! `unsafe_code`, no other Rust file of the crate has `unsafe` or allows it, and the module
//! has no intrinsics or raw pointers. Checked on the source, by the core crate's scanner.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../../scarletbook/tests/common/unsafe_scan.rs"]
mod scan;

#[test]
#[cfg_attr(miri, ignore = "reads the source files")]
fn unsafe_only_in_dispatch() {
    scan::check_crate(
        env!("CARGO_MANIFEST_DIR"),
        &["fir1", "dec2", "interp2_odd", "pick", "quantise", "interleave"],
    );
}
