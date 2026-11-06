// SPDX-License-Identifier: MIT OR Apache-2.0

//! The checks behind each crate's `tests/unsafe_policy.rs` (CONTRIBUTING, "Code"), on the
//! source text. Comments and literals are blanked first and the rest is compared token by
//! token, so spacing, line breaks and text inside strings make no difference.
//!
//! The dispatch module is held to what it may contain, not to a list of what it may not:
//! `unsafe` there is either a call of one wrapper or the wrapper's own declaration. The
//! list of banned names only covers what a wrapper's safe body could reach: intrinsics
//! are safe to call inside `#[target_feature]`, and raw pointers can be made in safe code.

use std::path::{Path, PathBuf};

/// `text` with comments and string, raw string and character literals replaced by spaces.
/// Line numbers stay the same.
pub fn code(text: &str) -> String {
    let c: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < c.len() {
        match skipped_to(&c, i) {
            Some(end) => {
                out.extend(c[i..end].iter().map(|&x| if x == '\n' { '\n' } else { ' ' }));
                i = end;
            }
            None => {
                out.push(c[i]);
                i += 1;
            }
        }
    }
    out
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Where the comment or literal starting at `i` ends, if one starts there.
fn skipped_to(c: &[char], i: usize) -> Option<usize> {
    let at = |n: usize| c.get(n).copied().unwrap_or('\0');
    match c[i] {
        '/' if at(i + 1) == '/' => Some((i..c.len()).find(|&n| c[n] == '\n').unwrap_or(c.len())),
        '/' if at(i + 1) == '*' => {
            let (mut n, mut depth) = (i + 2, 1);
            while depth > 0 && n < c.len() {
                match (c[n], at(n + 1)) {
                    ('/', '*') => (depth, n) = (depth + 1, n + 2),
                    ('*', '/') => (depth, n) = (depth - 1, n + 2),
                    _ => n += 1,
                }
            }
            Some(n)
        }
        '"' => {
            let mut n = i + 1;
            while n < c.len() && c[n] != '"' {
                n += if c[n] == '\\' { 2 } else { 1 };
            }
            Some((n + 1).min(c.len()))
        }
        // A character literal, or else a lifetime, which is left alone.
        '\'' if at(i + 1) == '\\' => {
            let mut n = i + 3;
            while n < c.len() && c[n] != '\'' {
                n += 1;
            }
            Some((n + 1).min(c.len()))
        }
        '\'' if at(i + 2) == '\'' => Some(i + 3),
        // A raw string: `r`, `br` or `cr` at the start of a word, then `#`s and a quote.
        'r' => {
            let before = i.checked_sub(1).map(|n| c[n]);
            let starts = match before {
                Some('b' | 'c') => i < 2 || !is_word(c[i - 2]),
                Some(b) => !is_word(b),
                None => true,
            };
            let hashes = (i + 1..c.len()).take_while(|&n| c[n] == '#').count();
            if !starts || at(i + 1 + hashes) != '"' {
                return None;
            }
            let mut n = i + 2 + hashes;
            while n < c.len() && !(c[n] == '"' && (1..=hashes).all(|h| at(n + h) == '#')) {
                n += 1;
            }
            Some((n + 1 + hashes).min(c.len()))
        }
        _ => None,
    }
}

/// The words and single punctuation characters of `code`, with their 0-based lines.
fn tokens(code: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    for (line, l) in code.lines().enumerate() {
        let mut rest = l.trim_start();
        while let Some(first) = rest.chars().next() {
            let len = if is_word(first) {
                rest.find(|c| !is_word(c)).unwrap_or(rest.len())
            } else {
                first.len_utf8()
            };
            out.push((line, &rest[..len]));
            rest = rest[len..].trim_start();
        }
    }
    out
}

/// Every `.rs` file under `dir`, build output aside.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if path.file_name().is_none_or(|n| n != "target") {
                    dirs.push(path);
                }
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// What a file outside the dispatch modules and `scarletbook-ffi` does wrong: `unsafe` in
/// any form, or lint attributes on `unsafe_code` other than `deny` and `forbid`.
pub fn outside_dispatch(text: &str) -> Vec<String> {
    let code = code(text);
    let t = tokens(&code);
    let mut problems = Vec::new();
    for (i, &(line, word)) in t.iter().enumerate() {
        if word == "unsafe" {
            problems.push(format!("line {}: has unsafe", line + 1));
        }
        let denied = i >= 2 && t[i - 1].1 == "(" && matches!(t[i - 2].1, "deny" | "forbid");
        if word == "unsafe_code" && !denied {
            problems.push(format!("line {}: lints unsafe_code other than deny or forbid", line + 1));
        }
    }
    problems
}

/// Names a dispatch module may not use, anywhere in the file.
const BANNED: &[&str] = &[
    "asm",
    "global_asm",
    "naked_asm",
    "transmute",
    "transmute_copy",
    "MaybeUninit",
    "zeroed",
    "from_raw_parts",
    "from_raw_parts_mut",
    "from_raw",
    "into_raw",
    "get_unchecked",
    "get_unchecked_mut",
    "unreachable_unchecked",
    "assume_init",
    "as_ptr",
    "as_mut_ptr",
    "ptr",
    "addr_of",
    "addr_of_mut",
    "NonNull",
    "union",
    "extern",
];

/// What a dispatch module does wrong (see the module doc); `wrappers` are the names its
/// `#[target_feature]` functions may have.
pub fn dispatch(text: &str, wrappers: &[&str]) -> Vec<String> {
    let code = code(text);
    let t = tokens(&code);
    let raw: Vec<&str> = text.lines().map(str::trim).collect();
    let word = |i: usize| t.get(i).map_or("", |x| x.1);
    let mut problems = Vec::new();
    let mut declared = Vec::new();
    for (i, &(line, w)) in t.iter().enumerate() {
        let mut problem = |what: String| problems.push(format!("line {}: {what}", line + 1));
        match w {
            "unsafe" if word(i + 1) == "fn" => {
                declared.push(word(i + 2));
                if !wrappers.contains(&word(i + 2)) {
                    problem(format!("wrapper {} is not on the test's list", word(i + 2)));
                }
                if !context_above(&raw, line).any(|l| l.starts_with("#[target_feature(")) {
                    problem("unsafe fn without #[target_feature]".into());
                }
            }
            "unsafe" if word(i + 1) == "{" => {
                if !is_one_wrapper_call(&t[i + 2..], wrappers) {
                    problem("the unsafe block is not one call of a wrapper with plain arguments".into());
                }
                if !context_above(&raw, line).any(|l| l.starts_with("//") && l.contains("SAFETY:")) {
                    problem("no SAFETY comment of its own".into());
                }
            }
            "unsafe" => problem(format!("unsafe {}", word(i + 1))),
            "unsafe_code" if !(word(i.wrapping_sub(2)) == "allow" && word(i + 2) == "]") => {
                problem("lints unsafe_code other than the module's allow".into())
            }
            "arch" if !(word(i.wrapping_sub(3)) == "std" && word(i + 3) == "is_x86_feature_detected") => {
                problem("std::arch beyond feature detection".into())
            }
            "const" | "mut" if i > 0 && word(i - 1) == "*" => problem(format!("raw pointer *{w}")),
            "const" | "mut" if i > 1 && word(i - 1) == "raw" && word(i - 2) == "&" => {
                problem(format!("raw pointer &raw {w}"))
            }
            "mut" if i > 0 && word(i - 1) == "static" => problem("static mut".into()),
            w if BANNED.contains(&w) || w.starts_with("_mm") || w.starts_with("__m") => {
                problem(format!("uses {w}"))
            }
            _ => {}
        }
    }
    if t.iter().filter(|x| x.1 == "unsafe_code").count() != 1 {
        problems.push("must have #![allow(unsafe_code)] once".into());
    }
    if t.iter().filter(|x| x.1 == "target_feature").count() != declared.len() {
        problems.push("a #[target_feature] function is not one `unsafe fn` wrapper".into());
    }
    // The fast `Isa` comes from one place, and only this module can build one.
    let fast = t.windows(6).filter(|w| {
        w[0].1 == "Isa" && w[1].1 == "(" && w[2].1 == "Level" && w[3].1 == ":" && w[5].1 != "Generic"
    });
    if fast.count() != 1 || !code.contains("struct Isa(Level);") {
        problems.push("the fast Isa must have a private field and one constructor, in detect".into());
    }
    // Every feature a wrapper enables is one that detection checks for.
    for l in raw.iter().filter(|l| !l.starts_with("//")) {
        let Some(features) = l.strip_prefix("#[target_feature(enable = \"") else { continue };
        for f in features.split('"').next().unwrap_or("").split(',') {
            if !raw.iter().any(|l| l.contains(&format!("is_x86_feature_detected!(\"{f}\")"))) {
                problems.push(format!("feature {f} is enabled but not detected"));
            }
        }
    }
    problems
}

/// The comment and attribute lines right above line `line`, nearest first.
fn context_above<'a>(raw: &'a [&'a str], line: usize) -> impl Iterator<Item = &'a str> {
    raw[..line].iter().rev().copied().take_while(|l| l.starts_with("//") || l.starts_with("#["))
}

/// Whether `t`, after `unsafe {`, is `avx2::wrapper(a, b, c) }`.
fn is_one_wrapper_call(t: &[(usize, &str)], wrappers: &[&str]) -> bool {
    let word = |i: usize| t.get(i).map_or("", |x| x.1);
    if word(0) != "avx2" || word(1) != ":" || word(2) != ":" || !wrappers.contains(&word(3)) || word(4) != "("
    {
        return false;
    }
    let mut i = 5;
    while word(i) != ")" {
        if !word(i).starts_with(is_word) || !matches!(word(i + 1), "," | ")") {
            return false;
        }
        i += if word(i + 1) == "," { 2 } else { 1 };
    }
    word(i + 1) == "}"
}

/// Checks every `.rs` file under `dir` except those in `skip` with [`outside_dispatch`], and
/// returns the problems, with the file named.
pub fn tree(dir: &Path, skip: &[&Path]) -> Vec<String> {
    let mut problems = Vec::new();
    for path in rust_files(dir) {
        if skip.iter().any(|s| path.starts_with(s)) {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        problems.extend(outside_dispatch(&text).iter().map(|p| format!("{}: {p}", path.display())));
    }
    problems
}

/// The policy of one crate, rooted at `manifest_dir`, with `wrappers` as in [`dispatch`].
pub fn check_crate(manifest_dir: &str, wrappers: &[&str]) {
    let root = Path::new(manifest_dir);
    let module = root.join("src/dispatch.rs");
    let mut problems = tree(root, &[&module]);
    let lib = code(&std::fs::read_to_string(root.join("src/lib.rs")).unwrap());
    if !lib.contains("#![deny(unsafe_code)]") {
        problems.push("lib.rs must deny unsafe_code".into());
    }
    let module = std::fs::read_to_string(&module).unwrap();
    problems.extend(dispatch(&module, wrappers).iter().map(|p| format!("dispatch.rs: {p}")));
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
