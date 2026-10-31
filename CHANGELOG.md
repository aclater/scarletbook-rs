# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). The version
is shared by every crate of the workspace; releases carry the `bench` program and, from
0.3.0, the C library, as binary assets. Nothing is published on crates.io yet.

## [Unreleased]

### Added

- `DecodeOn` and `TrackAudioCursor::set_decode_on` (`parallel` feature, #36): a cursor
  decodes on a rayon pool of its own, whatever thread reads it, or on the calling thread
  only; continuations keep the choice. The default stays the current pool.
- Parallel decoding through the C interface (#36): `sb_pool_new`, `sb_pool_threads`,
  `sb_pool_free` and `sb_track_audio_set_pool` decode a track or range handle's DST frames
  on a pool of the caller's own (N threads, 0 for one per CPU), with frames returned in
  order on the calling thread, at most `min(16 x N, 256)` positions ahead, and output
  identical to the calling thread's. Pools are reference-counted by the handles given
  them; the library starts no global pool. `SB_MAX_POOL_THREADS` and
  `SB_STATUS_THREADS_UNAVAILABLE` are new. Additive: the ABI version stays 1. The C smoke
  test also decodes every track on a pool and compares.

### Changed

- The C library now always links rayon; its `parallel` feature only makes handles without
  a pool decode on rayon's global pool, as before.

### Fixed

- The `scarletbook` package carries the licence texts and the README; its `include`
  patterns are anchored, so the test fixtures' READMEs no longer slip in (#48).

## [0.3.0] - 2026-10-31

### Added

- `DstDecoder::decode_many`: two to four independent DST frames decoded in lockstep on one
  thread, so that their arithmetic-decoder chains overlap (#31, #41).
- A `dist` cargo profile (release with one codegen unit) for the generic `bench` asset, and
  a labelled x86-64-v3 asset, gated by the correctness check (#32, #41).
- Criterion benchmarks (`dst_frames`, `pipeline`) and `perf/` scripts for cycles per
  symbol, flame graphs, A/B comparison, build configurations and PGO; CI compiles the
  benchmarks (#30, #41).
- Frame ranges (#35, #44): decoded audio of any frame range `[first, end)` of an area
  (specification 4, 4.7.1), the track-with-pause range of each track, and continuation
  across ranges; in C, `sb_range_check`, `sb_range_audio_open`, `sb_range_audio_open_next`,
  `sb_track_range_with_pause` and `SB_STATUS_FRAME_RANGE_INVALID`. Additive: the ABI version
  stays 1.
- C library release assets (#34, #42): on each tag, `scarletbook-ffi-<version>-<target>`
  archives (header, shared and static library) for Linux glibc 2.17+ and musl 1.2+ on x86_64
  and aarch64, macOS x86_64 (10.12+) and arm64 (11.0+), and Windows x86_64 (MinGW-w64, UCRT),
  each with a SHA-256 file and build provenance; a documented ABI compatibility policy,
  enforced in CI against `crates/scarletbook-ffi/abi/abi-1.txt`.
- Repository hygiene (#46): Dependabot, OpenSSF Scorecard, CodeQL, zizmor, dependency
  review and coverage workflows; `SECURITY.md`, `CONTRIBUTING.md` (with the clean-room
  rule) and this changelog; README badges; `readme` in the crate metadata.
- Diagnostic log (#38, #43): an optional per-disc log (`scarletbook::log`, `Disc::open_with_log`,
  `Disc::set_log`) of reads, sector-format detection, the TOC copies used, area scans, and
  sector and frame problems with their position, at five levels; off by default, with
  nothing formatted without a sink. In C, `sb_disc_set_log`, the `_with_log` variants of the
  three openers and `SB_LOG_OFF` to `SB_LOG_TRACE`; the callback may also run on worker
  threads. Additive: the ABI version stays 1.
- Stored content without decoding, for DSDIFF passthrough of DST audio (#37, #49):
  each position of a track or frame range as stored (DST bytes, plain bytes or missing,
  with the missing count), the area's stored frame list, and DST equivalents
  (specification 4, 4.6.1): `Disc::range_frames`, `stored_frame_list`, `dst_equivalent` and
  their cursor forms. In C, the `SbRawFrames` handle (`sb_raw_frames_open`, `_open_next`,
  `_read`, `_read_dst_equivalent`, `sb_stored_frame_list_open`, ...), `SbFrameKind`,
  `SbRawFrameInfo` and `SB_MAX_FRAME_BYTES`, with caller-provided buffers. Additive: the ABI
  version stays 1.

### Changed

- Release notes start with the tag's section of this changelog; a release without one
  stops (#50).
- `TrackAudioCursor` decodes groups of consecutive DST frames together, on the thread pool
  and on the calling thread: `bench decode` is 1.13 to 1.43 times faster on one thread,
  depending on build and channel count (#31, #41).
- The x86-64-v3 `bench` asset is renamed `bench-<version>-x86-64-v3-linux`, outside the
  benchmark asset patterns reserved by sacd-spec `34f78a3` (#45).
- The vendored conformance corpus is that of sacd-spec `fa602f9` (136 cases, all passing)
  (#45).
- CLEANROOM.md records the round 3 allowance for general references, the owner's
  decision B, the survey sources and a dated patent-hygiene table (#33, #40, #41).

## [0.2.0] - 2026-10-30

### Added

- Signed build provenance attestations for the release tarballs
  (`gh attestation verify`), for a public repository (#25, #26).

### Changed

- Parallel decoding: each DST frame goes to the thread pool as soon as it is read, with a
  window of up to 16 frames per thread (at most 256) ahead of the caller, so the pool no
  longer idles between batches; the caller helps with pending jobs while it waits. Memory
  stays bounded (about 24 MB) and results are unchanged (#21, #23, #24, #26).
- DST prediction uses per-byte phase tables: order-128 filters are faster (about 22% at
  2 channels, 36% at 6), order-32 filters up to 10% slower (#22, #26).
- CLEANROOM.md: a home directory path is redacted (#20).

### Fixed

- `sb_version()` returns the crate version instead of a hard-coded "0.1.0" (#26).

## [0.1.0] - 2026-10-30

First release: a clean-room reader for SACD images, written from sacd-spec, passing all
129 conformance corpus cases.

### Added

- Image reading: plain, raw-2064 and raw-2054 sector detection; master TOC, locales, disc
  text and the CJK character sets; area TOCs, track text and the spec 7 diagnostic codes
  as typed errors and warnings (#1).
- Audio sector demultiplexing (`Disc::area_frames`, `Disc::track_frames`,
  `Disc::check_track`) with the per-track checks of spec 4.8 (#2).
- A DST decoder written from spec 5, `Disc::track_audio` with the missing and
  invalid-frame fills, and the `parallel` feature (rayon) (#3).
- The C ABI, `scarletbook-ffi`: opaque handles, status codes, no panics across the
  boundary, a cbindgen-generated header and a C smoke test (#4).
- One pass per area across consecutive tracks (`sb_track_audio_open_next` in C),
  `master-toc-copies-differ` and `audio-sector-malformed` warnings as sacd-spec defines
  them, and more regression tests (#5 to #12, #13).
- `bench`, the benchmark program of sacd-spec `benchmarks/README.md`, with its correctness
  gate; a release workflow publishing generic x86-64 and AMD Zen 5 builds with SHA-256
  checksums; `scarletbook-synth` for large synthetic images (#14, #15, #18, #19).
- Fuzz targets `toc`, `demux` and `dst`, a weekly long fuzzing workflow, and CI on Linux,
  macOS and Windows with MSRV, Miri and cargo-deny checks (#1 to #13).

### Changed

- DST decoding about 3.8 times faster on one thread, and parallel decoding that overlaps
  demultiplexing with decoding within a bounded memory (#16, #17, #19).

[Unreleased]: https://github.com/aclater/scarletbook-rs/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/aclater/scarletbook-rs/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/aclater/scarletbook-rs/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/aclater/scarletbook-rs/releases/tag/v0.1.0
