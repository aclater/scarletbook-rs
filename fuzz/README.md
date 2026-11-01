# Fuzz targets

Requires nightly and `cargo-fuzz`. Seeds come from the conformance corpus:

```sh
cargo run -p scarletbook-conformance --bin scarletbook-seeds -- fuzz/corpus
cargo +nightly fuzz run toc fuzz/corpus/toc -- -max_total_time=60
```

| Target | Input |
|---|---|
| `toc` | the image from LSN 510 on (510 empty sectors are prepended); an input that opens is also read through the C interface's `sb_disc_toc`, and its last 32 bytes choose a range, writer's choices, DST lengths and a raw ID3 tag whose DSF, DSDIFF and DSDIFF-DST layouts are checked (`harness/output.rs`) |
| `demux` | byte 0: the area channel count; then 2048-byte audio sectors. One input in 4096, chosen by a hash, is also converted to PCM: the bytes from byte 1 on as DSD frames of `byte 0 % 6 + 1` channels, the last three bytes the run length, gain and configuration; the whole run must equal it frame by frame and split in two, and the C interface's `sb_pcm_convert_run` must give the same bytes |
| `dst` | byte 0: the channel count (`byte % 6 + 1`); then one DST frame |
| `seek_index` | a saved seek index of a fixed synthetic area, without or with its checksum appended; the same bytes are also kept by a synthetic disc (`Disc::set_seek_index`, binding its TOC fingerprint) and read through speculatively, then verified |

The harness lives in `crates/scarletbook/tests/harness/mod.rs` and is shared with the
regression tests. A crash, once minimised (`cargo fuzz tmin`), goes into
`crates/scarletbook/tests/regressions/<target>/` and is replayed by `cargo test`.

## In CI

Every pull request fuzzes each target for 60 seconds (`ci.yml`), a smoke test. The
`Long fuzz` workflow (`fuzz-long.yml`) runs each target for 40 minutes on 4 workers, weekly
and on demand (`gh workflow run "Long fuzz" -f minutes=90`), and uploads crashing inputs
and the grown corpus as artifacts.
