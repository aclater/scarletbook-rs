// SPDX-License-Identifier: MIT OR Apache-2.0

//! The correctness gate of sacd-spec `benchmarks/README.md`, on the conformance corpus:
//! `bench ... --sink hash` must print the manifest's hashes for every case, at one thread
//! and at all threads; rejected images must fail with their error class.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus")
}

fn cases() -> Vec<Value> {
    let m: Value =
        serde_json::from_str(&std::fs::read_to_string(corpus().join("manifest.json")).unwrap()).unwrap();
    m["cases"].as_array().unwrap().clone()
}

fn bench(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_bench")).args(args).output().unwrap()
}

fn image(case: &Value) -> String {
    corpus()
        .join("images")
        .join(format!("{}.iso", case["id"].as_str().unwrap()))
        .to_string_lossy()
        .into_owned()
}

/// The lines `--sink hash` must print for one expected area.
fn expected_lines(area: &Value) -> Vec<String> {
    area["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            let n = t["number"].as_u64().unwrap();
            if t["outcome"] == "ok" {
                let hashes: Vec<&str> =
                    t["sha256"].as_array().unwrap().iter().map(|h| h.as_str().unwrap()).collect();
                format!("{n}\t{}", hashes.join("\t"))
            } else {
                format!("{n}\t{}", t["error"].as_str().unwrap())
            }
        })
        .collect()
}

#[test]
fn hash_sink_matches_the_manifest() {
    let mut checked = 0;
    for case in cases() {
        let img = image(&case);
        let expected = &case["expected"];
        if expected["outcome"] == "reject" {
            let out = bench(&["decode", &img, "--area", "2ch", "--threads", "1", "--sink", "hash"]);
            assert!(!out.status.success(), "{}: rejected image exited 0", case["id"]);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert_eq!(stderr.lines().next(), expected["error"].as_str(), "{}: {stderr}", case["id"]);
            continue;
        }
        for area in expected["audio"].as_array().unwrap() {
            let flag = if area["area"] == "2ch" { "2ch" } else { "mc" };
            for threads in ["1", "0"] {
                let out = bench(&["decode", &img, "--area", flag, "--threads", threads, "--sink", "hash"]);
                assert!(
                    out.status.success(),
                    "{} {flag}: {}",
                    case["id"],
                    String::from_utf8_lossy(&out.stderr)
                );
                let got: Vec<String> =
                    String::from_utf8(out.stdout).unwrap().lines().map(str::to_owned).collect();
                assert_eq!(got, expected_lines(area), "{} {flag} --threads {threads}", case["id"]);
                checked += 1;
            }
        }
        // An area the disc does not have.
        let slots: Vec<&str> =
            expected["audio"].as_array().unwrap().iter().filter_map(|a| a["area"].as_str()).collect();
        if !slots.contains(&"multichannel") {
            let out = bench(&["decode", &img, "--area", "mc", "--threads", "1", "--sink", "null"]);
            assert!(!out.status.success());
            assert_eq!(String::from_utf8_lossy(&out.stderr).lines().next(), Some("no-such-area"));
        }
    }
    assert!(checked > 200, "only {checked} area runs checked");
}

#[test]
fn raw_and_null_sinks() {
    let case = cases().into_iter().find(|c| c["id"] == "dst-features-5ch").unwrap();
    let dir = std::env::temp_dir().join(format!("scarletbook-bench-raw-{}", std::process::id()));
    let area = &case["expected"]["audio"][0];
    let flag = if area["area"] == "2ch" { "2ch" } else { "mc" };
    let out = bench(&[
        "decode",
        &image(&case),
        "--area",
        flag,
        "--threads",
        "0",
        "--sink",
        "raw",
        dir.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.stdout.is_empty());
    for t in area["tracks"].as_array().unwrap() {
        let n = t["number"].as_u64().unwrap();
        for (c, h) in t["sha256"].as_array().unwrap().iter().enumerate() {
            let bytes = std::fs::read(dir.join(format!("{n:02}-{c}.dsd"))).unwrap();
            let got = sha256_hex(&bytes);
            assert_eq!(got, h.as_str().unwrap(), "track {n} channel {c}");
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
    let out = bench(&["decode", &image(&case), "--area", flag, "--threads", "1", "--sink", "null"]);
    assert!(out.status.success() && out.stdout.is_empty());
    let bad = bench(&["decode", "x.iso", "--area", "5ch", "--threads", "1", "--sink", "null"]);
    assert!(!bad.status.success());
    assert_eq!(String::from_utf8_lossy(&bad.stderr).lines().next(), Some("usage-error"));
    let missing = bench(&["decode", "/nonexistent.iso", "--area", "2ch", "--threads", "1", "--sink", "null"]);
    assert_eq!(String::from_utf8_lossy(&missing.stderr).lines().next(), Some("io-error"));
    let v = bench(&["--version"]);
    assert!(v.status.success() && String::from_utf8_lossy(&v.stdout).starts_with("scarletbook "));
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}
