// SPDX-License-Identifier: MIT OR Apache-2.0

//! Every case of the sacd-spec conformance corpus.

use scarletbook_conformance::{check_case, corpus_dir, load_cases};

#[test]
fn corpus() {
    let dir = corpus_dir();
    let cases = load_cases(&dir).unwrap_or_else(|e| panic!("manifest: {e}"));
    assert_eq!(cases.len(), 129, "unexpected case count");
    let mut failed = Vec::new();
    for case in &cases {
        let id = case["id"].as_str().unwrap_or("?");
        let fails = check_case(&dir, case);
        if !fails.is_empty() {
            eprintln!("FAIL {id}:");
            for f in &fails {
                eprintln!("  {f}");
            }
            failed.push(id.to_owned());
        }
    }
    assert!(failed.is_empty(), "{} of {} cases failed: {failed:?}", failed.len(), cases.len());
}
