// SPDX-License-Identifier: MIT OR Apache-2.0

//! Prints the TOC of a disc image as JSON, in the shape of the sacd-spec manifest:
//! `{"outcome", "error", "warnings", "toc"}`.

use std::fs::File;
use std::io::BufReader;
use std::process::ExitCode;

use scarletbook::Disc;
use scarletbook_conformance::json;
use serde_json::{Value, json};

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let (Some(path), None) = (args.next(), args.next()) else {
        eprintln!("usage: scarletbook-toc IMAGE");
        return ExitCode::from(2);
    };
    let file = match File::open(&path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{}: {e}", path.to_string_lossy());
            return ExitCode::from(2);
        }
    };
    let out = match Disc::open(BufReader::new(file)) {
        Ok(disc) => json!({
            "outcome": "ok",
            "error": Value::Null,
            "warnings": disc.warnings().iter().map(json::warning).collect::<Vec<_>>(),
            "toc": json::toc(disc.toc()),
        }),
        Err(e) => match e.code() {
            Some(code) => json!({ "outcome": "reject", "error": code, "warnings": [], "toc": Value::Null }),
            None => {
                eprintln!("{}: {e}", path.to_string_lossy());
                return ExitCode::FAILURE;
            }
        },
    };
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    ExitCode::SUCCESS
}
