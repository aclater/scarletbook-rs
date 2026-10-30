# Fuzz targets

Requires nightly and `cargo-fuzz`. Seeds come from the conformance corpus:

```sh
cargo run -p scarletbook-conformance --bin scarletbook-seeds -- fuzz/corpus
cargo +nightly fuzz run toc fuzz/corpus/toc -- -max_total_time=60
```

| Target | Input |
|---|---|
| `toc` | the image from LSN 510 on (510 empty sectors are prepended) |
| `demux` | byte 0: the area channel count; then 2048-byte audio sectors |

The harness lives in `crates/scarletbook/tests/harness/mod.rs` and is shared with the
regression tests. A crash, once minimised (`cargo fuzz tmin`), goes into
`crates/scarletbook/tests/regressions/<target>/` and is replayed by `cargo test`.
