<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Measured performance, per release

Numbers for consumers deciding which version and which build to use. Each release adds a
section, newest first, measured on **lennon**: AMD Ryzen 9 3950X (Zen 2, 16 cores / 32
threads, SMT on), 125 GB RAM, NVMe, `performance` governor, Linux. That is the consumers' Zen 2
class; other CPUs differ (#224 also measures Zen 5 and Raptor Lake, through the kit).

## Method

- **Builds:** the release's benchmark kits (`scarletbook-benchkit-<version>-x86_64-linux.tar.gz`,
  generic, and `...-v3.tar.gz`, x86-64-v3), or kits built the same way from the release tag
  (`benchmarks/compare.sh build`): statically linked, profile `dist`. The generic build also
  picks the AVX2 DST decoder and PCM kernels at run time on CPUs that have them.
- **Cases and images:** `benchmarks/runlist.json`, unchanged unless the section says so:
  synthetic images from `scarletbook-synth` (DST from the generated stress frames, so every
  machine writes the same bytes), in the page cache (warm), output to `/dev/null`.
- **Runs:** each case runs every build of both versions in turn, alternating (ABBA), after one
  untimed run of each; 5 rounds (3 for `callpattern` and the 20-minute one-thread DST case); each run starts a fresh copy of its
  program (`benchmarks/README.md`, "Fresh copies"). Figures are medians over the rounds: wall
  time, and CPU time (user + system) of the process. For DSD to PCM, `callpattern`'s own
  figure: CPU milliseconds per second of stereo DSD64 converted to 176.4 kHz 24-bit, at 1, 8
  and 256 frames per call (the sacdfs call pattern is a few frames per call).
- **Ratio:** the newer version's median wall time over the older one's, per build; below 1
  is faster.

To reproduce, from a checkout of the newer version (on lennon, build under `lennon.sh run`
and time under `lennon.sh bench`):

```sh
benchmarks/compare.sh build ../cmp 3eb96dc v0.5.6   # git revisions or source directories
benchmarks/compare.sh run ../cmp > table.md         # about 20 minutes on the Zen 2 benchmark host, about 8.6 GB of images
```

`compare.sh run` passes extra options to `run.py`, for example `--case dst-decode-2ch-t1`.

## Correction, 2026-11-05: what the lock excluded

Every figure in this file from the Zen 2 benchmark host taken before 2026-11-05 was timed under a lock that excluded only
this project's own work, not other users' timed runs on the machine. So those runs were exposed to
other users' load. Whether any such run actually overlapped is not known. Alternating runs and
per-round ratios should still hold, since both builds of a round saw the same conditions, but that has
not been tested. **Absolute seconds, and above all the slowest-frame and slowest-window figures of
`bench realtime`, should be read with this in mind.** From 2026-11-05, timing runs go inside the
machine's shared `benchlock`, and idle fuzzing pauses whenever any `benchlock` process exists (#259).

## Not measured

Not in the run list yet, so not in these tables:

- **Cold runs** (image evicted from the page cache) of the whole-area cases.
- **The first 128 KiB** of 5 or 6 channels, or without a complete seek index.
- **The C interface:** `bench` calls the Rust API.
- **DSD to PCM from an image** (`callpattern` converts generated DSD in memory) and DST areas
  to PCM.

In the run list, but not in every table:

- **TOC open** and the **first 128 KiB** of a track, stereo, through a complete seek index: from
  `bench open` and `bench first` (#224), so for versions from 0.5.9. Earlier sections do not have
  them.
- Released versions before 0.5.6 have no `bench dsf` and no `callpattern`, so their cells for
  DSF and DSD to PCM are blank, unless an equivalent revision is compared instead (0.5.6
  below uses `3eb96dc` for 0.5.5).

## 0.5.9

Measured on the Zen 2 benchmark host (AMD Ryzen 9 3950X), 2026-11-06, with `compare.sh` (generic kits, statically linked, profile `dist`): 0.5.8 = `v0.5.8` and 0.5.9 = the release-prep tree. Timed under the machine's shared benchmark lock. 122 runs, none failed. The kit was built just before #283 merged; #283 changes only comments, Miri-only tests and CI, no timed code.

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

Medians of 5 rounds (3 for the 20-minute DST case and DSD to PCM), a fresh copy of each program and image per round, images in the page cache, output to `/dev/null`. The DSD-to-PCM cells are at 1 / 8 / 256 frames per call. `open` and `first` are new in 0.5.9's kit: their cells are the program's own median in microseconds, with no 0.5.8 figure. Spread between rounds above 5 %, so read as a range: plain-DSD 2 channels 0.537 to 0.567 s (0.5.8) and 0.532 to 0.566 s (0.5.9); plain-DSD 5 channels 0.562 to 0.644 s and 0.538 to 0.624 s; DST area to DSF 4.021 to 4.455 s (0.5.8); open, page cache 14.7 to 18.7 µs; open, evicted 192.6 to 235.5 µs; first 128 KiB plain DSD, evicted 236.5 to 253.4 µs; first 128 KiB DST all threads, evicted 4159.3 to 4798.9 µs.

**Plain-DSD rows:** differences under about 2 % are unresolved (#259), so they aren't stated as "within 1 %". The 2-channel row (1.006; 3 of 5 rounds slower, 2 faster) shows no change either way. The 5-channel row is 0.982 as a ratio of medians, under that line, and 0.970 as the median of per-round ratios, with all 5 rounds below 1. That agrees with #287's own 0.970 over 7 rounds, which is why it is reported as a likely gain and not a settled figure.

No row is slower than 0.5.8 beyond noise.

**5 channels (#288).** The run list has no 5-channel DST case, so this is a separate run, same method, 5 rounds, 20 runs, none failed:

| Path (not in the run list) | Build | 0.5.8 wall s | 0.5.8 CPU s | 0.5.9 wall s | 0.5.9 CPU s | 0.5.9 / 0.5.8 | Median of per-round ratios [range] |
|---|---|---|---|---|---|---|---|
| DST decode, one thread, 5 channels, corpus frames, 3 min | generic | 8.147 | 8.11 | 7.459 | 7.41 | 0.916 | 0.915 [0.913-0.923] |
| DST decode, all threads, 5 channels, corpus frames, 10 min | generic | 1.877 | 59.01 | 1.608 | 50.41 | 0.856 | 0.856 [0.853-0.857] |

### `bench realtime` (#255), under the shared lock

Generic kit, one pinned core, background fuzzing paused, image in the page cache, a fresh program copy per run, 5 runs per image, one short lock hold per run. Median [range]. This replaces the 0.5.8 section's `bench realtime` table, which predates the lock. 0.5.8's column is from #255 (2026-11-05); 0.5.9's was taken on 2026-11-06, in separate runs, not alternating with 0.5.8.

| Figure | Need | 6ch DST stress, 10 min, 0.5.8 | 6ch, 0.5.9 | Stereo DST stress, 50 min, 0.5.8 | Stereo, 0.5.9 |
|---|---|---|---|---|---|
| Mean real-time factor | ≥ 4× | 15.46× [15.19-15.52] | 15.80× [15.55-15.85] | 47.52× [47.36-47.58] | 47.94× [46.63-48.02] |
| Slowest 1-second window | ≥ 2× | 12.36× [6.69-13.83] | 14.56× [14.50-15.01] | 38.51× [13.33-38.60] | 37.17× [24.71-39.17] |
| 99.9th percentile frame | | 2.055 ms [2.023-2.317] | 1.930 ms [1.914-1.961] | 0.639 ms [0.636-0.839] | 0.634 ms [0.631-0.657] |
| Slowest frame | ≤ 6.7 ms | 4.743 ms [2.859-5.878] | 4.166 ms [2.451-4.675] | 3.432 ms [3.292-3.450] | 3.433 ms [3.268-3.463] |
| Frames over 6.7 ms | 0 | 0 in all runs | 0 in all runs | 0 in all runs | 0 in all runs |
| Seek to first frame, median of 200 | ≤ 100 ms | 1.906 ms [1.894-1.964] | 1.859 ms [1.855-1.869] | 0.636 ms [0.635-0.638] | 0.636 ms [0.634-0.640] |
| Seek to first frame, slowest of 200 | ≤ 100 ms | 2.836 ms [2.783-2.848] | 2.734 ms [2.704-2.815] | 0.922 ms [0.920-0.945] | 0.916 ms [0.912-0.922] |

All needs are met in every run. The tails still vary between runs: treat the slowest-window and slowest-frame rows as floors to check, not as figures to compare between versions.

Not measured for 0.5.9: the x86-64-v3 build; AVX-512 builds, Zen 5 and Raptor Lake; real output files; the first 128 KiB of 5 or 6 channels or without a seek index; cold runs of the whole-area cases; the C interface (see "Not measured"); `--lanes` is not in the run list (#318 has its figures). For `bench realtime`: an i9-13900H (#255's reference host), real discs, the corpus's own 6-channel areas, cold seeks and all-thread decoding.

## 0.5.8

Measured on lennon (3950X), 2026-11-05, with `compare.sh` (generic kits, statically linked, profile
`dist`): 0.5.7 = `v0.5.7` and 0.5.8 = the release-prep tree. No path's code changed for speed in this
release, and no row moves by more than 1 % in this run. On the plain-DSD rows that isn't a resolved
figure: differences under about 2 % are unresolved (see "Compare within this section only"), so their
ratios mean "no change resolved", not "within 1 %".

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

Medians of 5 rounds (3 for the 20-minute DST case and DSD to PCM; 82 runs), a fresh copy of each
program and image per round, images in the page cache, output to `/dev/null`. The DSD-to-PCM cells
are at 1 / 8 / 256 frames per call.

**Compare within this section only.** The plain-DSD rows read 0.555 s and 0.614 s for 0.5.7 here,
against 0.483 s and 0.540 s for the same library in the 0.5.7 section. The DST rows agree within 1 %.
Byte-identical fresh copies of an image differ by that much (#259). Free-memory state changes these
seconds: `run.py` on the same library gave rounds like this table's in one run (9 of 10 at 0.540 s or
above, at 36 GB free under 2 MiB and 9 GB above), medians of 0.517 to 0.533 s in three others, and some
rounds near 0.48 s in two of those, never as a median (`README.md`, "Fresh copies"). That makes it a
candidate: nothing recorded the memory state during this run, the state doesn't yet predict a table's median,
and other users' runs, which the shared lock did not yet exclude, can't be tested after the fact.

Within a round both builds read the same copy, and the per-round ratios stayed within 0.7 % in this run.
That isn't a general bound: over 20 later stereo rounds they stayed within 1.8 %, except one 1.042 after
reclaim started. So this table's comparison holds for differences above about 2 % on the plain-DSD rows;
smaller ones are unresolved.

### `bench realtime` (#255, new)

One thread, `lennon.sh bench`, pinned with `taskset -c 2`, 5 runs of a fresh copy; median [range]:

| Figure | 6ch DST stress, 10 min | Stereo DST stress, 50 min |
|---|---|---|
| Mean real-time factor | 15.20× [15.08-15.39] | 47.42× [47.04-47.64] |
| Slowest 1-second window | 12.74× [9.96-14.03] | 19.24× [12.77-33.58] |
| 99.9th percentile frame | 2.118 ms [1.974-2.529] | 0.700 ms [0.634-0.790] |
| Slowest frame | 4.651 ms [3.127-4.752] | 3.273 ms [3.266-3.433] |
| Frames over 6.7 ms | 0 in all runs | 0 in all runs |
| Seek to first frame, median of 200 | 1.940 ms [1.927-1.961] | 0.639 ms [0.637-0.645] |
| Seek to first frame, slowest of 200 | 2.718 ms [2.687-2.790] | 0.928 ms [0.927-3.459] |

The slowest-window row varies widely between runs: treat it as a floor to check, not a figure to
compare between versions.

Not measured for 0.5.8: the x86-64-v3 build; TOC open, first 128 KiB, cold runs and the C interface
(see "Not measured"); for `bench realtime`, an i9-13900H (#255's reference host), real discs, cold
seeks and all-thread decoding.

## 0.5.7

Measured on lennon (3950X), 2026-11-02, with `compare.sh` (generic kits, statically linked,
profile `dist`): the **generic** build of each version, medians of alternating runs (5 rounds; 3
for `dst-decode-2ch-t1-corpus` and `callpattern`), a fresh copy of each program per run, images in
the page cache. The library revisions are 0.5.6 = `v0.5.6` (`61aecd0`) and 0.5.7 = `da90c24`
(its library and `bench` sources are those of the release commit). A third kit, **0.5.6 with
0.5.7's `bench`** (the v0.5.6 tree with `crates/scarletbook-bench` of `da90c24`), ran in the same
rounds to separate the tool change below from the library change. Ratios are the medians of the
per-round ratios of wall times.

| Path | Run-list case | 0.5.6 | 0.5.6, new `bench` | 0.5.7 | 0.5.7 / 0.5.6 | tool¹ | library¹ |
|---|---|---|---|---|---|---|---|
| plain-DSD stereo area (78 min) to DSF, one thread | `dsf-plain-2ch` | 0.704 s | 0.600 s | 0.483 s | 0.693 | 0.857 | 0.805 |
| plain-DSD 5-channel area (30 min) to DSF, one thread | `dsf-plain-5ch` | 0.977 s | 0.894 s | 0.540 s | 0.553 | 0.913 | 0.605 |
| plain-DSD 6-channel area (25 min) to DSF, one thread | (not in the run list)² | 0.712 s | 0.624 s | 0.510 s | 0.719 | 0.874 | 0.816 |
| DST decode, stereo (20 min, corpus frames), one thread | `dst-decode-2ch-t1-corpus` | 20.62 s | 20.62 s | 20.18 s | 0.979 | 1.001 | 0.979 |
| DST decode, stereo (5 min, stress frames), one thread | `dst-decode-2ch-t1` | 6.23 s | 6.21 s | 6.10 s | 0.981 | 0.996 | 0.985 |
| DST decode, 6 channels (2 min, stress frames), one thread | `dst-decode-6ch-t1` | 7.68 s | 7.66 s | 7.62 s | 0.994 | 0.999 | 0.993 |
| DST decode, stereo (50 min, stress frames), all threads | `dst-decode-2ch-tall` | 3.99 s | 4.02 s | 4.03 s | 1.005 | 1.005 | 1.001 |
| DST decode, 6 channels (10 min, stress frames), all threads | `dst-decode-6ch-tall` | 2.61 s | 2.59 s | 2.59 s | 0.990 | 0.990 | 0.997 |
| DSD to PCM, stereo DSD64 to 176.4 kHz 24-bit, 8 frames per call | `pcm-callpattern` | 4.08 ms per s | 4.12 ms per s | 4.09 ms per s | 1.002 | – | – |

¹ **The measuring tool changed between these versions.** #239 made `bench` open images with
`BufReader::new` instead of a 1 MiB `BufReader`, as consumers' code does (the 1 MiB buffer copied
every byte twice). The 0.5.7 / 0.5.6 column therefore includes a tool effect that is not a library
gain: "tool" is 0.5.6 with the new `bench` over 0.5.6, "library" is 0.5.7 over 0.5.6 with the new
`bench`. The library gains are #230 (every DSF row) and #237 (5 channels; 0.875 s to 0.637 s in
#237's own same-tool measurement), and #241 (stereo DST on one thread).
² `dsf-plain-5ch`'s command on an image written by `scarletbook-synth IMAGE --minutes 25 --area mc
--mc-channels 6 --kind dsd`, through a run-list override (`run.py --runlist`).

The stereo all-threads case was slower in every round (by 0.3-1.2 %), and by 1.3 % (median of
per-round ratios) in an earlier separate run of 0.5.6 against 0.5.7 only; the third kit puts it on
the tool change. An earlier run of the one-thread cases in a `lennon.sh bench1` slot (one core
complex) gave the same ratios within 1 %.

**Correction (#248).** The seconds of the three plain-DSD rows in the first version of this
section were inflated by `run.py` (0.861 / 0.695 / 0.579 s stereo, 1.092 / 0.957 / 0.607 s 5
channels, 0.757 / 0.652 / 0.538 s 6 channels), and the 0.5.6 DSF cells did not match the 0.5.6
section. The images it timed were byte-identical files whose page-cache pages made every read of
them slower: `dd bs=8k` read the stereo image in 0.50 s against 0.41 s for a copy of it, and once
the same file was re-read into other pages it timed like the copy. `run.py` now times a fresh
copy of the image in each round (`README.md`, "Fresh copies"). The rows above are a re-run of the
same three kits with the fixed runner, on lennon, 2026-11-02, and agree within 1.5 % with direct
`bench dsf` runs. The 0.5.7 / 0.5.6 changes moved by at most 2 points; the split between tool and
library moved by up to 5 points (stereo: tool 0.805 to 0.857, library 0.834 to 0.805). The DST
rows did not move (two re-run within 0.8 %), and the DSD-to-PCM row reads no image.

Not measured for 0.5.7: the x86-64-v3 build; DST to DSF; TOC open, first 128 KiB, cold runs and
the C interface (see "Not measured").

## 0.5.6

Measured on lennon (3950X) by the owner, 2026-11-02: the **generic** build of each version,
medians of alternating runs (5 rounds for DSF, 3 for DST and DSD to PCM; the rounds of each
cell within 1 % of each other), images in the page cache. The library revisions are 0.5.5 =
`3eb96dc` (the merge of #212: the library is identical to v0.5.5, and it is the first revision
with `bench dsf`) and 0.5.6 = `6765ff2`. The images are those of the run list, written by
`scarletbook-synth`; each is byte-identical to the run list's recipe (checked with `cmp`).

| Path | Run-list case | Command | 0.5.5 | 0.5.6 | 0.5.6 / 0.5.5 |
|---|---|---|---|---|---|
| plain-DSD stereo area (78 min) to DSF | `dsf-plain-2ch` | `bench dsf IMAGE --area 2ch --threads 1 --out /dev/null` | 2.61 s | 0.70 s | 0.27 |
| plain-DSD 5-channel area (30 min) to DSF | `dsf-plain-5ch` | `bench dsf IMAGE --area mc --threads 1 --out /dev/null` | 2.45 s | 0.97 s | 0.40 |
| DST decode, stereo (50 min, stress frames), all threads | `dst-decode-2ch-tall` | `bench decode IMAGE --area 2ch --threads 32 --sink null` | 4.05 s | 3.98 s | 0.98 |
| DST decode, 6 channels (10 min, stress frames), all threads | `dst-decode-6ch-tall` | `bench decode IMAGE --area mc --threads 32 --sink null` | 2.84 s | 2.75 s | 0.97 |
| DST decode, stereo (20 min, corpus frames), one thread | `dst-decode-2ch-t1-corpus` | `bench decode IMAGE --area 2ch --threads 1 --sink null` | 20.26 s | 20.83 s | 1.03 |
| DSD to PCM, stereo DSD64 to 176.4 kHz 24-bit, 8 frames per call | `pcm-callpattern` | `callpattern 60 7 8` | 5.09 ms per s | 4.08 ms per s | 0.80 |

Times are wall clock; DSD to PCM is CPU milliseconds per second of audio. `--threads 32` is
`--threads 0` on this machine. The one-thread stereo DST figure is 3 % slower in 0.5.6, a
regression tracked on #203.

Not measured for 0.5.6: the x86-64-v3 build; DST decode on one thread for the stress images
and for 6 channels; DST to DSF; TOC open, first 128 KiB, cold runs and the C interface (see
"Not measured").
