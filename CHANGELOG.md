# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). The version
is shared by every crate of the workspace; releases carry the `bench` program and, from
0.3.0, the C library, as binary assets. Nothing is published on crates.io yet.

## [Unreleased]

## [0.5.0] - 2026-10-31

### Added

- The remaining TOC fields through the C interface (#104): `sb_disc_genre` (album and disc genres, `SB_GENRE_LIST_*`, `SbGenre`), `sb_disc_text_channel_count` and `sb_disc_text_channel` (language codes as stored and character set codes, `SbTextChannel`), `sb_disc_catalog_number_stored` (catalogue numbers up to the first 0x00, padding kept), `sb_disc_manufacturer` (the manufacturer data, `SB_MANUFACTURER_BYTES`), `sb_area_details` (version, maximum available channels, mute flags, track attribute and offset, text channel count and character set, `SbAreaDetails`) and `sb_area_text_channel`. Additive to ABI 2 (#104, #114).
- C interface: `sb_disc_read_sectors` copies the 2048-byte user data of a range of logical sectors, whatever the image's stored sector size, on every disc handle (clones and callback discs included), many sectors per source read; a range past the last sector fails whole with the new `SB_STATUS_SECTOR_RANGE_INVALID` (#105, #113).
- Cancelling and observing long reads (#106, part 1, #115): `CancelToken`, one per image and
  shared by every disc made from it (`Disc::cancel_token`, `Image::cancel_token`). While
  cancellation is requested, reads of audio sectors return the new `Error::Cancelled`
  (`ErrorClass::Cancelled`) within 32 sectors, the scan before a range's first frame, seek
  index replays, provisional searches, the stored frame list and seek index extension
  included. The cursor stays where it was and continues exactly once the request is
  withdrawn. `Progress`, from `TrackAudioCursor::progress`, `TrackCursor::progress`,
  `AreaCursor::progress` and their iterators: the sectors read so far and the sectors
  expected, readable from any thread, updated with every read of at most 32 sectors.
- Cancelling and observing long calls in the C interface (#106, part 2, #117):
  `SB_STATUS_CANCELLED` (-19); `SbCancel` from `sb_disc_cancel_token` (one per image,
  shared by a disc's clones), with `sb_cancel_request`, `sb_cancel_withdraw`,
  `sb_cancel_requested` and `sb_cancel_free`; `SbProgress` from `sb_track_audio_progress` and
  `sb_raw_frames_progress`, read with `sb_progress_get` (sectors read, sectors expected) and
  freed with `sb_progress_free`. Both may be used by several threads at once, without
  borrowing the disc or handle they come from. Additive on ABI 2. README section and ctypes
  bindings.
- Stored and decoded frames of one range in one pass (#107, part 1, #118):
  `TrackAudioCursor::keep_stored` (also on `TrackAudio`), then
  `TrackAudioCursor::next_frame_with_stored` / `TrackAudio::next_with_stored` give each
  position decoded, as `next_frame` does (on the cursor's pool, with the same positions held
  ahead), and its stored content, as `TrackCursor::next_frame` gives it, from one read of the
  area's sectors. The new `Stored` (a DST frame's bytes, `Plain` or `Missing`; a plain
  frame's bytes are the decoded data, not copied again) with `Stored::content` and
  `Stored::bytes`; `TrackAudioCursor::dst_equivalent`. Continuations keep the mode. The
  `pipeline` benchmark has `/stored` cases.
- Stored and decoded frames of one range in one pass in the C interface (#107, part 2, #119):
  `sb_track_audio_keep_stored`, then `sb_track_audio_read_with_stored` writes each position's
  decoded audio (as `sb_track_audio_read`) and its stored content (as `sb_raw_frames_read`,
  or its DST equivalent as `sb_raw_frames_read_dst_equivalent`) from one read of the area's
  sectors, decoding on the handle's pool; a raw buffer too small holds the position for the
  next call. Additive on ABI 2. README section and ctypes bindings.
- C interface: `sb_build_info()`, a static string of how the library was built (version, target, profile and optimisation level, enabled features, target CPU features, compiler, commit), recorded by a build script of the FFI crate; the release smoke test checks that it names the package's target and `sb_version()`'s version (#108, #116).

### Changed

- `Locale::language_code` and `MasterToc::album_catalog_number_stored` and `disc_catalog_number_stored` hold the stored bytes behind the trimmed language and catalogue numbers, and `text::fixed_bytes` gives a fixed text field's bytes up to the first 0x00 (#104, #114). `Locale` and `MasterToc` are not `#[non_exhaustive]`, so the new public fields are a breaking change for code that constructs them as struct literals; code that only reads them is unaffected.

## [0.4.1] - 2026-10-31

### Added

- Provisional reading in the C interface (#94, part 3; specification 4, 4.2 item 8,
  sacd-spec `82eac6c`): `sb_disc_seek_policy_set` with `SB_SEEK_POLICY_EXACT` (the default)
  or `SB_SEEK_POLICY_SPECULATIVE` and `SB_SEEK_DEFAULT_MIN_GAP`; the `SbSpeculation` struct
  (40 bytes, no padding) and `SB_SPECULATION_*` states, filled by
  `sb_track_audio_speculation` and `sb_raw_frames_speculation`; verdicts through
  `sb_disc_seek_verify` and `sb_disc_seek_event_next` (a queue shared by a disc's clones),
  and for caller-kept indexes `sb_seek_index_verify` and `sb_seek_index_event_next`.
  `TrackCursor::slot` and `TrackAudioCursor::slot` in Rust; `AreaIndex::verify` refuses an
  image of another sector count. Additive on ABI 2. README ctypes bindings for them.
- Cold-seek measurements (#94, part 4; fixes #94): `scarletbook-seekcost` also measures
  provisional reading (no seek index, `SeekPolicy::Speculative`) against the range's position:
  read calls, sectors, time, a network model (`--rtt-ms`, default 0.5; `--mbps`, default 110)
  and whether the frames equal the exact ones. On a 50-minute 2-channel DST image: 4 to 7 read
  calls and 80 to 139 sectors wherever the range lies (3.5 to 5.6 ms modelled), against up
  to 470 272 sectors without an index. `.github/scripts/clib-seekcost.py` measures the same
  through the C interface (`sb_disc_open_callbacks` with counting callbacks, from ctypes):
  4 to 7 read calls against up to 14 696; the C package test runs it.
- C interface tests of provisional reading: on every well-formed corpus image a provisional
  range read equals an exact one (frames, statuses, missing and invalid counts) and is
  confirmed; a refutation reaches the C caller with `exact_from`, and an exact re-read below
  it completes a straight read; a clone checking readings on another thread settles the
  original handle's reading and queues the event on the original disc. The C package smoke
  test reads provisionally through the README's ctypes bindings and checks the readings with
  a disc-kept and a caller-kept seek index.

### Fixed

- A caller-kept seek index could confirm a provisional reading against another image of the
  same size and layout (#102, in 0.4.0): it took the reading without binding to the image the
  reading had read, so a later `AreaIndex::extend` or `AreaIndex::verify` with the other image
  checked it against that image's state (specification 4, 4.2 item 8). Taking a provisional
  reading now binds an index that has no image yet to the reading's image, and an index of
  another image refuses it; a reading also remembers its image and is never checked against
  another (it becomes `Speculation::Unverified`, for example after `Disc::set_seek_index`
  moves an index to a disc of another image).

## [0.4.0] - 2026-10-31

### Added

- Loudspeaker layouts (#75): `Area::loudspeaker_layout` gives an area's `LoudspeakerLayout`
  from its channel count and the low 3 bits of its configuration byte, as specification 3,
  3.4.1 defines them (stereo, 5-channel 3/2 and 5.1), and `Unspecified` with both fields as
  stored for any other combination. `Area::channel_speakers` gives the `Speaker` of each
  channel in decoded-frame order for the defined layouts; the order is informative. A
  corpus test checks every area's layout against the manifest.
- `DecodeOn` and `TrackAudioCursor::set_decode_on` (`parallel` feature, #36): a cursor
  decodes on a rayon pool of its own, whatever thread reads it, or on the calling thread
  only; continuations keep the choice. The default stays the current pool.
- Parallel decoding through the C interface (#36): `sb_pool_new`, `sb_pool_threads`,
  `sb_pool_free` and `sb_track_audio_set_pool` decode a track or range handle's DST frames
  on a pool of the caller's own (N threads, 0 for one per CPU), with frames returned in
  order on the calling thread, at most `min(16 x N, 512)` positions ahead, and output
  identical to the calling thread's. Pools are reference-counted by the handles given
  them; the library starts no global pool. `SB_MAX_POOL_THREADS` and
  `SB_STATUS_THREADS_UNAVAILABLE` are new. Additive: the ABI version stays 1. The C smoke
  test also decodes every track on a pool and compares.
- A test of several discs decoded at once through the C interface (#39): 8 threads, each
  with a disc of its own opened from memory or from a file, decode every track on the
  calling thread or on a shared pool and match the conformance manifest; each thread's
  error message stays its own, and a track handle is refused with another thread's disc.
- Documented C struct layouts (#39): each struct of `scarletbook.h` states its size,
  alignment and padding on 64-bit targets, which a test checks field by field, and the
  header's conventions state the C types used (fixed-width integers, `size_t` for lengths,
  `bool`, the int-sized `SbStatus`, no variadic functions). Comments only: the ABI is
  unchanged.
- README "Using from Python (ctypes)" (#39): the ABI version check at load, the six
  structs and the `argtypes`/`restype` of the main functions, a decoding example, and the
  rules for threads, error messages, freeing and callbacks. The ctypes smoke test of every
  release package runs these bindings and checks them against the header.
- CI runs the `parallel` decoding path under Miri with Tree Borrows (#28): the C
  interface's pools and the read-error test on pull requests, plus the continuation
  property test and the window cap on `main`.
- Seek index measurements and corpus checks (#68, part 5): `scarletbook-seekcost` prints
  the cost of opening a range against its position with and without an index (flat with
  one: 32 to 96 sectors and 2.3 to 2.4 ms on a 50-minute DST area, against up to 470 272
  sectors and 146 ms without). Conformance tests read every corpus area's tracks, tracks
  with pause and random ranges through indexes (K = 1 and 64), fresh and continued, and on
  two threads at once through a clone and a share of one disc keeping one index, against
  plain reads.
- Seek indexes kept by a `Disc` (#68), opt-in per area: `Disc::enable_seek_index`,
  `set_seek_index`, `take_seek_index`, `seek_index`, `extend_seek_index` and
  `absorb_seek_checkpoints`. With one enabled, every track and range cursor the disc makes
  starts from it, its borrowing iterators and continuations hand recorded checkpoints back
  as they read, and a continuation resumes from the index instead of the area's first
  sector when it cannot reuse the earlier cursor (or jumps ahead when the index is more than
  two checkpoints ahead of it). Results are those without an index; the opening is logged
  "from the seek index" at Info, the resume point at Debug. The indexes live in the state
  that discs of one image share (`Disc::share`, `clone`, #74): every disc of the image, on
  any thread, uses and grows the same index (a read lock to make a cursor, a write lock to
  take checkpoints); only cursors that read that image give it checkpoints.
  `Continuation::log_audio` and `log_frames` now return the `Opening` they log (the
  earlier scan continues, a start from the index, or a new scan), which `audio_as` and
  `frames_as` follow, so that the log and the cursor agree. README section "Seeking within
  an area".
- Seek indexes in the C interface (#68, part 4): the `SbSeekIndex` handle (`sb_seek_index_new`,
  `_extend`, `_frontier`, `_memory`, `_merge`, `_save`, `_load`, `_free`),
  `sb_range_audio_open_indexed`, `sb_range_audio_open_next_indexed`,
  `sb_raw_frames_open_indexed`, `sb_raw_frames_open_next_indexed`, `sb_track_audio_absorb`
  and `sb_raw_frames_absorb`, and `SB_SEEK_DEFAULT_INTERVAL`, `SB_SEEK_MAX_INTERVAL` and
  `SB_SEEK_DEFAULT_INLINE_BUDGET`; `sb_seek_index_load` takes the caller's inline budget.
  A disc can keep the index itself (`sb_disc_seek_index_enable`, `_set`, `_get`,
  `_disable`, `_extend`), shared with its clones (`sb_disc_clone`): handles opened through
  any of them start from it and their reads build it. Additive: the ABI version stays 1. The `_open_next`
  forms log while `prev` is still borrowed and run no callback once it is taken (#43).
- Seeking within an area (#68, #73): `AreaIndex` keeps the reading state of an area every
  `K` sectors (default 64), and `TrackCursor::for_range_indexed` /
  `TrackAudioCursor::for_range_indexed` start a range at its last checkpoint whose next
  acceptable frame number is at most the range's first frame, instead of at the area's
  first sector (sacd-spec `da6b900`: specification 4, 4.2 item 7, 4.4.5 items 2 and 4, 4.5
  item 6 and 4.7.1 item 6, and 7.1 items 2 and 3). Results, missing counts, malformed
  counts and warnings equal those of a scan from the first sector; only reads and log lines
  differ. Once the index covers a range, opening it reads at most `W + K` sectors (`W` = 32)
  before the range's own frames, wherever it lies. Indexes are built lazily by the cursors
  made from them (`AreaIndex::absorb`) or eagerly in steps (`AreaIndex::extend`). Frames
  under assembly spread over more than `W` sectors are stored up to an inline budget
  (default 1 MiB). Opt-in; no change to existing cursors.
- Saved seek indexes (#68, #83): `AreaIndex::to_bytes` and `AreaIndex::from_bytes` (a
  versioned little-endian format with the area's key and a checksum; every count, bound and
  invariant is checked before anything is allocated, so untrusted bytes give an error; the
  loaded inline budget is at most the caller's, stored frames beyond it are replayed instead),
  and `AreaIndex::merge` for indexes built apart. A cursor compares each checkpoint's sector
  digest with the sector it reads there, and on a mismatch (a stale index, or one of another
  image of the same size) discards the index, logs a warning and scans from the area's first
  sector (`ResumedFrom::Discarded`); `extend` on such an index fails. A `seek_index` fuzz
  target loads untrusted index bytes and reads through them.
- Seek index refinements (#68, review of #73): the checkpoints within one frame under
  assembly share one stored buffer (one frame's bytes, however many checkpoints it spans);
  a read error during a replay no longer counts replayed malformed sectors twice;
  `AreaIndex::absorb` takes any `SeekCursor` (a sealed trait of `AreaCursor`,
  `TrackCursor` and `TrackAudioCursor`) instead of `AsRef<AreaCursor>`; the documentation
  says what the sector digest catches and that a cursor continued backwards without its
  index scans from the area's first sector.
- Positional reads (#74): `ReadAt` (a source read at an offset through a shared reference: `File` with `pread` on Unix and `seek_read` on Windows, `[u8]`, `Vec<u8>`, `&T`, `Box<T>`, `Arc<T>`, and `read_at::Locked` for any `Read + Seek` reader, serialised) and `ReadAtReader`, a `Read + Seek` reader with a position of its own over such a source, so that readers of one shared file or buffer can run on several threads at once.
- Discs of one image on several threads (#74): `Disc::share` and `Disc::share_with_log` open the same image through another reader, checked against the image's length and its TOC sectors, and `clone` does so for a disc read through a `ReadAtReader` with no I/O. They share the parsed TOC, warnings, sector format and image identifier, so a cursor made or read through one reads through any other; reads, bounds and errors are those of one reader. `Image::id` is now shared by such discs.
- `sb_disc_clone` and `sb_disc_clone_with_log` (#74): a clone of a disc opened from a file or from memory, with a reading position of its own, for reads of one image on several threads at once; track and raw frames handles may be read and continued through any clone of their disc. Discs opened from a file now read it at offsets through one shared handle. Additive: the ABI version stays 1.
- `sb_disc_open_callbacks_at[_with_log]` and `sb_disc_clone_callbacks[_with_log]` (#74): discs read through a positional read callback (`SbReadAtFn`, which may be called from several threads at once), which can be cloned, and clones of any disc through new read and seek callbacks. Additive: the ABI version stays 1.
- Loudspeaker layouts through the C interface: `sb_area_loudspeaker_layout` gives an area's `SB_LAYOUT_*` (stereo, 3/2, 5.1, or `SB_LAYOUT_UNSPECIFIED` for a combination specification 3, 3.4.1 does not define) and `sb_area_channel_speaker` the `SB_SPEAKER_*` of each channel in decoded-frame order (`SB_SPEAKER_UNSPECIFIED` without a defined layout). Additive: the ABI version stays 1 (#75).
- Provisional reading, cold seeks without a prior pass (#94, part 1; specification 4, 4.2
  item 8, 4.4.5 item 5 and 7.1 item 3, sacd-spec `82eac6c`): `SeekPolicy` (`Exact`, the
  default, or `Speculative(SpeculativeOptions)`), set per disc handle with
  `Disc::set_seek_policy` (clones and shares start with a copy), and the caller-kept forms
  `TrackCursor::for_range_speculative` and `TrackAudioCursor::for_range_speculative`. When
  an exact start would read more than `min_gap_sectors` before a range, a cursor allowed to
  searches the frame records' timecodes (interpolation with a bisection safeguard, a few
  small reads, at most a constant times `log2(n + 1)` sectors) and begins at a checkpoint
  boundary before the range's first frame `u`, with no frame under assembly and `u` as the
  next acceptable frame number. Its positions are marked provisional
  (`ResumedFrom::Speculative`, `Speculation::Pending`): they are exact when no frame numbered
  `u` or more was accepted before `L` and the frame under assembly at `L` is none or numbered
  below `u`, which holds on well-formed discs but cannot be established without reading the
  sectors before `L`. Its malformed count and warnings begin at `L` (`counts_from`).
  Verification and confirmation come in part 2.
- Checking provisional readings (#94, part 2; specification 4, 4.2 item 8, sacd-spec
  `82eac6c`). A seek index takes the provisional readings of cursors made from it (through a
  disc's iterators and continuations, `Disc::absorb_seek_checkpoints` or
  `AreaIndex::absorb`) and checks them once its exact checkpoints reach their `L`
  (`AreaIndex::verify`, which `AreaIndex::extend`, `Disc::extend_seek_index` and the new
  `Disc::verify_speculation` run): a reading is `Speculation::Confirmed` when (a) and (b)
  hold, and `Speculation::Refuted` ("possibly different") otherwise, with the frame from
  which its positions are exact in any case (at least the specification's `a*`). Verdicts
  reach the cursors at once (`speculation()`, `speculation_id()`) and an event queue
  (`Disc::take_seek_events`, shared by a disc's clones and shares; `AreaIndex::take_events`),
  and refutations are logged as warnings. At most `MAX_CLAIMS` (256) readings wait per
  index (in `merge` too); older ones are dropped as `Speculation::Unverified`, as is a reading
  whose checkpoint's sector digest does not match the image (a stale or foreign index never
  confirms). At most `MAX_EVENTS` (1 024) verdicts wait per queue. A provisional reading records
  provisional checkpoints, kept apart from the exact ones (not saved, used only by
  provisional starts), from which later provisional starts nearby begin without a search.

### Changed

- A cursor continued or dropped with DST frames still queued on the pool no longer has
  them decoded for nothing: the abandoned jobs skip decoding (#29).
- With `parallel`, a cursor holds up to 512 positions ahead instead of 256
  (`MAX_PARALLEL_BATCH`; still 16 per pool thread, so pools of up to 16 threads are
  unchanged). Since 0.3.0 a job decodes 2 or 3 frames together, so at 32 threads a cap of
  256 left under 3 jobs per thread and the pool ran short of work (#61). On a 16-vCPU
  machine with 32 threads, synthetic 10-minute 2- plus 6-channel images decode 4-9 %
  faster (generic and znver5 builds); the most held is about 48 MB instead of 24 MB.
- The C library now always links rayon; its `parallel` feature only makes handles without
  a pool decode on rayon's global pool, as before. On x86_64 Linux (`dist` profile) the
  shared library grows from 2.01 MB to 2.17 MB (1.85 MB to 1.97 MB stripped) and the
  static library from 25.2 MB to 25.6 MB.
- CI: fuzz targets run as parallel jobs (30 s each on pull requests, 60 s on main), Miri runs as parallel slices with the long core check split by frame count, and a newer push to main cancels an older run instead of queuing behind it (#76).
- Change records: each pull request adds `changelog.d/` and `cleanroom.d/` fragments, folded into CHANGELOG.md and the CLEANROOM.md Log when a release is prepared (`.github/scripts/fragments.py`), so that independent pull requests no longer conflict (#76).
- CI: the long Miri slices (core unit tests, fast-loop checks, long parallel-path tests) move to `miri-full.yml`, which runs on every push to main, nightly and on demand, and queues instead of cancelling, so later merges can no longer keep them from completing (#84).
- A seek index is of one image (#90): within a process, an `AreaIndex` (and an
  `SbSeekIndex`) is of the image of the first cursor it takes checkpoints from or the first
  `extend`, and refuses cursors, reads and indexes of any other image of the same size and
  layout with `Error::InvalidArgument` (`SB_STATUS_INVALID_ARGUMENT`; a refused cursor stops
  recording), so that two images can no longer mix their state in one index. The identity
  is not saved: a loaded index takes the image of its first absorb or extend.
  `AreaIndex::absorb` takes the cursor by `&mut` and drops what it took from it;
  `sb_track_audio_absorb` and `sb_raw_frames_absorb` do likewise. Docs: keep one index per
  image; loading is safe, not exact.
- **Breaks the C ABI: `SB_ABI_VERSION` 2** (#39). Every declaration now has the same size and layout on every target: functions return `SbStatus` as an `int32_t` (in C23 and C++ an enumeration with that underlying type) instead of `enum SbStatus`, and the other enumerations are `uint32_t`; the flags `SbDiscInfo.hybrid`, `SbDiscInfo.manufacturer_sector`, `SbTrackInfo.has_genre` and the `*complete` out-parameters of the seek index functions are `uint8_t` instead of `bool`; `SbFrameInfo.size` and `SbRawFrameInfo.size` are `uint32_t`, and the seek index's `inline_budget` arguments, `SB_SEEK_DEFAULT_INLINE_BUDGET` and `sb_seek_index_memory`'s `*bytes` are `uint64_t`, instead of `size_t`, which remains only for caller buffer lengths; no struct has padding (`SbDiscInfo` starts with `sectors` and is 32 bytes, `SbTrackInfo` gains `reserved[3]`, `SbWarning` is ordered `lsn`, `slot`, `track`, `code`, `SbFrameInfo` is 16 bytes and `SbRawFrameInfo` 12). `sb_dst_decode` refuses input longer than `SB_MAX_FRAME_BYTES` with `SB_STATUS_INVALID_ARGUMENT`. README, "ABI compatibility", has the migration notes from ABI 1.

### Removed

- `scarletbook.h` no longer includes `<stdbool.h>`, since no declaration uses `bool` (C ABI 2, #39).

### Fixed

- `sb_disc_open_memory` returns the new `SB_STATUS_OUT_OF_MEMORY` (-17) instead of aborting
  the process when the copy of the caller's image cannot be allocated (#39). Additive: the
  ABI version stays 1.
- The `scarletbook` package carries the licence texts and the README; its `include`
  patterns are anchored, so the test fixtures' READMEs no longer slip in (#48).
- A call into the C library from a thread-local destructor, after the library's per-thread
  error message was destroyed while the thread exits, aborted the process (a panic outside
  `guard`); it now returns its status, and `sb_last_error_message` returns an empty
  string (#39). A new test checks, on the source, that every function the header declares
  runs under `guard` or is on a short list of functions that cannot panic.

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

[Unreleased]: https://github.com/aclater/scarletbook-rs/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/aclater/scarletbook-rs/compare/v0.4.1...v0.5.0
[0.4.1]: https://github.com/aclater/scarletbook-rs/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/aclater/scarletbook-rs/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/aclater/scarletbook-rs/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/aclater/scarletbook-rs/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/aclater/scarletbook-rs/releases/tag/v0.1.0
