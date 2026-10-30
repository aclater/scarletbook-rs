# scarletbook-rs

A clean-room, permissively licensed Rust library for reading Super Audio CD
(Scarletbook) disc images, written only from the specifications and conformance
corpus of [aclater/sacd-spec](https://github.com/aclater/sacd-spec).

Work in progress, built in stages (one pull request each): TOC and sector detection,
audio sector multiplexing, DST decoding, C FFI. The clean-room record is in
[CLEANROOM.md](CLEANROOM.md).

## Layout

| Path | Content |
|---|---|
| `crates/scarletbook` | the library (`#![forbid(unsafe_code)]`, no I/O beyond the reader it is given) |
| `crates/scarletbook-conformance` | the corpus runner and `scarletbook-toc`, which prints a TOC as manifest-shaped JSON |
| `corpus/` | the sacd-spec conformance corpus, copied unchanged (CC0-1.0) |
| `fuzz/` | cargo-fuzz targets, seeded from the corpus |

```sh
cargo test --workspace                      # includes every corpus case
cargo run -p scarletbook-conformance --bin scarletbook-toc -- corpus/images/hostile-base.iso
```

Minimum supported Rust version: 1.85.

## Licence

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
