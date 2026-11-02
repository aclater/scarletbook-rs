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
  untimed run of each; 5 rounds (3 for `callpattern`); each run starts a fresh copy of its
  program (`benchmarks/README.md`, "Fresh copies"). Figures are medians over the rounds: wall
  time, and CPU time (user + system) of the process. For DSD to PCM, `callpattern`'s own
  figure: CPU milliseconds per second of stereo DSD64 converted to 176.4 kHz 24-bit, at 1
  and at 256 frames per call (the sacdfs call pattern is 1 to a few frames per call).
- **Ratio:** the newer version's median wall time over the older one's, per build; below 1
  is faster.

To reproduce, from a checkout of the newer version (on lennon, build under `lennon.sh run`
and time under `lennon.sh bench`):

```sh
benchmarks/compare.sh build ../cmp v0.5.5 v0.5.6    # git revisions or source directories
benchmarks/compare.sh run ../cmp > table.md         # about 25 minutes, about 8 GB of images
```

`compare.sh run` passes extra options to `run.py`, for example `--case dst-decode-2ch-t1`.

## Not measured

Not in the run list yet, so not in these tables:

- **TOC open** (open to parsed TOC, page cache and evicted): `bench` has no command for it.
  The criterion benchmark `toc_read` measures it in development builds.
- **First 128 KiB** of a track (seek), page cache and evicted: no `bench` command.
- **Cold runs** (image evicted from the page cache) of any case.
- **The C interface:** `bench` calls the Rust API.
- **DSD to PCM from an image** (`callpattern` converts generated DSD in memory) and DST areas
  to PCM.
- Versions before 0.5.6 have no `bench dsf` and no `callpattern`, so their cells for DSF and
  DSD to PCM are blank.

## 0.5.6

Pending: the 0.5.5 to 0.5.6 figures, measured on lennon with the method above.
