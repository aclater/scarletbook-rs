Fixed
- The `scarletbook` package includes `tests/common/`, which the unit tests load with `#[path]`; before, `cargo test --lib` on the unpacked crates.io tarball failed to compile (#268, #270).
