// SPDX-License-Identifier: MIT OR Apache-2.0

//! Writes fuzz seeds derived from the conformance corpus into `OUT/<target>/`.
//!
//! `toc`: each image from LSN 510 on, cut to 160 sectors (the TOC region of every case).

use std::path::PathBuf;
use std::process::ExitCode;

use scarletbook_conformance::{corpus_dir, load_cases};

const TOC_SEED_SECTORS: usize = 160;

fn main() -> ExitCode {
    let Some(out) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: scarletbook-seeds OUTDIR");
        return ExitCode::from(2);
    };
    match run(&out) {
        Ok(n) => {
            eprintln!("wrote {n} seeds to {}", out.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn run(out: &std::path::Path) -> Result<usize, String> {
    let corpus = corpus_dir();
    let toc_dir = out.join("toc");
    std::fs::create_dir_all(&toc_dir).map_err(|e| e.to_string())?;
    let mut n = 0;
    for case in load_cases(&corpus)? {
        let id = case["id"].as_str().unwrap_or_default();
        let bytes =
            std::fs::read(corpus.join("images").join(format!("{id}.iso"))).map_err(|e| e.to_string())?;
        let stored = case["expected"]["toc"]["sector_size"].as_u64().unwrap_or(2048) as usize;
        let from = (510 * stored).min(bytes.len());
        let to = (from + TOC_SEED_SECTORS * stored).min(bytes.len());
        std::fs::write(toc_dir.join(id), &bytes[from..to]).map_err(|e| e.to_string())?;
        n += 1;
    }
    Ok(n)
}
