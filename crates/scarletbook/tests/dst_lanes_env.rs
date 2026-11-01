// SPDX-License-Identifier: MIT OR Apache-2.0

//! EXPERIMENT (#203, #61), not for main: the decoded audio of every DST corpus image is the
//! same, to the bit, for every `SCARLETBOOK_DST_LANES` value (1 to 4) and without it. The
//! variable is read once per process, so each value runs in a child process of this test.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

use scarletbook::Disc;

const CHILD: &str = "SB_DST_LANES_TEST_CHILD";

/// FNV-1a over the number, status and audio of every frame of every track of the
/// `dst-features-*` corpus images.
fn digest() -> (u64, usize) {
    let images = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/images");
    let mut paths: Vec<_> = std::fs::read_dir(images)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("dst-features-"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty());
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut eat = |bytes: &[u8]| {
        for &b in bytes {
            h = (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
        }
    };
    let mut frames = 0;
    for path in &paths {
        let mut disc = Disc::open(std::io::BufReader::new(std::fs::File::open(path).unwrap())).unwrap();
        let areas: Vec<_> = disc.toc().areas.iter().map(|a| (a.slot, a.tracks.len() as u32)).collect();
        for (slot, tracks) in areas {
            for n in 1..=tracks {
                for frame in disc.track_audio(slot, n).unwrap() {
                    let frame = frame.unwrap();
                    eat(&frame.number.to_le_bytes());
                    eat(format!("{:?}", frame.status).as_bytes());
                    eat(&frame.data);
                    frames += 1;
                }
            }
        }
    }
    (h, frames)
}

#[test]
#[cfg_attr(miri, ignore = "starts processes")]
fn every_lane_count_decodes_the_same() {
    if std::env::var_os(CHILD).is_some() {
        let (h, frames) = digest();
        // The lane counts in force, so that the parent sees the variable took effect.
        let lanes: Vec<usize> = (1..=6).map(scarletbook::dst::lanes).collect();
        println!("DIGEST {h:016x} {frames} LANES {lanes:?}");
        return;
    }
    // The digest and the lane counts of a child.
    let run = |lanes: Option<&str>| -> (String, String) {
        let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", "every_lane_count_decodes_the_same", "--nocapture", "--test-threads", "1"]);
        cmd.env(CHILD, "1");
        match lanes {
            Some(k) => cmd.env("SCARLETBOOK_DST_LANES", k),
            None => cmd.env_remove("SCARLETBOOK_DST_LANES"),
        };
        let out = cmd.output().unwrap();
        assert!(out.status.success(), "lanes {lanes:?}: {}", String::from_utf8_lossy(&out.stderr));
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        // The harness prints the test's name on the same line.
        let at = text.find("DIGEST ").expect("no digest");
        let line = text[at..].lines().next().unwrap_or_default();
        let (digest, lanes) = line.split_once(" LANES ").expect("no lane counts");
        (digest.to_owned(), lanes.to_owned())
    };
    let (default, _) = run(None);
    assert!(!default.ends_with(" 0"), "no frames decoded: {default}");
    for k in ["1", "2", "3", "4"] {
        let (digest, lanes) = run(Some(k));
        assert_eq!(lanes, format!("[{k}, {k}, {k}, {k}, {k}, {k}]"), "SCARLETBOOK_DST_LANES={k} not applied");
        assert_eq!(digest, default, "SCARLETBOOK_DST_LANES={k}");
    }
}
