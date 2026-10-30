// SPDX-License-Identifier: MIT OR Apache-2.0

//! Writes a large synthetic disc image for benchmarking:
//!
//! ```text
//! scarletbook-synth OUT --minutes M --area 2ch|mc|both --kind dsd|dst
//!     [--sector-size 2048|2064|2054] [--mc-channels 5|6] [--track-minutes T] [--corpus DIR]
//! ```
//!
//! `M` and `T` may be fractional; each area lasts `M` minutes, with one track per `T`
//! minutes (default 5). DST frames are taken from the conformance corpus (default: this
//! repository's `corpus/`). See `scarletbook_conformance::synth` for the layout.

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::process::ExitCode;

use scarletbook::SectorFormat;
use scarletbook_conformance::corpus_dir;
use scarletbook_conformance::synth::{Config, DstSource, Kind, write_image};

const USAGE: &str = "usage: scarletbook-synth OUT --minutes M --area 2ch|mc|both --kind dsd|dst \
                     [--sector-size 2048|2064|2054] [--mc-channels 5|6] [--track-minutes T] [--corpus DIR]";

fn frames(minutes: &str) -> Result<u32, String> {
    let m: f64 = minutes.parse().map_err(|_| format!("not a number of minutes: {minutes}"))?;
    let f = (m * 60.0 * 75.0).round();
    if !(f >= 1.0 && f <= f64::from(u32::MAX)) {
        return Err(format!("duration out of range: {minutes} minutes"));
    }
    Ok(f as u32)
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<(PathBuf, Config, PathBuf), String> {
    let out = PathBuf::from(args.next().ok_or(USAGE)?);
    let mut cfg = Config {
        frames: 0,
        two_channel: false,
        multichannel: false,
        mc_channels: 5,
        kind: Kind::Dsd,
        format: SectorFormat::Plain,
        track_frames: 5 * 60 * 75,
    };
    let (mut area, mut kind) = (false, false);
    let mut corpus = corpus_dir();
    while let Some(flag) = args.next() {
        let v = args.next().ok_or_else(|| format!("{flag} needs a value\n{USAGE}"))?;
        match flag.as_str() {
            "--minutes" => cfg.frames = frames(&v)?,
            "--track-minutes" => cfg.track_frames = frames(&v)?,
            "--area" => {
                (cfg.two_channel, cfg.multichannel) = match v.as_str() {
                    "2ch" => (true, false),
                    "mc" => (false, true),
                    "both" => (true, true),
                    _ => return Err(format!("--area must be 2ch, mc or both, not {v}")),
                };
                area = true;
            }
            "--kind" => {
                cfg.kind = match v.as_str() {
                    "dsd" => Kind::Dsd,
                    "dst" => Kind::Dst,
                    _ => return Err(format!("--kind must be dsd or dst, not {v}")),
                };
                kind = true;
            }
            "--sector-size" => {
                cfg.format = match v.as_str() {
                    "2048" => SectorFormat::Plain,
                    "2064" => SectorFormat::Raw2064,
                    "2054" => SectorFormat::Raw2054,
                    _ => return Err(format!("--sector-size must be 2048, 2064 or 2054, not {v}")),
                }
            }
            "--mc-channels" => cfg.mc_channels = v.parse().map_err(|_| format!("bad --mc-channels {v}"))?,
            "--corpus" => corpus = PathBuf::from(v),
            _ => return Err(format!("unknown argument {flag}\n{USAGE}")),
        }
    }
    if cfg.frames == 0 || !area || !kind {
        return Err(format!("--minutes, --area and --kind are required\n{USAGE}"));
    }
    scarletbook_conformance::synth::check(&cfg)?;
    Ok((out, cfg, corpus))
}

fn run(out: PathBuf, cfg: &Config, corpus: PathBuf) -> Result<(), String> {
    let src = if cfg.kind == Kind::Dst { DstSource::from_corpus(&corpus)? } else { DstSource::default() };
    let file = File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let layout = write_image(BufWriter::with_capacity(1 << 22, file), cfg, &src)
        .map_err(|e| format!("{}: {e}", out.display()))?;
    let bytes = layout.sectors * cfg.format.stored_size();
    eprintln!("{}: {} sectors, {bytes} bytes", out.display(), layout.sectors);
    for a in &layout.areas {
        eprintln!(
            "  {:?}: {} channels, TOC at {} and {}, audio {}..={}, {} tracks",
            a.slot,
            a.channels,
            a.toc_lsns.0,
            a.toc_lsns.1,
            a.audio.0,
            a.audio.1,
            a.tracks.len()
        );
    }
    Ok(())
}

fn main() -> ExitCode {
    match parse(std::env::args().skip(1)).and_then(|(out, cfg, corpus)| run(out, &cfg, corpus)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
