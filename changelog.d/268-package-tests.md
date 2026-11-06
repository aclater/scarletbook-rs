Fixed
- `cargo test --lib` of the packaged `scarletbook` crate passes: the two dispatch tests that read the corpus are skipped, with a message, when it is absent (only in an unpacked crate; in the repository a missing corpus still fails), and the decoding tests that need more than the 2 MiB default stack at opt-level 0 run on a larger one. Tests only (#268, #315).
