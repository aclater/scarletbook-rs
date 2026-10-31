// SPDX-License-Identifier: MIT OR Apache-2.0

//! Every case of the sacd-spec conformance corpus.

use scarletbook_conformance::{check_case_report, corpus_dir, load_cases};

#[test]
fn corpus() {
    let dir = corpus_dir();
    let cases = load_cases(&dir).unwrap_or_else(|e| panic!("manifest: {e}"));
    assert_eq!(cases.len(), 136, "unexpected case count");
    let mut failed = Vec::new();
    let mut compared = 0;
    let mut ranges = 0;
    let mut stored = 0;
    for case in &cases {
        let id = case["id"].as_str().unwrap_or("?");
        let report = check_case_report(&dir, case);
        compared += report.tracks_compared;
        ranges += report.ranges_compared;
        stored += report.stored_compared;
        let fails = report.failures;
        if !fails.is_empty() {
            eprintln!("FAIL {id}:");
            for f in &fails {
                eprintln!("  {f}");
            }
            failed.push(id.to_owned());
        }
    }
    eprintln!(
        "tracks with audio compared: {compared}, frame ranges compared: {ranges}, stored content compared: {stored}"
    );
    // Every track the manifest expects audio for.
    assert_eq!(compared, 457);
    // Every play range and track with pause the manifest expects audio for (outcome
    // `ok`): of 205 play ranges and 737 tracks with pauses, the rest are skipped or
    // refused, which `check_range` compares by code.
    assert_eq!(ranges, 662);
    // Corpus README step 7, in the 203 areas with `stored_frames`: their stored frame
    // lists (203) and the stored content of every `ok` track (457), play range (198) and
    // track with pause (464); and the 2 areas without, whose lists are refused with
    // channel-count-unsupported.
    assert_eq!(stored, 1324);
    assert!(failed.is_empty(), "{} of {} cases failed: {failed:?}", failed.len(), cases.len());
}
