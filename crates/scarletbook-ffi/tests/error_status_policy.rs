// SPDX-License-Identifier: MIT OR Apache-2.0

//! Every core `Error` has its own arm in `From<Error> for Failure`, so that a new variant is
//! given its C status on purpose (#308): `Error` is `#[non_exhaustive]`, so the compiler can't
//! ask for one, and the fallback through `Error::class()` only gets the class right. The
//! fallback's own match names every `ErrorClass` in the same way. Neither maps to
//! `SB_STATUS_PANIC`, which means a bug in this library. Checked on the source.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::Path;

/// From `header` to the end of its item (a `}` in the first column), without comment lines.
fn item(source: &str, header: &str) -> String {
    let start = source.find(header).unwrap_or_else(|| panic!("no `{header}`"));
    let rest = &source[start..];
    let end = rest.find("\n}").unwrap_or_else(|| panic!("`{header}` has no end"));
    rest[..end].lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
}

/// The variants of a rustfmt-formatted enum: the capitalised names at its first indentation.
fn variants(body: &str) -> BTreeSet<String> {
    body.lines()
        .filter_map(|l| l.strip_prefix("    "))
        .filter(|l| l.starts_with(|c: char| c.is_ascii_uppercase()))
        .map(|l| l.split(|c: char| !c.is_alphanumeric() && c != '_').next().unwrap().to_owned())
        .collect()
}

/// The variants named by a match's arms: a line that starts with `prefix`, or with `| prefix`,
/// and every alternative left of its `=>`. Code inside an arm's block does not count.
fn arms(body: &str, prefix: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for line in body.lines() {
        let line = line.trim_start();
        let line = line.strip_prefix("| ").unwrap_or(line);
        if !line.starts_with(prefix) {
            continue;
        }
        for alternative in line.split("=>").next().unwrap().split(" | ") {
            let name = alternative.trim().strip_prefix(prefix).unwrap_or(alternative);
            names.insert(name.split(|c: char| !c.is_alphanumeric() && c != '_').next().unwrap().to_owned());
        }
    }
    names
}

/// One line for each way the variants of `declared` and the arms named `prefix` differ.
fn compare(declared: &str, mapping: &str, prefix: &str) -> Vec<String> {
    let (variants, arms) = (variants(declared), arms(mapping, prefix));
    let mut problems = Vec::new();
    if variants.is_empty() {
        problems.push(format!("no {prefix} variants found"));
    }
    for name in variants.difference(&arms) {
        problems.push(format!("{prefix}{name} has no arm of its own"));
    }
    for name in arms.difference(&variants) {
        problems.push(format!("the arm {prefix}{name} is not a variant"));
    }
    problems
}

fn check(error_rs: &str, ffi_lib_rs: &str) -> Vec<String> {
    let mapping = item(ffi_lib_rs, "impl From<Error> for Failure {");
    let fallback = item(ffi_lib_rs, "fn class_status(class: ErrorClass) -> SbStatus {");
    let mut problems = compare(&item(error_rs, "pub enum Error {"), &mapping, "Error::");
    problems.extend(compare(&item(error_rs, "pub enum ErrorClass {"), &fallback, "ErrorClass::"));
    if !mapping.contains("_ => class_status(e.class()),") {
        problems.push("the conversion does not fall back through the error's class".to_owned());
    }
    if mapping.contains("SbStatus::Panic") || fallback.contains("SbStatus::Panic") {
        problems.push("an error maps to SbStatus::Panic".to_owned());
    }
    problems
}

#[test]
#[cfg_attr(miri, ignore = "reads the source files")]
fn every_error_has_its_own_arm() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let error_rs = std::fs::read_to_string(crates.join("scarletbook/src/error.rs")).unwrap();
    let ffi_lib_rs = std::fs::read_to_string(crates.join("scarletbook-ffi/src/lib.rs")).unwrap();
    let problems = check(&error_rs, &ffi_lib_rs);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// Sources that keep the rule, shaped like the real ones.
const ERROR_RS: &str = r#"
/// An error.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Not one (`Mentioned`).
    #[error("not one")]
    NotOne,
    /// With fields.
    #[error(
        "Capitalised {major}"
    )]
    Fields {
        /// A field.
        major: u8,
    },
    /// A tuple.
    #[error("{0}")]
    Tuple(#[from] Inner),
}

pub enum ErrorClass {
    /// Rejected.
    Rejected,
    /// Usage.
    Usage,
}
"#;

const FFI_LIB_RS: &str = r#"
fn class_status(class: ErrorClass) -> SbStatus {
    match class {
        ErrorClass::Rejected => SbStatus::NotOne,
        // A later one.
        ErrorClass::Usage => SbStatus::Arg,
        _ => SbStatus::Arg,
    }
}

impl From<Error> for Failure {
    fn from(e: Error) -> Self {
        let status = match &e {
            Error::NotOne => SbStatus::NotOne,
            Error::Fields { .. } => {
                SbStatus::Fields
            }
            Error::Tuple(Inner::First) => SbStatus::First,
            Error::Tuple(_) => SbStatus::Arg,
            _ => class_status(e.class()),
        };
        Failure(status, e.to_string())
    }
}
"#;

#[test]
#[cfg_attr(miri, ignore = "text processing on fixed strings, with no unsafe code to check")]
fn the_check_catches_each_breach() {
    assert_eq!(check(ERROR_RS, FFI_LIB_RS), Vec::<String>::new());

    let new_variant = "    NotOne,\n    /// New.\n    #[error(\"new\")]\n    New { lsn: u32 },";
    let new_class = "    Rejected,\n    /// Later.\n    Later,";
    for (from, to) in [("    NotOne,", new_variant), ("    Rejected,", new_class)] {
        assert!(ERROR_RS.contains(from), "{from}");
        let text = ERROR_RS.replacen(from, to, 1);
        assert!(!check(&text, FFI_LIB_RS).is_empty(), "not caught: {to}");
    }

    let arm = "            Error::NotOne => SbStatus::NotOne,";
    let class_arm = "        ErrorClass::Usage => SbStatus::Arg,";
    let fallback = "            _ => class_status(e.class()),";
    for (from, to) in [
        // An arm gone, in several disguises.
        (arm, ""),
        (arm, "            // Error::NotOne => SbStatus::NotOne,"),
        (arm, "            _ if false => { let _ = Error::NotOne; SbStatus::Arg }"),
        // An arm for what isn't a variant.
        (arm, "            Error::NotOne => SbStatus::NotOne,\n            Error::Gone => SbStatus::Arg,"),
        (class_arm, ""),
        // Back to the catch-all, or to the status it stood for.
        (fallback, "            _ => SbStatus::Panic,"),
        (fallback, "            _ => SbStatus::Arg,"),
        (arm, "            Error::NotOne => SbStatus::Panic,"),
        (class_arm, "        ErrorClass::Usage => SbStatus::Panic,"),
    ] {
        assert!(FFI_LIB_RS.contains(from), "{from}");
        let text = FFI_LIB_RS.replacen(from, to, 1);
        assert!(!check(ERROR_RS, &text).is_empty(), "not caught: {to}");
    }

    // A new variant with its own arm, in any of rustfmt's shapes, is what the rule asks for.
    let text = ERROR_RS.replacen("    NotOne,", new_variant, 1);
    for arm_shape in [
        "            Error::New { .. } => SbStatus::New,",
        "            Error::New { lsn: 0 }\n            | Error::New { .. } => SbStatus::New,",
    ] {
        let ffi = FFI_LIB_RS.replacen(arm, &format!("{arm}\n{arm_shape}"), 1);
        assert_eq!(check(&text, &ffi), Vec::<String>::new(), "{arm_shape}");
    }
}
