# Clean-room record

This file records how scarletbook-rs was written: the environment, every source consulted,
and the statement that no forbidden source was accessed. It is kept up to date as the work
proceeds; each stage adds to the log at the end.

## Rules followed

- The only SACD/DST material consulted is the repository `aclater/sacd-spec`
  (specifications 0 to 7, the charset tables, the conformance corpus and its README, and its
  `PROVENANCE.md`).
- Not read, searched for, fetched or installed: the source of any SACD or DST
  implementation, including pysacd, sacd-ripper, `sacd_extract`, FFmpeg's DST decoder,
  sacd-rs, the dst-decoder crate and the Philips/ISO reference decoder. No web search for
  SACD or DST implementations was made.
- Where the specification is ambiguous or incomplete, an issue is opened on
  `aclater/sacd-spec`, the affected code is marked with a `TODO` naming the issue, and work
  continues elsewhere. The issues are listed below.
- From round 3 of the performance work (2026-10-30), by the owner's written instruction,
  general references that are neither SACD nor DST material may also be consulted for
  performance work: algorithm books and papers, Rust performance and SIMD guides,
  profilers and their documentation, CPU optimisation manuals, and general techniques of
  unrelated projects. Every such source is recorded under "General references consulted"
  below. The prohibition on SACD and DST implementations, and on searching for them,
  is unchanged; agents doing this work were instructed not to use search terms that name
  SACD, DSD or DST, and to report any such result they came across without opening it.

## Specification version

| Item | Value |
|---|---|
| Repository | https://github.com/aclater/sacd-spec |
| Branch | `main` (the specifications and the corpus were both merged there) |
| Commit | `37a16a37511e721eac8f119073d56702a1c591da` ("Merge pull request #2 from aclater/corpus", 2026-10-30) for stages 1 to 4; `07adda25b2c342883d5d5ed1500b30923e9fbe5b` ("Merge pull request #6 from aclater/fix/issues-4-5") for the follow-ups; `af3c3dd5` ("Merge pull request #10 from aclater/bench-output") for the benchmark program; `775f4ab` ("Merge pull request #11 from aclater/results-round1") for round 2, whose only change is the round 1 results in `benchmarks/README.md` and their `PROVENANCE.md` entry |
| Corpus | `corpus/` of that commit, copied unchanged into this repository's `corpus/` (CC0-1.0); manifest `format_version` 1, `spec_version` `1.0-draft`, 129 cases |
| Charset tables | `spec/charsets/*.txt` of that commit, copied unchanged into `crates/scarletbook/data/charsets/` (CC0-1.0) |

The clone was first on the `corpus` branch (commit `8544f6b`); it was switched to `main`
before any specification was read, and all work uses `main` at the commits above.

**Update to `07adda2`.** It changes specifications 0, 1, 4 and 7 (answering the two issues
below), adds the informative `benchmarks/README.md`, and extends `PROVENANCE.md`. Its
corpus and charset tables are byte-identical to `37a16a3`
(`git diff --stat 37a16a3 07adda2 -- corpus/ spec/charsets/` is empty), so the vendored
copies are unchanged. `benchmarks/README.md` was read: it defines a benchmark program
interface and quotes speed figures of two other readers as numbers only; its provenance
entry states that no implementation detail crosses the wall, and none was found in it.
No benchmark program has been written.

## Environment

Work was done on 2026-10-30 in an isolated VM (hostname `scarletbook-cleanroom`, Fedora 44,
kernel 6.19.10-300.fc44.x86_64) containing a clone of `aclater/sacd-spec`, a Rust toolchain,
git and gh. The GitHub token is scoped to `aclater/sacd-spec` and `aclater/scarletbook-rs`.
The home directory also held `cleanroom-verification.txt`, the operator's record of how the
VM was built (base image checksum, packages, no shared folders); it was read and contains no
implementation code.

### Checks made before any other work

Run on 2026-10-30, before any specification was read.

```
$ pip show pysacd
/bin/bash: line 1: pip: command not found
exit=127
$ python3 -m pip show pysacd
/usr/bin/python3: No module named pip
exit=1
```

`pip` is not installed, so pysacd cannot have been installed with it; the check fails as
required (if for that reason).

```
$ find / -xdev \( -iname '*pysacd*' -o -iname '*dstdec*' -o -iname '*sacd*extract*' \)
(no output)
```

**Caveat found and covered:** `/`, `/home`, `/var` and `/boot` are separate btrfs subvolumes
with different device numbers (`stat -c %d`: 34, 49, 52, 50), so `find / -xdev` does not
descend into `/home` or `/var`. The search was therefore repeated on each of them:

```
$ for m in / /home /var /boot /tmp; do find $m -xdev \( -iname '*pysacd*' -o -iname '*dstdec*' -o -iname '*sacd*extract*' \); done
(no output)
```

and widened, by name only, across every file system except `/proc` and `/sys`:

```
$ find / \( -path /proc -o -path /sys \) -prune -o \( -iname '*sacd*' -o -iname '*dsdiff*' \
    -o -iname '*dst*dec*' -o -iname '*ffmpeg*' -o -iname 'libavcodec*' \) -print
/home/<user>/src/sacd-spec
```

The only match is the sacd-spec clone. No SACD or DST implementation code was found on the
machine.

### Tools installed during the work

| Tool | Source | Why |
|---|---|---|
| Rust 1.85.0 (MSRV) and nightly-2026-10-30 (Miri, cargo-fuzz) | rustup | CI parity |
| `gcc-c++` 16.2.1 | Fedora 44 repositories (`dnf`) | libFuzzer needs a C++ compiler |
| crates: thiserror, rayon, proptest, serde_json, sha2, libfuzzer-sys and their dependencies | crates.io | dependencies; none is SACD- or DST-specific |
| cbindgen 0.29.4 | crates.io (`cargo install`) | generating the C header |
| `perf` (Linux perf 7.2) and GNU `time` | Fedora 44 repositories (`dnf`) | profiling and timing our own code |

## Sources consulted

| Source | What for |
|---|---|
| sacd-spec `spec/00` to `spec/05`, `spec/07`, `spec/charsets/` | the implementation |
| sacd-spec `spec/06-ps3-protocol.md` | not read (the network protocol is out of scope) |
| sacd-spec `corpus/README.md`, `corpus/manifest.json`, `corpus/images/` | conformance tests |
| sacd-spec `README.md`, `PROVENANCE.md`, `LICENSE` | process and licensing |
| sacd-spec `benchmarks/README.md` (at `07adda2`) | read when the specification was updated; informative, not used by the code |
| Rust standard library documentation (from memory of its API) | the implementation |
| apache.org `LICENSE-2.0.txt` | licence text |
| GitHub API (`gh api repos/<action>/releases`, `.../commits/<tag>`) | resolving action SHAs to pin |
| crates.io (`cargo search`, `GET /api/v1/crates/scarletbook`) | dependency versions; the crate name check |

The crate name check used `cargo search scarletbook` and the crates.io API only.
`cargo search` listed an unrelated crate, `sacd-rs`, by name and one-line description; it
was not opened, and neither its page, repository nor source was viewed.

## A provenance note on the specification

sacd-spec's own `PROVENANCE.md` states that its authors read pysacd's source while writing
the specifications: its TOC reader, demultiplexer and decoding engine (GPL-2.0-or-later),
and its DST decoder `csrc/dstdec.c` (LGPL-2.1-or-later), which it describes as a port of
FFmpeg's DST decoder. It also states that the prose DST description that specification 5 is
based on was written by a party that studied the Philips/ISO reference decoder, and it asks
whoever relies on the clean room to weigh these facts. This project's wall is between the
specifications and this code: no implementation was read here. The provenance of the
specifications themselves is recorded in sacd-spec and is not changed by this project.

## Performance work

The DST decoder was made faster (#16, #17) only by measuring and profiling this
repository's own code (`perf` on the corpus frames and on synthetic images built from
them) and applying general, non-DST-specific techniques: loop specialisation, keeping
state in registers, lookup tables, branch-free updates, batching and overlapping work
across threads. What crossed the wall from the other side is limited to what
`benchmarks/README.md` publishes: target figures and the other readers' timings, as numbers
only. No profile, explanation or technique of another reader was consulted.

Parts of #16, #17 and #18 were done by agent sessions started from this session, each in
its own git worktree on this VM, under the same rules: the same allowed sources (sacd-spec
only, at the commit above, plus Rust documentation), the same prohibitions, and the same
instruction to stop and report on finding any SACD or DST implementation (none was found).
Their commits were reviewed, re-tested (corpus, property tests, Miri) and re-measured here
before being merged into this branch.

### Round 2 (#21 to #25)

Benchmark round 1 (sacd-spec `775f4ab`, `benchmarks/README.md`, Results) was run by the
measuring side on real discs this project has never had. What was used from it is exactly
what it publishes: the machine and conditions, the figures of `bench` and of the other
readers, the targets met and missed, and the notes. Each missed target and each note became
an issue (#21 to #25). Nothing else crossed the wall: no profile, disc content or technique.

The work on those issues again used only our own measurements: profiling the corpus DST
frames grouped by the features that vary (filter order, segments, ptable lengths, channels),
a stress set of frames encoded with this repository's own annex 5.A test encoder (written
from specification 5), and synthetic images from `scarletbook-synth`. The finding that
long prediction filters drive the per-frame cost comes from that profiling; that real discs
probably use them is an inference from comparing our order-128 figures with the published
real-disc figure, not information about the discs. The resulting prediction change speeds up
order-128 frames (about 22% at 2 channels, 36% at 6) and slows order-32 frames by up to 10%,
a trade-off accepted on that inference; round 2 tests it.

The parallel decoding change (#21, #23, #24) and the decoder change (#22) were done by two
agent sessions under the same rules as before, with sacd-spec at `775f4ab` as the only
SACD/DST source; their commits were reviewed, re-tested and re-measured here before being
merged. The build provenance attestation (#25) uses GitHub's
`actions/attest-build-provenance` (pinned), which is not SACD-related.

`scarletbook-synth`, the generator of large test images used for these measurements, is
built only from specifications 1 to 4 (layout, TOCs, audio sector packing) and from the
corpus's own DST frames, which it repacks. Figures measured on its images are not
measurements of the benchmark discs of `benchmarks/README.md`: its DST frames are the
corpus's feature-test frames, and its plain DSD is pseudo-random.

## General references consulted

Performance round 3 (#30, #31, #32). Each entry: the source, and what was taken from it.
None is an SACD or DST implementation or describes one.

**Third-party source code read during the survey, and the owner's decision.** The survey
read two source files of unrelated video codecs: FFmpeg's `libavcodec/cabac_functions.h`
(the H.264 arithmetic decoder, LGPL) and dav1d's `src/msac.c` (the AV1 entropy decoder,
BSD-2-Clause). Neither is SACD or DST code, and the round 3 allowance covers general
techniques of unrelated projects. But FFmpeg is the tree that also contains a forbidden DST
decoder, and its code is LGPL. On 2026-10-30 the owner decided (option "B"):

- the reading is recorded here, as consulted for general techniques only;
- **no code is copied** from either file;
- **no technique is adopted on the strength of those two files alone**: every technique this
  project uses must also be supported by a paper, article, book or documentation recorded
  in this section;
- from that point on, no source file is opened from any repository that also contains an
  SACD or DST implementation (so nothing more from FFmpeg), and papers, articles and
  documentation are preferred to code.

The full source list of the survey follows when it reports (issue #33).

## Statement

None of the forbidden sources listed under "Rules followed" was read, searched for, fetched
or installed while writing this repository. Every behaviour implemented here comes from the
sacd-spec specifications at the commit above, checked against its corpus.

## Specification issues filed

| Issue | Gap | Resolution | scarletbook-rs |
|---|---|---|---|
| [aclater/sacd-spec#4](https://github.com/aclater/sacd-spec/issues/4) | 1.5.1 / 7.7.3 did not define what is compared between master TOC copies; 0.2 item 6 named only the area TOC comparison | answered in `07adda2` (1.5.1 item 3): one 10-sector read per copy, all 20 480 bytes compared, unreadable copies skipped, reported at most once | the code waited with a `TODO` until then; implemented as specified (#8) |
| [aclater/sacd-spec#5](https://github.com/aclater/sacd-spec/issues/5) | per-sector `audio-sector-malformed` warnings (4.4.5) versus the allocation bound of 7.1 item 2; capping, early stop and repeated scans unspecified | answered in `07adda2` (4.4.5 items 1 to 3, 7.1 item 2) | the code kept only a count, with a `TODO`, until then; implemented as specified (#11) |
| [aclater/sacd-spec#9](https://github.com/aclater/sacd-spec/issues/9) | `benchmarks/README.md` did not define the `bench` output for tracks without audio, a missing area, or track numbers of 100 or more | answered in `af3c3dd` (the `bench` interface and its error codes) | interim behaviour marked with `TODO`s; it already matched the answer, which `bench` now follows exactly (#14) |

Stages 1 to 4 met no gap that required a guess. Both issues concern optional warnings
that the corpus does not compare, found while working through the review follow-ups.
Neither was resolved by guessing; no spec-gap `TODO` remains in the code.

## Crate name

`cargo search scarletbook` and `GET https://crates.io/api/v1/crates/scarletbook` (HTTP 404)
on 2026-10-30: the name `scarletbook` is not taken on crates.io. Nothing was published.

## Log

| Date | Stage | Notes |
|---|---|---|
| 2026-10-30 | environment | checks above |
| 2026-10-30 | 1: TOC and sector detection | specifications 0 to 3 and 7; all 129 corpus cases pass the TOC, warning and error comparison |
| 2026-10-30 | 2: audio sector multiplexing | specification 4; every corpus track without DST frames (245 of 437) matches its per-channel SHA-256 and frame counts; the 192 tracks with DST frames await stage 3. No new sources consulted. |
| 2026-10-30 | 3: DST decoding | specification 5 (including informative annex 5.A, used only to write a test encoder); all 129 corpus cases pass in full: TOC, warnings, rejections and every track's per-channel SHA-256 and frame counts (437 tracks). The prediction sum is evaluated with per-filter lookup tables over history bytes, a regrouping of the sum in 5.5.3 derived here, not taken from any decoder. No new sources consulted. |
| 2026-10-30 | 4: C FFI | no specification content; cbindgen 0.29.4 (MPL-2.0, a build tool, not linked) installed from crates.io to generate the header. No new sources consulted. |
| 2026-10-30 | follow-ups | review fixes and issues #5 to #12; two specification gaps filed (sacd-spec#4, #5), answered upstream in `07adda2` and then implemented as specified. No new sources consulted beyond sacd-spec. |
| 2026-10-30 | benchmarking | `bench` program and release workflow (#14, #15); a third specification gap filed (sacd-spec#9, answered in `af3c3dd`); DST and parallel performance work (#16, #17) and large-image measurements (#18), see "Performance work". No new sources consulted beyond sacd-spec. |
| 2026-10-30 | round 2 | the round 1 results (sacd-spec `775f4ab`, numbers and conditions only) turned into issues #21 to #25; parallel decoding, decoder and release-provenance work, see "Round 2"; version 0.2.0. No new sources consulted beyond sacd-spec. |
