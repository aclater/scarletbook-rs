// SPDX-License-Identifier: MIT OR Apache-2.0

//! No panic crosses the C boundary: every function `scarletbook.h` declares either runs
//! its body under `guard` (which catches panics), directly or through the one function it
//! forwards to, or is on a short list of functions that cannot panic. Checked on the
//! source, so that a new function without `guard` fails here rather than in the field.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};

use scarletbook_ffi::*;

const LIB: &str = include_str!("../src/lib.rs");
const HEADER: &str = include_str!("../include/scarletbook.h");

/// Exported functions without `guard`, and why they cannot panic. Adding one here needs
/// the same argument: no allocation, no indexing, no arithmetic that can overflow, no
/// `RefCell` borrow that can fail by panicking, no call into the decoder.
const PANIC_FREE: &[(&str, &str)] = &[
    ("sb_abi_version", "returns a constant"),
    ("sb_version", "returns a static string"),
    ("sb_build_info", "returns a static string"),
    (
        "sb_last_error_message",
        "reads the thread-local message with try_with and a shared borrow it never holds mutably across a call",
    ),
    ("sb_status_name", "matches an integer to a static string"),
    ("sb_track_audio_channels", "try_borrow of the handle; 0 when it fails"),
    ("sb_raw_frames_channels", "try_borrow of the handle; 0 when it fails"),
    ("sb_pool_threads", "reads a field"),
];

/// The functions `scarletbook.h` declares.
fn declared() -> BTreeSet<String> {
    // Without comments, which mention functions with arguments in examples.
    let mut code = String::new();
    let mut rest = HEADER;
    while let Some(i) = rest.find("/*") {
        code.push_str(&rest[..i]);
        rest = &rest[i + rest[i..].find("*/").unwrap() + 2..];
    }
    code.push_str(rest);
    let code: String =
        code.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
    // Each `sb_` identifier followed by `(`.
    code.match_indices("sb_")
        .filter(|&(i, _)| !code[..i].ends_with(|c: char| c.is_alphanumeric() || c == '_'))
        .filter_map(|(i, _)| {
            let name: String = code[i..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            code[i + name.len()..].starts_with('(').then_some(name)
        })
        .collect()
}

/// Every function of `src/lib.rs` (at the top level or in an `impl`): its name, whether
/// it is exported (`#[unsafe(no_mangle)] ... extern "C"`), and its body's lines without
/// comments.
fn functions(lib: &str) -> BTreeMap<String, (bool, Vec<String>)> {
    let lines: Vec<&str> = lib.lines().collect();
    let mut out = BTreeMap::new();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        let Some(at) = t.find("fn ") else { continue };
        let head = &t[..at];
        if !head
            .split_whitespace()
            .all(|w| ["pub", "pub(crate)", "unsafe", "extern", "\"C\"", "const"].contains(&w))
        {
            continue;
        }
        let name: String = t[at + 3..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        let indent = line.len() - t.len();
        let exported = head.contains("extern \"C\"") && lines[i - 1].trim() == "#[unsafe(no_mangle)]";
        // The body ends at the first closing brace at the function's own indentation.
        let close = format!("{}}}", " ".repeat(indent));
        let end = (i + 1..lines.len()).find(|&j| lines[j] == close).unwrap();
        let start = (i..end).find(|&j| lines[j].ends_with('{')).unwrap();
        let body = lines[start + 1..end]
            .iter()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty() && !l.starts_with("//"))
            .map(str::to_owned)
            .collect();
        out.insert(name, (exported, body));
    }
    out
}

/// The exported functions of `lib` (the source of `src/lib.rs`) that are not guarded: a
/// function is guarded when its body starts with `guard(` (after at most clearing its
/// output pointer, which cannot panic), or consists of one call to a guarded function.
fn unguarded(lib: &str) -> BTreeSet<String> {
    let fns = functions(lib);
    let mut guarded = BTreeSet::new();
    loop {
        let before = guarded.len();
        for (name, (_, body)) in &fns {
            let starts_with_guard = body
                .iter()
                .find(|l| !matches!(l.as_str(), "unsafe { clear_out(out) };" | "unsafe { clear_out(out) }"))
                .is_some_and(|l| l.starts_with("guard(") || l.starts_with("let _ = guard("));
            let forwards = match body.as_slice() {
                [one] => {
                    let call = one.trim_start_matches("unsafe {").trim_start();
                    guarded.iter().any(|g: &String| call.starts_with(&format!("{g}(")))
                }
                _ => false,
            };
            if starts_with_guard || forwards {
                guarded.insert(name.clone());
            }
        }
        if guarded.len() == before {
            break;
        }
    }
    fns.into_iter().filter(|(n, (exported, _))| *exported && !guarded.contains(n)).map(|(n, _)| n).collect()
}

#[test]
#[cfg_attr(miri, ignore = "scans the source; no unsafe code")]
fn every_exported_function_is_guarded() {
    let exported: BTreeSet<String> =
        functions(LIB).into_iter().filter(|(_, (e, _))| *e).map(|(n, _)| n).collect();
    assert_eq!(exported, declared(), "the exported functions are the declared ones");
    let allowed: BTreeSet<String> = PANIC_FREE.iter().map(|&(n, _)| n.to_owned()).collect();
    assert_eq!(unguarded(LIB), allowed, "exported functions without guard, against the PANIC_FREE list");
}

/// The check finds a function whose `guard` is removed, directly or behind a forwarder.
#[test]
#[cfg_attr(miri, ignore = "scans the source; no unsafe code")]
fn the_check_finds_a_missing_guard() {
    let without = |name: &str| {
        let at = LIB.find(&format!("fn {name}(")).unwrap();
        let g = at + LIB[at..].find("guard(").unwrap();
        let lib = format!("{}run({}", &LIB[..g], &LIB[g + "guard(".len()..]);
        unguarded(&lib)
    };
    let expect = |extra: &[&str]| -> BTreeSet<String> {
        PANIC_FREE.iter().map(|&(n, _)| n).chain(extra.iter().copied()).map(str::to_owned).collect()
    };
    assert_eq!(without("sb_track_check"), expect(&["sb_track_check"]));
    // `raw_read` is the guarded body both raw frames readers forward to.
    assert_eq!(without("raw_read"), expect(&["sb_raw_frames_read", "sb_raw_frames_read_dst_equivalent"]));
}

/// A call made while a thread exits, from a thread-local destructor that runs after the
/// library's own (the error message's), returns its status instead of aborting: the
/// message is then empty and is not recorded.
#[test]
fn calls_while_a_thread_exits_do_not_abort() {
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
    static STATUS: AtomicI32 = AtomicI32::new(i32::MIN);
    static EMPTY: AtomicBool = AtomicBool::new(false);
    struct Late;
    impl Drop for Late {
        fn drop(&mut self) {
            // SAFETY: NULL handles and output pointers are accepted.
            let status = unsafe { sb_disc_info(std::ptr::null(), std::ptr::null_mut()) };
            STATUS.store(status as i32, Ordering::SeqCst);
            // SAFETY: sb_last_error_message returns a valid NUL-terminated string.
            let message = unsafe { std::ffi::CStr::from_ptr(sb_last_error_message()) };
            EMPTY.store(message.is_empty(), Ordering::SeqCst);
        }
    }
    thread_local! {
        static LATE: Late = const { Late };
    }
    std::thread::spawn(|| {
        // Destructors run in the reverse order of first use (on the platforms that order
        // them at all): `Late`'s after the error message's.
        LATE.with(|_| {});
        // SAFETY: as above.
        let status = unsafe { sb_disc_info(std::ptr::null(), std::ptr::null_mut()) };
        assert_eq!(status, SbStatus::InvalidArgument);
    })
    .join()
    .unwrap();
    assert_eq!(STATUS.load(Ordering::SeqCst), SbStatus::InvalidArgument as i32);
    assert!(EMPTY.load(Ordering::SeqCst) || cfg!(not(target_os = "linux")));
}
