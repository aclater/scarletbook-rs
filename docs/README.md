<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
[scarletbook](../README.md) › Documentation

# Documentation

The guides for scarletbook, a clean-room reader of SACD images (`.iso`) for Rust and C. Start
with the [Quick start](../README.md#quick-start) in the README.

## Install

- [Rust guide: Installing](rust.md#installing): the crates on crates.io
- [Releases and verification](releases.md): the C library archives for each platform,
  checksums and attestations

## Use

- [Rust guide](rust.md): reading audio, stored frames, DSF and DFF (DSDIFF) files, seek
  indexes and provisional reading, features
- [C ABI guide](c-abi.md): the C library, seeking, output files, parallel decoding,
  cancelling, DSD to PCM and linking
- [Using from Python (ctypes)](python.md): bindings for every struct, and the whole TOC in
  one call
- [Migrating to 0.7.0](migrating-0.7.md) and [Migrating to 0.6.0](migrating-0.6.md): from an
  earlier release

## Reference

- [Stability and versions](stability.md): the 1.0 promise, versions and the minimum supported
  Rust version
- [ABI compatibility](c-abi.md#abi-compatibility): the rules for `SB_ABI_VERSION`
- [Rust and C names](c-abi.md#rust-and-c-names): where the two interfaces name the same
  thing differently
- [Release assets](releases.md#release-assets): the archives, their targets and contents
- [API documentation on docs.rs](https://docs.rs/scarletbook)

## Internals

- [Performance and benchmarks](performance.md): the benchmark program, criterion benchmarks
  and profiling scripts
- [Contributing and the clean room](contributing.md): the repository layout and changing the
  C ABI
- [Evidence record](provenance/EVIDENCE.md): the technique inventory and its dated,
  non-patent sources (#40)

## Project

- [CONTRIBUTING.md](../CONTRIBUTING.md) and [CLEANROOM.md](../CLEANROOM.md): the clean-room
  rule and its record
- [SECURITY.md](../SECURITY.md): reporting a vulnerability
- [CHANGELOG.md](../CHANGELOG.md): the changes in each release
- [RELEASING.md](../RELEASING.md): how a release is made

## See also

- [README](../README.md): platforms, Quick start and licence
