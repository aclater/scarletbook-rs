// SPDX-License-Identifier: MIT OR Apache-2.0

//! Runs scarletbook against the sacd-spec conformance corpus (`corpus/README.md`).

pub mod json;

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use scarletbook::{Disc, WarningCode};
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

/// Checks one case and returns the list of mismatches (empty when it passes).
pub fn check_case(corpus: &Path, case: &Value) -> Vec<String> {
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
        (_, Ok(disc)) => {
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
        }
    }
    fails
}
