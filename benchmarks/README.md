<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Benchmark kit and run list

This directory defines what a consumer's machine runs to time scarletbook without access to
anything else (issue #224, #226): a **kit** of statically linked programs, a **run list**
(`runlist.json`) of the cases, and a reference **runner** (`run.py`). The results come back
as JSON lines, one per timed run. The figures are black-box: wall and CPU time of a process,
measured from outside it.

## The kit

Every release publishes two archives, built by `.github/workflows/benchkit.yml`:

| Asset | Build | Runs on |
|---|---|---|
| `scarletbook-benchkit-<version>-x86_64-linux.tar.gz` | generic x86-64 | any x86_64 Linux |
| `scarletbook-benchkit-<version>-x86_64-linux-v3.tar.gz` | `-C target-cpu=x86-64-v3` | x86-64-v3 CPUs only (AVX2, BMI1/2, FMA, MOVBE, ...) |

Each has a `.sha256` checksum and the release's build provenance attestation; verify both
before extracting (see "The sandboxed-runner contract", step 0). Match the names **exactly**: a prefix or glob of the generic name also matches the
`-v3` one, which stops with SIGILL on older CPUs. The names share no prefix with the
`bench-<version>-...` benchmark assets of sacd-spec or the `scarletbook-ffi-<version>-...` C
library archives. Every push to main uploads the same two archives as workflow artifacts,
`benchkit-x86_64-linux[-v3]-<commit>`, with archives named
`scarletbook-benchkit-<version>-<commit>-x86_64-linux[-v3].tar.gz` (the short commit after
the last released version), so they cannot be taken for release kits. They carry no
attestation; use them for development only.

An archive holds one directory:

| File | What it is |
|---|---|
| `bench` | the benchmark program (`bench --version`, `bench decode`, `bench dsf`, `bench realtime`; see `crates/scarletbook-bench`) |
| `scarletbook-synth` | writes the synthetic images of the run list |
| `callpattern` | the scarletbook-pcm `callpattern` example: DSD64 stereo to 176.4 kHz 24-bit, generated input |
| `runlist.json` | the cases (below) |
| `corpus/` | the two corpus images `scarletbook-synth` takes DST frames from (CC0, `corpus/LICENSE`) |
| `run.py` | the reference runner (below) |
| `table.py` | prints the Markdown table of `RESULTS.md` from the runner's output |
| `README.txt`, `README.md`, `LICENSE-*` | version, commit and build; this file; the licences |

The programs are statically linked against glibc (`x86_64-unknown-linux-gnu` with
`-C target-feature=+crt-static`, `profile dist`): they need no shared library, no network and
no privileges, and run on any x86_64 Linux kernel 3.2 or later. They are not musl builds: on
lennon (3950X) musl builds of `bench` took 11 % longer than glibc ones, from musl's allocator
(all-thread DST decoding: 4.46 s against 4.02 s) and memcpy (plain DSD to DSF: 2.03 s
against 1.81 s), while the static glibc build timed the same as the dynamic one (4.02 s and
1.81 s). Both builds pick the same run-time paths as any other build of their level: the
generic one also runs the AVX2 DST decoder and PCM kernels on CPUs that have them.

## The run list

`runlist.json` (`format_version` 1) has:

- `builds`: the build ids (`generic`, `v3`), their archive names, and the `/proc/cpuinfo`
  flags a build needs. A runner skips a build whose flags the CPU lacks and says so.
- `images`: each image's `scarletbook-synth` arguments after the output path. The images are
  deterministic (DST frames from the generated stress set, or from the corpus images in the
  kit), so any kit writes the same bytes; `approx_bytes` is for disk planning (about 8.3 GB in all).
- `defaults` and `cases`. Each case has an `id`, the `path` of #224 it measures, an `image`
  (or `null`), a `command` with placeholders, and, unless the default applies:
  - `rounds` (default 5): timed runs per build;
  - `alternation` (default `abba`): in each round every build runs once, in list order on even
    rounds and reversed on odd ones, so drift over time falls on both builds alike;
  - `cache` (default `warm`): the image is read once before the case, so it is in the page
    cache; `cold` is not used yet;
  - `warmup_runs` (default 1): untimed runs per build before the rounds;
  - `stdout` (default `discard`): `keep` sends the program's stdout back (callpattern prints
    its own CPU time per second of audio, per call size, and an output checksum);
  - `frames`: the area's frames, for frames per second.
- `missing`: the #224 paths the kit cannot time yet, and why.

Placeholders: `{kit}` the unpacked directory of the build being timed, `{image}` the case's
image file, `{scratch}` a writable directory. Threads `0` mean one per logical CPU.

| Case | #224 path | Image |
|---|---|---|
| `dst-decode-2ch-t1` | DST decode, one thread, 2 ch | 5 min 2 ch DST |
| `dst-decode-2ch-t1-corpus` | DST decode, one thread, 2 ch, corpus frames | 20 min 2 ch DST (frames of the corpus images in the kit) |
| `dst-decode-2ch-tall` | DST decode, all threads, 2 ch | 50 min 2 ch DST |
| `dst-decode-6ch-t1` | DST decode, one thread, 6 ch | 2 min 6 ch DST |
| `dst-decode-6ch-tall` | DST decode, all threads, 6 ch | 10 min 6 ch DST |
| `dsf-plain-2ch` | plain-DSD area to DSF, 2 ch, one thread (to `/dev/null`) | 78 min 2 ch plain DSD |
| `dsf-plain-5ch` | plain-DSD area to DSF, 5 ch, one thread (to `/dev/null`) | 30 min 5 ch plain DSD |
| `dsf-dst-2ch-tall` | DST area to DSF, all threads (to `/dev/null`) | 50 min 2 ch DST |
| `pcm-callpattern` | DSD to PCM, one thread | none (generated in memory) |

Not in the kit yet (`missing`): TOC open and the first 128 KiB (no `bench` command for either),
cold runs (no eviction step), the C interface (`bench` uses the Rust API), and DSD to PCM from
an image.

## The sandboxed-runner contract

**Inputs:** one or both kit archives of one release, verified and unpacked; about 26 GB of free
disk: 8.3 GB for the images, 0.85 GB for the self-check, and the per-round copies of one case's
image (up to 5 × 3.3 GB); nothing else. **Network:** none, after the download and verification.
**Privileges:** none.

**What the runner executes**, in this order:

0. **Verify each archive before extracting it** (required). The checksum proves the bytes are
   intact; the attestation proves this repository's release workflow built them:

   ```sh
   sha256sum -c scarletbook-benchkit-<version>-x86_64-linux.tar.gz.sha256
   gh attestation verify scarletbook-benchkit-<version>-x86_64-linux.tar.gz -R aclater/scarletbook-rs \
     --signer-workflow aclater/scarletbook-rs/.github/workflows/release.yml
   ```

   and the same for the `-v3` archive. Stop if either fails.

1. For each image a selected case needs: `<kit>/scarletbook-synth <work>/<image id>.iso <synth
   arguments>`, with either kit (the bytes are the same). Images can be kept between versions
   while their arguments are unchanged.
2. For each case: `rounds` rounds, alternating the builds as `alternation` says. Each round
   first writes a fresh copy of the case's image (a plain read/write copy, flushed to disk) and,
   if `cache` is `warm`, reads it once; every build of that round times that copy, and the
   first round also runs each build's `warmup_runs` on it. Each run is the case's `command` with
   the placeholders filled in, started directly (no shell), from any directory, with the program
   (`command[0]`) copied to a new file first and that copy started (see "Fresh copies"). Copies
   of programs and images are deleted after the case, not between runs or rounds.
3. Nothing runs in the background meanwhile; an otherwise idle machine gives the most useful
   figures.

**Outputs:** one JSON object per timed run, one per line (JSON Lines), with at least:

| Field | Value |
|---|---|
| `case` | the case id |
| `build` | `generic` or `v3` |
| `version` | the last word of `bench --version` |
| `label` | the kit's label (`--kit BUILD@LABEL=DIR`), by default its version |
| `round` | 0, 1, ... |
| `exit` | the exit code; a non-zero one fails the run (`stderr` holds its first 4000 bytes) |
| `wall_s` | wall-clock seconds from start to exit |
| `user_s`, `sys_s`, `cpu_s` | the child's user, system and total CPU seconds (`wait4` rusage) |
| `max_rss_kib` | the child's peak resident set |
| `free_below_2m_gib`, `free_from_2m_gib` | free memory at the start of the round, before its image copy is written, in blocks under 2 MiB and at or over it (`/proc/buddyinfo`; null without that file) |
| `stdout` | the program's stdout, for cases with `stdout: keep` |
| `host` | `cpu` (model name), `logical_cpus`, `usable_cpus` (affinity), `smt`, `governor`, `mem_bytes`, `kernel` |

Extra fields are welcome (for example the machine's name, the date or the storage).

## Fresh copies

On lennon (3950X), one file of `bench` decoded a DST area 42 % slower than byte-identical
copies of it: 1.77 s of CPU against 1.24 s, with the same instruction count, on every run, until
that file's page cache was dropped (`posix_fadvise(POSIX_FADV_DONTNEED)`), after which it ran at
1.24 s too. Eight further copies all ran at 1.23-1.25 s. The physical pages that happen to hold
a program's code can cost that much, and the cost sticks to the file while it stays cached. A
run list that started the same file for every round would carry such an offset through all of
them; a fresh copy per run makes it a rare outlier, which the median of the rounds absorbs.

**Images too (#248).** The first 0.5.7 table timed plain-DSD-to-DSF 10-18 % slower through
`run.py` than direct runs of the same binaries, and the runner was not the cause: on the same
image file, `run.py` and a direct shell run agreed within 1 %. The two methods had read different,
byte-identical image files. One of them cost about 90 ms more system time per 3.3 GB pass in
every run (`dd bs=8k` from the page cache: 0.50 s against 0.41 s for a copy); evicting it and
reading it back left it as slow (the freed pages were taken again), but reading it back while
8 GB of other memory was held put it on other pages, and it then timed like the copy. Copies
written right after other large files were deleted were slower in the same way, for the first
round or two. Which physical pages hold an image matters for the cases that stream gigabytes
from the page cache. So the runner writes a fresh copy of the image in each round and keeps the
copies until the case ends, so that no copy takes pages another just freed; the median of the
rounds absorbs a slow first round. On lennon the plain-DSD rows then agree with direct runs on
images in the fast state within 1.5 %.

**Fresh copies differ too (#259).** A fresh copy removes the stuck offset, but not the spread between
copies. On the Zen 2 benchmark host, ten byte-identical fresh copies of the 78-minute stereo image, one
0.5.7 `bench`, three runs each: 0.484 to 0.543 s (12 %), median 0.507 s, while the runs of any one copy
agree within 1.2 %.

**Only the kernel's cycles move.** Over 56 measurements of about 30 copies, with perf counters, wall time
ran from 0.475 to 0.589 s:

| Counter | Range | Correlation with wall time |
|---|---|---|
| Kernel cycles | 973-1478 M | 0.996 |
| Kernel cache misses | 10.3-25.2 M | 0.967 |
| dTLB load misses | 0.14-1.77 M | 0.976 |
| User cycles | 1194-1246 M | 0.18 |
| Kernel instructions | 322-326 M | |
| User instructions | 2923-2931 M | |
| Clock | 4.49-4.61 GHz | |

So the code path and the clock stay the same, and the same kernel instructions take up to 52 % more
cycles. `wait4`'s user time ran from 0.24 to 0.30 s while user cycles stayed flat, so that split is too
coarse to show this. A copy keeps its time: five copies re-timed at once and again 60 s later stayed
within 3 %. Temperature had no visible effect: one copy read 0.58-0.60 s from 35 to 65 °C (Tctl), before
and after 40 s of load on all threads.

**The free memory a copy is written into predicts its time.** Two chains of 12 fresh copies, all kept, each
staged and then timed. "Taken from" is the net fall in free memory by block size while the copy was written:

| Taken from | Wall time |
|---|---|
| Blocks under 256 KiB (falling as those blocks run out) | 0.50-0.59 s |
| Partly blocks of 256 KiB to 2 MiB | 0.49 s |
| Mainly blocks of 256 KiB and over | 0.475-0.48 s |
| None free, kswapd reclaiming page cache | 0.52-0.56 s |

The real `run.py`, `dsf-plain-2ch`, two builds, five rounds, predicted from that before it ran:

| State at the start | Rounds, slowest build | Median wall |
|---|---|---|
| 36 GB free under 2 MiB, 9 GB at or over | 0.596, 0.588, 0.584, 0.571, 0.540 s | 0.584 s |
| 27 GB under, 9 GB at or over | 0.545, 0.538, 0.539, 0.531, 0.513 s | 0.534 s |
| Under 1 GB under, 7 GB at or over (page cache filled first) | 0.486, 0.482, 0.532, 0.578, 0.573 s | 0.531 s |
| Under 1 GB under, 7 GB at or over, repeated | 0.483, 0.482, 0.517, 0.534, 0.542 s | 0.517 s |

In the last two, 7 GB lasts two copies, then reclaim starts. **So the memory state alone, with the same
code and the same staging, gives both figures #259 set out to explain,** about 0.48 s and rounds nearly
all at 0.540 s or above, under the shared benchmark lock. This is stereo only; the 5-channel case wasn't
tested.

**Not known.** Why smaller free blocks cost more kernel cycles: the physical layout of a file's pages
can't be read without root, so the misses above are a correlation, not a mechanism. What the memory state
was during the published 0.5.7 and 0.5.8 runs: nothing recorded it, so this reproduces the gap between
those tables but doesn't show it was the cause. `run.py` now records the state for each round. What
leaves memory in a given state, such as builds, idle fuzzing or other users' work, wasn't tested.

**Other users' runs stay a candidate.** The load history during the earlier runs isn't available, so it
can't be tested directly. The lock doesn't remove the effect: every figure above ran under it, and the
56 measurements saw at most 0.42 CPUs of other work. Competing work has a different look: 8 busy CPUs
took one copy from 0.58 to 0.62 s and the clock from 4.6 to 4.2 GHz, and 16 took it to 0.64-0.90 s with
user times of 0.31-0.44 s, above the 0.5.8 stereo rounds' 0.25-0.28 s. That argues against 16 or more
busy CPUs throughout those rounds, but not against smaller load.

So **seconds can differ between tables by more than 10 % with no change in the code.** The builds of a
round read the same copy, and their per-round ratios agreed within 0.7 % while those rounds spread 7 %
and 18 %. `table.py` lists any cell whose rounds differ by more than 5 %. Judge a change on the
ratios, and compare seconds only as ranges.

**The self-check.** Before the cases, `run.py` times plain-DSD-to-DSF on a 10-minute stereo
image eight times through its own path (a fresh program copy, `wait4`) and eight times as a
plain shell command (`sh -c 'exec bench ... >/dev/null 2>&1'`), ABBA, both on one staged copy of
the image, and exits with status 1 if the median of the per-round ratios is off 1 by more than
`--selftest-tolerance` percent (default 5). That catches anything the runner adds to a timed
run, which is what a kit's figures rely on. 5 % is above the run-to-run noise of this short case
(on lennon the rounds varied by up to 4 % and the median ratio was 0.99) and well below the 10-18 %
of #248. `--no-selftest` skips it; `--selftest-only` runs only it.

In `benchkit.yml` the self-check does not gate the build: the smoke step runs `--smoke
--no-selftest`, and a separate step with `continue-on-error: true` runs `--selftest-only` and
writes its ratio to the job summary. That workflow also runs on the release tag, and a shared
CI runner is too noisy for a 5 % check on runs of 0.1-0.3 s to fail a release. On a consumer's
machine the check stays on by default, with the strict 5 %.

**The run list is code.** It names the programs the runner starts, so a consumer uses the
`runlist.json` inside the verified kit, never one from elsewhere. `run.py` also checks it: every
command must start a program of the kit (`{kit}/NAME`, NAME of letters, digits, `_` and `-`), and
image and case ids must match `^[a-z0-9-]+$`, as they become file names; anything else is
rejected with an error before anything runs. `run.py --runlist FILE` is a developer override,
for trying a changed run list from a checkout.

## The reference runner

`run.py` implements the contract with the Python 3.8+ standard library:

```sh
tar -xzf scarletbook-benchkit-<version>-x86_64-linux.tar.gz
tar -xzf scarletbook-benchkit-<version>-x86_64-linux-v3.tar.gz
g=scarletbook-benchkit-<version>-x86_64-linux
python3 -I $g/run.py --kit generic=$g --kit v3=$g-v3 --work work > results.jsonl
```

`--case ID` (repeatable) selects cases; `--smoke` runs every case once on tiny images, which
is what `benchkit.yml` does to check each kit (with the self-check in a non-blocking step). A full run of two builds takes about 10 minutes
on a 16-core Zen 2 machine and writes about 8.3 GB of images. `--kit BUILD@LABEL=DIR` times kits
of several versions in one alternation; `python3 -I table.py results.jsonl` prints the medians
as a Markdown table.

## Published numbers

[`RESULTS.md`](RESULTS.md) holds the measured figures per release on lennon (3950X), generic
and x86-64-v3, with the method and what was not measured. `compare.sh` reproduces a section:
it builds kits from two revisions and times them against each other with this run list.
