# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). The version
is shared by every crate of the workspace, except `scarletbook-pcm`, which stays at 0.7.0 from
1.0.0 (#398); releases carry the `bench` program and, from
0.3.0, the C library, as binary assets. The crates are on crates.io from 0.5.7 (`scarletbook-ffi`
from 0.5.8).

## [Unreleased]

## [1.2.0] - 2026-11-25

**1.2.0 reads damaged discs: sectors can be marked unreadable, and the frames that touch them are reported lost (#435, #439); ranges opened past them resume from the state of the read that met them (#441, #442).** It is a semver minor release: the Rust API of `scarletbook` and ABI 2 of the C library gain items, nothing is removed, and a disc with no unreadable sectors reads exactly as before. One behaviour changes, for C read callbacks that return -2 (below). Rust code that builds against 1.1.1 builds against 1.2.0 with only the version changed; C programs need no change (`SB_ABI_VERSION` 2, and the header only gains declarations).

- **Unreadable sectors, for Rust users.** `Disc::unreadable_sectors()` (and `Image::unreadable_sectors()`) returns the disc's `UnreadableSectors` set, shared by its clones and shares, with `insert(lsn, count)`, `remove`, `clear`, `contains`, `is_empty` and `ranges`. A reader or `ReadAt` source can instead report sectors as it reads them, by returning `read_at::unreadable_error()`. Every timeline position whose frame has a byte in a marked sector, or starts in one, is then lost: `FrameStatus::Lost` in decoded reads (filled with 0x69), `TrackFrame::is_lost()` with `FrameContent::Missing` in stored reads, and `AreaFrame::lost_from` in the stored frame list. Cursors and iterators count them with `lost_frames()`, apart from `missing_frames()`. A lost frame is never decoded from the part of it that could be read; reading resumes at the next frame that starts in a readable sector, at its own frame number, and every other position is as it would be without the marks. `Image::read_sectors` and `AreaIndex::extend` stop at a marked sector with `Error::SectorUnreadable`, and a seek index's own checkpoints and hints never come from past one (its damage record, below, holds that state). [`docs/rust.md`](docs/rust.md#unreadable-sectors) has the details.
- **Unreadable sectors, for C users.** `sb_disc_unreadable_add`, `sb_disc_unreadable_remove` and `sb_disc_unreadable_range` mark, unmark and list sectors; a read callback can return `SB_READ_UNREADABLE` (-2) for bytes it cannot read. Lost positions are `SB_FRAME_STATUS_LOST` in `sb_track_audio_read` (filled with 0x69) and `SB_FRAME_KIND_LOST` in `sb_raw_frames_read`, and the stored frame list gets one `LOST` entry for each; `sb_track_audio_lost` and `sb_raw_frames_lost` count them, outside the missing counts. `sb_disc_read_sectors` and the seek index extension calls stop at a marked sector with `SB_STATUS_SECTOR_UNREADABLE` (-22). [`docs/c-abi.md`](docs/c-abi.md#unreadable-sectors) has the details. ABI 2 gains five functions, the `SB_READ_UNREADABLE` constant and three enum values.
- **Behaviour change: -2 from a read callback now means "unreadable".** In 1.1.1 and before, a read callback (`SbReadFn`, `SbReadAtFn`) that returned -2 failed the read with `SB_STATUS_IO`, like -1. From 1.2.0 it means that the bytes asked for cannot be read: the library reads the request again one sector at a time, marks each sector that still fails, and reads on, reporting the frames that touch those sectors as lost. The documented error value was always -1; a callback that returns -2 for other failures should return -1.
- **A known read failure is tried once, not repeated.** A request that the reader or callback reports unreadable is never made again: it is split into single sectors at once, a single sector that fails is marked and not retried, and a marked sector is never asked for. `Image::read_sectors` makes one request and then the single sectors. On a drive, where each failed read can take seconds, a damaged stretch costs one failed request for the whole read and one for each bad sector.
- **Ranges opened past a marked sector resume from the read that met it (#441, #442).** A seek index now also keeps a damage record: the state of the scans that met unreadable sectors, at its checkpoints past the first of them, each keyed by the marked runs that scan processed before it. A range opened past a marked sector resumes from the recorded state of the scan that met it when the marks before that point are the same as when it was recorded. The first range past a new mark, or past marks that changed before a recorded point, still scans from the area's start, and records the state again; sequential reads continued with `continue_*` or `*_open_next` read on as before. What ranges return is unchanged: every resumed read equals a scan from the area's start with the same marks (a property test of 50 000 cases on hostile and well-formed areas). The record is kept by `AreaIndex::absorb` and the disc's own index, and saved with the index. Read calls for 75-frame ranges, all past one sector marked at a quarter of the area, opened one after another (a counting reader, DST frames of 1500 to 6000 bytes, `K` = 64, the index built complete before the mark; I/O counted, not timed):

  | area | range | before #442: calls / sectors | after: calls / sectors |
  |---|---|---|---|
  | 1500 frames | the first past the mark (at 0.50) | 49 / 1535 | 49 / 1535 |
  | | three later ones beyond earlier reads (0.60, 0.75, 0.90) | 57-83 / 1791-2623 | 7-12 / 321-482 |
  | | three inside earlier reads (0.30, 0.55, 0.80) | 32-75 / 991-2367 | 3-4 / 193-225 |
  | 6000 frames | the first past the mark (at 0.50) | 178 / 5663 | 178 / 5663 |
  | | three later ones beyond earlier reads | 212-316 / 6751-10079 | 32-50 / 1121-1697 |
  | | three inside earlier reads | 109-281 / 3455-8959 | 3 / 193-194 |

  So later ranges past the mark went from 57-316 calls to 7-50, and to 3 or 4 inside what earlier reads covered, where a range costs about what it costs from an index with no marks.
- **Compatibility of saved seek indexes.** An index saved with no damage record (no marks were met, or `AreaIndex::without_damage()` dropped it) is byte-identical to what 1.1.x saves, and every earlier saved index loads unchanged (checked against two indexes saved by 1.1.1). An index saved **with** a damage record sets flags bit 2 and carries one more section, and 1.1.x refuses it with `InvalidArgument("seek index flags invalid")` (`SB_STATUS_INVALID_ARGUMENT` in C), as it refuses any flag it doesn't know. If saved indexes are handed to 1.1.x readers, save `AreaIndex::without_damage()` in Rust. C callers have no equivalent yet (`sb_seek_index_save` saves the record when it holds one); that is pending #443.
- **Open specification point: sacd-spec#55.** How unreadable audio sectors are handled is this library's own derivation from the specification (chapter 4: 4.4.3 item 5, 4.4.2, 4.5, 4.2 items 7 to 9), proposed to it as sacd-spec#55, which is still open; the code is marked `TODO(sacd-spec#55)`. pysacd's needs on #435 are met by this release whatever the specification settles.
- **Checked against 1.1.1:** `cargo semver-checks --baseline-rev v1.1.1 --release-type minor` finds no semver update required for `scarletbook` (default and all features); its one new item since #439 is `AreaIndex::without_damage`. For `scarletbook-ffi` it reports one lint, `enum_variant_added`, for the three Rust enums that mirror the C enums gaining values (`SbStatus::SectorUnreadable`, `SbFrameStatus::Lost`, `SbFrameKind::Lost`); the Rust items of `scarletbook-ffi` are outside the 1.x promise, whose interface is the C ABI ([`docs/stability.md`](docs/stability.md)). `abi-check.py` passes with `SB_ABI_VERSION` 2, and `abi/abi-2.txt` only gains lines since v1.1.1.
- **No benchmark table.** Output with no sectors marked is unchanged: the existing test suite, the conformance corpus among it, passes unchanged, and `no_marks_changes_nothing` checks that a disc whose marks were added and then removed reads exactly as one never marked. On that path the per-sector check is one atomic load (the set's "any marks" flag), so no throughput change is expected; a short timed check was not taken for this release (the benchmark host was busy), and 1.0.0's table stands.
- **Not measured:** real damaged discs and drives (the tests use zero-filled, marked sectors of generated images and readers that fail on chosen sectors), timings with marks set (#442's figures above are read counts), and the C interface's timings.

### Added

- Unreadable sectors: a reader can mark sectors as unreadable (`Disc::unreadable_sectors`, `UnreadableSectors`; `sb_disc_unreadable_add`, `_remove` and `_range`) or report them from a reader or read callback (`read_at::unreadable_error`, `SB_READ_UNREADABLE`). Every position whose frame touches such a sector is reported lost (`FrameStatus::Lost`, `TrackFrame::is_lost`, `SB_FRAME_STATUS_LOST`, `SB_FRAME_KIND_LOST`) and counted apart (`lost_frames`, `sb_track_audio_lost`, `sb_raw_frames_lost`), never decoded from part of its data; reading resumes at the next readable frame with its own timecode, in decoded, raw-frame and stored-frame reads, from files and callbacks alike. `Image::read_sectors` and seek index extension stop at such a sector with `Error::SectorUnreadable` / `SB_STATUS_SECTOR_UNREADABLE`. Nothing changes for a disc with no unreadable sectors. ABI 2 gains functions and enum values (#435, #439).
- Seek indexes keep the reading state of the scans that met unreadable sectors, each point keyed by the marked runs before it, and save it with the index: a range opened past a marked sector now starts near its first frame once a read has passed that point with the same marks, instead of scanning the area from its first sector every time. What ranges return is unchanged. Adding or removing a mark before a point makes the state there unused until a read records it again. `AreaIndex::without_damage` drops that state (#441, #442).

### Changed

- A read callback (`SbReadFn`, `SbReadAtFn`) that returns -2 now means "these bytes are unreadable" (`SB_READ_UNREADABLE`): the library rereads the request one sector at a time, marks each sector that still fails as unreadable and reads on, reporting the frames that touch it as lost, where before -2 failed the read with `SB_STATUS_IO` like -1. The documented error value was always -1; a callback that returns -2 for other failures should return -1 (#435, #439).
- A saved seek index that holds reading state past unreadable sectors sets flags bit 2 and carries one more section before its checksum. Readers before 1.2.0 refuse it ("seek index flags invalid"), as any index they cannot use; save `AreaIndex::without_damage` for them. An index without that state saves exactly as before, and every earlier saved form loads unchanged (#441, #442).

## [1.1.1] - 2026-11-25

**1.1.1 is a bug-fix release: two fixes to provisional seeking, no API or ABI change.** Both change only which sectors a `SeekPolicy::Speculative` reading reads, never the frames it returns. Rust code that builds against 1.1.0 builds against 1.1.1 with only the version changed; C programs need no change (`SB_ABI_VERSION` 2, the header is unchanged).

- **No more exact-scan fallback in the speculative search (#434, #436).** Where the bit rate rises, the search's read back from its first probe could start just past the frame before the one wanted, and the reading then fell back to an exact scan of the area. The search now goes on below that read. On #434's image (6 minutes, 2 channels, uneven DST bit rate), the 7-frame range from frame 18443 took 1,113 reads and 72.9 MB, and now takes 5 reads and 194,560 bytes. A sweep of every frame of 8 synthetic images (plain DSD and DST, regular and uneven, 2, 5 and 6 channels; ranges of 1, 7 and 75 frames) found 130 exact-scan fallbacks before at each range length and none after, and the worst 7-frame seek on #434's image fell from 1,113 reads / 72.9 MB to 65 reads / 4.3 MB (a range within `min_gap_sectors` of the area's start, read exactly by design). Speculative still equals Exact for every frame of every image in the sweep.
- **A track's head now begins at the track's start (#431, #437).** A provisional reading of a range that begins at a track's first frame, opened from a saved seek index, began at the nearest seek hint below it, which after a remount is often where the previous track's tail read began. It now begins at the track's start LSN from the TOC, with the same single read call. Replaying sacdfs's head-then-tail sequence on synthetic images, pass 2 (index loaded) reads 19.64 MB, against 25.75 MB on 1.1.0 and 23.54 MB on 0.6.2, at the same 215 read calls and with the same frames. On sacdfs's real library (7,519 DSF files, head and tail of each), image bytes went from +7.9 % over 0.6.2 (1.0.2 and 1.1.0) to −8.6 % with #437, with identical output. This resolves 1.1.0's "Known item (#431)" for bytes.
- **The remaining ~4 % more read calls than 0.6.2 are expected (#390, #433).** Since 0.7.0, `Layout::positions_for` gives only the positions of the bytes asked for, so a position cache that fetches only what is missing makes one more fetch when a later request reads the rest of a DSF row. Rounding each request out to whole rows of the sound data gives 0.6.2's call count exactly; [`docs/rust.md`](docs/rust.md#dsf-and-dsdiff-files) gives the recipe. A helper that does the rounding is being discussed on #431 for 1.2.0.
- **Checked against 1.1.0:** `cargo semver-checks --baseline-rev v1.1.0 --release-type patch` finds no semver update required for `scarletbook` or `scarletbook-ffi` (default and all features), and `abi-check.py` passes with `SB_ABI_VERSION` 2.
- **No benchmark table:** both fixes change how many sectors are read, not how fast anything runs, and read counts are exact, so they were measured by count (above) rather than timed; 1.0.0's table stands.
- **Not measured:** real discs for #434 (how often they hit its trigger, a local bit rate well above the area's average), and for #437 anything beyond sacdfs's DSF library: 1-, 3- and 4-channel areas, 2064- and 2054-byte sectors, and timings.

### Changed

- Clarification for 0.7.0, no code change. From 0.7.0 on, `Layout::positions_for` gives only the positions of the DSF bytes asked for (#307, #390), not every position of the 4096 x C-byte rows they touch. A consumer that caches positions and fetches only those a request is missing can therefore make one more fetch, and one more seek, when a later request reads the rest of a row. sacdfs measured +3.6 % image reads and +7.9 % image bytes for the head and tail of every file of a 7,519-file library, with identical output. To fetch as 0.6.2 did, round each request out to whole rows of the sound data before calling `positions_for`; `docs/rust.md` gives the recipe (#431, #433).

### Fixed

- A provisional reading (`SeekPolicy::Speculative`) of a range that begins at a track's first frame, opened from a seek index whose nearest seek hint lies before the track's start sector, now begins at the track's start LSN from the TOC when that sector's first frame record is at most the range's first frame, with the same single read call. Before, it began at the hint, which after a remount is often where the previous track's tail read began: 67,584 bytes of the previous track read again for each track head on sacdfs's plain DSD disc, 75,776 on the synthetic ones. Replaying sacdfs's head-then-tail call sequence on synthetic images, the reads with a loaded index fall from 25,751,552 to 19,636,224 bytes (0.6.2: 23,537,664) at the same 215 read calls, with the same frames. Positions stay provisional exactly as from a hint (specification 4, 4.2 item 8, which names track start LSNs as a way to choose the start); a hint with a saved confirmation is still preferred (#431, #437).
- A provisional reading (`SeekPolicy::Speculative`) no longer falls back to an exact scan of the area when the search's read back from its first probe starts just past the frame before the one wanted, as it could where the bit rate rises. The search now goes on below that read. On a 6-minute 2-channel image of uneven DST bit rate, the 7-frame range from frame 18443 took 1,113 reads and 72.9 MB, and now takes 5 reads and 194,560 bytes. A sweep of every frame of 8 synthetic images (plain DSD and DST, regular and uneven, 2, 5 and 6 channels) found no exact fallbacks left, against 130 before. The frames returned are unchanged (#434, #436).

## [1.1.0] - 2026-11-24

**1.1.0 deprecates the C library's DSD-to-PCM functions; nothing breaks.** It is a semver minor release because a deprecation is an addition: no API is removed, no ABI changes, and nothing changes in behaviour or output. Besides the versions, `scarletbook` is unchanged since 1.0.2, and `scarletbook-ffi` differs only in attributes and documentation. Rust code that builds against 1.0.2 builds against 1.1.0 with only the version changed (calls to the `sb_pcm_*` functions from Rust now get a deprecation warning). C programs need no change (`SB_ABI_VERSION` 2); a program that calls an `sb_pcm_*` function gets a compiler warning, which `-DSB_DEPRECATED(m)=` (or defining `SB_DEPRECATED(msg)` as nothing before including the header) silences.

- **The four `sb_pcm_*` C functions are deprecated (#398, #399).** `sb_pcm_converter_new`, `sb_pcm_converter_free`, `sb_pcm_samples_per_frame` and `sb_pcm_convert_run` are `#[deprecated(since = "1.1.0")]` in `scarletbook-ffi`, and `scarletbook.h` declares them with the new `SB_DEPRECATED(msg)` attribute macro, so GCC, Clang and MSVC warn where they are called. They keep working, unchanged, and stay in ABI 2; removing them would take a new `SB_ABI_VERSION`. Take the DSD from `sb_track_audio_read` and convert it to PCM in your own code instead. Details under "Deprecated" below.
- **`scarletbook-pcm` is decommissioned (#398).** It is frozen at 0.7.0 on crates.io, with no further releases; this release neither changes nor publishes it, and `scarletbook-ffi` 1.1.0 still depends on `scarletbook-pcm` `=0.7.0`. Consumers convert DSD to PCM in their own crate instead. The README and the guides (`docs/c-abi.md`, `docs/rust.md`, `docs/stability.md`, `docs/releases.md`, `docs/contributing.md`) and RELEASING.md say so.
- **The Quick start runner no longer stops on cargo's own home (#428, #429).** Its work-directory guard refused any `.cargo` directory above the work directory, so its first run after v1.0.2 stopped before running anything on the runner's `~/.cargo`; it now refuses only a `Cargo.toml`, `rust-toolchain` or `rust-toolchain.toml` above the work directory, or a `.cargo/config` or `.cargo/config.toml` above it that is not in `$CARGO_HOME`. `quickstart-test.py` (7 checks) runs in CI.
- **RELEASING.md lists `link-check` among the checks `preflight` requires (#412, #427),** since it is now a required check on `main`.
- **Checked against 1.0.2:** `cargo semver-checks --baseline-rev v1.0.2 --release-type minor` finds no semver update required for `scarletbook` or `scarletbook-ffi` (default and all features). With `--release-type patch`, `scarletbook` passes and `scarletbook-ffi` fails only `function_marked_deprecated` on the four `sb_pcm_*` functions ("semver requires new minor version"), which is why this is 1.1.0. `abi-check.py` passes with `SB_ABI_VERSION` 2: `abi/abi-2.txt` gains only the `SB_DEPRECATED` macro's preprocessor lines, and the attribute itself is not ABI.
- **No benchmark table:** no code path that runs changed since 1.0.2 (attributes and documentation only), so nothing was measured for this release; 1.0.0's table stands.
- **Known item (#431):** since 0.7.0, a consumer measures about 8 % more image bytes (+3.6 % reads) when reading the head and tail of every DSF file in a library, with identical output. The cause is in the library and is being located; 1.1.0 reads exactly as 0.7.0 to 1.0.2 do. A fix, if one is needed, follows in a patch release.

### Deprecated

- The C library's `sb_pcm_converter_new`, `sb_pcm_converter_free`, `sb_pcm_samples_per_frame` and `sb_pcm_convert_run` are deprecated (`#[deprecated(since = "1.1.0")]` in scarletbook-ffi). `scarletbook.h` declares them with the new `SB_DEPRECATED(msg)` attribute macro, so GCC, Clang and MSVC warn where they are called; define it as nothing before including the header to silence it. They keep working, unchanged, and stay in the C ABI (`SB_ABI_VERSION` stays 2; `abi-check.py` ignores the attribute); removing them later would be a scarletbook-ffi ABI bump. `scarletbook-pcm`, which they wrap, is decommissioned: frozen at 0.7.0 on crates.io, no further releases, and unchanged by this release. Consumers convert DSD to PCM in their own crate instead (#398, #399).

## [1.0.2] - 2026-11-23

**1.0.2 changes documentation and comments only.** No code change, no API change, no ABI change, no change in behaviour or output: besides the versions, the sources of `scarletbook` and `scarletbook-ffi` differ from 1.0.1 only in doc comments, the C header's comments and the docs.rs settings. Rust code that builds against 1.0.1 builds against 1.0.2 with only the version changed; C programs need no change at all (`SB_ABI_VERSION` 2; the header differs from 1.0.1 only in comments).

- **docs.rs builds `scarletbook` with the `parallel` feature (#415).** `DecodeOn` and the `parallel` APIs (`TrackAudioCursor::decode_on` and `set_decode_on`) now appear on docs.rs, each marked as `parallel`-only.
- **The C library archives' `README.txt` links docs pages that exist (#419).** It linked a README anchor that the README split (#410) removed; it now links `docs/c-abi.md` (linking and the compatibility policy) and `docs/releases.md` ("Verifying a download"), both at the archive's own tag. The 1.0.1 archives keep the dead link.
- **The documentation audit (#412: #416, #417, #418, #421, #423, #424).** The README, the guides under `docs/`, SECURITY.md, CONTRIBUTING.md, RELEASING.md, the benchmark README, the crate docs and the C header comments were checked against 1.0.1 and corrected where they were wrong; `docs/` has an index and a shared layout (#417). The README itself is now short, with the rest moved into guides under `docs/` (#408, #410). Details under "Changed" below.
- **Attestation commands pass `--deny-self-hosted-runners` (#411).** Every `gh attestation verify` command in the README, the guides, SECURITY.md, the benchmark README and the release notes now refuses a provenance signed on a self-hosted runner.
- **CI additions.** An internal link check runs on every pull request and every push to `main` and fails on a broken relative link or anchor in the repository's Markdown, with a weekly check of external URLs (#413); contributors run it with `python3 -I .github/scripts/linkcheck.py` (#424). The README's Quick start now runs by itself after each release, and weekly, and its output is compared with the README's (#414).
- **Checked against 1.0.1:** `cargo semver-checks --baseline-rev v1.0.1 --release-type patch` finds no semver update required for `scarletbook` or `scarletbook-ffi` (default and all features), and `abi-check.py` passes with `SB_ABI_VERSION` 2.
- **No benchmark table:** no code changed since 1.0.1, so nothing was measured for this release; 1.0.0's table stands.

### Changed

- The README is short and user-facing, in the layout shared with the related projects: badges, platforms, the tested Quick start for Rust and C, links to the guides, the licence. The rest moved unchanged into guides under `docs/`, except one corrected command: the `decode_pool` benchmark in `docs/performance.md` now passes an absolute image path (`cargo bench` runs in the crate's directory, so the relative path failed with `SB_STATUS_IO`) and builds `scarletbook-synth` first. The guides: `rust.md`, `c-abi.md` (with "Linking" and "ABI compatibility"), `python.md` (the ctypes bindings, which `clib-smoke.py` now reads from there), `releases.md` (release assets and verifying a download), `stability.md`, `performance.md` and `contributing.md` (#408, #410).
- The benchmark results, the changelog and the benchmark scripts' comments name the benchmark machine only as the Zen 2 benchmark host (AMD Ryzen 9 3950X), and describe its exclusive timing lock instead of naming a private script. No figure, date or fact changed (#412, #421).
- Documentation only: the crate docs and the C header comments, checked against 1.0.1. The header's references to the README now name `docs/c-abi.md` ("ABI compatibility") and `docs/python.md` ("The whole TOC in one call"), where #410 moved those sections. The header's "Layouts" convention names all four structs whose size differs on 32-bit targets (`SbMarker`, `SbComment` and `SbPositionData` as well as `SbWarning`). Its "Threads" convention lists all the global state: the counter of provisional readings and the character-set and PCM filter tables were missing. The `output` module says it needs the `output` feature (on by default). docs.rs now builds `scarletbook` with `parallel`, so `DecodeOn` and `TrackAudioCursor::decode_on`/`set_decode_on` appear there, each marked as `parallel`-only. No C ABI change (#412, #415).

## [1.0.1] - 2026-11-23

**1.0.1 is 1.0.0's code, released again so that `scarletbook-ffi` 1.x reaches crates.io.** No API change, no ABI change, no change in behaviour or output: the crates' sources are those of 1.0.0, under a new version. Rust code that builds against 1.0.0 builds against 1.0.1 with only the version changed; C programs need no change at all (`SB_ABI_VERSION` 2, the same header).

- **The first `scarletbook-ffi` 1.x on crates.io (#401).** v1.0.0's publish stopped partway: `scarletbook` 1.0.0 is on crates.io, but `scarletbook-ffi` 1.0.0 never was. The release had packed it against a locally packed `scarletbook-pcm` 0.7.0 instead of the one on crates.io, and the check before upload, which compares each package with the one the build verified, correctly refused it. 1.0.1 publishes `scarletbook` and `scarletbook-ffi`; `scarletbook-pcm` stays at 0.7.0, already on crates.io, and is not published again.
- **The v1.0.0 GitHub release and its C library archives are fine as they are.** They are unchanged and need nothing; 1.0.1's archives are built from the same sources.
- **Checked against 1.0.0:** `cargo semver-checks --baseline-rev v1.0.0 --release-type patch` finds no change at all in `scarletbook` or `scarletbook-ffi` (default and all features), `scarletbook.h` is byte-identical to v1.0.0's, and `abi-check.py` passes with `SB_ABI_VERSION` 2. Since v1.0.0, only the versions, the notes and the release workflow moved.
- **Release workflow (#402).** The `build` job packs and verifies only the crates whose version is not on crates.io yet, so a crate that is skipped resolves from crates.io, as it does for users. `release-crates.py lockcheck` checks the `Cargo.lock` pins of the packed crates, and CI runs the same packing as a dry run on every pull request and push (`release-publish.sh dry-run`). RELEASING.md, "Pack only what is published".
- **No benchmark table:** no code changed since 1.0.0, so nothing was measured for this release; 1.0.0's table stands.

## [1.0.0] - 2026-11-23

**1.0.0 is the 0.7 interface, frozen.** `scarletbook` and `scarletbook-ffi` are 0.7.0's code under a 1.0 version: no API change, no ABI change, no change in behaviour or output. Code that builds against 0.7 builds against 1.0.0 with only the version changed; C programs need no change at all (`SB_ABI_VERSION` 2, the same header).

- **What 1.0 promises (#39).** From 1.0.0 the Rust API of `scarletbook` and the C ABI of `scarletbook-ffi` follow Semantic Versioning: no breaking change without 2.0.0. A `1.Y.0` release may add to them, a `1.y.Z` release only fixes; each release is checked with `cargo semver-checks` against the one before (RELEASING.md). The C library keeps `SB_ABI_VERSION` 2 and the names `libscarletbook_ffi.so.2` and `libscarletbook_ffi.2.dylib` throughout 1.x; ABI 2 is the 1.0 ABI. Not covered, as before: `#[doc(hidden)]` items, the `fuzzing` feature of `scarletbook-ffi`, its Rust items (its interface is the C ABI), and the unpublished crates. README, "Stability".
- **Checked against 0.7.0:** `cargo semver-checks --baseline-rev v0.7.0 --release-type patch` finds no change at all in `scarletbook` or `scarletbook-ffi` (default and all features), and `scarletbook.h` is byte-identical to v0.7.0's. The crates' sources are unchanged since v0.7.0; only versions, notes and the release workflow moved.
- **`scarletbook-pcm` stays at 0.7.0, outside the freeze.** It is not part of 1.0: it keeps its own version, 0.7.0, which this release does not publish again, and it is being deprecated (#398): DSD-to-PCM conversion moves to its consumers, and its deprecation lands in a later release. Until then it follows Cargo's `0.y.z` rules and promises nothing beyond its deprecation. `scarletbook-ffi` 1.0.0 depends on it at `=0.7.0`. The `sb_pcm_*` functions of the C library belong to ABI 2 and stay, even once they are marked deprecated.
- **Coming from 0.6 or earlier:** read [`docs/migrating-0.7.md`](docs/migrating-0.7.md) (and [`docs/migrating-0.6.md`](docs/migrating-0.6.md) before it). From 0.7, change the version only.
- **Release workflow:** the three crates no longer need to share one version. The release workflow takes each crate's version from its manifest, and the `publish` job skips any whose version is already on crates.io (`release-crates.py`, tested in CI).
- **Measured** on the Zen 2 benchmark host (AMD Ryzen 9 3950X), generic build, 1.0.0 against 0.7.0 (the same code, so no change is expected):

  | Path | Build | 0.7.0 wall s | 0.7.0 CPU s | 1.0.0 wall s | 1.0.0 CPU s | 1.0.0 / 0.7.0 | Median of per-round ratios [range] |
  |---|---|---|---|---|---|---|---|
  | DST decode, one thread, 2 channels | generic | 5.575 | 5.53 | 5.586 | 5.56 | 1.002 | 1.003 [0.989-1.085] |
  | DST decode, one thread, 2 channels, corpus frames | generic | 18.494 | 18.40 | 18.498 | 18.40 | 1.000 | 1.000 [0.978-1.002] |
  | DST decode, all threads, 2 channels | generic | 4.234 | 109.12 | 4.232 | 109.24 | 1.000 | 1.000 [0.995-1.004] |
  | DST decode, one thread, 6 channels | generic | 7.627 | 7.59 | 7.590 | 7.54 | 0.995 | 0.996 [0.973-1.004] |
  | DST decode, all threads, 6 channels | generic | 2.770 | 72.58 | 2.779 | 72.47 | 1.003 | 1.004 [0.992-1.025] |
  | plain-DSD area to DSF, 2 channels | generic | 0.490 | 0.48 | 0.488 | 0.48 | 0.997 | 0.997 [0.991-1.017] |
  | plain-DSD area to DSF, 5 channels | generic | 0.538 | 0.53 | 0.538 | 0.53 | 1.001 | 1.001 [0.991-1.009] |
  | DST area to DSF, all threads | generic | 3.560 | 112.86 | 3.566 | 112.71 | 1.002 | 1.003 [0.990-1.007] |
  | DSD to PCM, stereo DSD64 to 176.4 kHz 24-bit, one thread | generic | 3.90 / 3.82 / 3.82 ms per s | - | 3.89 / 3.82 / 3.82 ms per s | - | 1.000 | - |
  | open to parsed TOC, page cache | generic | 14.9 µs | - | 14.7 µs | - | 0.987 | -, within spread |
  | open to parsed TOC, evicted | generic | 214.7 µs | - | 216.8 µs | - | 1.010 | -, within spread |
  | first 128 KiB, 2 channels, plain DSD, page cache | generic | 24.9 µs | - | 24.7 µs | - | 0.994 | - |
  | first 128 KiB, 2 channels, plain DSD, evicted | generic | 269.2 µs | - | 275.0 µs | - | 1.021 | -, within spread |
  | first 128 KiB, 2 channels, DST, one thread, page cache | generic | 3696.2 µs | - | 3695.5 µs | - | 1.000 | - |
  | first 128 KiB, 2 channels, DST, one thread, evicted | generic | 4091.9 µs | - | 4091.7 µs | - | 1.000 | - |
  | first 128 KiB, 2 channels, DST, all threads, page cache | generic | 2201.3 µs | - | 2215.6 µs | - | 1.007 | - |
  | first 128 KiB, 2 channels, DST, all threads, evicted | generic | 4214.5 µs | - | 4298.4 µs | - | 1.020 | -, within spread |

  `benchmarks/compare.sh` with v0.7.0 against this release's tree, the standard run list at 7 rounds per case, alternating, a fresh copy of each program per run, under the machine's shared benchmark lock; 238 runs, none failed. **No change:** every whole-area median of per-round ratios is between 0.996 and 1.004, and every `open`, `first` and DSD-to-PCM cell is within its spread between rounds. Absolute all-thread seconds differ from the 0.7.0 section's (other load on the machine on another day), for both versions alike; compare within this table. Method and spreads: `benchmarks/RESULTS.md`, "1.0.0".
- **Unmeasured:** the x86-64-v3, `znver5` and AVX-512 builds; Zen 5 and Intel CPUs on this release (the code is 0.7.0's, so 0.7.0's consumer figures stand); real output files; the first 128 KiB of 5 or 6 channels; cold runs of whole areas; the C interface; `bench realtime`.
- **Known item: single-frame 6-channel DST decoding on Intel P-cores (#383), carried over from 0.7.0.** On an Intel Core i9-13900H (Raptor Lake) P-core, one thread, single-frame 6-channel `DstDecoder::decode` of a real disc's frames was measured by sacdfs at about 7 % slower than 0.5.9 for 0.7.0's code (median of per-round ratios 1.071), and grouped `decode_many` about 2.5 % (1.027); E-cores and Zen 2 are at parity. pysacd, which decodes through the C ABI in groups, measures the 6-channel P-core gap at about 1.5-1.9 % with 0.7.0. 1.0.0 has the same decoder, so these stand. Tracked in #383.

## [0.7.0] - 2026-11-23

**0.7.0 removes the 36 Rust items deprecated in 0.6.0, as announced, and moves the boundaries between the files of a gapless DSF chain to where the specification now puts them (#359). It also stops partial DSF rows from asking for positions they don't cover (#307). Decoded DSD and PCM output is unchanged. The C ABI is unchanged (`SB_ABI_VERSION` 2; the header differs from 0.6.2 only in comments).**

- **Breaking: read [`docs/migrating-0.7.md`](docs/migrating-0.7.md).** The 36 items deprecated in 0.6.0 are removed from `scarletbook` (35) and `scarletbook-pcm` (`Cutoff::Default`); the page gives the replacement for each. Each was another name or call for one that stays, so code that builds on 0.6 without deprecation warnings builds on 0.7.0 unchanged, and nothing behaves differently. C programs are not affected (#39, #389).
- **DSF chain boundaries move (#359, #388).** This affects only gapless DSF chains, planned with `plan_dsf_chain` / `OutputOptions::with_dsf_chain` in Rust or `sb_output_dsf_chain_plan` in C. A file of a chain that ended exactly on a 4096-byte block boundary now ends one block earlier, and the next file starts with that block (specification 8, 8.2.7 items 3 and 4, amended at sacd-spec `67b8563`). Only such a file and the file after it differ from 0.6.2 (for example ranges of 128 frames from the chain's start); every other file is byte-for-byte the same, the last file is never shortened, and the files still concatenate to the same bytes in the same order. Single DSF files, DSDIFF, PCM and raw DSD are not affected. If you store hashes or sizes of chain files, expect those boundary files to change.
- **DSF partial rows (#307, #390).** A read of part of a DSF row now asks only for the positions it covers: a bad neighbouring position no longer fails it, `Layout::positions_for` and `sb_output_layout_positions` give only those positions, and reads under 4 KiB from a DST area no longer decode the row's other position. The bytes are unchanged.
- **DST decoder tables (#383, #392).** A new `DstDecoder` writes its tables once instead of zeroing them first. Output unchanged; neutral on the Zen 2 benchmark host.
- **Measured** on the Zen 2 benchmark host (AMD Ryzen 9 3950X), generic build, 0.7.0 against 0.6.2:

  | Path | Build | 0.6.2 wall s | 0.6.2 CPU s | 0.7.0 wall s | 0.7.0 CPU s | 0.7.0 / 0.6.2 | Median of per-round ratios [range] |
  |---|---|---|---|---|---|---|---|
  | DST decode, one thread, 2 channels | generic | 5.562 | 5.52 | 5.574 | 5.53 | 1.002 | 1.003 [0.997-1.007] |
  | DST decode, one thread, 2 channels, corpus frames | generic | 18.536 | 18.43 | 18.498 | 18.40 | 0.998 | 0.998 [0.994-1.006] |
  | DST decode, all threads, 2 channels | generic | 3.587 | 112.90 | 3.562 | 111.76 | 0.993 | 0.992 [0.986-0.996] |
  | DST decode, one thread, 6 channels | generic | 6.997 | 6.96 | 7.104 | 7.06 | 1.015 | 1.009 [0.992-1.021], unresolved |
  | DST decode, all threads, 6 channels | generic | 2.340 | 73.73 | 2.350 | 73.87 | 1.004 | 1.004 [0.997-1.009] |
  | plain-DSD area to DSF, 2 channels | generic | 0.456 | 0.45 | 0.455 | 0.45 | 0.997 | 0.999 [0.973-1.008] |
  | plain-DSD area to DSF, 5 channels | generic | 0.506 | 0.50 | 0.506 | 0.49 | 1.000 | 0.999 [0.989-1.007] |
  | DST area to DSF, all threads | generic | 3.591 | 113.74 | 3.555 | 112.61 | 0.990 | 0.989 [0.985-0.995] |
  | DSD to PCM, stereo DSD64 to 176.4 kHz 24-bit, one thread | generic | 3.90 / 3.82 / 3.81 ms per s | - | 3.90 / 3.81 / 3.81 ms per s | - | 0.997 | - |
  | open to parsed TOC, page cache | generic | 14.6 µs | - | 14.0 µs | - | 0.960 | -, within spread |
  | open to parsed TOC, evicted | generic | 224.4 µs | - | 231.3 µs | - | 1.031 | -, within spread |
  | first 128 KiB, 2 channels, plain DSD, page cache | generic | 25.1 µs | - | 24.7 µs | - | 0.985 | -, within spread |
  | first 128 KiB, 2 channels, plain DSD, evicted | generic | 273.5 µs | - | 269.9 µs | - | 0.987 | -, within spread |
  | first 128 KiB, 2 channels, DST, one thread, page cache | generic | 3674.2 µs | - | 3702.2 µs | - | 1.008 | - |
  | first 128 KiB, 2 channels, DST, one thread, evicted | generic | 4130.4 µs | - | 4119.2 µs | - | 0.997 | -, within spread |
  | first 128 KiB, 2 channels, DST, all threads, page cache | generic | 2221.5 µs | - | 2210.1 µs | - | 0.995 | - |
  | first 128 KiB, 2 channels, DST, all threads, evicted | generic | 4593.6 µs | - | 4364.4 µs | - | 0.950 | -, within spread |

  `benchmarks/compare.sh` with v0.6.2 against this release's tree, the standard run list at 7 rounds per case, alternating, a fresh copy of each program per run, under the machine's shared benchmark lock; 238 runs, none failed. No row moves by more than about 2 % beyond its spread between rounds: all-thread stereo DST decoding and DST to DSF are about 1 % faster in every round; one-thread 6-channel DST is 1.009 [0.992-1.021], unresolved (#259). `open` and `first` are the program's own microseconds; the per-round column is left out for them. Method and spreads: `benchmarks/RESULTS.md`, "0.7.0".
- **Unmeasured:** the x86-64-v3, `znver5` and AVX-512 builds; Zen 5, and Intel CPUs other than the #383 figures below; real output files, including DSF chains; the first 128 KiB of 5 or 6 channels; cold runs of whole areas; the C interface; `bench realtime`.
- **Known item: single-frame 6-channel DST decoding on Intel P-cores (#383).** On an Intel Core i9-13900H (Raptor Lake) P-core, one thread, single-frame 6-channel `DstDecoder::decode` of a real disc's frames is still about 7 % slower than 0.5.9 (24.53 s against 22.92 s; median of per-round ratios 1.071), and grouped `decode_many` about 2.5 % (1.027). On its E-cores, and on Zen 2, 0.7.0 is at parity with 0.5.9 (E-core single 0.989, grouped 0.986). #392 leaves the P-core figure where 0.6.2 had it (1.000 single, 1.003 grouped against 0.6.2). sacdfs measured this on #383 against `446994c`, the code of this release; the table is in its latest comment. pysacd, which decodes through the C ABI in groups of frames, measured a real 5.1 disc to DSF on the same CPU's P-cores at 1.8 % slower than 0.5.9 with one thread and 0.9 % with five (0.6.2; #383). The gap is tracked in #383.

### Changed

- **DSF chain boundaries move.** A file of a DSF chain (`plan_dsf_chain`, `OutputOptions::with_dsf_chain`, `sb_output_dsf_chain_plan`) whose range would end exactly on a 4096-byte block boundary now ends one block earlier, and the next file starts with that block: each file after the first now carries 1 to 4096 bytes of the range before it instead of 0 to 4095 (specification 8, 8.2.7 items 3 and 4 as amended at sacd-spec `67b8563`). Only such files and the file after each change bytes, for example ranges of 128 frames from the chain's start; every other file is byte-for-byte unchanged, the last file of a chain is never shortened, and the files still concatenate to the chain's bytes in the same order with the same total. The Rust API and the C ABI are unchanged, but files written by earlier versions differ at those boundaries, which is why this is in 0.7.0 (#359, #388).
- A new `DstDecoder`, or a frame with more filters or ptables than any before it, now writes each new byte table and probability table once, rather than filling it with zeros first and then computing over the zeros. Output unchanged (#383, #392).

### Removed

- **Breaking:** the 36 items deprecated in 0.6.0 are removed, as announced (#39, R1). `scarletbook`: `Disc::{track_audio_cursor, range_audio_cursor, range_audio_cursor_indexed, track_frames_cursor, range_frames_cursor, range_frames_cursor_indexed}`; `for_range`, `for_range_indexed` and `for_range_speculative` on `TrackCursor` and `TrackAudioCursor`; `speculation_id()` and `counts_from()` on `AreaCursor`, `TrackCursor`, `TrackFrames`, `TrackAudioCursor` and `TrackAudio`; `Image::{open_with_log, with_format, with_format_and_log}` and `Disc::{open_with_log, open_with_format, open_with_format_and_log}`; `Stored::Plain`; `AreaIndex::{checkpoints, hints, confirmations, pending_claims}`; `dst::{FL, MAX_CHANNELS}`. `scarletbook-pcm`: `Cutoff::Default`. Each was another name or call for one that stays (`Disc::{audio_cursor, frames_cursor}`, `open_range`, `claim()`/`claim_lsn()`, `Image::options()`/`Disc::options()`, `Stored::Dsd`, the `*_count` methods, the `audio` constants, `Cutoff::Standard`), so no behaviour changes. The C ABI does not change (`SB_ABI_VERSION` 2). Migration: [`docs/migrating-0.7.md`](docs/migrating-0.7.md) (#39, #389).

### Fixed

- DSF partial rows ask only for the positions a read covers again: a read of part of a DSF row fills only the stream bytes it covers, so a bad neighbouring position no longer fails a read whose own bytes lie elsewhere or are pad, `Layout::positions_for` (and `sb_output_layout_positions`) gives only those positions, and reads under 4 KiB from a DST area no longer fetch and decode the row's other position. The output is unchanged (#307, #390).

## [0.6.2] - 2026-11-20

**Single-frame DST decoding of 5 and 6 channels is back to its 0.5.x speed (#378), and DSD-to-PCM conversion is faster on 32-bit x86 (#372). Output is identical to the bit. No API or ABI change: a drop-in update from 0.6.0 and 0.6.1.**

- **Single DST frames (#378, #379).** Measured on the Zen 2 benchmark host (Ryzen 9 3950X), one thread, generated frames decoded one at a time with `DstDecoder::decode`, 7 alternating rounds against 0.6.1's decoder:

  | Path | 0.6.1 s | 0.6.2 s | Median of per-round ratios [range] |
  |---|---|---|---|
  | single frames, 5 channels | 34.20 | 32.67 | 0.956 [0.949-0.962] |
  | single frames, 6 channels | 42.60 | 40.82 | 0.956 [0.950-0.983] |
  | single frames, 1 and 2 channels | 23.51 / 25.05 | 23.55 / 24.98 | 1.002 / 0.998 |
  | 2 frames at a time (default), 6 and 2 channels | 34.75 / 18.55 | 34.56 / 18.57 | 0.994 / 1.001 |

  On these frames, 0.5.6 took 41.08 s for the 6-channel case. A consumer measured the regression at 11 % on real discs, which are not in our corpus; their re-timing is pending on #378.
- **DSD to PCM on i686 (#372: #374, #377).** Zen 2 benchmark host, one thread, i686 musl `-C target-cpu=pentium4`, ms of CPU per second of audio, 6 channels, 7-round ABBA per PR (the tables are in #374 and #377):

  | Rate, cutoff | 0.6.1 | 0.6.2 | Change |
  |---|---|---|---|
  | 176.4 kHz Standard | 26.85 | 25.11 | about 6.5 % less |
  | 176.4 kHz Low | 32.46 | 30.73 | about 5.3 % less |
  | 88.2 kHz Standard | 19.04 | 18.70 | about 1.8 % less |
  | 352.8 kHz Standard | 33.19 | 31.59 | about 4.8 % less |
  | 176.4 kHz Wide | 42.89 | 42.81 | unchanged |

  These chain two separate runs, so read them as approximate. sacdfs measured the conversion step at 7 to 9 % less time at 176.4 kHz on Gracemont E-cores (#372). x86-64 is unchanged.
- **Unmeasured:** in-order Atom cores; consumers' real-disc frames for #378; Zen 5, Raptor Lake and AVX-512 builds.

### Changed

- DSD-to-PCM conversion on 32-bit x86 (i686): the Fir1 stage computes 8 outputs at a time instead of 16, which fits SSE2's 8 vector registers there, so 176.4 and 352.8 kHz output take about 4 to 6 % less time. The output is identical to the bit. CI now also tests `scarletbook-pcm` on i686 (#372).
- DSD-to-PCM conversion on 32-bit targets: the first filter stage (S1) reads its input in 32-bit words instead of 64-bit ones, which i686 handles in single registers, about 1 to 2 % less time. The output is identical to the bit (#372).

### Fixed

- Decoding single DST frames of 5 or 6 channels (`DstDecoder::decode`, or `decode_many` with one frame) was about 4 % slower on synthetic frames than in 0.5.x, and up to 11 % on a consumer's real discs. The cause was 0.6.0's Q table (#330), which lengthens a lone frame's serial chain. A lone frame of 5 or 6 channels now takes Q by shifts again, the same to the bit. Groups of 2 or more frames, and 1 and 2 channels, are unchanged (#378).

## [0.6.1] - 2026-11-18

**Faster DST decoding on 32-bit x86 (i686), with the same output to the bit (#369, #370). No API or ABI change: a drop-in update from 0.6.0.**

- **Measured on the Zen 2 benchmark host** (Ryzen 9 3950X), one thread, static i686 musl builds with `-C target-cpu=pentium4`, before against the same code as this release, 7 alternating rounds:

  | Path | Before wall s | After wall s | Median of per-round ratios [range] |
  |---|---|---|---|
  | i686, 6-channel DST (10 min, synthetic) | 93.05 | 55.71 | 0.597 [0.592-0.601] |
  | i686, 2-channel DST (20 min, synthetic) | 61.59 | 31.29 | 0.508 [0.506-0.510] |
  | x86-64, 6-channel DST | 34.75 | 34.77 | 1.001 [0.984-1.012] |
  | x86-64, 2-channel DST | 18.71 | 18.67 | 0.999 [0.995-1.007] |

  On i686, 6-channel DST takes 40 % less time and 2-channel 49 % less. x86-64 and other 64-bit targets are unchanged: they keep the 64-bit form.
- **sacdfs's black-box figures** (on #369, i9-13900H, its own i686 musl pentium4 build, 0.6.0 against this code):
  - 6-channel DST took 0.583 of the time on one E-core and 0.587 on two E-cores with 4 threads;
  - 2-channel took 0.577 and 0.582;
  - every corpus file (910) and every file from its 3 real discs (94) was byte-identical to the x86-64 output.
- **No run-time SSE3/SSSE3 detection:** building with SSE3, SSSE3 and MOVBE measured no faster than the pentium4 baseline, so there is one i686 path for every CPU.
- **Unmeasured:** in-order Atom cores, such as the NAS units #369 is for (sacdfs projects about 2.2x real time for 6-channel DST there from its E-core ratio; that is a projection, not a measurement); more than one thread on the Zen 2 host; other 32-bit targets.

### Changed

- DST decoding on 32-bit targets (i686 and others) keeps the arithmetic decoder's state in a 32-bit word instead of a 64-bit one, so that each step is single-register arithmetic. The decoded output is identical to the bit. CI now tests i686 builds for the pentium4 baseline and for the Atom class (#369).

## [0.6.0] - 2026-11-15

**The interface change before 1.0 (#39), faster DST decoding, fixes to seek verdicts, ID3 tag sizes and debug builds, and the first release from the hardened release workflow.**

- **Breaking changes: read [Migrating to 0.6.0](https://github.com/aclater/scarletbook-rs/blob/v0.6.0/docs/migrating-0.6.md) first.** This is the release that changes the interface before it is frozen for 1.0 (#39). Rust: TOC and result structs and several enums and enum variants are `#[non_exhaustive]`; `Stored::Plain` is `Stored::Dsd` and `Cutoff::Default` is `Cutoff::Standard` (old names kept as deprecated constants); `Image::read_sectors` returns `Result<(), Error>`; plumbing, tuning values and the low-level text helpers are `#[doc(hidden)]` and out of the semver promise; the cursor twins of `Disc`, the `*with_log`/`*with_format*` constructors, `speculation_id`/`counts_from`, the count methods of `AreaIndex` and `dst::{MAX_CHANNELS, FL}` are deprecated, to be removed before 1.0. Each has a before-and-after on that page. **C library:** the ABI and the header's declarations do not change (`SB_ABI_VERSION` 2, now declared the 1.0 ABI), but the shared library is named after it: `libscarletbook_ffi.so.2` (SONAME) on Linux and `libscarletbook_ffi.2.dylib` on macOS, with the old names as links in the archives. A program linked against 0.6.0 needs the versioned file at run time; one that loads the library by path should move to the versioned name (#294, migration page M8). Windows is unchanged.
- **Faster DST decoding, same output.** Q now comes from a table in the fast loop for 1, 2, 5 and 6 channels (#330); against 0.5.9 on Zen 2 (table below), DST decoding takes 5 to 10 % less time: stereo 7.6 to 10 %, 5 channels 5.5 % at one thread and 8 % at all threads, 6 channels 5 %. The DST decoder also inlines one copy of most of its symbol loops instead of three (#338), for debug builds; release builds are about the same.
- **DST lanes: the default stays 2, and `dst::lanes` stays public** (owner's decision of 2026-11-07, #203). Callers can still choose their own count with `TrackAudioCursor::with_dst_lanes`.
- **`znver5` and 5 channels: unmeasured.** 0.5.9's `znver5` build decoded 5 channels about 1.9× slower than 0.5.8's at one thread. #330 removed the code pairing suspected behind it, so it may be fixed, but no 5-channel `znver5` figure of this release exists yet: it is unmeasured, and any fix that turns out to be needed ships in a patch release. Whether `lanes()` should be 6 on AVX-512 builds is still open (#203).
- **Stack use is documented** (#335): calls may use up to 68 KiB of stack; give threads that call the library at least 136 KiB.
- **Debug builds no longer overflow a 2 MiB thread stack** when they decode DST on a CPU with AVX2 (#336, #338): about 1.2 MiB per frame instead of 2.1 MiB. Output is unchanged to the bit.
- **Seek verdicts are reported once.** C: a verdict on a provisional reading that an index handle held when given to `sb_disc_seek_index_set` was returned twice, by the handle and by the disc; it is now returned once, by the handle (#326, #353). Rust: a verdict held on an index a disc kept reached `Disc::take_seek_events` late and more than once, and could push other verdicts out of its queue (#310, #327).
- **ID3 tag size** (#343, #352): a DSF ID3 tag that `output_layout` builds now counts its header and footer or padding against `MAX_TAG_BYTES`. A built tag with a footer could be up to 20 bytes over the maximum and is now refused with `InvalidArgument`; a padded tag of 1,048,567 to 1,048,576 bytes that was wrongly refused is now written.
- **New: area capacity figures for a plausibility check** (#340, #363): `Disc::area_capacity(slot)` (C: `sb_area_capacity`, `SbAreaCapacity`) returns what an area's TOC claims against the audio sectors the image holds: the sectors present, play time, largest track end, the capacity of specification 4.8 and the most plain DSD frames those sectors can carry, with `frames_per_sector()`. The consumer chooses the threshold. Additive to the Rust API and to ABI 2.
- **C: `_free` functions leave the thread's last error message alone** (#347, #362). A caller that frees its handles on an error path and then calls `sb_last_error_message()` now gets the failed call's message, not an empty string; a NULL handle or a free refused inside a callback no longer sets a message. No signature changes; `SB_ABI_VERSION` stays 2.
- **Control characters in TOC strings are documented** (#342, #360): ISRCs and catalogue numbers keep C0, DEL and C1, language codes keep everything but trailing NUL and space, and text keeps U+007F to U+009F, as the specification defines. Escape or filter them before printing them or putting them in tags or file names. No behaviour change.
- **Release security: the first release built by the new release workflow.** A `preflight` job refuses a tag whose commit is not on `main` or has not passed `main`'s required checks (#341); signing and publishing run in the environment `release`, which only `v*` tags can use; the jobs that sign and publish compile nothing, since building, testing and packing run in a job without write, signing or publishing permission (#345). These stop honest mistakes and stray tags, not a compromised token with write access: the crates.io trusted publishers name no environment and tag creation is not restricted (owner's decision of 2026-11-16), as RELEASING.md describes. **What changes for you:** the asset names and the 28 assets are as before. Check a downloaded file with the release workflow and the tag pinned, not only the repository (#344):
  `gh attestation verify <file> -R aclater/scarletbook-rs --signer-workflow aclater/scarletbook-rs/.github/workflows/release.yml --source-ref refs/tags/v0.6.0`.
  Without the last two flags the command accepts an attestation from any workflow run of the repository. The `.sha256` beside each archive detects a damaged download, not a replaced one: it is neither signed nor attested.

**Open in milestone 0.6.0, not blocking it:** #307 (DSF partial rows), #259 (plain-DSD benchmark variance), #202 and #203 (performance against a reference implementation).

| Path | Build | 0.5.9 wall s | 0.5.9 CPU s | 0.6.0 wall s | 0.6.0 CPU s | 0.6.0 / 0.5.9 | Median of per-round ratios [range] |
|---|---|---|---|---|---|---|---|
| DST decode, one thread, 2 channels | generic | 6.070 | 6.04 | 5.437 | 5.40 | 0.896 | 0.899 [0.890-0.958] |
| DST decode, one thread, 2 channels, corpus frames | generic | 20.056 | 19.96 | 18.543 | 18.45 | 0.925 | 0.924 [0.923-0.927] |
| DST decode, all threads, 2 channels | generic | 3.833 | 120.71 | 3.518 | 110.71 | 0.918 | 0.918 [0.910-0.927] |
| DST decode, one thread, 6 channels | generic | 7.364 | 7.32 | 7.010 | 6.96 | 0.952 | 0.949 [0.943-0.960] |
| DST decode, all threads, 6 channels | generic | 2.468 | 77.73 | 2.335 | 73.56 | 0.946 | 0.947 [0.942-0.955] |
| plain-DSD area to DSF, 2 channels | generic | 0.487 | 0.48 | 0.483 | 0.48 | 0.992 | 0.996 [0.990-0.998], unresolved |
| plain-DSD area to DSF, 5 channels | generic | 0.533 | 0.53 | 0.531 | 0.52 | 0.997 | 1.002 [0.983-1.006], unresolved |
| DST area to DSF, all threads | generic | 3.825 | 121.30 | 3.528 | 111.49 | 0.922 | 0.923 [0.917-0.926] |
| DSD to PCM, stereo DSD64 to 176.4 kHz 24-bit, one thread | generic | 3.89 / 3.81 / 3.80 ms per s | - | 3.90 / 3.81 / 3.81 ms per s | - | 1.000 | - |
| open to parsed TOC, page cache | generic | 14.7 µs | - | 15.2 µs | - | 1.039 | -, within spread |
| open to parsed TOC, evicted | generic | 200.7 µs | - | 200.2 µs | - | 0.997 | -, within spread |
| first 128 KiB, 2 channels, plain DSD, page cache | generic | 27.9 µs | - | 28.2 µs | - | 1.013 | -, within spread |
| first 128 KiB, 2 channels, plain DSD, evicted | generic | 248.1 µs | - | 245.2 µs | - | 0.989 | -, within spread |
| first 128 KiB, 2 channels, DST, one thread, page cache | generic | 3983.7 µs | - | 3681.9 µs | - | 0.924 | - |
| first 128 KiB, 2 channels, DST, one thread, evicted | generic | 4358.2 µs | - | 4101.1 µs | - | 0.941 | - |
| first 128 KiB, 2 channels, DST, all threads, page cache | generic | 2352.6 µs | - | 2228.2 µs | - | 0.947 | - |
| first 128 KiB, 2 channels, DST, all threads, evicted | generic | 4138.1 µs | - | 4165.4 µs | - | 1.007 | -, within spread |

Medians of alternating runs on the Zen 2 benchmark host (AMD Ryzen 9 3950X), 2026-11-16, under the machine's shared benchmark lock, generic build, kits from `benchmarks/compare.sh` (`v0.5.9` and this release's tree), a fresh copy of each program and image per round, images in the page cache, output to `/dev/null`: 5 rounds, 3 for the 20-minute DST case and for DSD to PCM; 162 runs, none failed. The DSD-to-PCM cells are at 1 / 8 / 256 frames per call. The `open` and `first` cells are the program's own medians in microseconds; their per-round column is left out, and those marked "within spread" vary between rounds by more than the difference (`benchmarks/RESULTS.md` lists the ranges).

**Plain-DSD rows:** differences under about 2 % are unresolved (#259). Neither row shows a change either way, and 0.6.0 has no change aimed at that path.

**5 channels.** The run list has no 5-channel DST case, so these come from a separate run with the same method (5 rounds, 20 runs, none failed):

| Path (not in the run list) | Build | 0.5.9 wall s | 0.5.9 CPU s | 0.6.0 wall s | 0.6.0 CPU s | 0.6.0 / 0.5.9 | Median of per-round ratios [range] |
|---|---|---|---|---|---|---|---|
| DST decode, one thread, 5 channels, corpus frames, 3 min | generic | 7.449 | 7.40 | 7.068 | 7.02 | 0.949 | 0.945 [0.938-0.949] |
| DST decode, all threads, 5 channels, corpus frames, 10 min | generic | 1.598 | 50.17 | 1.464 | 45.83 | 0.916 | 0.917 [0.911-0.925] |

No row is slower than 0.5.9 beyond noise. The table was measured on the release-prep tree at `90f480a`; the later changes (#360, #362, #363) touch the C interface's `_free` functions, documentation and a new capacity API, none of the measured paths. **Compare within these tables only.** Not measured: the x86-64-v3 build; AVX-512 builds (`znver5`), Zen 5 and Raptor Lake; real output files; the first 128 KiB of 5 or 6 channels or without a seek index; cold runs of the whole-area cases; the C interface; `bench realtime` (`benchmarks/RESULTS.md`).

### Added

- Stack use is documented, in the C header's conventions and the crate's "Threads" documentation: calls may use up to 68 KiB of stack; give threads that call the library at least 136 KiB (twice the measured maximum, leaving room for the caller's own frames and callbacks). Measured on x86-64 Linux in release builds by painting thread stacks: the deepest call is `Layout::read_at` / `sb_output_layout_read` over a DST area on a CPU with AVX2 (66 KiB; 34 KiB on the generic path); opening, decoding, seek indexes and PCM conversion stay under 15 KiB, and 2 and 6 channels need the same. Pool threads use about 20 KiB of their 2 MiB. Unoptimised builds of the crate need about 2.1 MiB to decode DST on a CPU with AVX2, more than a rayon thread's default; the Rust documentation says how to avoid that. Not measured: AVX-512 at run time, Windows, macOS and aarch64. No API or behaviour changes (#202, #335).
- What an area's TOC claims against the audio sectors the image holds, for a consumer's own plausibility check of the area's length (specification 4, 4.2 and 4.8): `Disc::area_capacity(slot)` and `audio::area_capacity(area, image_sectors)` return a `#[non_exhaustive]` `AreaCapacity` with the audio sectors present `n`, the play time `P`, the largest track end frame, the capacity `7 x n` of 4.8 item 3, the most plain DSD frames those sectors can carry, `floor(n x 2045 / (C x 4704))` (`None` when the channel count `C` is not 1 to 6), and `frames_per_sector()`, `max(P, largest track end) / n`, which is positive infinity, never NaN, when `n` is 0. They read nothing beyond what the opened disc already knows, and the consumer chooses the threshold. In C: `sb_area_capacity` and `SbAreaCapacity`, additive to ABI 2 (#340, #363).

### Changed

- The DST lane default is decided, and it doesn't change: 2 lanes for stereo, 5 and 6 channels, and `dst::lanes` stays public (owner's decision of 2026-11-07, #203, #39). The name is covered by semantic versioning; the number it returns is a tuning value and may change. On 0.5.9's generic build, 1 lane against 2: Raptor Lake, 2 lanes wins all nine measured cells (1 lane takes 5.3 % to 43.7 % longer); Zen 5, 2 lanes is best or equal in every cell with a usable figure (6 channels at all threads is 1.7 % in 1 lane's favour, within the spread); Zen 2, 1 lane wins only at all logical CPUs, by 5 to 7 %, and callers there can choose it with `with_dst_lanes`. Not covered: 3-lane settings, x86-64-v3, this release's code after #330, and Zen 5 stereo at physical cores, whose runs were disturbed. `lanes(6)` on AVX-512 builds is a separate open question on #203.
- DST decoding is faster with the same output: Q comes from a table in the fast loop for 1, 2, 5 and 6 channels. On the Zen 2 benchmark host (3950X, generic build): stereo about 7 % less time at one and at all threads, 6 channels 5 to 6 % at one thread and 7 to 8 % at all threads, 5 channels 5 % and 9 %, 1 channel about 7 % at one thread (generated frames). 3 and 4 channels keep their path (#203, #330).
- **Breaking:** the TOC model is `#[non_exhaustive]`, so that fields the specification defines can be exposed later without another break: `toc::Toc`, `MasterToc`, `Area`, `Track`, `Locale`, `DiscText`, `AreaText` and `TrackText`. Field access is unchanged. Outside the crate, struct patterns need `..`, and struct literals no longer compile: take an `Area` or `Track` from a parsed `Toc` (its fields stay public and assignable), and make a text struct from `Default` (#39, #298). Migration: `docs/migrating-0.6.md`, M1.
- **Breaking:** the result structs `Warning`, `audio::AreaFrame`, `audio::TrackFrame` and `audio::AudioFrame` are `#[non_exhaustive]`, so that they can gain fields later. Field access is unchanged. Outside the crate, struct patterns need `..`, and struct literals no longer compile: a `TrackFrame` for `audio::decode_frame` now comes from the new `TrackFrame::new(number, content)`; the others are produced only by the library (#39, #301). Migration: `docs/migrating-0.6.md`, M2.
- **Breaking:** twelve field-carrying enum variants are `#[non_exhaustive]`, so that they can gain fields later: `seek::Speculation::{Pending, Confirmed, Refuted, Unverified}`, `seek::ResumedFrom::{Checkpoint, Discarded, Speculative}` and `Error::{MasterTocVersionUnsupported, ChannelCountUnsupported, TrackExceedsCapacity, NoSuchTrack, FrameRangeInvalid}`. Outside the crate, a pattern on one of them needs `..` even when it names every field, and the variants can no longer be built: a program that made its own `Error::NoSuchTrack` needs its own error type or variant for that case (#39, #302). Migration: `docs/migrating-0.6.md`, M3.
- **Breaking:** six enums are now `#[non_exhaustive]`, so that later variants are not breaking changes: `audio::FrameStatus`, `audio::DecodeOn` (`parallel` feature), `SectorFormat`, `toc::LoudspeakerLayout`, `toc::Speaker` and `toc::TrackTextItem`. Outside the crate, a `match` on one of them needs a wildcard arm. The variant `LoudspeakerLayout::Unspecified` is `#[non_exhaustive]` as well: a pattern on it needs `..`, and `LoudspeakerLayout::new` makes one. The other exhaustive enums are now documented as frozen, each with its reason, and a new test holds them to it, so a `match` on them keeps working with no wildcard arm: `Slot`, `toc::AreaKind`, `toc::DiscTextItem`, `toc::AreaTextItem`, `audio::FrameKind`, `audio::FrameContent`, `audio::Stored` and `log::Level`. So are the structs `toc::Version`, `Genre`, `Date`, `Timecode` and `AreaPointer`, which need no migration (#39, #304). Migration: `docs/migrating-0.6.md`, M4.
- **Breaking for anyone relying on these items (hidden, still compiling):** FFI plumbing, tuning values and low-level text helpers are out of the semver promise: `#[doc(hidden)]`, each with a note that it may change or go in any release. That covers the plumbing methods of `Continuation` and the enum `Opening`, `audio::{MAX_PARALLEL_BATCH, PARALLEL_FRAMES_PER_THREAD}`, `seek::{MAX_CLAIMS, MAX_EVENTS, HINT_REACH, REPLAY_SECTORS}` and `text::{Charset, decode, decode_string, fixed_bytes, fixed_text, language, string_bytes, text_string}`. `dst::lanes` stays public. No behaviour or C ABI changed, and calls still compile (#39, #297). Migration: `docs/migrating-0.6.md`, M5.
- **Breaking, in small part:** `scarletbook_pcm::Cutoff::Default` is renamed `Cutoff::Standard`, the same filter with the same output bytes; `Cutoff::default()` and `SB_PCM_CUTOFF_DEFAULT` are unchanged. `Cutoff::Default` stays as a deprecated constant, to be removed before 1.0, so expressions and patterns written `Cutoff::Default` on a `Cutoff` value still compile. What breaks now: `use Cutoff::Default` and `Cutoff::Default` in a pattern on a `&Cutoff` no longer compile, `{:?}` of the filter prints `Standard`, and **under `use Cutoff::*` a pattern `Default` silently becomes a catch-all binding, with only warnings, so a match takes the wrong branch**. Migration: `docs/migrating-0.6.md`, N5 (#39, #291).
- **Breaking, in small part:** `audio::Stored::Plain` is renamed `Stored::Dsd`, as in `FrameContent::Dsd`, `FrameKind::Dsd`, `FrameStatus::Dsd` and `SB_FRAME_KIND_DSD`; its meaning does not change. `Stored::Plain` stays as a deprecated constant, to be removed before 1.0, so expressions and patterns written `Stored::Plain` on a `Stored` value still compile. What breaks now: `use Stored::Plain` and `Stored::Plain` in a pattern through a reference no longer compile, `{:?}` of the value prints `Dsd`, and **under `use Stored::*` a pattern `Plain` silently becomes a catch-all binding, with only warnings, so a match takes the wrong branch; treat those warnings as errors while migrating**. Migration: `docs/migrating-0.6.md`, S1 (#39, #300).
- **Breaking:** `Image::read_sectors` returns `Result<(), Error>` instead of `Result<bool, Error>`. A range past the image's last sector, formerly `Ok(false)`, is the new `Error::SectorRangeInvalid { lsn, count, sectors }`, and leaves the buffer as it was. An empty buffer, formerly `InvalidArgument`, reads nothing and succeeds for any `lsn` up to the sector count, as `sb_disc_read_sectors` does with a count of 0. It can't be deprecated first, since the name stays and only its return type changes. Code that uses the `bool` directly gets a compile error. Code that doesn't still builds and behaves differently for a read past the end, and `cargo semver-checks` does not detect it: `?;` now returns the error, `.unwrap();` panics, and `.is_ok()` is false. The C ABI is unchanged. Migration: `docs/migrating-0.6.md`, S3 (#39, #303).
- **C library, breaking for packaging: the shared library is now named after the C ABI on Linux and macOS, and ABI 2 is declared the 1.0 ABI.** The header and the ABI do not change, and archive names stay the same. In the archives, the Linux library is the file `lib/libscarletbook_ffi.so.2` with that SONAME, and the macOS one is `lib/libscarletbook_ffi.2.dylib` with the install name `@rpath/libscarletbook_ffi.2.dylib` and compatibility version 2.0.0; `libscarletbook_ffi.so` and `libscarletbook_ffi.dylib` are now links to them. **What a C consumer changes:** no source. A program linked against this release needs the versioned file at run time, so ship or install that file under that name, not only a file called `libscarletbook_ffi.so`. A program linked against an earlier release recorded the old name, so it needs the link kept next to the library until it is relinked. Programs loading the library by path, as with ctypes, keep working under the old name if the link is kept, and should move to the versioned one. On Windows nothing changes: `scarletbook_ffi.dll` keeps its name (#39, #294). Migration: `docs/migrating-0.6.md`, M8.
- The documentation now names every accessor that can return a control character, in the Rust docs and on each C function that returns these strings. ISRCs (`Track::isrc`, `SB_TRACK_STRING_ISRC`) and catalogue numbers (`MasterToc::album_catalog_number`, `disc_catalog_number`, `SB_DISC_STRING_*_CATALOG_NUMBER`) keep C0, DEL and C1 (specification 2, 2.6.5 item 4). Language codes (`Locale::language`; `SbTextChannel.language` in C) keep every character but trailing NUL and space, U+0000 included (2.5.1 item 1). Disc, area and track text (`DiscText`, `AreaText`, `TrackText`; `sb_disc_string`, `sb_area_string`, `sb_track_string`, `sb_disc_toc`) replace C0 with a space, so ESC is the only escape-sequence starter removed, but keep U+007F and U+0080 to U+009F (2.6.4 item 3). The docs give U+009B (CSI) as an example: it starts a terminal control sequence on terminals that honour 8-bit controls. They say that callers that print these values or put them in tags or file names should escape or filter them for that use. For C they say that `*len` and `SbTocStr.length` give the authoritative length (not strlen), and that language codes are two bytes that are not NUL-terminated. They point to the stored-byte accessors where they exist. Documentation only; no behaviour change (#342, #360).
- C ABI: the `_free` functions no longer clear or set the thread's last error message, so a C caller that frees its handles on an error path and then calls `sb_last_error_message()` gets the failed call's message instead of an empty string. This holds for a NULL handle and for a free refused because the handle is in use from a callback, which no longer leaves a message about the refusal. Only a panic caught inside a `_free` function still sets the message, as a bug report. `sb_last_error_message` is empty if the last call that returns a status succeeded. No signature changes; `SB_ABI_VERSION` stays 2 (#347, #362).

### Deprecated

- **One reading entry point per concept.** `Disc::{track_audio_cursor, range_audio_cursor, range_audio_cursor_indexed, track_frames_cursor, range_frames_cursor, range_frames_cursor_indexed}`, and `for_range`, `for_range_indexed` and `for_range_speculative` on `TrackCursor` and `TrackAudioCursor`, are deprecated in favour of `Disc::{audio_cursor, frames_cursor}` and `open_range`, with the new `FrameSpan` and `CursorOptions`. They still work and give the same frames, errors and log lines, but **will be removed before 1.0, which will break callers then**. No C ABI changed (#39, #305). Migration: `docs/migrating-0.6.md`, M6.
- `dst::MAX_CHANNELS` (a `usize`) and `dst::FL` repeat `audio::MAX_CHANNELS` (a `u8`) and `audio::FRAME_BYTES_PER_CHANNEL`, and are deprecated, to be removed before 1.0. Nothing breaks until then, except for builds that deny warnings. `scarletbook_pcm::MAX_CHANNELS` and `scarletbook_pcm::FRAME_BYTES_PER_CHANNEL` stay, because that crate does not depend on `scarletbook`. Migration: `docs/migrating-0.6.md`, N3 (#39, #293).
- The names of provisional readings and the sector they start at now match the C interface's `SbSpeculation.claim`: on `AreaCursor`, `TrackCursor`, `TrackFrames`, `TrackAudioCursor` and `TrackAudio`, `speculation_id()` is renamed `claim()` and `counts_from()` is renamed `claim_lsn()`. The old names stay as deprecated methods, to be removed before 1.0; nothing breaks until then, except for builds that deny warnings. Migration: `docs/migrating-0.6.md`, S1 (#39, #300).
- `Image::open_with_log`, `Image::with_format`, `Image::with_format_and_log`, `Disc::open_with_log`, `Disc::open_with_format` and `Disc::open_with_format_and_log` give way to one options value, `Image::options()` and `Disc::options()` (the new `ImageOptions` and `DiscOptions`). `open` and `share_with_log` stay. The old constructors are deprecated, to be removed before 1.0; nothing breaks until then, except for builds that deny warnings. Migration: `docs/migrating-0.6.md`, S1 (#39, #300).
- `AreaIndex::{checkpoints, hints, confirmations, pending_claims}`, which read like collections and return a count, are renamed `checkpoint_count`, `hint_count`, `confirmation_count` and `pending_claim_count`. The old names stay as deprecated methods, to be removed before 1.0; nothing breaks until then, except for builds that deny warnings. Migration: `docs/migrating-0.6.md`, S2 (#39, #299).

### Fixed

- A verdict held on a seek index that a disc kept reached `Disc::take_seek_events` late and more than once, and the repeats could push other verdicts out of its bounded queue. This took an index set with `Disc::set_seek_index` before `AreaIndex::take_events` took its verdicts, for example after `AreaIndex::merge` past `seek::MAX_CLAIMS`, or a merge past that limit inside `Disc::extend_seek_index` when another disc replaced the index during the call. Such verdicts now go to the queue at once, once (#310, #327).
- C interface: a verdict on a provisional reading that a seek index handle held when given to `sb_disc_seek_index_set` was returned twice, by `sb_seek_index_event_next` on that handle and by `sb_disc_seek_event_next`, with the same claim number. The disc now keeps a copy without the handle's pending verdicts, so each is returned once, by the handle: drain the handle's queue to see them (#326, #353).
- Unoptimised (debug-profile) builds of the crate no longer overflow the 2 MiB stack of a rayon or new Rust thread when they decode DST on a CPU with AVX2: the DST decoder inlines one copy of most of its symbol loops where it inlined three, so decoding a frame needs about 1.2 MiB instead of 2.1 MiB and `Layout::read_at` about 1.6 MiB instead of 2.5 MiB (x86-64 Linux, debug info on). Output is unchanged to the bit; release builds are as fast or slightly faster and use less stack. No API changes (#336, #338).
- A DSF ID3 tag that `output_layout` builds now counts its 10-byte header and its footer or padding against `MAX_TAG_BYTES` (1,048,576 bytes, header and footer included), as a caller's raw tag always has. With the footer (the default) a built tag could be up to 20 bytes over the maximum, a tag the library itself refuses as a raw tag; such a tag is now refused with `Error::InvalidArgument("the tag is larger than MAX_TAG_BYTES")` (`SB_STATUS_INVALID_ARGUMENT` from the C API). Without a footer the check counted 10 bytes too many, so a tag of 1,048,567 to 1,048,576 bytes with padding was refused; it is now written. A tag of exactly `MAX_TAG_BYTES` is written either way, and reads back unchanged as a raw tag (#343, #352).
- C header documentation: the run-time handle check is described as catching a call made from a callback on the same thread, not as a guard between threads, and the Threads convention says that using one handle from two threads at once is undefined behaviour unless the caller locks around it. The Status convention and `sb_last_error_message` say that every `_free` function overwrites the thread's last error message, so read it before freeing handles on an error path. The Handles convention and `sb_track_audio_open_next` say that passing the address of `prev` as `out` to any `_open_next` function leaks `prev` on failure. No function's behaviour changes (#347, #349).
- TOC documentation: `Area::slot` and `Area::kind` now say why both exist (the kind comes from the signature, so a slot can yield an area of the other kind), and the module says that TOC LSNs keep their stored `u32` width where `Image` and `seek` use `u64`. No behaviour changes (#39, #323).
- C header documentation: the conventions now state how each kind of call reports the buffer size it needs, what a `const` handle pointer does and does not promise, and all twelve opaque handles. `sb_track_audio_read` and `sb_dst_decode` now state what `SB_STATUS_BUFFER_TOO_SMALL` does; `sb_dst_decode`'s `out` may be NULL. No function's behaviour changes (#39, #322).
- The documentation builds without default features again: two comments linked `TrackAudioCursor::set_decode_on`, which exists only with the `parallel` feature, and CI now builds the docs without default features too (#337).
- Documentation of `MAX_PARALLEL_BATCH`: a DST decoder lane's tables are about 0.9 MB in use, but keep the capacity they grew to, and up to 1.33 MB per lane was measured in the memory soak (#246); the note said only "about 0.9 MB". Documentation only; no behaviour changes (#224, #356).

## [0.5.9] - 2026-11-06

**A DST lanes setting to measure with, faster multichannel DST, a correction to what `verify_speculation` promised, and DSF partial rows that can now fail on a bad neighbour.**

- **DST lanes.** `TrackAudioCursor::with_dst_lanes` sets how many DST frames decode together on each thread, and `bench decode --lanes K` exposes it in the kit. The default and the decoded output are unchanged. On the Zen 2 benchmark host 1 lane wins only at all 32 logical CPUs (stereo 6.6 %, 6 channels 5.0 %) and costs 10 to 26 % at 16 threads and one thread, so the default stays until 0.6.0 and is chosen from consumers' numbers on their own CPUs (#203, #318).
- **Faster DST decoding, same output.** Against 0.5.8, medians of per-round ratios, 5 channels: 8.5 % on one thread, 14.4 % on all threads. 6 channels: 3.5 % and 3.9 %. Stereo: about 1 % and 3.5 %. Measured on Zen 2 only: AVX-512 builds (the `znver5` benchmark build) and Zen 5 are not measured (#203, #288).
- **`verify_speculation`: check your use of it.** Code relying on the old `Disc::verify_speculation` (C: `sb_disc_seek_verify`) promise may have seen an unverified reading treated as complete. It could report complete while a reading registered by another disc during the call had no verdict and no event. Existing `while !verify_speculation(..)` loops now finish the job; a caller that must have every verdict runs the loop once more after its readers finish and checks each reading. No delivered frame changes (#310, #313).
- **DSF partial rows: a good read can now fail because of a bad neighbour.** A hard read error in a neighbouring position can fail a read whose own bytes lie elsewhere, and an `sb_output_layout_fill` call that supplies fewer positions than `sb_output_layout_positions` lists now gets `SB_STATUS_INVALID_ARGUMENT`. Reads under 4 KiB from a DST area do more work: with `Layout::read_at`, stereo 1 KiB calls take 1.6 % longer and 512-byte calls 5.1 % longer. In return, plain-DSD areas read in order gain 28 to 51 % at 128 KiB calls, as measured by #287; "Changed" has the full entry (#202, #287).
- **PCM:** DSD to PCM is about 8 % faster on this host, byte-identical (#224, #285, #286).

**#202 and #203 are not closed by this release.** Neither has consumer numbers from this kit: please re-measure with it. `docs/migrating-0.6.md` is an unfinished skeleton, and nothing on it applies to 0.5.x.

| Path | Build | 0.5.8 wall s | 0.5.8 CPU s | 0.5.9 wall s | 0.5.9 CPU s | 0.5.9 / 0.5.8 | Median of per-round ratios [range] |
|---|---|---|---|---|---|---|---|
| DST decode, one thread, 2 channels | generic | 6.131 | 6.09 | 6.079 | 6.05 | 0.991 | 0.992 [0.981-0.998] |
| DST decode, one thread, 2 channels, corpus frames | generic | 20.296 | 20.19 | 20.013 | 19.91 | 0.986 | 0.988 [0.984-0.988] |
| DST decode, all threads, 2 channels | generic | 4.010 | 126.66 | 3.880 | 122.07 | 0.968 | 0.965 [0.962-0.979] |
| DST decode, one thread, 6 channels | generic | 7.645 | 7.61 | 7.373 | 7.33 | 0.964 | 0.965 [0.962-0.984] |
| DST decode, all threads, 6 channels | generic | 2.593 | 81.48 | 2.483 | 78.23 | 0.958 | 0.961 [0.951-0.964] |
| plain-DSD area to DSF, 2 channels | generic | 0.553 | 0.55 | 0.557 | 0.55 | 1.006 | 1.004 [0.987-1.006], unresolved |
| plain-DSD area to DSF, 5 channels | generic | 0.611 | 0.60 | 0.600 | 0.59 | 0.982 | 0.970 [0.957-0.982], see below |
| DST area to DSF, all threads | generic | 4.041 | 128.03 | 3.900 | 123.38 | 0.965 | 0.965 [0.875-0.970] |
| DSD to PCM, stereo DSD64 to 176.4 kHz 24-bit, one thread | generic | 4.25 / 4.16 / 4.16 ms per s | - | 3.89 / 3.82 / 3.82 ms per s | - | 0.918 | - |
| open to parsed TOC, page cache | generic | - | - | 16.9 µs | - | - | - |
| open to parsed TOC, evicted | generic | - | - | 213.3 µs | - | - | - |
| first 128 KiB, 2 channels, plain DSD, page cache | generic | - | - | 29.5 µs | - | - | - |
| first 128 KiB, 2 channels, plain DSD, evicted | generic | - | - | 244.5 µs | - | - | - |
| first 128 KiB, 2 channels, DST, one thread, page cache | generic | - | - | 4003.0 µs | - | - | - |
| first 128 KiB, 2 channels, DST, one thread, evicted | generic | - | - | 4407.9 µs | - | - | - |
| first 128 KiB, 2 channels, DST, all threads, page cache | generic | - | - | 2362.9 µs | - | - | - |
| first 128 KiB, 2 channels, DST, all threads, evicted | generic | - | - | 4335.2 µs | - | - | - |

Medians of alternating runs on the Zen 2 benchmark host (AMD Ryzen 9 3950X), 2026-11-06, under the machine's shared benchmark lock, generic build, kits from `benchmarks/compare.sh`, a fresh copy of each program and image per round, images in the page cache, output to `/dev/null`: 5 rounds, 3 for the 20-minute DST case and for DSD to PCM; 122 runs, none failed. The DSD-to-PCM cells are at 1 / 8 / 256 frames per call. `open` and `first` are new in this kit, so 0.5.8 has no figure for them.

**Plain-DSD rows:** differences under about 2 % are unresolved (#259), so they aren't stated as "within 1 %". The 2-channel row (1.006; 3 of 5 rounds slower, 2 faster) shows no change either way. The 5-channel row is 0.982 as a ratio of medians, under that line, and 0.970 as the median of per-round ratios, with all 5 rounds below 1. That agrees with #287's own 0.970 over 7 rounds, which is why it is reported as a likely gain and not a settled figure.

No row is slower than 0.5.8 beyond noise. The run list has no 5-channel DST case, so those figures come from a separate run with the same method (5 rounds, 20 runs, none failed):

| Path (not in the run list) | Build | 0.5.8 wall s | 0.5.8 CPU s | 0.5.9 wall s | 0.5.9 CPU s | 0.5.9 / 0.5.8 | Median of per-round ratios [range] |
|---|---|---|---|---|---|---|---|
| DST decode, one thread, 5 channels, corpus frames, 3 min | generic | 8.147 | 8.11 | 7.459 | 7.41 | 0.916 | 0.915 [0.913-0.923] |
| DST decode, all threads, 5 channels, corpus frames, 10 min | generic | 1.877 | 59.01 | 1.608 | 50.41 | 0.856 | 0.856 [0.853-0.857] |

**Compare within these tables only.** Not measured: the x86-64-v3 build; AVX-512 builds, Zen 5 and Raptor Lake; real output files; the first 128 KiB of 5 or 6 channels or without a seek index; cold runs of the whole-area cases; the C interface (`benchmarks/RESULTS.md`).

### Added

- `bench dsf --via tracks|tracks-scan|tracks-speculative|tracks-unverified`: one DSF file per track from the public `Layout` API, in order or from N workers, to measure a per-track writer (#202, #306).
- The `scarletbook::output` documentation and the README give a recipe for writing an area's tracks as separate DSF files: one after another with `Disc::continue_range_audio`, or from workers, and which forms establish exactness under specification 4, 4.2 item 8 (#202, #309).
- `TrackAudioCursor::with_dst_lanes` sets how many DST frames decode together on each thread, for measuring on your own CPU, and `bench decode --lanes K` exposes it in the benchmark kit. The default and the decoded output are unchanged; an out-of-range count is the new `Error::DstLanesInvalid` (#203, #318)
- `bench open` (open to parsed TOC) and `bench first` (first bytes of a track from a seek), timed inside the process, in the page cache or, with `--evict`, with the image evicted before each iteration. `benchmarks/evict.py` evicts without root and fails if any page stays cached; the run list gains warm and cold cases for both, and `table.py` shows their medians in microseconds (#224, #281).
- `scarletbook-synth --dst-frames stress` now writes 5-channel areas, and `--dst-order N` writes stress frames of one filter order, to time one history-byte branch of the DST decoder. The 2- and 6-channel images and the `dst_frames` benchmark's frames are unchanged (#264, #276).
- `docs/migrating-0.6.md`, a skeleton for moving to 0.6.0, is on `main` and **not finished**: most of its sections still say "Not written yet", it has no entry to act on for 0.5.x, and each breaking change adds its before-and-after when it lands (#39).
- A "Threads" section in the documentation of `scarletbook` and `scarletbook-pcm` says which public types are `Send` and `Sync` and what that means for reading, and compile-time assertions in both crates now hold them to it. No type changed (#39).
- A "Stability" section in the README states the version policies: semantic versioning of the Rust API and the C ABI from 1.0 and what it does not cover, the version the crates share, and when the minimum supported Rust version can rise (#39).

### Changed

- DSF output fills the rows that a read only touches, at the ends of each `Layout::fill` or `Layout::read_at` call, whole in a scratch row and copies from it, instead of a byte at a time. The output is unchanged.
  - **A good read can now fail because of a bad neighbour.** A read of part of a row now asks for all of that row's positions, which `Layout::positions_for` always allowed. If one of them fails, the read fails, even when its own bytes lie in another position or are only pad: on damaged media, a read that used to succeed can now return an I/O error. A `Layout::fill` source that errors only on the extra position fails it the same way, and an `sb_output_layout_fill` call that supplies fewer positions than `sb_output_layout_positions` lists now gets `SB_STATUS_INVALID_ARGUMENT`.
  - **Reads under 4 KiB from a DST area do more work.** With `Layout::read_at`, stereo: 1 KiB calls fetch 15 % more frames and take 1.6 % longer, 512-byte calls 22 % more frames and 5.1 % longer. 6 channels at 1 KiB: 12 % more frames, time not measured. At 4 KiB and above frames rise by at most 0.3 % and no time gets worse.
  - **Gains, on plain-DSD areas.** Reading in order (`Layout::fill`): 2 to 5 % at 4 MiB calls, 28 to 51 % at 128 KiB. `Layout::read_at`: about 2 % at 4 MiB, 11 to 28 % at 128 KiB, 7 to 10 % at 4 KiB. On DST areas, `Layout::read_at` from 4 KiB up gains 0 to 3 % (#202, #287).
- DST decoding on one thread is faster for 5 and 6 channels (and slightly for stereo), with the same output: fewer instructions per symbol, and 5 channels in 2 frames use the smaller loop body. Measured on Zen 2 only: AVX-512 builds (the `znver5` benchmark build) and Zen 5 are not measured (#203, #288)
- `scarletbook-pcm`: the conversion to 24-bit integers vectorises on AVX2. Output is byte-identical (#224, #286).
- `scarletbook-pcm`: the first filter stage reads its input window as 64-bit words instead of one byte at a time. Output is byte-identical (#224, #285).
- `benchmarks/run.py` records free memory by block size before and after each round's image copy (`free_kib_by_order_before`, `free_kib_by_order_after`) and the host's `page_bytes`. `benchmarks/README.md` reports that free-memory state changes plain-DSD seconds between byte-identical image copies; it is a candidate for the gap between tables, not an established cause (#259, #289).
- Correction to the 0.5.8 notes: "no row moves by more than 1 %" does not hold for plain-DSD rows, where differences under about 2 % are unresolved (#259, #289).
- `benchmarks/table.py` lists under the table any cell whose rounds differ by more than 5 % in wall time, with its range: byte-identical fresh copies of an image can differ by 12 % on the plain-DSD cases, which a median of a few rounds does not even out. `benchmarks/README.md` says to judge changes on ratios and to compare seconds only as ranges (#259).
- `CONTRIBUTING.md` and the crate comments describe what the two `dispatch` modules cover today: in `scarletbook-pcm` also `pick`, `quantise` and `interleave`, in the core crate also the output path. No code changes (#269).
- `unsafe_code` is now denied in every target outside `scarletbook-ffi`, through the workspace lints and the `scarletbook-conformance` and fuzz manifests, not only in the core and PCM libraries and the `bench` binary: their integration tests, benches and examples could otherwise add `unsafe` unnoticed. No code changes; nothing was using it (#269).
- A comment in `bench dsf --via tracks*` states that its speculative verified mode makes one `verify_speculation` call and checks events, so a reading with no verdict passes silently (#310, #317).
- The documentation of `DecodeOn` and the README now state that, with the `parallel` feature, rayon 1 is part of the public Rust API by decision: `DecodeOn::Pool` keeps taking an `Arc<rayon::ThreadPool>`, and a rayon 2 would need a new major version of `scarletbook`. Documentation only: nothing breaks and no code changes (#39, #292).

### Fixed

- `bench realtime --seeks` and `bench dsf --chunk` are limited (1,000,000 seeks, 256 MiB) and give a `usage-error` beyond it, instead of sizing an allocation from an unbounded number (#255, #316).
- Tests now fail if run-time CPU detection stops choosing the AVX2 code on a capable CPU, in `scarletbook` and `scarletbook-pcm`, or if a new `DstDecoder` or `Converter` stops using it; the bit-identity tests would still have passed (#267, #282).
- Miri now runs the dispatched AVX2 code. Until now it ran only the generic code on both sides of the dispatch tests, because `is_x86_feature_detected!` under Miri reports only what the build enabled. The `core-dispatch-avx2` and `pcm-dispatch-avx2` slices of the main and nightly Miri run build with the target features, and fail if those flags are lost; `scarletbook-pcm`'s dispatch test was in no slice before. Pull request CI is unchanged (#267, #283).
- The `scarletbook` package includes `tests/common/`, which the unit tests load with `#[path]`; before, `cargo test --lib` on the unpacked crates.io tarball failed to compile (#268, #270).
- `cargo test --lib` of the packaged `scarletbook` crate passes: the two dispatch tests that read the corpus are skipped, with a message, when it is absent (only in an unpacked crate; in the repository a missing corpus still fails), and the decoding tests that need more than the 2 MiB default stack at opt-level 0 run on a larger one. Tests only (#268, #315).
- `benchmarks/run.py` skips a case for a kit whose `bench` has no such command, as it does for a missing program, instead of counting quick usage errors as failed runs, which could hide a real failure when comparing an older version. A case no kit can run fails the run (#290, #296).
- C interface: a core `Error` with no status of its own would have reached C callers as `SB_STATUS_PANIC`. It now gets a status of its class, and a test fails until every variant has its own. No error that exists today changes status or message (#308, #311).
- `Disc::verify_speculation` and C `sb_disc_seek_verify` could report complete while a provisional reading registered by another disc during the call had no verdict and no event. Code relying on the old promise may have seen an unverified reading treated as complete. They now report not complete until a later call has checked it, so existing `while !verify_speculation(..)` loops finish the job; each call still makes one round within its `max_sectors`. Complete covers the readings the index held at the call's last look: one that joins later has no verdict until a later call, so a caller that must have every verdict runs the loop once more after its readers finish and checks each reading. `extend_seek_index` still reports only whether the index is complete. No delivered frame changes (#310, #313).
- `Disc::extend_seek_index`, `Disc::verify_speculation` and their C forms made no progress from an empty kept index when `max_sectors` was at most the index's interval: `AreaIndex::merge` dropped the first checkpoint, so a loop over them never ended. Every call now adds a checkpoint, as documented, including at `max_sectors` 0 (#310, #313).

## [0.5.8] - 2026-11-05

**A seek-index fix, a real-time report in `bench`, a corrected benchmark runner, and the first
`scarletbook-ffi` on crates.io. No speed change.** `AreaIndex::from_bytes` now refuses a form of
saved index that it used to load and then save differently (#253, found by fuzzing). The library
never writes that form, so saved indexes keep loading. `bench realtime` reports what a real-time
player needs from one thread (#255). `benchmarks/run.py` times a fresh copy of the image each round,
which removes the cause of the high kit timings found under #248: **seconds from this kit are not
comparable with seconds from the 0.5.7 kit's `run.py`.** Please use this kit to re-measure #202 and #203, which are not at
target yet and continue towards 0.6.0.

| Path | Build | 0.5.7 wall s | 0.5.7 CPU s | 0.5.8 wall s | 0.5.8 CPU s | 0.5.8 / 0.5.7 |
|---|---|---|---|---|---|---|
| DST decode, one thread, 2 channels | generic | 6.131 | 6.10 | 6.129 | 6.10 | 1.000 |
| DST decode, one thread, 2 channels, corpus frames | generic | 20.302 | 20.21 | 20.343 | 20.24 | 1.002 |
| DST decode, all threads, 2 channels | generic | 4.030 | 126.82 | 4.012 | 126.63 | 0.996 |
| DST decode, one thread, 6 channels | generic | 7.592 | 7.54 | 7.593 | 7.55 | 1.000 |
| DST decode, all threads, 6 channels | generic | 2.582 | 81.26 | 2.584 | 81.41 | 1.001 |
| plain-DSD area to DSF, 2 channels | generic | 0.555 | 0.55 | 0.555 | 0.55 | 1.001 |
| plain-DSD area to DSF, 5 channels | generic | 0.614 | 0.61 | 0.618 | 0.61 | 1.007 |
| DST area to DSF, all threads | generic | 4.025 | 127.45 | 4.037 | 128.04 | 1.003 |
| DSD to PCM, stereo DSD64 to 176.4 kHz 24-bit, one thread | generic | 4.23 / 4.12 / 4.12 ms per s | - | 4.25 / 4.15 / 4.14 ms per s | - | 1.007 |

Medians of alternating runs on the Zen 2 benchmark host (AMD Ryzen 9 3950X), generic build, kits
from `benchmarks/compare.sh`, a fresh copy of each program and image per round, images in the page
cache: 5 rounds, 3 for the 20-minute DST case and for DSD to PCM.

No row changes by more than 1 %. **Compare within this table only:** its plain-DSD rows read about
14 % above the figures published for the same 0.5.7 library in `benchmarks/RESULTS.md` (0.555 s against
0.483 s stereo, 0.614 s against 0.540 s 5-channel), on the same fixed runner. The cause has not been
found, and #248's fix did not rule out every such difference between runs. The DST rows agree within
1 %, and the ratios within this table are unaffected. Not measured: the x86-64-v3 build; TOC open, first 128 KiB, cold
runs and the C interface (`benchmarks/RESULTS.md`).

### Added

- crates.io: `scarletbook` and `scarletbook-pcm` are published there from 0.5.7, and `scarletbook-ffi` from 0.5.8; every release tag publishes them from the release workflow with crates.io trusted publishing, after all release checks (RELEASING.md). The README has install lines and a note to pin `=0.5.x` where PCM bytes must not change (#249, #251).
- `bench realtime IMAGE --area 2ch|mc [--seeks N]`: what a real-time player needs from one thread: the mean real-time factor, the slowest 1-second window, the 99.9th percentile and slowest frame times, the frames over 6.7 ms, and the time from a seek to its first frame with a complete seek index (#255).

### Fixed

- `AreaIndex::from_bytes` refuses a saved index (format version 3) with more confirmations than its saved maximum of seek hints. It used to load it and drop them silently, so that saving the index gave other bytes. The library never writes such an index, so saved indexes keep loading (#253).

## [0.5.7] - 2026-11-02

**Faster plain-DSD output, and the 0.5.6 one-thread stereo DST regression won back.** Reading an
area no longer copies each audio sector before demultiplexing it (#230), 5-channel DSF rows are
separated in 32-bit words (#237), and stereo DST decoding on one thread is back to 0.5.5's speed
(#241). A raw-image disc no longer keeps its TOC read buffer once opened (#228). Leak checks now
run in CI: LeakSanitizer over the C interface's tests and the C smoke test, a test that the heap
does not grow over open/seek/decode/close cycles, and a weekly memory soak (#238). Each release
also publishes a benchmark kit for timing scarletbook on other machines, and pull requests that
touch the library run a benchmark regression check (#226). Output is identical to the bit in
every case.

Measured on the Zen 2 benchmark host, an AMD Ryzen 9 3950X (16 cores / 32 threads), generic x86-64
build, 0.5.6 against 0.5.7, with `benchmarks/compare.sh` (statically linked kits, synthetic images
of `benchmarks/runlist.json` in the page cache, output to `/dev/null`, a fresh copy of each
program per run, alternating ABBA rounds: 5 per case, 3 for the 20-minute DST case and PCM). Times
are the medians of the wall times; changes are the medians of the per-round ratios. Other CPUs may
differ.

| Path | 0.5.6 | 0.5.7 | Change | `bench` change (#239)¹ | Library change¹ |
|---|---|---|---|---|---|
| 78-min plain-DSD stereo area to DSF, one thread | 0.704 s | 0.483 s | 31 % faster | 14 % faster | 20 % faster |
| 30-min plain-DSD 5-channel area to DSF, one thread | 0.977 s | 0.540 s | 45 % faster | 9 % faster | 40 % faster |
| 25-min plain-DSD 6-channel area to DSF, one thread² | 0.712 s | 0.510 s | 28 % faster | 13 % faster | 18 % faster |
| DST stereo, 20-min area (corpus frames), one thread | 20.62 s | 20.18 s | 2.1 % faster | ±0 | 2.1 % faster |
| DST stereo, 5-min area (stress frames), one thread | 6.23 s | 6.10 s | 2.0 % faster | 0.5 % faster | 1.5 % faster |
| DST 6-channel, 2-min area, one thread | 7.68 s | 7.62 s | 0.6 % faster | 0.1 % faster | 0.7 % faster |
| **DST stereo, 50-min area, all 32 threads** | **3.99 s** | **4.03 s** | **0.5 % slower** | 0.5 % slower | 0.1 % slower |
| DST 6-channel, 10-min area, all 32 threads | 2.61 s | 2.59 s | 1.0 % faster | 1.0 % faster | 0.3 % faster |
| DSD64 stereo to 176.4 kHz 24-bit PCM, 8 frames per call (CPU per second of audio) | 4.08 ms | 4.09 ms | ±0 (unchanged code) | – | – |

¹ **The measuring tool changed in this release.** #239 made `bench` open images with
`BufReader::new` instead of a 1 MiB `BufReader`, as consumers' code does; the 1 MiB buffer copied
every byte twice. So the 0.5.6 → 0.5.7 DSF and DST rows include a tool effect that is not a
library gain. To separate them, the 0.5.6 library was also timed with 0.5.7's `bench` in the same
rounds: "`bench` change" is that build against 0.5.6, "Library change" is 0.5.7 against it. The
library gains are #230 (all DSF rows) and #237 (5 channels): in #237's own same-tool measurement,
the 30-minute 5-channel area went from 0.875 s to 0.637 s.
² Not in the run list: the command of `dsf-plain-5ch` on a 25-minute 6-channel image
(`scarletbook-synth IMAGE --minutes 25 --area mc --mc-channels 6 --kind dsd`).

**Correction (#248):** the seconds of the three plain-DSD rows in the first version of this table
were inflated by the benchmark runner `run.py` (0.861 / 0.579 s stereo, 1.092 / 0.607 s 5
channels, 0.757 / 0.538 s 6 channels): the image files it timed sat in page-cache pages that made
every read slower. The rows above are a re-run with the fixed runner. The "Change" column moved by
at most 2 points; the `bench` / library split moved by up to 5 points. The DST and PCM rows are
unchanged.

The stereo all-threads row was slower in every round (0.3-1.2 %), and 1.3 % slower in a second,
separate run; the split puts it on the tool change, not the library.

**#241 wins back the 0.5.6 regression.** 0.5.6 decoded stereo DST on one thread 2.9 % slower
than 0.5.5. In #241's fresh-copy measurements on the Zen 2 benchmark host, stereo on one
thread is ±0 % against 0.5.5 and 2.67 % faster than 0.5.6, and 6 channels on one thread 5.9 %
faster than 0.5.5.

**Not yet at target:** the DSF output targets of #202 and the DST decoding targets of #203 are
not met in 0.5.7; the work continues in 0.5.8.

### Added

- `bench dsf --via parallel`: one thread reads the area while `--threads` workers fill whole DSF rows and write them at their offsets, a prototype of a parallel DSF writer on the public `Layout` API (#202, #224, #232).
- A CI benchmark regression check: `.github/workflows/bench-regress.yml` builds `bench` and the scarletbook-pcm `callpattern` example from the base and the head, times single-threaded DST decoding, plain DSD to DSF and DSD to PCM alternately on one runner (a fresh copy of each program per run), and fails when the median of the per-round ratios head / base of CPU time exceeds 1.15. It runs beside CI on pull requests that touch library sources, on main and nightly, and is not a required check (#226, #233).
- A benchmark kit for timing scarletbook on consumers' machines: each release publishes `scarletbook-benchkit-<version>-x86_64-linux.tar.gz` and its `-v3` (x86-64-v3) twin, with statically linked `bench`, `scarletbook-synth` and the scarletbook-pcm `callpattern` example, a machine-readable run list of the #224 cases (`benchmarks/runlist.json`) and a reference runner that prints one JSON line per timed run; pushes to main upload the same archives as workflow artifacts. A release now has 28 assets instead of 24 (`benchmarks/README.md`; #226, #231).
- `benchmarks/RESULTS.md`: measured figures per release on a Zen 2 machine (3950X), generic and x86-64-v3 builds, with the method and the paths not measured; `benchmarks/compare.sh` reproduces them by timing two revisions against each other with the run list (#226, #231).

### Changed

- `bench` opens images with `BufReader::new`, as the documentation does, instead of a 1 MiB `BufReader`: area reads (64 KiB) then go straight into the cursor's buffer, one copy from the kernel, as with `sb_disc_open`. The 1 MiB buffer copied every byte twice. A 78-minute plain-DSD stereo area writes to DSF in 0.50 s instead of 0.61 s on one thread on the Zen 2 benchmark host (3950X). The library is unchanged; compare figures across this change with that in mind (#202, #239).
- Reading an area no longer copies each 2048-byte audio sector out of the read buffer before demultiplexing it: a 78-minute plain-DSD stereo area reads in 0.47 s instead of 0.58 s, and writes to DSF in 1.66 s instead of 1.79 s, in generic builds on the Zen 2 benchmark host (3950X) (#202, #230).
- 5-channel DSF output is faster: the channels of a 5-channel row are separated four frames at a time in 32-bit words, which the compiler vectorises, where it had been inserting one byte at a time. A 30-minute plain-DSD 5-channel area writes to DSF in 0.64 s instead of 0.88 s on one thread in generic builds on the Zen 2 benchmark host (3950X). The output is unchanged (#202, #237).
- A disc of a raw image (2064 or 2054 bytes per stored sector) no longer keeps the read buffer of its TOC reads once opened or shared: it held the largest TOC read, 20 KiB or more per disc (about 80 KiB on the discs of #228), for as long as the disc was kept. The first audio read allocates it again; the output is unchanged (#228, #234).

### Fixed

- The `core-dispatch` Miri slice has room under its 25-minute limit again: under Miri, the output-path dispatch test compares one 2-channel area and three of its layouts (DSF, DSDIFF and a chain file starting inside a DSF row) instead of two areas and five layouts each (#202, #236).
- Stereo DST decoding on one thread is no longer 2.9 % slower than in 0.5.5 on Zen 2: the decoder again uses byte probability tables and the 0.5.5 arithmetic-decoder step for 1, 2, 5 and 6 channels, and keeps the 0.5.6 ones for 3 and 4 channels, chosen per channel count at compile time. 6-channel DST keeps its 0.5.6 gain and is a little faster still. The output is the same to the bit (#203, #241).
- The `core-lib` Miri slice fits its 25-minute limit again: under Miri, the #228 read-buffer unit test checks one raw format and reads only the first frame (#228, #235).

## [0.5.6] - 2026-11-02

**Faster plain-DSD output and DSD-to-PCM conversion in the generic build.** On x86-64 CPUs with
AVX2, the DSF and DSDIFF fill now runs a build of the same safe code compiled for AVX2, chosen at
run time; elsewhere it runs as before. Together with whole-row DSF filling (#214), frames reserved
at full size (#215), faster `scarletbook-pcm` input and output loops (#216: #217, #219, #223) and
a smaller DST probability block (#218), plain-DSD output and DSD-to-PCM conversion are much faster,
multichannel and all-threads DST decoding slightly faster, and **stereo DST decoding on one thread
2.9 % slower** on the Zen 2 machine below (being won back under #203). Output is identical to the
bit in every case (#202, #225).

Measured on an AMD Ryzen 9 3950X (Zen 2, 16 cores / 32 threads), generic x86-64 build, 0.5.5
against 0.5.6, synthetic images (`scarletbook-synth`), medians of alternating runs (5 rounds for
DSF, 3 for DST and PCM; rounds agree within 1 %). Plain DSD: `bench dsf --threads 1 --out
/dev/null`; DST: `bench decode --sink null` on stress frames; PCM: `scarletbook-pcm`'s
`callpattern` example (a filesystem's call pattern, #216). Other CPUs may differ; the stereo DST
change in particular was −0.3 % on the development machine (Raptor Lake).

| Path | 0.5.5 | 0.5.6 | Change |
|---|---|---|---|
| 78-min plain-DSD stereo area to DSF (3.3 GB, one thread) | 2.61 s | 0.70 s | 3.7× faster |
| 30-min plain-DSD 5-channel area to DSF (3.2 GB, one thread) | 2.45 s | 0.97 s | 2.5× faster |
| DST stereo, 50-min area, all 32 threads (wall) | 4.05 s | 3.98 s | 1.7 % faster |
| DST 6-channel, 10-min area, all 32 threads (wall) | 2.84 s | 2.75 s | 3.2 % faster |
| **DST stereo, 20-min area, one thread (wall)** | **20.26 s** | **20.83 s** | **2.9 % slower** |
| DSD64 stereo to 176.4 kHz 24-bit PCM, 8 frames per call, one thread (CPU per second of audio) | 5.09 ms | 4.08 ms | 20 % less CPU |
| the same, as CPU time for a 78-min stereo area | 23.8 s | 19.1 s | 20 % less CPU |

Since the generic build now selects the AVX2 code at run time, the `x86-64-v3` archives are no
faster on these paths: in #225's measurements they were within 1 % of the generic build.

### Added

- `bench dsf IMAGE --area 2ch|mc --threads N --out FILE [--via stream|read-at] [--chunk BYTES]` writes an area's play range as one DSF file through `output::Layout`, to time the DSF output path end to end (#202, #212).

### Changed

- DSF output fills whole rows a position at a time, separating every channel in one pass with a loop per channel count: a plain-DSD stereo area to DSF takes 1.10 s instead of 2.86 s in x86-64-v3 builds and 1.93 s in generic ones (to `/dev/null`, on the development machine, a Raptor Lake VM; see the table above for the Zen 2 figures); the bytes are unchanged (#202, #214).
- A plain DSD frame is assembled in a buffer of its final size, `C x 4704` bytes, reserved when it starts, instead of one grown sector by sector: reading a plain-DSD area takes 0.71 s instead of 0.85 s for 78 minutes of stereo on the development machine (#202, #215).
- `output::Layout::fill` and `Layout::read_at` run through the core crate's run-time dispatch (#146) on x86-64 CPUs with AVX2, BMI1, BMI2, LZCNT and MOVBE: the DSF channel separation beneath them is built with those features, so generic builds write a plain-DSD stereo area to DSF as fast as x86-64-v3 builds; the bytes are unchanged. The DST decoder and the output path share the one `#[target_feature]` wrapper (#202, #225).
- The DST decoder's inner loop reads each channel's partial prediction shares and probabilities from one 1 KiB block per channel, with 64-entry probability tables (5.5.4's `min(|Spred| / 8, M_q - 1)`) instead of 8 KiB ones and a shorter arithmetic-decoder step: 6-channel DST decodes about 5 % faster on one thread in the default build, 2-channel is unchanged, and the output is the same to the bit (#203, #218).
- DSD-to-PCM conversion de-interleaves its input, rounds to 24 bits and interleaves its output with AVX2 when the CPU has it, chosen at run time like its filters: 3-8 % less CPU per second of audio; the output is unchanged (#216, #223).
- DSD-to-PCM conversion sums the first stage's table lookups with the table count known at compile time: about 10 % less CPU per second of audio at 176.4 kHz and 88.2 kHz, 3-7 % at 352.8 kHz; the output is unchanged (#216, #217).
- DSD-to-PCM conversion no longer zeroes its scratch buffers before the filters overwrite them: 1.5-3 % less CPU per second of audio; the output is unchanged (#216, #219).

### Fixed

- The `core-lib` Miri slice of miri-full finishes again: the DSF row test of #214 takes a Miri-sized set of cases under Miri and makes each position once, and the slice has a 25-minute limit (#202, #205, #222).

## [0.5.5] - 2026-11-01

**Faster DST decoding in the generic build.** On x86-64 CPUs with AVX2, BMI1, BMI2, LZCNT and
MOVBE, the DST decoder now runs a build of the same safe code compiled for those features,
chosen at run time; everywhere else it runs as before. The output is identical to the bit.
The generic C library and Rust builds gain it with no change on the consumer's side: a
50-minute DST stereo disc, all threads, decodes in 6.21 s instead of 7.45 s (97.7 CPU-s instead
of 115.6). Multichannel areas gain less: a 6-channel DST synthetic decodes 5 % faster with all
threads and 7 % faster on one thread (#146, #200).

**New x86-64-v3 archives: match release asset names exactly.** The x86_64 Linux archives now
come in two builds, the unchanged baseline one and a `-v3` one compiled entirely for
x86-64-v3 (#198). A prefix or glob such as `scarletbook-ffi-<version>-x86_64-unknown-linux-gnu*`
also matches `...-gnu-v3.tar.gz`, which stops with SIGILL on CPUs older than x86-64-v3.

### Added

- x86-64-v3 C library archives for x86_64 Linux: `scarletbook-ffi-<version>-x86_64-unknown-linux-gnu-v3.tar.gz` and `...-x86_64-unknown-linux-musl-v3.tar.gz`, built with `-C target-cpu=x86-64-v3` next to the unchanged baseline archives. Same ABI, different instruction set: they need an x86-64-v3 CPU (AVX2, BMI2, FMA and the rest of the level); `sb_build_info()` lists their target features. They are compiled entirely for x86-64-v3, for benchmarks and consumers who build per CPU (#146, #198).

### Changed

- The DST decoder runs a build of itself with AVX2, BMI1, BMI2, LZCNT and MOVBE, chosen at run time, on x86-64 CPUs that have them, with output unchanged to the bit: a generic build now decodes DST as fast as an `x86-64-v3` build (a 50-minute 2-channel synthetic, all threads, median of 5: 6.21 s against 7.45 s before; one thread: 60.7 s against 70.0 s). The generic C library gains it automatically; other targets and older CPUs keep the portable decoder. The core crate now denies `unsafe_code` instead of forbidding it, and allows it in one module, which only calls that `#[target_feature]` build of the safe decoder (#146, #200).

## [0.5.4] - 2026-11-01

**DSDIFF departures D1 and D2 write files that do not conform to DSDIFF.** They exist only so
that a consumer can reproduce another program's files byte for byte, and are made only when the
caller names them (`DsdiffDeparture::D1`/`D2`, or C options 35/36). With neither named, which is
the default, every file is written as before. Specification 8, 8.7 (#169, #187).

This release adds the writer's choices that pysacd asked for in #169, for DSF and DSDIFF files
compatible with an existing extractor's: caller chunks, `DIIN` without `EMID` and with the source
track's text, an empty `DIIN`, caller-supplied ID3v2.3 tags, gapless DSF files from a chain, and
the two departures. Every default is unchanged.

### Added

- DSDIFF caller chunks (specification 8, 8.4.2 item 5 and 8.6 row 33, at sacd-spec `e069569`): chunks whose ID and data the caller supplies, written in order after the last local chunk, padded by 8.4.1 item 1 and counted in the `FRM8` size and the layout's last region, with plain DSD or DST and in an edited master. IDs that break 8.4.1 item 3, or are those of the chunks specification 8 defines, are refused. In `scarletbook`: `output::CallerChunk` and `OutputOptions::dsdiff_caller_chunks` (`with_dsdiff_caller_chunks`); in `scarletbook-ffi`: `sb_output_options_add_chunk` and `sb_output_options_clear_chunks` (#169, #183).
- DSDIFF `DIIN` choices of specification 8, 8.4.8 and 8.6 row 24 as changed at sacd-spec `e069569`: `OutputOptions::dsdiff_edited_master_id_written` leaves `EMID` out of a file that is not an edited master (default `true`; an edited master, which requires `EMID` by 8.4.11 item 1, refuses `false`), and `OutputOptions::dsdiff_artist_title_source` (`ArtistTitleSource::Album`, the default, `Track` or `TrackThenAlbum`) fills `DIAR` and `DITI` from the source track's performer and title. `OutputOptions::dsdiff_empty_info` (8.6 row 34 at sacd-spec `048aa1a`) leaves out a `DIIN` that would hold none of its chunks instead of writing it with `D = 0` (the default; in C, option 39 from #192). In `scarletbook-ffi`, output options 33 (`SB_OUTPUT_OPTION_DSDIFF_EDITED_MASTER_ID_WRITTEN`) and 34 (`SB_OUTPUT_OPTION_DSDIFF_ARTIST_TITLE_SOURCE`), additive to ABI 2 (#169, #184).
- DSDIFF departures D1 and D2 (specification 8, 8.7, at sacd-spec `e069569`), made only when named: D1 writes plain DSD's `CMPR` with `D = 20`, counting its pad byte; D2 writes `DIAR`, `DITI`, marker and comment text as ISO 8859-1, removing every other code point. A file written with either is not a conforming DSDIFF file. `scarletbook`: `OutputOptions::dsdiff_departures`, `with_dsdiff_departures` and `DsdiffDeparture`. `scarletbook-ffi`: options `SB_OUTPUT_OPTION_DSDIFF_DEPARTURE_D1` (35) and `SB_OUTPUT_OPTION_DSDIFF_DEPARTURE_D2` (36); see Changed for `SbMarker` text (#169, #187).
- DSF files holding a byte range of a chain (specification 8, 8.2.7 and 8.6 row 32, sacd-spec `e069569`): `output::plan_dsf_chain` and `Disc::plan_dsf_chain` plan the files of consecutive ranges, such as the tracks with pause of consecutive tracks, from the TOC alone, with each skipped or refused range splitting the chain; `OutputOptions::dsf_chain_first` and `dsf_chain_end` (`with_dsf_chain`) make a DSF layout hold its file's byte range, so that every file but the last ends on a whole, unpadded block and the files' streams concatenate to the timeline unchanged; `Layout::byte_range` gives the bytes a DSF file holds. For such a file, `Layout::positions_for` (and so the positions a `PositionSource` is asked for) may include `first - 1`, the position its carried bytes come from, outside `Layout::range`. In the C API: `sb_output_dsf_chain_plan` with `SbDsfChainFile`, and the output options `SB_OUTPUT_OPTION_DSF_CHAIN_FIRST` (37) and `SB_OUTPUT_OPTION_DSF_CHAIN_END` (38) for `sb_output_layout_new` (#169, #186).
- A caller-supplied ID3v2.3 tag as the DSF metadata (specification 8, 8.2.6 item 4 and 8.6 row 3, sacd-spec `e069569`): `Tags::Raw` and `sb_output_options_set_raw_tag` now also take an ID3v2.3 tag, checked only for its header (version 03 00, the low five flag bits 0, a 28-bit size of `m - 10`) and written unchanged; any other tag is still refused with `InvalidArgument`. The ID3v2.4 path is unchanged (scarletbook, scarletbook-ffi) (#169, #185).
- `scarletbook-ffi`: output option 39, `SB_OUTPUT_OPTION_DSDIFF_EMPTY_INFO` (specification 8, 8.6 row 34 at sacd-spec `048aa1a`): 0 leaves out a `DIIN` that would hold none of its chunks instead of writing it with `D = 0` (the default), as `OutputOptions::dsdiff_empty_info` does in Rust. Additive to ABI 2 (#169, #192).

### Changed

- `scarletbook-ffi`: the text of an `SbMarker` is now written one byte per code point, so that departure D2 can write U+00A0 to U+00FF as single bytes. A marker with a code point above U+00FF is now refused by `sb_output_options_add_marker` itself (`SB_STATUS_INVALID_ARGUMENT`), not later when the layout is made. Without D2, text outside U+0020 to U+007E is handled as before (8.4.9, row 26), and ASCII text is unchanged (#169, #187).

### Removed

- An unused private accessor of the sealed `SeekCursor` trait; the public API and behaviour are unchanged (#191).

### Fixed

- The docs of the source-track text choice (`OutputOptions::dsdiff_artist_title_source`, `SB_OUTPUT_OPTION_DSDIFF_ARTIST_TITLE_SOURCE`) cite specification 3, 3.8 (track text), not 3.5 (#193).

## [0.5.3] - 2026-11-01

**scarletbook-pcm: invalidate cached PCM.** Every PCM output changes in this release: the Low
and Wide filters have new stopband edges (#170), the first stage cancels DSD silence exactly
(#171), and every other filter's output can change in its last bit. A missing neighbour at an
area's edge now converts as the DSD idle pattern 0x69 instead of as the value 0. PCM converted
with 0.5.2 or earlier, and kept in a cache, should be converted again. The Rust API only gains
items.

### Added

- The `demux` fuzz target also converts one input in 4096 to PCM: the whole run must equal
  each frame converted with its neighbours and the run split in two, wrong lengths must be
  refused, and the C interface's `sb_pcm_converter_new` and `sb_pcm_convert_run` (through
  `scarletbook-ffi`'s hidden `fuzz_pcm_convert_run` behind the non-default `fuzzing`
  feature) must refuse the same arguments and give the same bytes and clip counts in both
  formats, writing nothing outside the output (#130, #168).
- `scarletbook_pcm::Converter::convert`, for a frame cache: a `Run` whose frames are one buffer or one buffer per frame (`Frames::Separate(&[&[u8]])`, read in place), output as `i32` or 3-byte little-endian samples written directly (`PcmOut::S24le`, the bytes of `convert_run` + `pack_s24le`), and optional clipped-sample counts per frame of the run, which sum to `clipped_total` and do not depend on how frames are grouped into runs. New errors `PcmError::OutputBytes` and `FrameCounts`; `convert_run` is unchanged (#172, #175).

### Changed

- **Breaking behaviour** in `scarletbook-pcm` (and so in the `sb_pcm_*` C functions): a neighbour of `None` (the edge of an area) is now the idle pattern 0x69, not the value 0, so it gives exactly the output of a neighbour frame of 0x69 bytes. The idle pattern 0x69 now converts to samples of exactly 0 at every rate, cutoff and gain, and so do the edges of an area, with no transient there; 0x55 gives exact zeros within a run of 0x55 (a change between 0x55 and 0x69, or 0x55 at an area's edge, is not silent). Samples near an area's edges, and all samples of silent passages, therefore differ from 0.5.2 (#171, #176).
- **Breaking behaviour** in `scarletbook-pcm`: `Low` now attenuates at least 105 dB from 36 kHz (was 38 kHz; -3 dB at 32.0 kHz, was 32.7 kHz). `Wide` at 176.4 kHz has its own filter, at least 105 dB from 88.2 kHz (was 96 kHz; -3 dB at 82.7 kHz, was 85.3 kHz). Every output of these filters differs from 0.5.2. `Cutoff::stopband_hz` of `Low` is 36 000, and the new `Cutoff::stopband_hz_at(rate)` gives the edge at a rate (#170, #176).
- **Breaking behaviour** in `scarletbook-pcm` (and the `sb_pcm_*` C functions): the first stage is redesigned for exact silence, so every output of every filter, `Default` and `Wide` at 352.8 kHz included, can differ from 0.5.2, mostly in the last LSB (#171, #176).
- `scarletbook-pcm` runs its filters with AVX2 and FMA, chosen at run time, on x86-64 CPUs that have them, with output unchanged to the bit: 176.4 kHz `Default` stereo, one thread, on a generic build is about 298× real time (222× without). Other targets and older CPUs use the portable kernels. The crate now denies `unsafe_code` instead of forbidding it, and allows it in one module, which only calls the `#[target_feature]` builds of the safe kernels (#173, #180).
- `scarletbook-pcm` converts 3 to 15 % faster on a generic x86-64 build, one thread (176.4 kHz `Default` stereo: 193× to 222× real time), with output unchanged to the bit: S1 writes the halfband's even and odd inputs directly, its sums need no overflow checks, the filter loops have no bounds checks and run 16 outputs at a time, and the input bytes are de-interleaved a frame at a time (#173, #179).

## [0.5.2] - 2026-11-01

### Added

- Saved confirmations (specification 4, 4.2 item 9, sacd-spec `cb9530b`): when verification
  confirms a provisional reading, a seek index whose TOC fingerprint a disc has checked
  (`Disc::enable_seek_index`, `Disc::set_seek_index`; an index used directly never) keeps (`L`, `u_L`) and the digest
  of sector `L`, saves them (format version 3; versions 1 and 2 still load and are still
  written when there is nothing to add), and after a remount reports a reading from that hint
  for positions `u_L` or more `Speculation::Confirmed` at once, when sector `L` has that digest.
  `AreaIndex::confirmations`; `AreaIndex::without_checkpoints` keeps only the hints,
  confirmations and fingerprint, a few kilobytes to save per area (#124, #144).
- Seek hints (specification 4, 4.2 item 8 locator samples, sacd-spec `cb9530b`): a seek
  index records, from every cursor made from it, sectors with the frame number of their first
  frame record (`IndexOptions::hint_spacing`, default every 8 frames, and `max_hints`,
  default 2 048, `with_hints`; `AreaIndex::hints`; `seek::DEFAULT_HINT_SPACING`,
  `DEFAULT_MAX_HINTS`, `MAX_HINTS`, `HINT_REACH`). They are saved with the index (format
  version 2; version 1 still loads and is still written when there is nothing to add), so
  that after a remount a provisional reading near a range read before begins at a hint with
  one read call through the next hint, without a search. Its positions stay provisional
  (`Speculation::Pending`). A hint whose sector does not hold its frame record is dropped with
  a warning, and the cursor searches instead. An index kept by a disc carries a fingerprint
  of the disc's TOC (master and area TOC fields, sector count and format), saved with it:
  `Disc::set_seek_index` refuses an index of a disc with another TOC. `scarletbook-seekcost`
  measures seeks after a remount (#124, #133).
- The `seek_index` fuzz target also reads speculatively from a loaded index's seek hints,
  and has a synthetic disc keep the index (binding its TOC fingerprint, so that saved
  confirmations are used) and read and verify through it; readings from seek hints alone
  must equal reads without an index from their first exact position on. New seeds hold that
  disc's hints and confirmations (#124).
- The x86-64 Linux C libraries (`libscarletbook_ffi.so` and every object in
  `libscarletbook_ffi.a`, glibc and musl) carry the Intel CET shadow stack (SHSTK) property,
  so a SHSTK-marked program that links them stays marked and glibc with shadow stacks enabled
  loads them; `clib-test.sh` checks it with `readelf -n`. Indirect branch tracking (IBT) is
  not marked: stable Rust's standard library has no `ENDBR64` (#125, #157).
- `scarletbook-pcm`, a new crate: DSD64 to 24-bit PCM at 88.2, 176.4 and 352.8 kHz, with a 40 kHz (default), 30 kHz or 80 kHz (176.4 and 352.8 kHz only) linear-phase low-pass, stopband at least 105 dB and ripple within ±0.01 dB. Output is aligned to the DSD timeline, has an optional gain with clip counting and no dither, and is bit-exact per frame: converting a frame with its neighbours gives the bytes of a whole-area conversion. `Converter` (one per worker) converts frames or runs of frames (#130, #148).
- C ABI for DSD to PCM (`scarletbook-pcm`): `sb_pcm_converter_new`, `sb_pcm_samples_per_frame`, `sb_pcm_convert_run` and `sb_pcm_converter_free` convert runs of plain DSD64 frames (4704 bytes, 37 632 samples per channel) to 24-bit PCM at 88.2, 176.4 or 352.8 kHz as `int32_t` (`SB_PCM_S32`) or 3-byte little-endian (`SB_PCM_S24LE`) samples, with the cutoffs `SB_PCM_CUTOFF_DEFAULT`, `_LOW` and `_WIDE`, a gain of -120 to +24 dB and per-channel clip counts. A frame converted with its two neighbours is byte for byte the same frame of a whole-area conversion. Additive on ABI 2; README section and ctypes bindings (#130, #160).
- DSDIFF output layouts with plain DSD sound data (specification 8, 8.4.1 to 8.4.5): `output::Format::DsdiffDsd`, its size and every chunk from the TOC alone, channel IDs and loudspeaker configuration from the area's layout. Writer's choices are fields of `output::OutputOptions`: the property chunk order (`PropertyChunk`), channel IDs and `LSCO` for unspecified layouts, `LSCO` for defined layouts, and the absolute start time (#131, #141).
- DSDIFF output layouts with DST sound data (specification 8, 8.4.6): `output::DstLengths` reads the lengths of a range's DST equivalents (decoding nothing unless invalid frames are replaced) and can be saved and loaded; `Layout::new_dst` and `Disc::output_layout_dst` give the exact layout from them, with `FRTE`, the `DSTF` frames unchanged, optional `DSTC` frame CRCs and the `DSTI` sound index. `PositionSource::dst_equivalent` and `Layout::input` tell a caller what to supply. Writer's choices: local chunk order (`LocalChunk`), invalid frames (`InvalidDst`), CRCs, the index and its offsets (`DstIndexOffset`) `Layout::read_at` refuses a disc of another image than the one a layout was made from (#131, #143).
- DSDIFF text (specification 8, 8.4.7 item 5, 8.4.8 and 8.4.9): the artist (`DIAR`) and title (`DITI`) of `DIIN` from the file's tag items, by default in edited masters; comments (`COMT`, `output::Comment`, `CommentKind`); marker text from track titles (`MarkerText`); every reported string written as text bytes. Writer's choices: the `DIIN` chunk order (`InfoChunk`), the replacement of other code points, and whether empty text is written (#131, #154).
- DSF output layouts (specification 8, 8.2): `output::Layout` and `Disc::output_layout` give a DSF file's exact size and header from the TOC alone; `Layout::positions_for`, `Layout::fill` and `Layout::read_at` produce any byte range of it from the positions' content. Writer's choices are fields of `output::OutputOptions` (bits per sample, channel type of unspecified layouts), and `Error::OutputRefused` reports an area the options can't write. Behind the default `output` feature (#131, #139).
- DSDIFF markers and edited masters (specification 8, 8.4.7, 8.4.8, 8.4.10 and 8.4.11): the `DIIN` chunk with `EMID` and markers (`output::Marker`, `MarkerKind`, `Markers`): the play range's program, one-track programs of a source track, clipped programs of frame ranges, or the caller's own; and edited-master output, which refuses an area or range that 8.4.11 excludes with `OutputRefusal::NotEditedMaster` and the failing `EditedMasterCheck`. `output::SourceTrack` and `Layout::source_track` give a file's source track (8.1 item 7). Writer's choices: `DIIN` and `EMID`, one-track and clipped programs, and whether `MARK` sizes count the text's pad byte (#131, #145).
- DSF and DSDIFF output through the C ABI: `sb_output_options_*` (every writer's choice of specification 8, 8.6, as `SbOutputOption`, `SbOutputString` and `SbOutputOrder`, with markers and comments), `sb_tags_*` (`SbTagItem`), `sb_dst_lengths_scan`, `_load` and `_get`, and `sb_output_layout_new`, `_new_dst`, `_info`, `_region`, `_positions`, `_read` and `_fill` (`SbPositionData`). New status codes `SB_STATUS_OUTPUT_LAYOUT_UNSPECIFIED` and `SB_STATUS_NOT_EDITED_MASTER`; `SB_ABI_VERSION` stays 2. The README's ctypes bindings cover the output API, and `clib-smoke.py` checks a DSF and a DSDIFF file made through the C library against the Rust API's output (#131, #161).
- DSF tags (specification 8, 8.2.6 and 8.3): DSF layouts end with an ID3v2.4 tag built from the TOC (`output::TagSet::from_toc`: track text, track number, ISRC, genres, album or disc text, album set, date), from a caller's `TagSet`, or the caller's own tag bytes (`output::Tags`). Every ID3 writer's choice of 8.6 is a field of `output::Id3Options`: footer or padding, frame order, text encoding, terminators, control characters, splitting, and the frames and descriptions of the arranger, messages, album artist, copyright, album set and date (#131, #149).
- A criterion benchmark of opening a disc and reading its whole TOC, through the C
  interface (`sb_disc_open_callbacks_at` and every TOC query) and through the Rust API:
  `cargo bench -p scarletbook-ffi --bench toc_read` (#132, #135).
- C interface: `sb_disc_toc` writes the whole TOC into one caller-sized buffer (the disc,
  areas, tracks, strings, genres, text channels, stored timecodes and warnings that the
  per-field queries return) in a fixed, versioned binary layout: little-endian, fixed
  offsets, described by `SbTocHeader`, `SbTocDisc`, `SbTocArea`, `SbTocTrack`,
  `SbTocWarning` and `SbTocStr`, format `SB_TOC_MAJOR`.`SB_TOC_MINOR` 1.0. One call
  replaces about 100 to 160 per disc. Additive on ABI 2. README ctypes bindings and an
  example (#132, #147).
- The `toc` fuzz target also reads every input that opens through the C interface's
  `sb_disc_toc` (a 64 KiB buffer, then one of exactly the size given, then one byte short),
  through `scarletbook-ffi`'s hidden `fuzz_disc_toc` behind a non-default `fuzzing`
  feature that the C library does not enable (#132, #150).
- The suspect mark through the C interface (specification 4, 4.2 item 8, sacd-spec
  `eafaaba`): `sb_track_audio_suspect` and `sb_raw_frames_suspect` write an `SB_SUSPECT_*`
  reason (`SB_SUSPECT_NONE`, `SB_SUSPECT_AREA_LAYOUT`, `SB_SUSPECT_FRAME_NUMBERS_DECREASE`,
  `SB_SUSPECT_OTHER`), and `sb_disc_seek_policy_set` takes
  `SB_SEEK_POLICY_SPECULATIVE_EXACT_WHEN_SUSPECT`. Additive on ABI 2 (sacd-spec#37, #153)
- Suspect provisional readings (specification 4, 4.2 item 8, sacd-spec `eafaaba`): `suspect()`
  on `AreaCursor`, `TrackCursor`, `TrackAudioCursor`, `TrackFrames` and `TrackAudio` gives a
  `Suspect` reason for a provisional reading the library doubts: `AreaLayout` (the area's TOC
  places its audio past the image's end, or, for cursors a disc makes, over the other area's
  TOC or audio sectors) or `FrameNumbersDecrease` (the first frame records of the sectors read
  while choosing `L` decrease). The mark is informative and changes no verdict.
  `SpeculativeOptions::exact_when_suspect` (`with_exact_when_suspect`, default off) reads
  such areas and ranges exactly instead. The default policy is unchanged (sacd-spec#37, #151).

### Changed

- Provisional (cold) seeks take at most 5 read calls and 160 KiB up to the range's first frame on
  the 50-minute 2-channel DST area of uneven bit rate, at every one of 2 000 (and 30 000)
  positions (#123): when the search reaches its fifth read, that read takes what is left of
  160 KiB around its guess (or on from where a final read starts, past the probed bound), so
  that it ends the search and holds the first frame at once; if it finds no sector before the
  frame, the search goes on as before. Before, 21 of 1 991 positions took 6 or 7 calls (all
  within 144 KiB); now p50 / p90 / p95 / p99 / max are 3 / 4 / 5 / 5 / 5 calls and
  122 / 144 / 144 / 160 / 160 KiB. The constant-rate image is unchanged (#123, #164).
- Provisional (cold) seeks keep within about 144 KiB before the range's first frame, and take
  fewer read calls at the worst positions (#123): the search's first correction uses the
  geometric mean of the probe's own density and the average back to the other track start;
  once probes lie on both sides of the frame it uses regula falsi with the Illinois weighting;
  it reads a narrow enough interval at once; and the read after the provisional start is sized
  through the range's first frame and on toward the range's end within that budget, less than
  32 sectors when that suffices, then on to the range's end as before. On a 50-minute
  2-channel DST area of uneven bit rate (2 000 positions, to the first frame) the worst case
  goes from 8 read calls and 284 KiB to 7 and 144 KiB (99th percentile 7 → 6 calls, 206 → 144
  KiB); the median rises from 48 to 122 KiB, since the range's first frame now comes with the
  read that also takes the rest of the range (the whole range: 116 → 128 KiB, 4 calls).
  Reads sized after a resume from a seek index are now also exactly their size (#123, #159).
- Seek hints are recorded only from the ranges callers read (cursors made from a seek index,
  `TrackCursor::for_range_indexed` included), no longer by `AreaIndex::extend`,
  `Disc::extend_seek_index` and `Disc::verify_speculation`: a pass over a whole area recorded
  a hint every 8 frames, and thinning to `IndexOptions::max_hints` then dropped the hints of
  the positions actually read, which are the ones a later seek needs (#124, #144).
- `Disc::set_seek_index` refuses an index saved by a disc whose TOC fingerprint differs: one of
  another disc of the same layout (which 0.5 accepted and rebound to this disc), and one saved
  from a rip in another sector format (2054 or 2064 bytes against 2048) of the same disc.
  Every index a disc keeps now has that fingerprint, so its saved form is format version 2,
  which 0.5.1 and earlier refuse ("version not supported"): a cache of saved indexes shared
  with an older release stops loading there. `AreaIndex::from_bytes` keeps at most
  `max_inline_budget / 8` seek hints, now and as the index grows, in addition to the stored
  frames. `AreaIndex::merge` likewise refuses to merge two indexes whose TOC
  fingerprints differ (#124, #133).
- An exact read resumed from a seek index makes its first read sized to the range (the
  replay, the interval and up to 64 sectors), one read call instead of two or three.
  The changed cancellation and progress bound of #123 (a read of up to 512 sectors in
  progress) applies to it too (#124, #133).
- C interface: a call no longer allocates to clear the thread's error message
  (`sb_last_error_message`) when it is already empty, as it is after every successful
  call: about 8 ns less per call (#132, #140).
- Text strings of the TOC are decoded in one pass: control characters are replaced only
  when a string holds some, ASCII Latin-1 text is copied as it is, and fixed text fields
  are trimmed before they are decoded. The text is unchanged (#132, #142).
- Opening a disc is 2.5 to 4 times faster on typical discs: the digests of the TOC sectors
  that `Disc::share` (and `sb_disc_clone`) check a reader against are computed word by
  word in four lanes instead of byte by byte (FNV-1a). They are never stored, so nothing
  saved changes; seek index files keep their FNV-1a digests (#132, #138).
- Documentation: `DecodeOn::Pool` and `sb_pool_new` recommend a pool of one thread fewer than
  the logical CPUs when the calling thread does substantial work with each frame. Measured on
  16 vCPUs, decoding through the C interface and writing each frame to files took 7.7 % longer
  than `bench decode` with 16 threads and 4.6 % longer with 15 (about 3 % faster); with a
  caller that does little, 16 were as fast or slightly faster. Measure with `decode_pool`
  (#146, #158).
- Unreleased APIs made extensible before their first release, so later additions stay
  minor (#39, #166). Non-exhaustive now: `scarletbook_pcm::FrameRun`, with
  `FrameRun::new`, `with_before` and `with_after` to build it; the variants
  `scarletbook_pcm::PcmError::{Unsupported, InputLength, OutputLength}`,
  `OutputRefusal::UnspecifiedLayout` and `EditedMasterCheck::TrackTooShort` (match them with
  `..`). `output::Id3Options` gains `new` and a `with_*` method per field, and
  `output::GenreSources` gains `new`, `with_track`, `with_album` and `with_disc`; both were
  already non-exhaustive. Nothing released in 0.5.1 changes.

### Fixed

- `Disc::extend_seek_index` and `Disc::verify_speculation` never reported complete for an
  area without existing audio sectors (its first audio LSN at or past the image's end, as in
  the corpus image `area-audio-start-past-end`), so a loop until complete never ended: merging
  the extended copy back took only a farther frontier, and such an area's frontier does not
  move. `AreaIndex::merge` now also takes completeness. Affects 0.4.0 to 0.5.1 (#68, #136).
- A provisional reading (`SeekPolicy::Speculative`) with a seek index kept could start a range
  at the provisional checkpoint where an earlier reading far before the range ended, because a
  later reading had passed the range's first frame, and then read every sector from there:
  in one measured case 42 880 sectors in 32-sector calls instead of a few. A provisional
  checkpoint is now used only with the later one of the same reading (consecutive boundaries);
  otherwise the cursor searches as without them. Affects 0.4.1 to 0.5.1 (#94, #134).

## [0.5.1] - 2026-10-31

### Added

- TOC timecodes as stored: `Disc::track_timecodes_stored` gives a track's start and
  duration timecodes as stored in `SACDTRL2` (specification 3, 3.7.2), before the clamping
  of 3.9.2 and whatever their range (`Area::playtime` is already the stored play time). In
  the C interface, `SbTimecode` (3 bytes, no padding), `sb_area_playtime_stored` and
  `sb_track_timecodes_stored`. Frame counts are unchanged. Additive on ABI 2, no change to
  public Rust structs. README ctypes bindings (#127, #128).

### Changed

- Cheaper provisional (cold) seeks: a provisional reading (`SeekPolicy::Speculative`,
  specification 4, 4.2 item 8) now begins at the sector its search found, the last whose
  first frame record is numbered below the range's first frame, instead of a checkpoint
  boundary 32 to 95 sectors before it; the search interpolates between the area's track
  starts and then at the density of the frames it last read, uses every frame record of a
  probe, and stops once close; the cursor reads on from its last probe, and then up to the
  range's estimated end in reads sized to it, of at most the new `seek::MAX_READ_SECTORS`
  (512 sectors, 1 MiB) each, instead of 32. On a 50-minute 2-channel DST area of uneven
  bit rate, a 14-frame range costs 3.4 read calls and 81 KiB to its first frame (was 9.1 and
  266 KiB). `ResumedFrom::Speculative` and `Speculation` report that `L`, which is no longer
  a multiple of the index interval from the area's first sector. **Changed bound:** a
  cancellation request, and the progress counts, now wait for the read in progress, which
  is at most 32 sectors as before, or at most 512 for those reads sized to a provisional
  range (`CancelToken`, `Progress`, `sb_disc_cancel_token`, `sb_progress_get`).
  `scarletbook-seekcost` measures cold seeks on newly opened discs (`--positions`), and
  `scarletbook-synth --dst-frames uneven` writes an area of uneven bit rate (#123, #126).

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

[Unreleased]: https://github.com/aclater/scarletbook-rs/compare/v1.2.0...HEAD
[1.2.0]: https://github.com/aclater/scarletbook-rs/compare/v1.1.1...v1.2.0
[1.1.1]: https://github.com/aclater/scarletbook-rs/compare/v1.1.0...v1.1.1
[1.1.0]: https://github.com/aclater/scarletbook-rs/compare/v1.0.2...v1.1.0
[1.0.2]: https://github.com/aclater/scarletbook-rs/compare/v1.0.1...v1.0.2
[1.0.1]: https://github.com/aclater/scarletbook-rs/compare/v1.0.0...v1.0.1
[1.0.0]: https://github.com/aclater/scarletbook-rs/compare/v0.7.0...v1.0.0
[0.7.0]: https://github.com/aclater/scarletbook-rs/compare/v0.6.2...v0.7.0
[0.6.2]: https://github.com/aclater/scarletbook-rs/compare/v0.6.1...v0.6.2
[0.6.1]: https://github.com/aclater/scarletbook-rs/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/aclater/scarletbook-rs/compare/v0.5.9...v0.6.0
[0.5.9]: https://github.com/aclater/scarletbook-rs/compare/v0.5.8...v0.5.9
[0.5.8]: https://github.com/aclater/scarletbook-rs/compare/v0.5.7...v0.5.8
[0.5.7]: https://github.com/aclater/scarletbook-rs/compare/v0.5.6...v0.5.7
[0.5.6]: https://github.com/aclater/scarletbook-rs/compare/v0.5.5...v0.5.6
[0.5.5]: https://github.com/aclater/scarletbook-rs/compare/v0.5.4...v0.5.5
[0.5.4]: https://github.com/aclater/scarletbook-rs/compare/v0.5.3...v0.5.4
[0.5.3]: https://github.com/aclater/scarletbook-rs/compare/v0.5.2...v0.5.3
[0.5.2]: https://github.com/aclater/scarletbook-rs/compare/v0.5.1...v0.5.2
[0.5.1]: https://github.com/aclater/scarletbook-rs/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/aclater/scarletbook-rs/compare/v0.4.1...v0.5.0
[0.4.1]: https://github.com/aclater/scarletbook-rs/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/aclater/scarletbook-rs/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/aclater/scarletbook-rs/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/aclater/scarletbook-rs/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/aclater/scarletbook-rs/releases/tag/v0.1.0
