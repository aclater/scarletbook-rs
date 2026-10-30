// SPDX-License-Identifier: MIT OR Apache-2.0

//! Every case of the sacd-spec conformance corpus.

use scarletbook_conformance::{check_case_report, corpus_dir, load_cases};

#[test]
fn corpus() {
    let dir = corpus_dir();
    let cases = load_cases(&dir).unwrap_or_else(|e| panic!("manifest: {e}"));
    assert_eq!(cases.len(), 129, "unexpected case count");
    let mut failed = Vec::new();
    let (mut compared, mut pending) = (0, 0);
    for case in &cases {
        let id = case["id"].as_str().unwrap_or("?");
        let report = check_case_report(&dir, case);
        compared += report.tracks_compared;
        pending += report.dst_pending;
        let fails = report.failures;
        if !fails.is_empty() {
            eprintln!("FAIL {id}:");
            for f in &fails {
                eprintln!("  {f}");
            }
            failed.push(id.to_owned());
        }
    }
    eprintln!("tracks with audio compared: {compared}; with DST frames, not yet decoded: {pending}");
    assert!(compared > 0);
    assert!(failed.is_empty(), "{} of {} cases failed: {failed:?}", failed.len(), cases.len());
}
