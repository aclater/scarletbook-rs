# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). The version
is shared by every crate of the workspace; releases carry the `bench` program and, from
0.3.0, the C library, as binary assets. Nothing is published on crates.io yet.

## [Unreleased]

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

Measured on lennon, an AMD Ryzen 9 3950X (Zen 2, 16 cores / 32 threads), generic x86-64 build,
0.5.6 against 0.5.7, with `benchmarks/compare.sh` (statically linked kits, synthetic images of
`benchmarks/runlist.json` in the page cache, output to `/dev/null`, a fresh copy of each program
per run, alternating ABBA rounds: 5 per case, 3 for the 20-minute DST case and PCM). Times are
the medians of the wall times; changes are the medians of the per-round ratios. Other CPUs may
differ.

| Path | 0.5.6 | 0.5.7 | Change | `bench` change (#239)¹ | Library change¹ |
|---|---|---|---|---|---|
| 78-min plain-DSD stereo area to DSF, one thread | 0.861 s | 0.579 s | 33 % faster | 19 % faster | 17 % faster |
| 30-min plain-DSD 5-channel area to DSF, one thread | 1.092 s | 0.607 s | 44 % faster | 12 % faster | 36 % faster |
| 25-min plain-DSD 6-channel area to DSF, one thread² | 0.757 s | 0.538 s | 29 % faster | 14 % faster | 17 % faster |
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

The stereo all-threads row was slower in every round (0.3-1.2 %), and 1.3 % slower in a second,
separate run; the split puts it on the tool change, not the library.

**#241 wins back the 0.5.6 regression.** 0.5.6 decoded stereo DST on one thread 2.9 % slower
than 0.5.5. In #241's fresh-copy measurements on lennon, stereo on one thread is ±0 % against
0.5.5 and 2.67 % faster than 0.5.6, and 6 channels on one thread 5.9 % faster than 0.5.5.

**Not yet at target:** the DSF output targets of #202 and the DST decoding targets of #203 are
not met in 0.5.7; the work continues in 0.5.8.

### Added

- `bench dsf --via parallel`: one thread reads the area while `--threads` workers fill whole DSF rows and write them at their offsets, a prototype of a parallel DSF writer on the public `Layout` API (#202, #224, #232).
- A CI benchmark regression check: `.github/workflows/bench-regress.yml` builds `bench` and the scarletbook-pcm `callpattern` example from the base and the head, times single-threaded DST decoding, plain DSD to DSF and DSD to PCM alternately on one runner (a fresh copy of each program per run), and fails when the median of the per-round ratios head / base of CPU time exceeds 1.15. It runs beside CI on pull requests that touch library sources, on main and nightly, and is not a required check (#226, #233).
- A benchmark kit for timing scarletbook on consumers' machines: each release publishes `scarletbook-benchkit-<version>-x86_64-linux.tar.gz` and its `-v3` (x86-64-v3) twin, with statically linked `bench`, `scarletbook-synth` and the scarletbook-pcm `callpattern` example, a machine-readable run list of the #224 cases (`benchmarks/runlist.json`) and a reference runner that prints one JSON line per timed run; pushes to main upload the same archives as workflow artifacts. A release now has 28 assets instead of 24 (`benchmarks/README.md`; #226, #231).
- `benchmarks/RESULTS.md`: measured figures per release on a Zen 2 machine (3950X), generic and x86-64-v3 builds, with the method and the paths not measured; `benchmarks/compare.sh` reproduces them by timing two revisions against each other with the run list (#226, #231).

### Changed

- `bench` opens images with `BufReader::new`, as the documentation does, instead of a 1 MiB `BufReader`: area reads (64 KiB) then go straight into the cursor's buffer, one copy from the kernel, as with `sb_disc_open`. The 1 MiB buffer copied every byte twice. A 78-minute plain-DSD stereo area writes to DSF in 0.50 s instead of 0.61 s on one thread on lennon (3950X). The library is unchanged; compare figures across this change with that in mind (#202, #239).
- Reading an area no longer copies each 2048-byte audio sector out of the read buffer before demultiplexing it: a 78-minute plain-DSD stereo area reads in 0.47 s instead of 0.58 s, and writes to DSF in 1.66 s instead of 1.79 s, in generic builds on lennon (3950X) (#202, #230).
- 5-channel DSF output is faster: the channels of a 5-channel row are separated four frames at a time in 32-bit words, which the compiler vectorises, where it had been inserting one byte at a time. A 30-minute plain-DSD 5-channel area writes to DSF in 0.64 s instead of 0.88 s on one thread in generic builds on lennon (3950X). The output is unchanged (#202, #237).
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

[Unreleased]: https://github.com/aclater/scarletbook-rs/compare/v0.5.7...HEAD
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
