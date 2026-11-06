// SPDX-License-Identifier: MIT OR Apache-2.0

//! The crate's binaries, run as subprocesses on small inputs: `scarletbook-toc`,
//! `scarletbook-synth`, `scarletbook-seeds` and `scarletbook-seekcost`. Their exit codes,
//! the shape of their output and their usage and error paths.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use scarletbook::{Disc, SectorFormat, Slot};
use scarletbook_conformance::synth::{Config, DstSource, Kind, Layout, write_image};
use scarletbook_conformance::{corpus_dir, load_cases};
use serde_json::Value;

const TOC: &str = env!("CARGO_BIN_EXE_scarletbook-toc");
const SYNTH: &str = env!("CARGO_BIN_EXE_scarletbook-synth");
const SEEDS: &str = env!("CARGO_BIN_EXE_scarletbook-seeds");
const SEEKCOST: &str = env!("CARGO_BIN_EXE_scarletbook-seekcost");

/// An empty directory of its own for test `name`, under `target/`.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("bins").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(bin: &str, args: &[&dyn AsRef<std::ffi::OsStr>]) -> Output {
    Command::new(bin).args(args.iter().map(|a| a.as_ref())).output().unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).unwrap()
}

/// Asserts that `out` failed with exit code `code` and a message containing `want`.
#[track_caller]
fn fails(out: &Output, code: i32, want: &str) {
    assert_eq!(out.status.code(), Some(code), "stderr: {}", stderr(out));
    assert!(stderr(out).contains(want), "expected {want:?} in {:?}", stderr(out));
}

/// Writes a synthetic image of `frames` frames per area to `path`.
fn synth(path: &Path, frames: u32, kind: Kind, mc_channels: u8, src: &DstSource) -> Layout {
    let cfg = Config {
        frames,
        two_channel: true,
        multichannel: true,
        mc_channels,
        kind,
        format: SectorFormat::Plain,
        track_frames: frames.div_ceil(2),
    };
    write_image(BufWriter::new(File::create(path).unwrap()), &cfg, src).unwrap()
}

// scarletbook-toc

/// Warnings as a set: the manifest does not fix their order.
fn warning_set(v: &Value) -> BTreeSet<String> {
    v.as_array().unwrap().iter().map(Value::to_string).collect()
}

#[test]
fn toc_matches_the_manifest() {
    let corpus = corpus_dir();
    let cases = load_cases(&corpus).unwrap();
    // An accepted case without warnings, one with warnings and a rejected one.
    let outcome = |c: &&Value| c["expected"]["outcome"].as_str().unwrap().to_owned();
    let warned = |c: &&Value| c["expected"]["warnings"].as_array().is_some_and(|w| !w.is_empty());
    let picks = [
        cases.iter().find(|c| outcome(c) == "ok" && !warned(c)).unwrap(),
        cases.iter().find(|c| outcome(c) == "ok" && warned(c)).unwrap(),
        cases.iter().find(|c| outcome(c) == "reject").unwrap(),
    ];
    for case in picks {
        let id = case["id"].as_str().unwrap();
        let out = run(TOC, &[&corpus.join("images").join(format!("{id}.iso"))]);
        assert!(out.status.success(), "{id}: {}", stderr(&out));
        let got: Value = serde_json::from_str(&stdout(&out)).unwrap();
        let keys: Vec<_> = got.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["outcome", "error", "warnings", "toc"], "{id}");
        let want = &case["expected"];
        assert_eq!(got["outcome"], want["outcome"], "{id}");
        assert_eq!(got["error"], want["error"], "{id}");
        assert_eq!(got["toc"], want["toc"], "{id}");
        assert_eq!(warning_set(&got["warnings"]), warning_set(&want["warnings"]), "{id}");
    }
}

#[test]
fn toc_of_a_synthetic_image() {
    let dir = scratch("toc");
    let image = dir.join("dsd.iso");
    let layout = synth(&image, 20, Kind::Dsd, 5, &DstSource::default());
    let out = run(TOC, &[&image]);
    assert!(out.status.success(), "{}", stderr(&out));
    let got: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!((got["outcome"].as_str(), &got["error"]), (Some("ok"), &Value::Null));
    assert_eq!(got["warnings"], serde_json::json!([]));
    assert_eq!(got["toc"]["areas"].as_array().unwrap().len(), layout.areas.len());

    // Not an SACD image: rejected with a code, still exit 0.
    std::fs::write(dir.join("empty"), b"").unwrap();
    let out = run(TOC, &[&dir.join("empty")]);
    assert!(out.status.success());
    let got: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!((got["outcome"].as_str(), got["error"].as_str()), (Some("reject"), Some("not-scarletbook")));
    assert_eq!(got["toc"], Value::Null);
}

#[test]
fn toc_usage() {
    fails(&run(TOC, &[]), 2, "usage: scarletbook-toc IMAGE");
    fails(&run(TOC, &[&"a", &"b"]), 2, "usage: scarletbook-toc IMAGE");
    let missing = scratch("toc-usage").join("missing.iso");
    fails(&run(TOC, &[&missing]), 2, "missing.iso");
}

// scarletbook-synth

/// The sector count `scarletbook-synth` reports on its first line of stderr.
fn reported_sectors(out: &Output) -> u64 {
    let err = stderr(out);
    let first = err.lines().next().unwrap();
    let (_, rest) = first.rsplit_once(": ").unwrap();
    rest.split_once(" sectors").unwrap().0.parse().unwrap()
}

/// An image name, the arguments that write it, its sector format and its areas.
type Run<'a> = (&'a str, &'a [&'a str], SectorFormat, &'a [(Slot, u8)]);

#[test]
fn synth_writes_images() {
    let dir = scratch("synth");
    #[rustfmt::skip]
    let runs: [Run; 6] = [
        ("dsd.iso", &["--minutes", "0.02", "--area", "both", "--kind", "dsd", "--track-minutes", "0.01",
            "--sector-size", "2064"], SectorFormat::Raw2064, &[(Slot::TwoChannel, 2), (Slot::Multichannel, 5)]),
        ("dst.iso", &["--minutes", "0.02", "--area", "mc", "--kind", "dst", "--mc-channels", "6",
            "--sector-size", "2054", "--dst-frames", "corpus"], SectorFormat::Raw2054, &[(Slot::Multichannel, 6)]),
        ("uneven.iso", &["--minutes", "0.02", "--area", "2ch", "--kind", "dst", "--dst-frames", "uneven"],
            SectorFormat::Plain, &[(Slot::TwoChannel, 2)]),
        ("plain.iso", &["--minutes", "0.01", "--area", "2ch", "--kind", "dsd", "--sector-size", "2048"],
            SectorFormat::Plain, &[(Slot::TwoChannel, 2)]),
        ("stress5.iso", &["--minutes", "0.01", "--area", "mc", "--kind", "dst", "--dst-frames", "stress"],
            SectorFormat::Plain, &[(Slot::Multichannel, 5)]),
        ("order64.iso", &["--minutes", "0.01", "--area", "both", "--kind", "dst", "--dst-frames", "stress",
            "--dst-order", "64"], SectorFormat::Plain, &[(Slot::TwoChannel, 2), (Slot::Multichannel, 5)]),
    ];
    for (name, args, format, areas) in runs {
        let path = dir.join(name);
        let mut argv: Vec<&dyn AsRef<std::ffi::OsStr>> = vec![&path];
        argv.extend(args.iter().map(|a| a as &dyn AsRef<std::ffi::OsStr>));
        let out = run(SYNTH, &argv);
        assert!(out.status.success(), "{name}: {}", stderr(&out));
        // One line for the image, one per area.
        assert_eq!(stderr(&out).lines().count(), 1 + areas.len(), "{name}");
        let len = std::fs::metadata(&path).unwrap().len();
        assert_eq!(len, reported_sectors(&out) * format.stored_size(), "{name}");
        let disc = Disc::open(File::open(&path).unwrap()).unwrap();
        assert!(disc.warnings().is_empty(), "{name}: {:?}", disc.warnings());
        let got: Vec<_> = disc.toc().areas.iter().map(|a| (a.slot, a.channel_count)).collect();
        assert_eq!(got, areas, "{name}");
    }
}

#[test]
fn synth_usage() {
    let dir = scratch("synth-usage");
    let out = dir.join("out.iso");
    let o: &dyn AsRef<std::ffi::OsStr> = &out;
    let base = ["--minutes", "0.01", "--area", "2ch", "--kind", "dsd"];
    let with = |extra: &[&str]| {
        let mut argv = vec![o];
        argv.extend(base.iter().chain(extra).map(|a| a as &dyn AsRef<std::ffi::OsStr>));
        run(SYNTH, &argv)
    };
    fails(&run(SYNTH, &[]), 1, "usage: scarletbook-synth OUT");
    fails(&run(SYNTH, &[o, &"--minutes"]), 1, "--minutes needs a value");
    fails(&run(SYNTH, &[o, &"--area", &"2ch"]), 1, "--minutes, --area and --kind are required");
    fails(&with(&["--area", "3ch"]), 1, "--area must be 2ch, mc or both, not 3ch");
    fails(&with(&["--kind", "pcm"]), 1, "--kind must be dsd or dst, not pcm");
    fails(&with(&["--sector-size", "512"]), 1, "--sector-size must be 2048, 2064 or 2054, not 512");
    fails(&with(&["--mc-channels", "x"]), 1, "bad --mc-channels x");
    fails(&with(&["--dst-frames", "x"]), 1, "--dst-frames must be corpus, stress or uneven, not x");
    fails(&with(&["--bogus", "1"]), 1, "unknown argument --bogus");
    fails(&with(&["--minutes", "x"]), 1, "not a number of minutes: x");
    fails(&with(&["--minutes", "0"]), 1, "duration out of range: 0 minutes");
    fails(&with(&["--minutes", "NaN"]), 1, "duration out of range: NaN minutes");
    fails(&with(&["--track-minutes", "-1"]), 1, "duration out of range");
    // Rejected by synth::check.
    fails(&with(&["--area", "mc", "--mc-channels", "4"]), 1, "multichannel areas have 5 or 6 channels");
    fails(&with(&["--minutes", "1000"]), 1, "duration must be 1 to");
    fails(&with(&["--minutes", "4", "--track-minutes", "0.01"]), 1, "at most 255 tracks");
    // Checked before any frame is made.
    let stress = ["--kind", "dst", "--dst-frames", "stress"];
    fails(
        &with(&[&stress[..], &["--dst-order", "129"]].concat()),
        1,
        "--dst-order must be 1 to 128, not 129",
    );
    fails(&with(&["--dst-order", "32"]), 1, "--dst-order needs --dst-frames stress");
    let none = dir.join("no-corpus");
    let none = none.to_str().unwrap();
    fails(&with(&["--kind", "dst", "--corpus", none]), 1, "no-corpus");
    let nowhere = dir.join("no-such-dir").join("out.iso");
    fails(
        &run(SYNTH, &[&nowhere, &"--minutes", &"0.01", &"--area", &"2ch", &"--kind", &"dsd"]),
        1,
        "out.iso",
    );
    assert!(!out.exists());
}

// scarletbook-seeds

/// Files under `dir`, recursively, by subdirectory name.
fn count_files(dir: &Path) -> Vec<(String, usize)> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (e.file_name().into_string().unwrap(), std::fs::read_dir(e.path()).unwrap().count())
        })
        .collect();
    v.sort();
    v
}

#[test]
fn seeds_writes_every_target() {
    let dir = scratch("seeds");
    let out = run(SEEDS, &[&dir]);
    assert!(out.status.success(), "{}", stderr(&out));
    let counts = count_files(&dir);
    let names: Vec<_> = counts.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["demux", "dst", "seek_index", "toc"]);
    assert!(counts.iter().all(|&(_, n)| n > 0), "{counts:?}");
    // One toc seed per corpus case.
    assert_eq!(counts[3].1, load_cases(&corpus_dir()).unwrap().len());
    let total: usize = counts.iter().map(|&(_, n)| n).sum();
    assert_eq!(stderr(&out).trim(), format!("wrote {total} seeds to {}", dir.display()));
}

#[test]
fn seeds_usage() {
    fails(&run(SEEDS, &[]), 2, "usage: scarletbook-seeds OUTDIR");
    // An output directory that cannot be made.
    let file = scratch("seeds-usage").join("file");
    std::fs::write(&file, b"").unwrap();
    let out = run(SEEDS, &[&file]);
    assert_eq!(out.status.code(), Some(1));
    assert!(!stderr(&out).is_empty());
}

// scarletbook-seekcost

/// The rows of the markdown table under the header that starts with `header`.
fn table<'a>(text: &'a str, header: &str) -> Vec<Vec<&'a str>> {
    let mut lines = text.lines().skip_while(|l| !l.starts_with(header));
    let head = lines.next().unwrap_or_else(|| panic!("no table {header:?} in\n{text}"));
    let columns = head.matches('|').count() - 1;
    assert!(lines.next().unwrap().starts_with("|---"));
    lines
        .take_while(|l| l.starts_with('|'))
        .map(|l| {
            let cells: Vec<_> = l.trim_matches('|').split('|').map(str::trim).collect();
            assert_eq!(cells.len(), columns, "{l}");
            cells
        })
        .collect()
}

/// An image whose multichannel area takes provisional starts: DST frames of uneven size.
fn uneven_image(dir: &Path) -> PathBuf {
    let path = dir.join("uneven.iso");
    let src = DstSource::from_corpus(&corpus_dir()).unwrap().uneven();
    synth(&path, 900, Kind::Dst, 5, &src);
    path
}

#[test]
fn seekcost_tables() {
    let dir = scratch("seekcost");
    let image = uneven_image(&dir);
    #[rustfmt::skip]
    let out = run(SEEKCOST, &[&image, &"--slot", &"mc", &"--interval", &"16", &"--frames", &"4",
        &"--repeat", &"1", &"--rtt-ms", &"0.25", &"--mbps", &"200", &"--positions", &"4"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("play time 900 frames, 4 frames per range"), "{text}");
    assert!(text.contains("index (K = 16): built in"), "{text}");
    assert!(text.contains("network model 0.25 ms per read call, 200 MB/s"), "{text}");

    let exact = table(&text, "| Range starts at | Without index");
    let starts: Vec<_> = exact.iter().map(|r| r[0]).collect();
    #[rustfmt::skip]
    assert_eq!(starts, ["0 % (frame 0)", "16 % (frame 144)", "42 % (frame 378)", "72 % (frame 648)",
        "99 % (frame 891)"]);
    let cold = table(&text, "| Range starts at | Start |");
    assert_eq!(cold.len(), 5);
    assert!(cold.iter().all(|r| r[6] == "yes"), "{text}");
    assert!(cold.iter().any(|r| r[1].starts_with("provisional at LSN")), "{text}");

    let fresh = table(&text, "| Start | Positions |");
    assert!(fresh.iter().any(|r| r[0] == "provisional"), "{text}");
    let positions: u32 = fresh.iter().map(|r| r[1].parse::<u32>().unwrap()).sum();
    assert_eq!(positions, 4);
    assert!(text.contains("Equal to exact reads: 4 of 4"), "{text}");

    let warm = table(&text, "| Saved state |");
    let names: Vec<_> = warm.iter().map(|r| r[0]).collect();
    assert_eq!(names, ["seek hints, provisional", "seek hints and confirmations", "complete index, exact"]);
    assert!(warm.iter().all(|r| r[3] == "8 of 8"), "{text}");
}

#[test]
fn seekcost_without_positions() {
    let dir = scratch("seekcost-short");
    let image = dir.join("dsd.iso");
    synth(&image, 20, Kind::Dsd, 5, &DstSource::default());
    let out = run(SEEKCOST, &[&image, &"--frames", &"2", &"--repeat", &"2", &"--positions", &"0"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert_eq!(table(&text, "| Range starts at | Without index").len(), 5);
    assert_eq!(table(&text, "| Range starts at | Start |").len(), 5);
    assert!(!text.contains("| Start | Positions |") && !text.contains("| Saved state |"), "{text}");
}

#[test]
fn seekcost_usage() {
    let dir = scratch("seekcost-usage");
    let image = dir.join("2ch.iso");
    let cfg = Config {
        frames: 10,
        two_channel: true,
        multichannel: false,
        mc_channels: 5,
        kind: Kind::Dsd,
        format: SectorFormat::Plain,
        track_frames: 10,
    };
    write_image(BufWriter::new(File::create(&image).unwrap()), &cfg, &DstSource::default()).unwrap();
    let i: &dyn AsRef<std::ffi::OsStr> = &image;
    fails(&run(SEEKCOST, &[]), 1, "usage: scarletbook-seekcost IMAGE");
    fails(&run(SEEKCOST, &[i, &"--slot"]), 1, "--slot needs a value");
    fails(&run(SEEKCOST, &[i, &"--slot", &"4ch"]), 1, "--slot must be 2ch or mc, not 4ch");
    fails(&run(SEEKCOST, &[i, &"--frames", &"x"]), 1, "--frames must be a number, not x");
    fails(&run(SEEKCOST, &[i, &"--interval", &"-1"]), 1, "--interval must be a number, not -1");
    fails(&run(SEEKCOST, &[i, &"--rtt-ms", &"0"]), 1, "--rtt-ms must be positive");
    fails(&run(SEEKCOST, &[i, &"--mbps", &"inf"]), 1, "--mbps must be positive");
    fails(&run(SEEKCOST, &[i, &"--mbps", &"x"]), 1, "--mbps must be a number, not x");
    fails(&run(SEEKCOST, &[i, &"--bogus", &"1"]), 1, "unknown option --bogus");
    fails(&run(SEEKCOST, &[i, &"--slot", &"mc"]), 1, "no such area");
    fails(&run(SEEKCOST, &[&dir.join("missing.iso")]), 1, "missing.iso");
    std::fs::write(dir.join("empty"), b"").unwrap();
    let out = run(SEEKCOST, &[&dir.join("empty")]);
    assert_eq!(out.status.code(), Some(1));
    assert!(!stderr(&out).is_empty());
}
