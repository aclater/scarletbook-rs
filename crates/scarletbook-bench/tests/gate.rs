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
    // Cases without a 2-channel area, and without a multichannel area.
    let mut missing = [0; 2];
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
        // A slot that yields no area, in either direction.
        let slots: Vec<&str> =
            expected["audio"].as_array().unwrap().iter().filter_map(|a| a["area"].as_str()).collect();
        for (slot, flag) in [("2ch", "2ch"), ("multichannel", "mc")] {
            if !slots.contains(&slot) {
                let out = bench(&["decode", &img, "--area", flag, "--threads", "1", "--sink", "null"]);
                assert!(!out.status.success(), "{} --area {flag}", case["id"]);
                assert_eq!(String::from_utf8_lossy(&out.stderr).lines().next(), Some("no-such-area"));
                missing[usize::from(flag == "mc")] += 1;
            }
        }
    }
    assert!(checked > 200, "only {checked} area runs checked");
    assert!(missing[0] > 0 && missing[1] > 0, "missing-area checks: {missing:?}");
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

/// `bench dsf` (issue #202): the file is the same through one stream, through
/// `Layout::read_at` calls of any size and through the parallel writer, and its size is the
/// one its header gives.
#[test]
fn dsf_streams_and_read_at_calls_agree() {
    let dir = std::env::temp_dir().join(format!("scarletbook-bench-dsf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (id, flag) in
        [("dsd-stereo-mch-2048", "2ch"), ("dsd-stereo-mch-2048", "mc"), ("dst-features-5ch", "mc")]
    {
        let case = cases().into_iter().find(|c| c["id"] == id).unwrap();
        let img = image(&case);
        let mut files = Vec::new();
        let mut extras: Vec<&[&str]> =
            vec![&[], &["--via", "read-at", "--chunk", "5000"], &["--chunk", "4093"]];
        // The parallel writer needs `write_at`, which is Unix only.
        if cfg!(unix) {
            extras.push(&["--via", "parallel", "--chunk", "5000"]);
        }
        for (i, extra) in extras.iter().enumerate() {
            let path = dir.join(format!("{id}-{flag}-{i}.dsf"));
            let mut args =
                vec!["dsf", &img, "--area", flag, "--threads", "0", "--out", path.to_str().unwrap()];
            args.extend_from_slice(extra);
            let out = bench(&args);
            assert!(out.status.success(), "{id} {flag} {extra:?}: {}", String::from_utf8_lossy(&out.stderr));
            files.push(std::fs::read(&path).unwrap());
        }
        let f = &files[0];
        assert_eq!(&f[..4], b"DSD ");
        assert_eq!(u64::from_le_bytes(f[12..20].try_into().unwrap()), f.len() as u64, "{id} {flag}");
        assert!(files.iter().all(|g| g == f), "{id} {flag}: the files differ");
    }
    std::fs::remove_dir_all(&dir).unwrap();
    let bad = bench(&["dsf", "x.iso", "--area", "2ch", "--threads", "1"]);
    assert_eq!(String::from_utf8_lossy(&bad.stderr).lines().next(), Some("usage-error"));
}

/// `bench dsf --via tracks` (issue #202): one file per track, the same from any number of
/// workers and at any chunk size, and each the bytes of that track's own `Layout::read_at`.
#[test]
fn dsf_tracks_match_one_at_a_time_writes() {
    use scarletbook::output::{Format, OutputOptions};
    use scarletbook::{Disc, Slot};

    let root = std::env::temp_dir().join(format!("scarletbook-bench-tracks-{}", std::process::id()));
    for (id, flag, slot) in [
        ("dsd-stereo-mch-2048", "2ch", Slot::TwoChannel),
        ("dsd-stereo-mch-2048", "mc", Slot::Multichannel),
        ("dst-features-5ch", "mc", Slot::Multichannel),
    ] {
        let case = cases().into_iter().find(|c| c["id"] == id).unwrap();
        let img = image(&case);
        let file = std::fs::File::open(&img).unwrap();
        let mut disc = Disc::open(std::io::BufReader::new(file)).unwrap();
        let numbers: Vec<u32> =
            disc.toc().area(slot).unwrap().tracks.iter().map(|t| u32::from(t.number)).collect();
        assert!(!numbers.is_empty(), "{id} {flag}");
        for (via, threads, chunk) in [
            ("tracks", "1", "4194304"),
            ("tracks", "3", "5000"),
            ("tracks", "0", "4093"),
            ("tracks-scan", "1", "5000"),
            ("tracks-speculative", "2", "5000"),
            ("tracks-unverified", "3", "5000"),
        ] {
            let dir = root.join(format!("{id}-{flag}-{via}-{threads}"));
            std::fs::create_dir_all(&dir).unwrap();
            let out = bench(&[
                "dsf",
                &img,
                "--area",
                flag,
                "--threads",
                threads,
                "--out",
                dir.to_str().unwrap(),
                "--via",
                via,
                "--chunk",
                chunk,
            ]);
            assert!(out.status.success(), "{id} {flag} {threads}: {}", String::from_utf8_lossy(&out.stderr));
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), numbers.len(), "{id} {flag}");
            for &n in &numbers {
                let (first, end) = disc.track_range_with_pause(slot, n).unwrap();
                let layout =
                    disc.output_layout(slot, first, end, Format::Dsf, &OutputOptions::default()).unwrap();
                let mut want = vec![0; usize::try_from(layout.size()).unwrap()];
                assert_eq!(layout.read_at(&mut disc, 0, &mut want).unwrap(), want.len());
                let got = std::fs::read(dir.join(format!("{n:03}.dsf"))).unwrap();
                assert!(got == want, "{id} {flag} {via} --threads {threads}: track {n} differs");
            }
        }
    }
    std::fs::remove_dir_all(&root).unwrap();
    let bad = bench(&[
        "dsf",
        "/nonexistent.iso",
        "--area",
        "2ch",
        "--threads",
        "2",
        "--out",
        "/dev/null",
        "--via",
        "tracks",
    ]);
    assert_eq!(String::from_utf8_lossy(&bad.stderr).lines().next(), Some("io-error"));
}

/// `bench realtime` reports every figure, on a DST area of the corpus (#255).
#[test]
fn realtime_reports_its_figures() {
    let case = cases().into_iter().find(|c| c["id"] == "dst-features-5ch").unwrap();
    let out = bench(&["realtime", &image(&case), "--area", "mc", "--seeks", "7"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8(out.stdout).unwrap();
    let figure = |name: &str| -> f64 {
        let line = text.lines().find_map(|l| l.strip_prefix(&format!("{name}\t"))).unwrap_or_else(|| {
            panic!("no {name} in {text}");
        });
        line.parse().unwrap()
    };
    assert!(figure("frames") > 0.0);
    assert!(figure("mean_realtime_factor") >= figure("slowest_window_realtime_factor"));
    assert!(figure("max_frame_ms") >= figure("p999_frame_ms"));
    assert!(figure("frames_over_6.7_ms") >= 0.0);
    assert_eq!(figure("seeks"), 7.0);
    assert!(figure("seek_max_ms") >= figure("seek_median_ms"));
}

/// `open` and `first` print their figures, and a failing `--evict` program fails the run.
#[cfg(unix)]
#[test]
fn open_and_first_print_figures() {
    let img = corpus().join("images/dst-features-2ch-6ch.iso").to_string_lossy().into_owned();
    let figures = |args: &[&str]| {
        let out = bench(args);
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    };
    let open = figures(&["open", &img, "--iterations", "3", "--evict", "/bin/true"]);
    assert!(open.starts_with("iterations\t3\nmedian_us\t"), "{open}");
    for threads in ["1", "0"] {
        let first = figures(&[
            "first",
            &img,
            "--area",
            "2ch",
            "--threads",
            threads,
            "--bytes",
            "4096",
            "--iterations",
            "2",
        ]);
        assert!(first.contains("iterations\t2\nmedian_us\t"), "{first}");
    }
    for args in [
        &["open", &img, "--iterations", "1", "--evict", "/bin/false"][..],
        &["first", &img, "--area", "2ch", "--threads", "1", "--iterations", "1", "--evict", "/no/such"][..],
    ] {
        let out = bench(args);
        assert!(!out.status.success(), "{args:?}");
        assert_eq!(String::from_utf8_lossy(&out.stderr).lines().next(), Some("io-error"), "{args:?}");
    }
    // Asking for more than the track holds is an error, not a short figure. These, and a track
    // the area lacks and more iterations, seeks or chunk bytes than `bench` keeps, are usage errors, and none panics.
    for args in [
        &["first", &img, "--area", "2ch", "--threads", "1", "--bytes", "4000000000"][..],
        &["first", &img, "--area", "2ch", "--threads", "1", "--track", "99"][..],
        &["first", &img, "--area", "2ch", "--threads", "1", "--iterations", "18446744073709551615"][..],
        &["open", &img, "--iterations", "18446744073709551615"][..],
        &["realtime", &img, "--area", "2ch", "--seeks", "18446744073709551615"][..],
        &[
            "dsf",
            &img,
            "--area",
            "2ch",
            "--threads",
            "1",
            "--out",
            "/dev/null",
            "--chunk",
            "18446744073709551615",
        ][..],
    ] {
        let out = bench(args);
        assert!(!out.status.success(), "{args:?}");
        assert_eq!(String::from_utf8_lossy(&out.stderr).lines().next(), Some("usage-error"), "{args:?}");
    }
}
