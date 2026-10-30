// SPDX-License-Identifier: MIT OR Apache-2.0

//! Runs scarletbook against the sacd-spec conformance corpus (`corpus/README.md`).

pub mod json;

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use scarletbook::audio::{self, FRAME_BYTES_PER_CHANNEL};
use scarletbook::{Disc, Slot, WarningCode};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The corpus directory of this repository.
pub fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus")
}

/// Loads the manifest's cases.
pub fn load_cases(corpus: &Path) -> Result<Vec<Value>, String> {
    let text = std::fs::read_to_string(corpus.join("manifest.json")).map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if v["format_version"] != 1 {
        return Err("unsupported manifest format_version".into());
    }
    v["cases"].as_array().cloned().ok_or_else(|| "manifest has no cases".into())
}

/// Hex SHA-256.
pub fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn image_path(corpus: &Path, case: &Value) -> PathBuf {
    let id = case["id"].as_str().unwrap_or_default();
    corpus.join("images").join(format!("{id}.iso"))
}

/// Warnings that the corpus does not compare (corpus README, step 4).
fn optional(code: WarningCode) -> bool {
    matches!(code, WarningCode::AudioSectorMalformed | WarningCode::SampleFrequencyUnknown)
}

fn warning_key(v: &Value) -> (String, String, String) {
    let s = |k: &str| v.get(k).map(|x| x.to_string()).unwrap_or_default();
    (s("code"), s("area"), s("track"))
}

/// What checking one case found.
#[derive(Debug, Default)]
pub struct CaseReport {
    /// Mismatches; empty when the case passes.
    pub failures: Vec<String>,
    /// Tracks whose audio was compared.
    pub tracks_compared: usize,
}

/// The decoded audio of one track: per-channel SHA-256 and the frame counts.
#[derive(Debug, PartialEq, Eq)]
pub struct TrackAudio {
    /// One hash per channel.
    pub sha256: Vec<String>,
    /// Frames filled with 0x69.
    pub missing_frames: u64,
    /// DST frames filled with 0x55.
    pub invalid_dst_frames: u64,
}

/// Decodes a track and hashes its audio per channel.
pub fn track_audio<R: std::io::Read + std::io::Seek>(
    disc: &mut Disc<R>,
    slot: Slot,
    number: u32,
) -> Result<TrackAudio, scarletbook::Error> {
    let mut audio = disc.track_audio(slot, number)?;
    let channels = audio.channels();
    let mut hashers: Vec<Sha256> = (0..channels).map(|_| Sha256::new()).collect();
    let mut chan = vec![0u8; FRAME_BYTES_PER_CHANNEL];
    for frame in audio.by_ref() {
        let frame = frame?;
        for (c, h) in hashers.iter_mut().enumerate() {
            audio::deinterleave(&frame.data, channels, c as u8, &mut chan)?;
            h.update(&chan);
        }
    }
    Ok(TrackAudio {
        sha256: hashers.into_iter().map(|h| hex(&h.finalize())).collect(),
        missing_frames: audio.missing_frames(),
        invalid_dst_frames: audio.invalid_dst_frames(),
    })
}

fn slot_from(name: &str) -> Option<Slot> {
    Slot::ALL.into_iter().find(|s| s.name() == name)
}

fn check_audio<R: std::io::Read + std::io::Seek>(
    disc: &mut Disc<R>,
    expected: &Value,
    report: &mut CaseReport,
) {
    let fails = &mut report.failures;
    let exp_areas: Vec<&Value> = expected.as_array().map(|a| a.iter().collect()).unwrap_or_default();
    let got_slots: Vec<&str> = disc.toc().areas.iter().map(|a| a.slot.name()).collect();
    let exp_slots: Vec<&str> = exp_areas.iter().filter_map(|a| a["area"].as_str()).collect();
    if got_slots != exp_slots {
        fails.push(format!("audio areas {got_slots:?}, expected {exp_slots:?}"));
        return;
    }
    for ea in exp_areas {
        let Some(slot) = ea["area"].as_str().and_then(slot_from) else { continue };
        let Some(area) = disc.toc().area(slot).cloned() else { continue };
        let exp_tracks = ea["tracks"].as_array().cloned().unwrap_or_default();
        let exp_numbers: Vec<u64> = exp_tracks.iter().filter_map(|t| t["number"].as_u64()).collect();
        let got_numbers: Vec<u64> = area.tracks.iter().map(|t| u64::from(t.number)).collect();
        if exp_numbers != got_numbers {
            fails.push(format!("{slot}: tracks {got_numbers:?}, expected {exp_numbers:?}"));
            continue;
        }
        for (et, track) in exp_tracks.iter().zip(&area.tracks) {
            let n = u32::from(track.number);
            let what = format!("{slot} track {n}");
            let range = (u64::from(track.start_frame), u64::from(track.end_frame()));
            if (et["first_frame"].as_u64(), et["end_frame"].as_u64()) != (Some(range.0), Some(range.1)) {
                fails.push(format!(
                    "{what}: frame range {range:?}, expected [{}, {})",
                    et["first_frame"], et["end_frame"]
                ));
            }
            let outcome = et["outcome"].as_str().unwrap_or_default();
            match (outcome, disc.check_track(slot, n)) {
                ("ok", Ok(())) => {}
                ("ok", Err(e)) => {
                    fails.push(format!("{what}: no audio ({e}), expected audio"));
                    continue;
                }
                (_, Ok(())) => {
                    fails.push(format!("{what}: audio produced, expected {}", et["error"]));
                    continue;
                }
                (_, Err(e)) => {
                    if e.code() != et["error"].as_str() {
                        fails.push(format!("{what}: {:?}, expected {}", e.code(), et["error"]));
                    }
                    continue;
                }
            }
            match track_audio(disc, slot, n) {
                Err(e) => fails.push(format!("{what}: {e}")),
                Ok(got) => {
                    report.tracks_compared += 1;
                    let exp_sha: Vec<String> = et["sha256"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|h| h.as_str().map(str::to_owned)).collect())
                        .unwrap_or_default();
                    if got.sha256 != exp_sha {
                        fails.push(format!("{what}: audio hashes differ"));
                    }
                    let counts = (et["missing_frames"].as_u64(), et["invalid_dst_frames"].as_u64());
                    if counts != (Some(got.missing_frames), Some(got.invalid_dst_frames)) {
                        fails.push(format!(
                            "{what}: missing/invalid frames ({}, {}), expected {counts:?}",
                            got.missing_frames, got.invalid_dst_frames
                        ));
                    }
                }
            }
        }
    }
}

/// Checks one case and returns the list of mismatches (empty when it passes).
pub fn check_case(corpus: &Path, case: &Value) -> Vec<String> {
    check_case_report(corpus, case).failures
}

/// Checks one case, TOC, warnings and audio.
pub fn check_case_report(corpus: &Path, case: &Value) -> CaseReport {
    let mut report = CaseReport::default();
    report.failures = check_case_inner(corpus, case, &mut report);
    report
}

fn check_case_inner(corpus: &Path, case: &Value, report: &mut CaseReport) -> Vec<String> {
    let mut fails = Vec::new();
    let path = image_path(corpus, case);
    let mut bytes = Vec::new();
    if let Err(e) = File::open(&path).and_then(|mut f| f.read_to_end(&mut bytes)) {
        return vec![format!("cannot read {}: {e}", path.display())];
    }
    if case["input_sha256"].as_str() != Some(sha256_hex(&bytes).as_str()) {
        return vec!["image SHA-256 does not match the manifest".into()];
    }
    drop(bytes);
    let expected = &case["expected"];
    let file = match File::open(&path) {
        Ok(f) => f,
        Err(e) => return vec![e.to_string()],
    };
    let disc = Disc::open(BufReader::new(file));
    match (expected["outcome"].as_str(), disc) {
        (Some("reject"), Err(e)) => {
            if e.code() != expected["error"].as_str() {
                fails.push(format!("rejected with {:?}, expected {}", e.code(), expected["error"]));
            }
        }
        (Some("reject"), Ok(_)) => fails.push(format!("accepted, expected rejection {}", expected["error"])),
        (_, Err(e)) => fails.push(format!("rejected with {e}, expected acceptance")),
        (_, Ok(mut disc)) => {
            let got = json::toc(disc.toc());
            if got != expected["toc"] {
                fails.push(format!("TOC differs:\n  got      {got}\n  expected {}", expected["toc"]));
            }
            let got_w: BTreeSet<_> = disc
                .warnings()
                .iter()
                .filter(|w| !optional(w.code))
                .map(|w| warning_key(&json::warning(w)))
                .collect();
            let exp_w: BTreeSet<_> = expected["warnings"]
                .as_array()
                .map(|a| a.iter().map(warning_key).collect())
                .unwrap_or_default();
            if got_w != exp_w {
                fails.push(format!("warnings differ: got {got_w:?}, expected {exp_w:?}"));
            }
            check_audio(&mut disc, &expected["audio"], report);
            fails.append(&mut report.failures);
        }
    }
    fails
}
