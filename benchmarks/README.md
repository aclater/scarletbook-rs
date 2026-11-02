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
| `bench` | the benchmark program (`bench --version`, `bench decode`, `bench dsf`; see `crates/scarletbook-bench`) |
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

**Inputs:** one or both kit archives of one release, verified and unpacked; about 8.3 GB of free
disk for the images; nothing else. **Network:** none, after the download and verification.
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
2. For each case: if `cache` is `warm`, read the image once; run each build's `warmup_runs`;
   then `rounds` rounds, alternating the builds as `alternation` says. Each run is the case's
   `command` with the placeholders filled in, started directly (no shell), from any directory,
   with the program (`command[0]`) copied to a new file first and that copy started (see
   "Fresh copies"). Copies are deleted after the case, not between runs.
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
is what `benchkit.yml` does to check each kit. A full run of two builds takes about 10 minutes
on a 16-core Zen 2 machine and writes about 8.3 GB of images. `--kit BUILD@LABEL=DIR` times kits
of several versions in one alternation; `python3 -I table.py results.jsonl` prints the medians
as a Markdown table.

## Published numbers

[`RESULTS.md`](RESULTS.md) holds the measured figures per release on lennon (3950X), generic
and x86-64-v3, with the method and what was not measured. `compare.sh` reproduces a section:
it builds kits from two revisions and times them against each other with this run list.
