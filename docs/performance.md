<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Performance and benchmarks

How fast scarletbook is, and the tools to measure it. Published figures, with the machines
they were measured on, are in [benchmarks/RESULTS.md](../benchmarks/RESULTS.md); to time
scarletbook on your own machines, use the benchmark kit of [benchmarks/README.md](../benchmarks/README.md).

## The benchmark program

`bench` implements the interface of sacd-spec `benchmarks/README.md`:

```sh
cargo build --release -p scarletbook-bench
target/release/bench decode disc.iso --area 2ch --threads 0 --sink hash   # or null, or raw DIR
```

`bench dsf` is outside that interface (#202): it writes an area's whole play range as one
DSF file through `output::Layout`, to time the output path, read and write included:

```sh
target/release/bench dsf disc.iso --area 2ch --threads 0 --out out.dsf   # or /dev/null
target/release/bench dsf disc.iso --area mc --threads 0 --out out.dsf --via read-at --chunk 131072
```

`--via stream` (the default) reads the area once, in order, and fills `--chunk` bytes
(default 4 MiB) per `Layout::fill` call; `--via read-at` makes one `Layout::read_at` call
per chunk, as a filesystem serving the file would, with the area's seek index enabled;
`--via parallel` (Unix) reads the area on the calling thread and has `--threads` workers
fill whole DSF rows, about `--chunk` bytes at a time, and write each piece at its offset
with `write_at`: a prototype of a parallel writer built on the public `Layout` API (#224).

`bench realtime` is outside it too (#255): what a real-time player needs from one thread. It
prints the mean real-time factor, the slowest 1-second window, the 99.9th percentile and
slowest frame times, the frames over 6.7 ms (half a frame period), and the median and slowest
time from a seek to its first frame, with a complete seek index. Put the image in the page
cache first and pin the process to one core:

```sh
taskset -c 2 target/release/bench realtime disc.iso --area mc --seeks 200
```

`bench open` and `bench first` are outside it too (#224). They time what takes far less than
starting a process, so they time it inside the process, `--iterations` times (default 100, at
most 1,000,000), and print the median, fastest and slowest time in microseconds. `open` times
opening an image up to its parsed TOC. `first` times the first `--bytes` (default 128 KiB) of a
track's decoded audio, by default the area's middle track, on a disc already open with a complete
seek index:

```sh
target/release/bench open disc.iso
target/release/bench first disc.iso --area 2ch --threads 1 --track 3
target/release/bench first disc.iso --area 2ch --threads 0 --iterations 15 --evict benchmarks/evict.py
```

The image stays in the page cache unless `--evict PROGRAM` is given. `PROGRAM IMAGE` then runs,
untimed, before each iteration, and its failure fails the run. `benchmarks/evict.py` evicts the
image without privileges and fails if any page stays cached, as it does on tmpfs.

## Measuring performance

`scarletbook-seekcost IMAGE` (in `scarletbook-conformance`) measures what opening a frame
range costs against where it starts, without and with a seek index, and the index's build
time and size. On a 50-minute 2-channel DST image (`scarletbook-synth --minutes 50 --area
2ch --kind dst`) a 14-frame range at 99 % of the area reads 470 272 sectors (146 ms from the
page cache) without an index and 64 sectors (2.4 ms) with one; the index (K = 64) takes one
pass to build (162 ms) and 178 KB saved. Its second table measures cold seeks by provisional reading (no index;
`SeekPolicy::Speculative`): on the same image, 2 to 4 read calls and 42 to 52 sectors
wherever the range lies, 1.8 to 2.9 ms in its network model (0.5 ms per read call, 110 MB/s),
with the same frames as exact reads on that well-formed image. Its third table opens the
disc afresh for each of 200 positions (`--positions`), as a consumer that seeks cold sees it:
on an area of uneven bit rate (`scarletbook-synth ... --dst-frames uneven`), a provisional
start costs 3.5 read calls and 112 KiB on average up to the range's first frame (at most 5
and 160 KiB; it also prints the 50th to 99th percentiles), and 3.7 read calls and 131 KiB
for the 14-frame range decoded.

Through the C interface, `python3 .github/scripts/clib-seekcost.py PACKAGE_DIR IMAGE` opens the
image with `sb_disc_open_callbacks` (counting read calls and bytes in the callbacks, 64 KiB per
read call) and decodes the same ranges cold, exact and provisional: on the same image, 4 to 7
read calls (164 to 285 KB, 3.5 to 5.6 ms modelled) for a provisional range anywhere in the area,
against up to 14 696 read calls (963 MB) for an exact one at 99 %.

Three criterion benchmarks track the decoder and the TOC between releases:

```sh
cargo bench -p scarletbook --bench dst_frames             # DstDecoder::decode, one frame per iteration
cargo bench -p scarletbook --bench dst_frames -- stress/2ch
cargo bench -p scarletbook-conformance --bench pipeline   # Disc::open + TrackAudio, in-memory images
cargo bench -p scarletbook-ffi --bench toc_read           # open + every TOC query, one disc per iteration
```

- `dst_frames` reports the time per frame and symbols per second (37 632 x channels per
  frame) for two sets, per channel count: `corpus/Nch`, every coded DST frame of the corpus
  images `dst-*.iso` that decodes; and `stress/Nch-LAYOUT`, 2- and 6-channel frames
  generated at start-up by the annex 5.A encoder of `crates/scarletbook/tests/common/dst_enc.rs`
  from a seeded sigma-delta signal (order-128 filters, order-32 filters, and a layout with
  several filter and ptable segments). The stress frames are identical on every run.
- `pipeline` reads every track of a 2-channel and a 6-channel area of small synthetic
  images (plain DSD, DST, DST in 2064-byte sectors) on one thread.
- `toc_read` opens a disc and reads its whole TOC, as a consumer listing a library does
  (#132): `c/IMAGE` through `sb_disc_open_callbacks_at` over the image in memory and every
  TOC query of the C interface (disc, areas, tracks, strings, genres, text channels,
  timecodes, warnings), `rust/IMAGE` the same through `Disc::open`. The images are the
  corpus images with the most text and tracks and a synthetic disc of 2 x 255 tracks.
  `SCARLETBOOK_PERF_BENCH=list` prints the calls, read callbacks and bytes read per disc.

`decode_pool` (in `scarletbook-ffi`, not criterion) decodes every track of an area on a
pool the way a consumer of the C interface does, and prints wall and CPU time per round
(#146): `rust-install` (`TrackAudio` inside `pool.install`, as `bench decode`),
`rust-pool` (`DecodeOn::Pool`, the caller outside the pool), `c-read`
(`sb_track_audio_set_pool` and `sb_track_audio_read` into one caller buffer per frame) and
`c-stored` (`sb_track_audio_read_with_stored`), into a null sink or the raw output of
`bench decode --sink raw`:

```sh
scarletbook-synth dst50.iso --minutes 50 --area 2ch --kind dst
cargo bench -p scarletbook-ffi --bench decode_pool -- dst50.iso c-read --threads 0 --rounds 3 --sink raw /dev/shm/out
```

The scripts in `perf/` need Linux and `perf`:

| Script | What it does |
|---|---|
| `perf/cycles-per-symbol.sh [NAME...]` | cycles, instructions, IPC per decoded symbol and ms per frame for `dst_frames` benchmarks, from `perf stat` |
| `perf/flamegraph.sh NAME [OUT.svg]` | `perf record` and inferno (`cargo install --locked inferno --version 0.12.8`) flame graph of `DstDecoder::decode` |
| `perf/dispatch-inlining.sh` | disassembles the core crate's run-time dispatch (`dispatch.rs`) in a generic x86-64 build and fails if a dispatched function lost its AVX2 or BMI hot loop (or a generic one gained it); CI runs it on the release `libscarletbook_ffi.so` |
| `perf/compare.sh REV_A REV_B [FILTER]` | builds two revisions (`.` is the working tree) into separate target directories, runs `dst_frames` alternately, pinned, and prints per-benchmark and per-group changes |
| `perf/build-configs.sh` | builds `bench` in several release configurations (LTO, codegen units, PGO) for generic x86-64, x86-64-v3 and znver5, and times single-thread decoding on synthetic images, alternately, pinned, best of several rounds |
| `perf/pgo.sh TARGET_DIR` | a deterministic profile-guided build of `bench` (needs `rustup component add llvm-tools-preview`) |

Pinning: run on an otherwise idle machine and pin to one CPU (the scripts use `taskset`,
CPU 2 by default; set `CPU=`). Turbo and frequency scaling still move results by a few
per cent between sessions, so compare only figures taken in one session, alternately, as
`compare.sh` does. `RUSTFLAGS="-C target-cpu=znver5"` builds for the measuring machine's CPU.

Which figures count: these benchmarks are for development and catch regressions; the corpus
frames are small test streams and the stress frames are synthetic. The figures against the
targets come only from the benchmark rounds on real discs recorded in sacd-spec
`benchmarks/README.md`. CI compiles the benchmarks and times one small subset, to catch
regressions: `.github/workflows/bench-regress.yml` builds `bench` and `callpattern` from the
base and from the head, times single-threaded DST decoding, plain DSD to DSF and DSD to PCM
alternately on one runner, and fails when the median of the per-round ratios head / base of
CPU time exceeds 1.15 (`.github/scripts/bench-regress.py`, #226). It runs on pull requests that
touch the library sources or build settings, on main (against the previous main) and nightly
(against the latest release), and is informational, not a required check.
