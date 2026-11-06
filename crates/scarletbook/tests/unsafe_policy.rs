// SPDX-License-Identifier: MIT OR Apache-2.0

//! The core crate's exception to "`unsafe` lives only in scarletbook-ffi" (CONTRIBUTING) stays
//! one module: `src/dispatch.rs`, which only calls one `#[target_feature]` wrapper around safe
//! code: the DST decoder and the output path. The crate denies `unsafe_code`, no other Rust
//! file of the crate has `unsafe` or allows it, and the module has no intrinsics or raw
//! pointers. Checked on the source (`common/unsafe_scan.rs`). This test also checks the rest
//! of the workspace, whose `unsafe_code` lint a local `allow` could otherwise switch off.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/unsafe_scan.rs"]
mod scan;

use std::path::Path;

#[test]
#[cfg_attr(miri, ignore = "reads the source files")]
fn unsafe_only_in_dispatch() {
    scan::check_crate(env!("CARGO_MANIFEST_DIR"), &["run"]);
}

#[test]
#[cfg_attr(miri, ignore = "reads the source files")]
fn unsafe_only_in_ffi_and_dispatch_modules() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let ffi = crates.join("scarletbook-ffi");
    let core = crates.join("scarletbook/src/dispatch.rs");
    let pcm = crates.join("scarletbook-pcm/src/dispatch.rs");
    let mut problems = scan::tree(crates, &[&ffi, &core, &pcm]);
    problems.extend(scan::tree(&crates.parent().unwrap().join("fuzz"), &[]));
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// A module that keeps the policy, shaped like the real ones.
const GOOD: &str = r#"
#![allow(unsafe_code)]
pub(crate) struct Isa(Level);
fn detect() -> Isa {
    if std::arch::is_x86_feature_detected!("avx2") {
        return Isa(Level::Fast);
    }
    Isa(Level::Generic)
}
fn call(isa: Isa, a: u8) {
    match isa.0 {
        // SAFETY: detected.
        #[cfg(target_arch = "x86_64")]
        Level::Fast => unsafe { avx2::run(a) },
        Level::Generic => {}
    }
}
mod avx2 {
    #[target_feature(enable = "avx2")]
    pub(super) unsafe fn run(a: u8) {}
}
"#;

#[test]
#[cfg_attr(miri, ignore = "text processing on fixed strings, with no unsafe code to check")]
fn dispatch_rules_catch_each_breach() {
    assert_eq!(scan::dispatch(GOOD, &["run"]), Vec::<String>::new());
    let call = "unsafe { avx2::run(a) }";
    let body = "unsafe fn run(a: u8) {}";
    let edits = [
        (call, "unsafe { avx2::run(evil(a)) }"),
        (call, "unsafe { avx2::run(a); evil() }"),
        (call, "unsafe\n{ *p }"),
        (call, "unsafe { avx2::other(a) }"),
        ("// SAFETY: detected.", "// Detected."),
        (body, "unsafe fn other(a: u8) {}"),
        ("#[target_feature(enable = \"avx2\")]", "#[target_feature(enable = \"avx2,fma\")]"),
        ("#[target_feature(enable = \"avx2\")]", ""),
        (body, "unsafe fn run(a: u8) { core::arch::x86_64::_mm_pause() }"),
        (body, "unsafe fn run(a: u8) { std::arch::asm!(\"nop\") }"),
        (body, "unsafe fn run(a: u8) { let _ = &a as * const u8; }"),
        (body, "unsafe fn run(a: u8) { let _ = &raw const a; }"),
        (body, "unsafe fn run(a: u8) { [a].get_unchecked(0); }"),
        ("struct Isa(Level);", "struct Isa(pub Level);"),
        ("#![allow(unsafe_code)]", ""),
        ("#![allow(unsafe_code)]", "#![allow(unsafe_code)]\n#![warn(unsafe_code)]"),
        ("mod avx2", "unsafe impl Send for Isa {}\nmod avx2"),
        ("mod avx2", "unsafe trait T {}\nmod avx2"),
        ("mod avx2", "unsafe extern \"C\" {}\nmod avx2"),
        ("mod avx2", "#[unsafe(no_mangle)] fn f() {}\nmod avx2"),
        ("mod avx2", "static mut X: u8 = 0;\nmod avx2"),
        ("mod avx2", "fn g() -> Isa { Isa(Level::Fast) }\nmod avx2"),
    ];
    for (from, to) in edits {
        assert!(GOOD.contains(from), "{from}");
        let text = GOOD.replacen(from, to, 1);
        assert!(!scan::dispatch(&text, &["run"]).is_empty(), "not caught: {to}");
    }
}

#[test]
#[cfg_attr(miri, ignore = "text processing on fixed strings, with no unsafe code to check")]
fn outside_dispatch_sees_code_not_text() {
    for bad in [
        "unsafe  fn f() {}",
        "fn f() { unsafe\n{} }",
        "let a = \"//\"; unsafe {}",
        "/* /* a */ b */ unsafe {}",
        "let c = '\"'; unsafe {} let d = \"\";",
        "let r = r#\"\"\"#; unsafe {}",
        "fn f<'a>(x: &'a u8) { unsafe {} }",
        "#![allow(unsafe_code)]",
        "#![cfg_attr(test, expect( unsafe_code ))]",
    ] {
        assert!(!scan::outside_dispatch(bad).is_empty(), "not caught: {bad}");
    }
    for good in [
        "// unsafe",
        "/* /* unsafe */ unsafe */",
        "let a = \"unsafe\";",
        "let a = \"\\\"unsafe\";",
        "let a = r#\"\"unsafe\"#;",
        "let a = br\"unsafe\";",
        "let a = '\\''; // unsafe",
        "#![deny(unsafe_code)]",
        "#![forbid(unsafe_code)]",
        "let unsafe_ish = not_unsafe;",
    ] {
        assert_eq!(scan::outside_dispatch(good), Vec::<String>::new(), "{good}");
    }
}
