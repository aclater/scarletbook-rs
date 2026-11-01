// SPDX-License-Identifier: MIT OR Apache-2.0

//! The crate's exception to "`unsafe` lives only in scarletbook-ffi" (CONTRIBUTING) stays
//! one module: `src/dispatch.rs`, which only calls `#[target_feature]` builds of the safe
//! kernels. The crate denies `unsafe_code`, no other source file has `unsafe` or allows
//! it, and the module has no intrinsics or raw pointers. Checked on the source.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

/// Every `.rs` file under `src/`, as (path relative to `src/`, contents).
fn sources() -> Vec<(String, String)> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    let mut dirs = vec![src.clone()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let name = path.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
                out.push((name, std::fs::read_to_string(&path).unwrap()));
            }
        }
    }
    out.sort();
    out
}

/// The code of `text`, without `//` comments.
fn code(text: &str) -> String {
    text.lines().map(|l| l.split("//").next().unwrap_or("")).collect::<Vec<_>>().join("\n")
}

#[test]
#[cfg_attr(miri, ignore = "reads the source files")]
fn unsafe_only_in_dispatch() {
    let sources = sources();
    assert!(sources.iter().any(|(n, _)| n == "lib.rs") && sources.iter().any(|(n, _)| n == "dispatch.rs"));
    for (name, text) in &sources {
        let code = code(text);
        match name.as_str() {
            "lib.rs" => {
                assert!(code.contains("#![deny(unsafe_code)]"), "lib.rs must deny unsafe_code");
            }
            "dispatch.rs" => {
                assert!(code.contains("#![allow(unsafe_code)]"));
                for banned in
                    ["std::arch::x86_64", "core::arch", "_mm", "*const", "*mut", "unsafe impl", "transmute"]
                {
                    assert!(!code.contains(banned), "dispatch.rs uses {banned}");
                }
                // Every unsafe block has a SAFETY comment right above it.
                let lines: Vec<&str> = text.lines().collect();
                for (i, l) in lines.iter().enumerate() {
                    if code_line(l).contains("unsafe {") {
                        let above = lines[..i].iter().rev().take_while(|l| !l.trim().is_empty()).take(4);
                        assert!(
                            above.into_iter().any(|l| l.contains("SAFETY:")),
                            "dispatch.rs:{}: no SAFETY comment",
                            i + 1
                        );
                    }
                }
            }
            _ => {}
        }
        if name != "dispatch.rs" {
            assert!(!has_unsafe_keyword(&code), "{name} has unsafe");
            assert!(!code.contains("allow(unsafe_code)"), "{name} allows unsafe_code");
        }
    }
}

/// Whether `code` has the keyword `unsafe` (not `unsafe_code`).
fn has_unsafe_keyword(code: &str) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    code.match_indices("unsafe")
        .any(|(i, _)| !code[..i].ends_with(word) && !code[i + "unsafe".len()..].starts_with(word))
}

fn code_line(line: &str) -> &str {
    line.split("//").next().unwrap_or("")
}
