// SPDX-License-Identifier: MIT OR Apache-2.0

//! Writes fuzz seeds derived from the conformance corpus into `OUT/<target>/`.
//!
//! `toc`: each image from LSN 510 on, cut to 160 sectors (the TOC region of every case).
//! `demux`: for each area of each accepted case, its channel count followed by its first
//! 64 audio sectors.

use std::path::PathBuf;
use std::process::ExitCode;

use scarletbook::Disc;
use scarletbook_conformance::{corpus_dir, load_cases};

const TOC_SEED_SECTORS: usize = 160;
const DEMUX_SEED_SECTORS: u64 = 64;

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
    let demux_dir = out.join("demux");
    for d in [&toc_dir, &demux_dir] {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
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
        let Ok(mut disc) = Disc::open(std::io::Cursor::new(&bytes)) else { continue };
        for area in disc.toc().areas.clone() {
            let (start, end) = scarletbook::audio::audio_sector_range(&area, disc.toc().sectors);
            let mut seed = vec![area.channel_count];
            let mut sector = vec![0u8; 2048];
            for lsn in start..end.min(start + DEMUX_SEED_SECTORS) {
                if disc.image_mut().read_sectors(lsn, &mut sector).map_err(|e| e.to_string())? {
                    seed.extend_from_slice(&sector);
                }
            }
            std::fs::write(demux_dir.join(format!("{id}-{}", area.slot)), seed).map_err(|e| e.to_string())?;
            n += 1;
        }
    }
    Ok(n)
}
