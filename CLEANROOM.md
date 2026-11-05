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
  unrelated projects. Every such source that survives in the record is listed under
  "General references consulted" below. One gap: the first agent session on #31 failed
  before reporting, and its own list of sources was lost with its run (noted on #31). Its
  work-in-progress commit was kept; the agent that finished #31 read no external source
  beyond those already recorded (Giesen 2014, below), and the code was reviewed here. The prohibition on SACD and DST implementations, and on searching for them,
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

**Update to `1df1e6f` and `34f78a3`.** `1df1e6f` ("Merge pull request #13", round 2 results)
adds the round 1 real-world survey and the round 2 results to `benchmarks/README.md`, as
numbers and conditions only, and was the specification for round 3. `34f78a3` ("Merge pull
request #19") answers sacd-spec#14 (names of the checksum, CPU-specific and other `bench`
assets) and sacd-spec#18 (7.1 item 3 and the new 4.2 item 6: at most one pass over an area's
sectors per track or frame range requested). Neither changes the corpus or the charset
tables (`git diff --stat 775f4ab 34f78a3 -- corpus/ spec/charsets/` is empty), so the
vendored copies are unchanged. Following `34f78a3`, the x86-64-v3 `bench` asset was
renamed to `bench-<version>-x86-64-v3-linux.tar.gz`, outside the patterns of benchmark assets.

**Update to `fa602f9`** ("Merge pull request #20", answers to sacd-spec#15, #16 and #17).
It adds 4.6.1 (stored content of a position and its DST equivalent) and 4.7.1 (frame ranges
and tracks with pauses) to specification 4, mentions frame ranges in 7.1, 7.7.4 and 7.7.5,
and adds seven corpus cases (1-, 3- and 4-channel DST and plain DSD areas, and an area whose
channel count disagrees with its frames) and the per-area keys `play_range`,
`tracks_with_pauses` and `stored_frames`. The corpus was copied again, unchanged, into this
repository's `corpus/` (CC0-1.0): 136 cases, 457 tracks with audio, all passing on the
generic, x86-64-v3 and znver5 builds with and without `parallel`. The charset tables are
unchanged. Its PROVENANCE.md records that the spec side checked the new expected values
against another engine; that check happened on the specification side and nothing of it
crosses to this side beyond the published expected values. The new keys are checked by the
range and stored-frame work (#35, #37).

**Update to `e52a42b`** ("Merge pull request #21", a follow-up to #20). It defines the two
forms of stored content (4.6.1 item 2: an area's stored frame list, and the stored content of
a track or range, which the checks of 4.8 govern), bounds the memory and passes for them
(4.5 item 5, 7.1 item 3), and restates 4.2 items 2 and 6. Its corpus differs from `fa602f9`
only in `corpus/README.md`, copied again unchanged; the images and manifest are the same.

**Update to `da6b900`** ("Merge pull request #28", answers to sacd-spec#25, #26 and #27,
for issue #68). Specification 4 gains 4.2 item 7 (beginning at a sector `L` after `S0` with
the frame under assembly that processing `S0` to `L - 1` leaves, for positions `u` of at
least `a_L`, with a starting next acceptable frame number of at most `u`), 4.4.5 item 4
(counts and warnings of a reader that begins late; item 2 applies to the sectors it
processes only to rebuild the frame under assembly), 4.5 item 6 (informative: why such a
beginning is exact) and the last sentence of 4.7.1 item 6 (missing and invalid counts depend
only on the positions); specification 7 gains, in 7.1 item 2, the allocation allowance for
saved state and, in 7.1 item 3, one further pass per area to establish it. The range
`e52a42b..da6b900` also brings the round 3 benchmark results in `benchmarks/README.md`
(`0bf93ec`, numbers and conditions only, not needed for this work and not read for it),
repository hygiene (`204ee8e`: Scorecard, link check, Dependabot, `SECURITY.md`) and the
matching `PROVENANCE.md` entries. The corpus and the charset tables are unchanged
(`git diff --stat e52a42b da6b900 -- corpus/ spec/charsets/` is empty), so the vendored
copies are too.

**Update to `82eac6c`** ("Merge pull request #30", answer to sacd-spec#29, for issue #94).
Specification 4 gains 4.2 item 8 (provisional reading: beginning at a sector `L` after
`S0` without the state of item 7, with no frame under assembly and a next acceptable frame
number `u`, at the caller's request; its positions are exact when (a) `a_L <= u` and (b) the
frame under assembly at `L` is none or numbered below `u`, are reported as provisional until
both are established, and as possibly different if either fails; informative: positions
numbered `a*` or more are exact in any case, where `a*` is the next acceptable frame number of
a reader from `S0` just after it delivered or discarded the frame under assembly at `L`, while
below `a*` a position can differ whether or not it is missing), 4.4.5 item 5 (the counts of a
provisional reading begin at `L`, and the reader says so) and a sentence in 4.7.1 item 6
(counts of provisional positions are provisional); specification 7, 7.1 item 3 allows a
provisional reading to read at most a constant times `log2(n + 1)` sectors to choose `L`. Its
`corpus/README.md` gains four lines (provisional readings are not compared with the manifest
unless confirmed), copied unchanged into this repository's `corpus/README.md`; the images,
the manifest and the charset tables are unchanged (`git diff --stat da6b900 82eac6c --
corpus/images corpus/manifest.json spec/charsets/` is empty).

**Update to `cb9530b`** ("Merge pull request #32", answer to sacd-spec#31, for issue #124).
Specification 4, 4.2 item 8 names **locator samples** (pairs of a sector and the frame number
of its first frame record, saved from an earlier read) as one way to choose `L`: they only
steer that choice, never make a position exact, and need no check beyond ignoring a sample
outside `S0` to `S1'`. A new 4.2 item 9 says that what a pass over `S0` to `L - 1`
establishes (the state of item 7, (a) and (b) of item 8) stays established after it is saved
and loaded again, for an image whose sectors `S0` to `L - 1` are the bytes that pass
processed, checked as the reader chooses (a check of sectors at or after `L` alone is not
enough), and that (a) and (b) for `L` and `u_L` hold for any later reading from `L` with
`u >= u_L`. Specification 7, 7.1 item 2 allows saved locator samples: a constant number of
bytes each, their number chosen by the reader or the caller and checked when loaded. Only
`PROVENANCE.md` and those two specifications change (`git diff --stat 82eac6c cb9530b`); the
corpus and the charset tables are unchanged.

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
| `poppler-utils` and `tesseract` (OCR) | Fedora 44 repositories (`dnf`) | reading a scanned paper for the patent-hygiene record (#40) |
| criterion 0.7.0 (published 2025-07-25) | crates.io dev-dependency, no default features | the benchmark suite (#30); only its `--help` output and result files were used |
| inferno 0.12.8 (published 2026-07-18) | crates.io (`cargo install --locked`) | flame graphs (#30) |
| `llvm-tools-preview` for rustc 1.99.1 (LLVM 23.1.1) | rustup component | `llvm-profdata` for the PGO measurements (#32) |
| podman 5.8.1 (already installed, first used 2026-10-30) | Fedora 44 package `podman-5.8.1-1.fc44` | running the C library's Linux builds locally in the release build images (#42) |
| `quay.io/pypa/manylinux2014_x86_64@sha256:296069bc05d85275248f168f6e3b34ba8aa06181b67e40cd7447c64d56d3a851` and `quay.io/pypa/musllinux_1_2_x86_64@sha256:04e877afa939007f6784f9076d2aa797d1156f31281cfe19f203945d7b624970` (pulled 2026-10-30) | quay.io (PyPA) | the glibc 2.17 and musl 1.2 build images of `clib.yml`; the aarch64 images (`manylinux2014_aarch64@sha256:ec445e26...`, `musllinux_1_2_aarch64@sha256:1db34753...`) run only on CI (#42) |
| `docker.io/library/rust:1.99.1-alpine` (index `sha256:7e9e2d068a107a2adafd19930011178d14a78326053cd4c3a9f7ad3fd049557d`, pulled 2026-10-30) | Docker Hub | pulled while choosing a musl build image; not used (#42) |
| rustup-init 1.29.2 (x86_64-unknown-linux-gnu and -musl, inside the images above, 2026-10-30) | static.rust-lang.org `rustup/archive/1.29.2/`, checked against its pinned SHA-256 | installing Rust 1.99.1 in the build images (#42) |
| GNU binutils 2.46.1 (`objdump`, `readelf`, `nm`; already installed) and binutils of the PyPA images | Fedora 44 package `binutils-2.46.1-1.fc44`; the images | checking glibc symbol versions, SONAME, dependencies and exports of our libraries (#42). `otool`, `nm` and MinGW `objdump` ran only on the GitHub runners. |
| zizmor 1.31.1 (published 2026-10-15) | crates.io (`cargo install --locked`), 2026-10-30 | linting the workflows locally before pushing (#46) |

### Repository services and CI tools (#46)

Added on 2026-10-30 for repository hygiene (issue #46). None is SACD- or DST-related; they
analyse this repository's own code and workflows, and nothing from them enters the library.

| Tool or service | Version (pinned) | Used for | Read to set it up (2026-10-30) |
|---|---|---|---|
| OpenSSF Scorecard (`ossf/scorecard-action`) | v2.5.0 | repository security score, SARIF to code scanning, published badge | the action's `README.md` at the pinned commit |
| CodeQL (`github/codeql-action`) | v4.39.1 | static analysis of the Rust code and the workflows | GitHub's code scanning documentation (from memory of its inputs; `init`/`analyze`/`upload-sarif`) |
| zizmor (`zizmorcore/zizmor-action`, zizmor 1.31.1 locally) | v0.8.0 | security lint of the workflows, SARIF to code scanning | the action's `README.md` at the pinned commit |
| Dependency review (`actions/dependency-review-action`) | v5.0.0 | vulnerable dependencies added by a pull request | the action's `README.md` at the pinned commit |
| cargo-llvm-cov (installed by `taiki-e/install-action`) | 0.9.1 (published 2026-09-06) | line coverage of the workspace tests | its `--help` output and the crates.io version list |
| Codecov (`codecov/codecov-action`, codecov.io) | v7.1.1 | coverage reports and badge, uploaded with the workflow's OIDC token | the action's `README.md` at the pinned commit |
| Dependabot | GitHub service | weekly grouped updates of crates and actions, 14-day cooldown | GitHub's Dependabot configuration documentation (from memory) |
| `Swatinem/rust-cache` | v2.9.2 (2026-08-06; pinned `6323deb`) | caching dependency builds in `ci.yml`, saved only on `main` (#55) | the action's `README.md` at the pinned commit (inputs, `save-if`), read 2026-10-31 |

The action SHAs were resolved with `gh api repos/<owner>/<action>/git/ref/tags/<tag>`
(annotated tags dereferenced). The badge images come from shields.io, slsa.dev,
codecov.io and api.scorecard.dev; each URL was checked to return an SVG.

## Sources consulted

| Source | What for |
|---|---|
| sacd-spec `spec/00` to `spec/05`, `spec/07`, `spec/charsets/` | the implementation |
| sacd-spec `spec/06-ps3-protocol.md` | not read (the network protocol is out of scope) |
| sacd-spec `corpus/README.md`, `corpus/manifest.json`, `corpus/images/` | conformance tests |
| sacd-spec `README.md`, `PROVENANCE.md`, `LICENSE` | process and licensing |
| sacd-spec `benchmarks/README.md` (at `07adda2`) | read when the specification was updated; informative, not used by the code |
| Rust standard library documentation (from memory of its API) | the implementation |
| Rust standard library source documentation of `std::os::unix::fs::FileExt::read_at` and `std::os::windows::fs::FileExt::seek_read` (local `rust-src` of `nightly-2026-10-30`, 2026-10-31; online at https://doc.rust-lang.org/std/os/unix/fs/trait.FileExt.html and https://doc.rust-lang.org/std/os/windows/fs/trait.FileExt.html) | positional reads (#74): short reads, and the file cursor `seek_read` moves |
| apache.org `LICENSE-2.0.txt` | licence text |
| GitHub API (`gh api repos/<action>/releases`, `.../commits/<tag>`) | resolving action SHAs to pin |
| C library release (#34, #42), all 2026-10-30: the rustc book *Platform Support* (https://doc.rust-lang.org/nightly/rustc/platform-support.html: minimum glibc, musl, macOS and Windows versions per target); GitHub changelog *macOS 13 runner image is closing down* (https://github.blog/changelog/2025-09-19-github-actions-macos-13-runner-image-is-closing-down/: `macos-15-intel`) and a web search on that; actions/runner-images *Windows2025-Readme.md* (https://github.com/actions/runner-images/blob/main/images/windows/Windows2025-Readme.md: GCC and Python on `windows-latest`); the quay.io and Docker Hub registry APIs (image digests); https://static.rust-lang.org/rustup/release-stable.toml and `rustup/archive/1.29.2/<host>/rustup-init.sha256` | build targets, runners and pins of `clib.yml` |
| crates.io (`cargo search`, `GET /api/v1/crates/scarletbook`) | dependency versions; the crate name check |
| DSD-to-PCM conversion (#130), 2026-10-31: the papers and books listed under "General references consulted", "DSD-to-PCM conversion (#130)"; sacd-spec specification 0, 0.6 (bit and byte order of DSD bytes) | `scarletbook-pcm`; PCM conversion is outside the specifications |

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

### Round 3 (#30 to #32)

Round 3 again measured only this repository's code, on the corpus frames, the encoder's
stress set and `scarletbook-synth` images, and drew general techniques from the references
listed under "General references consulted" and dated under "Patent hygiene". What it merged:

- `DstDecoder::decode_many` (#31): two to four independent DST frames decoded in lockstep
  on one thread, so their arithmetic-decoder chains overlap; the lane count per channel
  count was chosen by measurement. `TrackAudioCursor` decodes groups of consecutive frames
  with it, on the pool and on the calling thread.
- Build configuration (#32): a `dist` profile (`codegen-units = 1`) for the generic `bench`
  asset and a labelled x86-64-v3 asset. Fat LTO and PGO were measured and not adopted.
- Tooling (#30): criterion benchmarks, `perf` scripts, and the tools listed under
  "Environment".

Two agent sessions worked on #31 under the same rules; the first failed before reporting
(see "Rules followed" for its lost source list), the second finished from its
work-in-progress commit. Their commits were reviewed, re-tested (corpus, property tests
at 1000 cases, Miri) and re-measured here before being merged.

## Patent hygiene (record-keeping rule, not a legal opinion)

By the owner's instruction (2026-10-30): no patent, live or expired, is read. Every source in
the corroboration record carries its publication date; techniques documented in
publications more than 20 years old (before 2006) are preferred; and every technique this
project adopts whose earliest non-code source found is from 2006 or later is flagged in the
table below, for review before 1.0. The research survey (#33) opened no patent page.

| Technique | Where used | Earliest non-code source found and read (date) | Before 2006 | Flag |
|---|---|---|---|---|
| Multi-bit renormalisation from a wider code register | arithmetic decoder (`dst.rs`) | A. Moffat, R. Neal, I. Witten, "Arithmetic Coding Revisited", ACM TOIS 16(3), 1998 | yes | |
| Normalisation by count-leading-zeros and one shift, instead of a loop | arithmetic decoder | M. S. Schmookler, K. J. Nowka, "Leading Zero Anticipation and Detection – A Comparison of Methods", ARITH-15, 2001 (leading-zero count for single-shift normalisation in floating-point units); Alpha Architecture Reference Manual, 4th ed., Compaq, January 2002 (CTLZ instruction) | yes | cleared (the match is to floating-point normalisation, not arithmetic coding) |
| Branch-free selection (if-conversion, conditional moves) | arithmetic decoder | S. A. Mahlke et al., "Effective Compiler Support for Predicated Execution Using the Hyperblock", MICRO-25, December 1992 (if-conversion); Alpha Architecture Reference Manual, 4th ed., January 2002 (CMOVxx "may be used to avoid branching") | yes | cleared |
| Reading zero bits past the end of the code | code-bit reader | A. Said, "Introduction to Arithmetic Coding – Theory and Practice", HP Labs HPL-2004-76, 2004 | yes | |
| Bit buffer with amortised refill (several bytes at a time) | code-bit reader | Full text read: A. Moffat, A. Turpin, "On the Implementation of Minimum Redundancy Prefix Codes", IEEE Trans. Communications 45(10), October 1997: p. 1203, a value "maintained as a w-bit window into the compressed bit stream", shifted left by a codeword's length with "the next length bits from the input stream" inserted into the vacated low bits (Algorithm ONE-SHIFT); p. 1204, w "chosen either as the next multiple of eight ... or as the machine word size"; p. 1206, footnote 1, decoding is faster "because of the use of a 32-bit input token in the compressed bit stream rather than an 8-bit input token". Also I. H. Witten, R. M. Neal, J. G. Cleary, CACM 30(6), June 1987, pp. 527-528 (byte buffer, reading past end-of-file) | yes | cleared (#40) |
| Precomputed tables of coefficient partial sums indexed by input bits | prediction (`dst.rs`) | Full text read: S. A. White, "Applications of Distributed Arithmetic to Digital Signal Processing: A Tutorial Review", IEEE ASSP Magazine 6(3), July 1989, pp. 4-19: p. 5, because each input bit "may take on values of 0 and 1 only", the sum of the coefficients it selects "may have only 2^K possible values. Rather than compute these values on line, we may precompute the values and store them in a ROM. The input data can be used to directly address the memory"; p. 7, a table indexed by several bits per input at once ("word capacity becomes 2^KL"). White dates the technique to Peled & Liu, 1974 (p. 4). Also NTU lecture slides on White 1989, PDF dated October 2002, slide pp. 5-8. L. B. Jackson, J. F. Kaiser, H. S. McDonald, IEEE Trans. Audio Electroacoustics AU-16(3), September 1968, pp. 413-421, read in full: does not state it | yes | cleared (#40) |
| Interleaving independent work in one thread to hide latency | `DstDecoder::decode_many` (`dst.rs`) and grouped decoding in `TrackAudioCursor` (`audio.rs`), #31 | Full text read: M. Lam, "Software Pipelining: An Effective Scheduling Technique for VLIW Machines", Proceedings of SIGPLAN '88, Atlanta, June 22-24, 1988, pp. 318-320: "multiple iterations, in different stages of their computations, are in progress simultaneously"; four in flight give "four times the speed". (University seminar copy, a scan read by OCR.) Applied here to independent decoders rather than loop iterations; F. Giesen, "Interleaved entropy coders", 2014, for that form | yes | cleared (the general technique) |
| Profile-guided and link-time optimisation | #32: measured, **not adopted** (fat LTO slower everywhere, PGO no consistent gain); only `codegen-units = 1` for the generic asset, plus an `x86-64-v3` asset | D. W. Wall, "Global Register Allocation at Link Time", SIGPLAN '86 Symposium on Compiler Construction, 1986 (link-time optimisation guided by profile data); DEC WRL Research Report 86/3 | yes | cleared |
| Multistage decimation, with each stage's stopband from its output rate minus the final stopband edge; polyphase evaluation of decimating FIRs | `scarletbook-pcm` (#130) | R. E. Crochiere, L. R. Rabiner, "Interpolation and decimation of digital signals — A tutorial review", Proc. IEEE 69(3), March 1981 (bibliographic record checked; full text not read in the #130 session) | yes | |
| Equiripple linear-phase FIR design by the Remez exchange, barycentric form | `scarletbook-pcm` generator (`design.rs`, test-only) | T. W. Parks, J. H. McClellan, IEEE Trans. Circuit Theory 19(2), March 1972; L. R. Rabiner, J. H. McClellan, T. W. Parks, Proc. IEEE 63(4), April 1975 (bibliographic records checked; full texts not read in the #130 session) | yes | |
| Halfband design from a one-band type II prototype (`H(z) = (z^-(2K-1) + G(z^2)) / 2`) | `scarletbook-pcm` generator | P. P. Vaidyanathan, T. Q. Nguyen, "A 'trick' for the design of FIR half-band filters", IEEE Trans. CAS 34(3), March 1987; F. Mintzer, IEEE Trans. ASSP 30(5), October 1982 (bibliographic records checked; full texts not read in the #130 session) | yes | |
| Precomputed tables of coefficient partial sums indexed by input bits, one table per input byte | `scarletbook-pcm` first stage | White 1989, as for `dst.rs` above (full text read for #40) | yes | cleared (#40) |
| Integer arithmetic carried exactly in IEEE-754 binary64 (operands and sums below 2^53), and rounding by adding and subtracting 1.5 x 2^52 | `scarletbook-pcm` kernels | D. Goldberg, "What every computer scientist should know about floating-point arithmetic", ACM Computing Surveys 23(1), March 1991 (bibliographic record checked; full text not read in the #130 session); IEEE Std 754-1985 (exact rounding of the basic operations) | yes | |

Texts the owner obtained for #40 (in `~/papers`, read in full; none is a patent):

- Moffat & Turpin, IEEE Trans. Communications 45(10), October 1997, pp. 1200-1207: states the
  wide-window refill (pp. 1203, 1204, 1206 as quoted above).
- Moffat & Turpin, *Compression and Coding Algorithms*, Kluwer, 2002 (preface dated January
  2002): front matter only (title, contents, preface; 10 pages). Does not state the technique;
  the sections the contents list for decoding (4.4, p. 63; 5.3, p. 98) were not in the file.
- Jackson, Kaiser & McDonald, IEEE Trans. Audio Electroacoustics AU-16(3), September 1968,
  pp. 413-421: does not state the table technique (coefficients in ROM, bit-serial
  multipliers).
- White, IEEE ASSP Magazine 6(3), July 1989, pp. 4-19: states the table technique (pp. 5,
  7, as quoted above). Its reference [3] (Croisier et al.) is a patent; it was not opened.
- NTU lecture slides on White 1989, PDF dated 8 October 2002: states the table technique
  (slide pp. 5-8).
- C.-W. Liu, NCTU lecture slides on distributed arithmetic, PDF dated 3 July 2011, and
  Nagajyothi & SriDevi, review paper, WiSPNET 2017: both state the table technique, both
  after 2006, so neither is relied on.
- Peled & Liu 1974 was not delivered; White 1989 cites it as the first description.

The dated search opened no patent; about twenty patent results (USPTO, Google Patents,
Justia) were skipped unopened. Sources it could only see as citations or bibliographic
records are marked as such above and do not clear a flag.

Where the rows above without a "Full text read" note were read: the dated search
(2026-10-30, an agent session under the same rules) opened each of these and extracted its
text locally, treating the file as data:

- Schmookler & Nowka, ARITH-15, 2001: author copy at
  https://userpages.cs.umbc.edu/phatak/645/supl/lza/lza-survey-arith01.pdf. Abstract: the
  leading-zero anticipator or detector "is pivotal to the normalization of results"; the
  introduction: the leading-digit position is found "so as to enable the normalization
  shift to start as soon as the addition completes".
- Alpha Architecture Reference Manual, 4th ed., Compaq, January 2002:
  https://csclub.uwaterloo.ca/~pbarfuss/alpha_arch_ref.pdf. CTLZ "Count leading zero"
  (CIX extension); CMOVxx, documented as usable to avoid branching.
- Mahlke et al., MICRO-25, December 1992, pp. 45-54: author copy at
  https://web.eecs.umich.edu/~mahlke/courses/583f18/reading/mahlke_micro92.pdf. If-conversion
  "replaces conditional branches with comparison instructions which set a predicate", so
  "control dependences are converted to data dependences".
- Wall, "Global Register Allocation at Link Time", SIGPLAN '86, as DEC WRL Research Report
  86/3: https://ftp.mirrorservice.org/sites/www.bitsavers.org/pdf/dec/tech_reports/WRL-86-3.pdf.
  Abstract: allocation "can be nearly that large if we use previously collected profile
  information to guide the allocation"; a link-time allocator rewrites the object code.

Techniques the specification itself prescribes (the decoding process of specification 5) are
not choices of this project and are not listed.

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

The research survey (issue #33) consulted the following. Its findings are in #33; the
corroboration required by the decision above is its table "Corroboration (decision B)".

- [S1] *Fast renormalization for H.264/MPEG4-AVC arithmetic coding*, D. Marpe, H. Kirchhoffer, G. Marten (Fraunhofer HHI), EUSIPCO 2006, https://new.eurasip.org/Proceedings/Eusipco/Eusipco2006/papers/1568982338.pdf — multi-bit register scheme; bit-scan or table shift count; 23.5–26.9% decoder gain.
- [S2] *Introduction to Arithmetic Coding — Theory and Practice*, A. Said, HP Labs HPL-2004-76 (arXiv 2302.00819), https://ar5iv.labs.arxiv.org/html/2302.00819 — renormalisation frequency and digit size; low-precision products; reading zero bits past the end.
- [S3] *Arithmetic Coding Revisited*, A. Moffat, R. Neal, I. Witten, ACM TOIS 16(3), 1998, https://pierre-hyvernat.apps.math.cnrs.fr/data/Enseignement/2425/info666/Files/acm98_arithmetic_coding_revisited.pdf — byte-wise renormalisation; shift/add; a specialised binary coder.
- [S4] *Range coder*, Hugi magazine #18 (describes Subbotin's coder), https://hugi.scene.org/online/coding/hugi%2018%20-%20corange.htm — byte-wise renormalisation about 2× faster for 0.01% loss.
- [S5] *[RFC] cmov-vs-branch optimization*, LLVM Discourse, 2021, https://discourse.llvm.org/t/rfc-cmov-vs-branch-optimization/6040 — Agner Fog's cmov-versus-branch rules.
- [S6] *Interleaved entropy coders*, F. Giesen, arXiv 1402.3392, 2014, https://ar5iv.labs.arxiv.org/html/1402.3392 — interleaving gains; branch-free SIMD decoder.
- [S7] *Low level conditional move optimization*, Acta Cybernetica, 2013, https://cyber.bibl.u-szeged.hu/index.php/actcybern/article/view/3827 — search summary only: cmov can help or regress.
- [S8] *Context-based adaptive binary arithmetic coding in the H.264/AVC video compression standard*, D. Marpe, H. Schwarz, T. Wiegand, IEEE TCSVT 13(7), 2003 — the bypass engine; renormalisation.
- [S9] Sze, Budagavi, high-throughput CABAC for HEVC, IEEE SiPS 2013, https://eems.mit.edu/wp-content/uploads/2013/11/sze_sips_2013.pdf — bypass bins skip the context feedback loop; diminishing returns past 4–8 bins per cycle.
- [S10] *Reading bits in far too many ways, part 2*, F. Giesen, 2018-02-20, https://fgiesen.wordpress.com/2018/02/20/reading-bits-in-far-too-many-ways-part-2/ — branch-free refill; 10% gain; padding.
- [S11] *Reading bits in far too many ways, part 1*, F. Giesen, 2018-02-19, https://fgiesen.wordpress.com/2018/02/19/reading-bits-in-far-too-many-ways-part-1/ — reads past the end; the 57-bit limit.
- [S12] *Discussing AMD's Zen 5 at Hot Chips 2024*, Chips and Cheese, 2024-09-15, https://chipsandcheese.com/p/discussing-amds-zen-5-at-hot-chips-2024 — 4-cycle L1D load-to-use; SMT rationale; one decode cluster per thread.
- [S13] *A whirlwind introduction to dataflow graphs*, F. Giesen, 2018-03-05, https://fgiesen.wordpress.com/2018/03/05/a-whirlwind-introduction-to-dataflow-graphs/ — latency versus throughput; independent chains.
- [S14] *Profiles*, The Cargo Book, https://doc.rust-lang.org/cargo/reference/profiles.html — LTO and codegen-units semantics.
- [S15] *Build Configuration*, The Rust Performance Book (N. Nethercote), https://nnethercote.github.io/perf-book/build-configuration.html — LTO 10–20%, PGO 10%+.
- [S16] *Faster Rust Toolchains for Android*, Google Android Developers Blog, Dec 2023, https://android-developers.googleblog.com/2023/12/faster-rust-toolchains-for-android.html — stepwise CGU1, LTO, PGO and BOLT gains.
- [S17] *Microarchitecture Levels*, Reloaded-III research notes, https://reloaded-project.github.io/Reloaded-III/Research/Microarchitecture-Levels.html — search snippet: x86-64-v3 and v4 contents.
- [S18] *Optimized Repositories*, CachyOS wiki, https://wiki.cachyos.org/ko/features/optimized_repos — a 5–20% v3 claim with no workload (judged weak).
- [S19] *Tracking issue for portable SIMD*, rust-lang/rust#86656, https://github.com/rust-lang/rust/issues/86656 — still unstable.
- [S20] *Announcing Rust 1.86.0*, Rust Blog, 2025-04-03, https://blog.rust-lang.org/2025/04/03/Rust-1.86.0/ — target_feature 1.1 stabilised.
- [S21] *RFC 2396: target_feature 1.1*, https://rust-lang.github.io/rfcs/2396-target-feature-1.1.html — unmatched-context calls still need `unsafe`.
- [S22] pulp documentation v0.22.3, https://docs.rs/pulp/latest/pulp/ — the safe dispatch API.
- [S23] pulp README, https://github.com/sarah-quinones/pulp — `#[inline(always)]` usage.
- [S24] multiversion documentation, https://docs.rs/multiversion/latest/multiversion/ — runtime dispatch through its `std` feature.
- [S25] multiversion attribute documentation, https://docs.rs/multiversion/0.6.1/multiversion/attr.multiversion.html — search summary: safe wrapper, `unsafe = true` contract.
- [S26] wide documentation v1.7.1, https://docs.rs/wide/latest/wide/ — SIMD types; no runtime dispatch.
- [S27] rayon `ThreadPoolBuilder` documentation, https://docs.rs/rayon/latest/rayon/struct.ThreadPoolBuilder.html — RAYON_NUM_THREADS; default thread count.
- [S28] rayon `IndexedParallelIterator` documentation, https://docs.rs/rayon/latest/rayon/iter/trait.IndexedParallelIterator.html — with_min_len / with_max_len; ordered collect.
- [S29] rayon-core sleep README, https://github.com/rayon-rs/rayon/blob/main/rayon-core/src/sleep/README.md — the worker sleep protocol (documentation, not code).
- [S30] rayon issue #576, https://github.com/rayon-rs/rayon/issues/576 — search summary: spinning on bursty workloads.
- [S31] *AMD's Ryzen 9950X: Zen 5 on Desktop*, Chips and Cheese, 2024-08-14, https://old.chipsandcheese.com/2024/08/14/amds-ryzen-9950x-zen-5-on-desktop/ — ROB partitioning under SMT; 1T IPC.
- [S32] uops.info, Zen 5 instruction latencies, https://uops.info/ — imul 3, lzcnt 1, cmov 1, shlx 1, shl r,cl 1.
- [S33] *Profile-guided optimization*, the rustc book, https://doc.rust-lang.org/rustc/profile-guided-optimization.html — the PGO workflow.
- [S34] cargo-pgo README and `ci/pgo.yml`, J. Beránek, https://github.com/Kobzol/cargo-pgo — PGO tooling and CI.
- [S35] awesome-pgo, A. Zaitsev, https://github.com/zamazan4ik/awesome-pgo — a survey of PGO results.
- [S36] **Read before the stricter rule (decision B):** FFmpeg `libavcodec/cabac_functions.h` (LGPL) and dav1d `src/msac.c` (BSD-2-Clause). Nothing copied; no finding above depends on them.
- [S37] AMD *Software Optimization Guide for the AMD Zen5 Microarchitecture*, pub. 58455 rev 1.00, Aug 2024 — identified by search, **not read**.
- From the first run: Agner Fog's *Instruction tables* (unreadable, not used) and Phoronix's Zen 5 SMT review (HTTP 403, not used).

Deliberately not opened under the rules: FFmpeg commit pages and mailing-list threads (the
whole FFmpeg repository is excluded), xz's `range_decoder.h`, the turbo-range-coder page, and
patent pages on CABAC renormalisation and branch-free arithmetic decoding. No search used
SACD, DSD or DST terms, and no result opened was about them.

### DSD-to-PCM conversion (#130)

The converter in `crates/scarletbook-pcm` and its filter generator were designed from the
general DSP literature below, from the author's knowledge of these well-known publications.
On 2026-10-31 each publication's bibliographic record (title, authors, venue, date) was
checked at `https://api.crossref.org/works/<DOI>` (metadata only). No full text was opened
in that session, and no code of any kind was read. Not opened: any DSD-to-PCM converter or
decimator (in any program or library), any file of a repository that also contains an SACD
or DST reader, aclater/sacd and its DECISIONS.md, the consumers' code, and any patent. No
web search was made. All sources are from before 2006.

- [P1] T. W. Parks, J. H. McClellan, "Chebyshev Approximation for Nonrecursive Digital Filters with Linear Phase", IEEE Trans. Circuit Theory 19(2), 1972, https://doi.org/10.1109/TCT.1972.1083419 — equiripple design by the Remez exchange.
- [P2] L. R. Rabiner, J. H. McClellan, T. W. Parks, "FIR digital filter design techniques using weighted Chebyshev approximation", Proc. IEEE 63(4), 1975, https://doi.org/10.1109/PROC.1975.9794 — weighting, the barycentric form, type II filters as a cosine polynomial times `cos(w/2)`. The 1973 program listing of the same authors was not used.
- [P3] O. Herrmann, L. R. Rabiner, D. S. K. Chan, "Practical Design Rules for Optimum Finite Impulse Response Low-Pass Digital Filters", Bell Syst. Tech. J. 52(6), 1973, https://doi.org/10.1002/j.1538-7305.1973.tb01990.x — length estimates (design only).
- [P4] J. F. Kaiser, "Nonrecursive digital filter design using the I0-sinh window function", Proc. IEEE ISCAS 1974, pp. 20-23 (citation only; no DOI record) — length estimates (design only).
- [P5] R. E. Crochiere, L. R. Rabiner, "Interpolation and decimation of digital signals — A tutorial review", Proc. IEEE 69(3), 1981, https://doi.org/10.1109/PROC.1981.11969; and *Multirate Digital Signal Processing*, Prentice-Hall, 1983 (citation only) — multistage and polyphase decimation, stage stopband edges, noble identities.
- [P6] F. Mintzer, "On half-band, third-band, and Nth-band FIR filters and their design", IEEE Trans. ASSP 30(5), 1982, https://doi.org/10.1109/TASSP.1982.1163950 — halfband properties.
- [P7] P. P. Vaidyanathan, T. Q. Nguyen, "A 'trick' for the design of FIR half-band filters", IEEE Trans. CAS 34(3), 1987, https://doi.org/10.1109/TCS.1987.1086124 — halfband design.
- [P8] A. Peled, B. Liu, "A new hardware realization of digital filters", IEEE Trans. ASSP 22(6), 1974, https://doi.org/10.1109/TASSP.1974.1162619; S. A. White, "Applications of distributed arithmetic to digital signal processing: a tutorial review", IEEE ASSP Magazine 6(3), 1989, https://doi.org/10.1109/53.29648 — tables of partial sums indexed by input bits (White read in full for #40, above).
- [P9] E. B. Hogenauer, "An economical class of digital filters for decimation and interpolation", IEEE Trans. ASSP 29(2), 1981, https://doi.org/10.1109/TASSP.1981.1163535 — CIC filters, considered and not used.
- [P10] D. Goldberg, "What every computer scientist should know about floating-point arithmetic", ACM Computing Surveys 23(1), 1991, https://doi.org/10.1145/103162.103163 — exact integer arithmetic in binary64.
- [P11] K. C.-H. Chao, S. Nadeem, W. L. Lee, C. G. Sodini, "A higher order topology for interpolative modulators for oversampling A/D converters", IEEE Trans. CAS 37(3), 1990, https://doi.org/10.1109/31.52724; R. Schreier, G. C. Temes, *Understanding Delta-Sigma Data Converters*, Wiley-IEEE Press, 2005 (citation only) — the test-only sigma-delta modulator (largest NTF gain 1.5).
- [P12] A. V. Oppenheim, R. W. Schafer, *Discrete-Time Signal Processing*, 2nd ed., Prentice-Hall, 1999 (citation only) — type I and II linear-phase FIRs, the bilinear transform, Butterworth poles (test modulator).

### Evidence record (#40)

On 2026-11-01, by the owner's decision on #40, the sources above and those of the #40
technique inventory were gathered into a preserved evidence record, indexed in
`docs/provenance/EVIDENCE.md`: dated Crossref and OpenAlex metadata for every source, legally
free full texts with their SHA-256 hashes and Wayback captures (kept outside the repository),
the page or section of each that supports a technique or the fact that it does not, and the
process records (GitHub exports, git history, the agent brief). Every source consulted for it
is listed in its Log row. No patent was opened.

## Statement

None of the forbidden sources listed under "Rules followed" was read, searched for, fetched
or installed while writing this repository. Every behaviour implemented here comes from the
sacd-spec specifications at the commit above, checked against its corpus.

**Update to `1517ab7` and `6db1251`** (sacd-spec PRs #34 and #36, answers to sacd-spec#33 and
#35, for issue #131). They add specification 8, "Output containers": DSF with an ID3v2.4 tag,
and DSDIFF with plain DSD or DST frames, markers, text and the edited master. The
specification side wrote it from three published documents, which its table cites (Sony's
DSF File Format Specification 1.01, Philips' DSDIFF 1.5 and ID3v2.4.0). This side has read
none of those documents, and no writer code; the formats come in only through specification
8. `6db1251` adds 8.4.10 item 4 (the clipped program of a frame range), the edited master's
range and post-roll (8.4.11) and empty text (8.4.8), and writer's choices 30 and 31. The
range `82eac6c..6db1251` also changes specifications 0, 3, 4 and 7 (sacd-spec#31); the #131
work uses only specification 8 from it, together with sections of 0, 2, 3 and 4 that it
doesn't change. The corpus and the charset tables are unchanged
(`git diff --stat 82eac6c 6db1251 -- corpus/ spec/charsets/` is empty).

**Update to `eafaaba`** ("Merge pull request #38", answer to sacd-spec#37). Specification 4,
4.2 item 8 gains an informative **suspect** mark: a reader MAY mark a provisional reading as
suspect when it has reasons of its own to expect (a) or (b) to fail before a pass has settled
them (examples: frame numbers that do not increase between the sectors read while choosing
`L`; an area whose TOC places its audio past the image's end or over another area's TOC or
audio sectors). The mark does not change the outcome, and a reader MAY read such areas or
readings exactly instead. Only `PROVENANCE.md` and specification 4 change
(`git diff --stat 6db1251 eafaaba`).

**Update to `e069569`** ("Merge pull request #40", answer to sacd-spec#39, for issue #169).
Specification 8 gains: DSF byte ranges built from chains of position ranges, for gapless
files (8.2.7); a caller-supplied ID3v2.3 tag in DSF, of which only the header is checked
(8.2.6 item 4); DSDIFF caller chunks after the local chunks (8.4.2 item 5); the `EMID` and
source-track clarifications of 8.4.8; writer's choices 32 and 33; and 8.7, departures from
the DSDIFF document (D1, the `CMPR` size counting its pad byte; D2, DSDIFF text as ISO
8859-1) that a writer makes only when its caller asks for one by number, and that make the
file non-conforming. Specification 0 changes in its 2-line table entry for specification 8 ("with
ID3v2 tags"), and the sacd-spec README in the same way. `PROVENANCE.md` adds the ID3v2.3.0
document ([ID3v2.3], for the header check of 8.2.6 item 4) and records that issue #39
restated a consumer's description of another extractor's observable output, with no
program's code read. This side has read neither that output nor the ID3v2.3 document; the
features come in only through specification 8. The corpus and the charset tables are
unchanged (`git diff --stat eafaaba e069569` lists only `PROVENANCE.md`, `README.md`,
`spec/00-conventions.md` and `spec/08-output-containers.md`).
**Update to `048aa1a`** ("Merge pull request #42", answer to sacd-spec#41, for issue #169;
spec commit `e07a5c3`). Specification 8, 8.4.8 adds that `DIIN` may hold none of its chunks
in a file that is not an edited master (no `EMID`, no markers, no `DIAR` or `DITI`), and
8.6 row 34 makes writing such a `DIIN` with `D = 0` (12 bytes) or leaving it out the
writer's choice. `PROVENANCE.md` records that the answer comes from the DSDIFF document's
section 3.7, which this side has not read. Only `PROVENANCE.md` and specification 8 change
(`git diff --stat e069569 048aa1a`).

**Update to `fa558a8`** ("Merge pull request #43", a cold read of the sacd-spec#39 and #41
text; spec commit `762683c`). Specification 8 gets eleven clarifications. None of them
changes the bytes of any file:
- the ID3v2.3 header check names `T[0]` to `T[2]`, and a frameless tag is written unchanged
  (8.2.6 item 4);
- the chain and which of its files are written are the caller's choice, every range is
  checked before any file is written, and an empty range splits the chain (8.2.7);
- a caller chunk with a bad ID is refused (8.4.2 item 5), and the `FRM8` data is the local
  chunks followed by the caller chunks;
- the track text items are specification 3, **3.8** (not 3.5) (8.4.8);
- D2's text rules (8.4.8, 8.7), and that a requested departure is allowed in an edited
  master;
- D1's reasoning names `MARK`, `DIAR` and `DITI`.

The code already did all of this: #183 (caller chunks), #185 (ID3v2.3), #187 (D1 and D2) and #186 (the DSF chain planner, checked against fa558a8 items 2, 3 and 7 with no code change and a new test for the empty-range split). The only change here is the section reference, which three
doc comments (and the generated header) and one test comment had copied as 3.5. `PROVENANCE.md` and specification 8
change (`git diff --stat 048aa1a fa558a8`). The black-box checks described in sacd-spec#43
were made on the specification side and are not part of this repository.

## Specification issues filed

| Issue | Gap | Resolution | scarletbook-rs |
|---|---|---|---|
| [aclater/sacd-spec#4](https://github.com/aclater/sacd-spec/issues/4) | 1.5.1 / 7.7.3 did not define what is compared between master TOC copies; 0.2 item 6 named only the area TOC comparison | answered in `07adda2` (1.5.1 item 3): one 10-sector read per copy, all 20 480 bytes compared, unreadable copies skipped, reported at most once | the code waited with a `TODO` until then; implemented as specified (#8) |
| [aclater/sacd-spec#5](https://github.com/aclater/sacd-spec/issues/5) | per-sector `audio-sector-malformed` warnings (4.4.5) versus the allocation bound of 7.1 item 2; capping, early stop and repeated scans unspecified | answered in `07adda2` (4.4.5 items 1 to 3, 7.1 item 2) | the code kept only a count, with a `TODO`, until then; implemented as specified (#11) |
| [aclater/sacd-spec#9](https://github.com/aclater/sacd-spec/issues/9) | `benchmarks/README.md` did not define the `bench` output for tracks without audio, a missing area, or track numbers of 100 or more | answered in `af3c3dd` (the `bench` interface and its error codes) | interim behaviour marked with `TODO`s; it already matched the answer, which `bench` now follows exactly (#14) |
| [aclater/sacd-spec#16](https://github.com/aclater/sacd-spec/issues/16) | frame ranges other than tracks (#35): no normative end of the area timeline, the 4.8 checks and per-track counts defined for tracks only, "tracks with pauses" informative | answered in `fa602f9` (sacd-spec PR #20; 4.7.1 frame ranges, play range, tracks with pauses, 4.8 applied to ranges), refined in `e52a42b` | implemented as specified (#44) |
| [aclater/sacd-spec#17](https://github.com/aclater/sacd-spec/issues/17) | raw frames (#37): no normative undecoded view of a position; plain and missing positions in DST passthrough undefined; "invalid" undefined without decoding | answered in `fa602f9` (sacd-spec PR #20; 4.6.1 stored content and DST equivalents), refined in `e52a42b` (stored frame list and per-range stored content) | implemented as specified, both forms and DST equivalents (#49) |
| [aclater/sacd-spec#18](https://github.com/aclater/sacd-spec/issues/18) | 7.1 item 3 described one pass over an area's sectors; range and out-of-order reads rescan | answered in `34f78a3` (4.2 item 6, 7.1 item 3): one pass per track or range is allowed; restated in `e52a42b` | conformant as written; the code cites 4.2 item 6 (#44) |
| [aclater/sacd-spec#33](https://github.com/aclater/sacd-spec/issues/33) | DSF and DSDIFF output (#131): no container format in the specifications; the published format documents are outside the clean room | answered in `1517ab7` (specification 8) | implemented as specified, every writer's choice of 8.6 an option (#131) |
| [aclater/sacd-spec#35](https://github.com/aclater/sacd-spec/issues/35) | specification 8: the edited master's range, markers of a frame range without a source track, post-roll, empty DSDIFF text | answered in `6db1251` (8.4.8, 8.4.10 item 4, 8.4.11; 8.6 rows 30 and 31) | implemented as specified (#131) |
| [aclater/sacd-spec#39](https://github.com/aclater/sacd-spec/issues/39) | specification 8: the writer's choices #169 needs (DSF byte ranges, a caller's ID3v2.3 tag, DSDIFF caller chunks, `EMID` and source-track text, files of other programs that depart from the DSDIFF document) | answered in `e069569` (8.2.6 item 4, 8.2.7, 8.4.2 item 5, 8.4.8, 8.7; 8.6 rows 3, 24, 32 and 33) | being implemented as specified, one PR per item (#169; caller chunks in #183) |
| [aclater/sacd-spec#41](https://github.com/aclater/sacd-spec/issues/41) | 8.4.8 at `e069569`: whether `DIIN` may hold none of its chunks once `EMID` is optional | answered in `048aa1a` (8.4.8; 8.6 row 34): it may, outside an edited master; written with `D = 0` or left out, the writer's choice | implemented as specified, both choices (#184) |

Stages 1 to 4 met no gap that required a guess. Both issues concern optional warnings
that the corpus does not compare, found while working through the review follow-ups.
Neither was resolved by guessing; no spec-gap `TODO` remains in the code.

## Crate name

`cargo search scarletbook` and `GET https://crates.io/api/v1/crates/scarletbook` (HTTP 404)
on 2026-10-30: the name `scarletbook` is not taken on crates.io. Nothing was published.

## Log

From 2026-10-31 (#76) each change's row is first written to `cleanroom.d/` in its pull
request and appended here when a release is prepared (`.github/scripts/fragments.py
assemble`), so that independent pull requests do not edit the same lines. Every change is
still recorded; until the next release, the rows of merged changes are in `cleanroom.d/`.

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
| 2026-10-30 | round 3 | interleaved DST decoding (#31), performance tooling (#30) and build configuration (#32), see "Round 3"; general references under the owner's round 3 allowance and decision B, see "General references consulted"; patent-hygiene table with every flag cleared on full texts read (#40). |
| 2026-10-30 | specification `34f78a3`, `fa602f9` and `e52a42b` | answers to sacd-spec#14 and #18 applied (the x86-64-v3 `bench` asset renamed outside the benchmark asset patterns); corpus of `fa602f9` vendored (136 cases, all passing); see "Specification version". No new sources consulted beyond sacd-spec. |
| 2026-10-30 | #35 frame ranges (#44) | decoded audio of any frame range, tracks with pauses. Specification 4, 4.2 item 6, 4.7, 4.7.1 and 4.8, and 7.1 item 3, at sacd-spec `e52a42b`. No new sources consulted. |
| 2026-10-30 | C library release (#34, #42) | `clib.yml` builds and tests the C library for Linux (glibc 2.17, musl 1.2), macOS and Windows (MinGW-w64), and the release publishes it with checksums and the attestation; ABI policy and its check (`abi-check.py`, baselines in `crates/scarletbook-ffi/abi/`). No SACD or DST source; general sources and tools as recorded above. |
| 2026-10-30 | repository hygiene (#46) | Dependabot, Scorecard, CodeQL, zizmor, dependency review and coverage (cargo-llvm-cov, Codecov) workflows; `SECURITY.md`, `CONTRIBUTING.md` (the clean-room rules for contributors, summarising "Rules followed", decision B and "Patent hygiene"), `CHANGELOG.md`; README badges. Repository infrastructure only; tools and the action READMEs read are under "Repository services and CI tools". No SACD or DST source consulted. |
| 2026-10-30 | diagnostic log (#38) | per-disc log hook in the library and the C ABI; the events follow specifications 1 to 5 and 7 (the 7.7 codes, the 4.4 to 4.6 frame rules). No new sources consulted beyond sacd-spec. |
| 2026-10-30 | #37 raw frames (#49) | stored content without decoding: the area's stored frame list, the stored content of tracks and ranges, DST equivalents. Specification 4, 4.4, 4.5 item 5, 4.6.1 and 4.8, specification 5, 5.2.1, and 7.1 item 3, at sacd-spec `e52a42b`. No new sources consulted. |
| 2026-10-31 | release hygiene (#50, #52) | release notes from CHANGELOG.md (stopping a release without its section before any build), `persist-credentials: false` in `clib.yml` and `release.yml`, one release run per tag. No new sources or tools. |
| 2026-10-31 | release 0.3.0 (#54) | version and changelog only: workspace version 0.3.0, the changelog section for the release notes. No new sources consulted. |
| 2026-10-31 | CI speed-up (#55) | Miri and fuzz gated on code changes, a Miri subset on pull requests (full on `main` and on manual runs), build cache. No new sources beyond the `rust-cache` README. |
| 2026-10-31 | package licences (#48) | the published `scarletbook` crate carries `LICENSE-MIT`, `LICENSE-APACHE` (copies checked against the root in CI) and the README. No new sources consulted. |
| 2026-10-31 | #36 decoding pool per cursor | `DecodeOn`: a cursor's DST frames decoded on a given rayon pool or on the calling thread; no change to decoding. Consulted the rayon-core 1.13.0 source in the local cargo registry (`ThreadPool` drop and build-failure behaviour), a general Rust library. No SACD or DST sources. |
| 2026-10-31 | #36 parallel decoding in C | `sb_pool_*` and `sb_track_audio_set_pool` over `DecodeOn::Pool`; no change to decoding. Consulted the rayon-core 1.13.0 source in the local cargo registry (thread pool build and drop), a general Rust library. No SACD or DST sources. |
| 2026-10-31 | #36 review follow-up | documentation of the C library's global state and pool-thread callbacks, one more re-entrancy probe, library sizes in the changelog. No new sources consulted. |
| 2026-10-31 | abandoned decode jobs (#29) | queued jobs of a continued or dropped cursor skip decoding; the transient bound documented. No new sources consulted. |
| 2026-10-31 | #39 concurrency test | N threads with N discs through the C interface. No new sources consulted. |
| 2026-10-31 | #39 struct layouts | C struct sizes, alignment and padding documented in the header and tested. No new sources consulted. |
| 2026-10-31 | #39 ctypes README | README section on using the C library from Python's ctypes, checked by clib-smoke.py. Python `ctypes` documentation (docs.python.org/3/library/ctypes.html) from memory, not fetched; a general reference. No SACD or DST sources. |
| 2026-10-31 | #39 guard coverage | every exported function under `guard` (source check), no panic from the error message's thread-local during thread exit. No new sources consulted. |
| 2026-10-31 | #28 Miri on the parallel path | a CI step runs the `parallel` tests and the C pool test under Miri with Tree Borrows; no change to the library. No new sources consulted. |
| 2026-10-31 | #39 audit follow-ups | `SB_STATUS_OUT_OF_MEMORY` for a failed image copy in `sb_disc_open_memory`; the header lists all global state. No new sources consulted. |
| 2026-10-31 | #61 window cap | `MAX_PARALLEL_BATCH` 256 to 512 after measurements of v0.2.0 and main on synthetic images (`scarletbook-synth`, corpus and stress frames) and of the round 3 figures of sacd-spec PR #24; no change to decoding. No new sources consulted. |
| 2026-10-31 | #75 loudspeaker layouts | `LoudspeakerLayout` and `Speaker` from specification 3, 3.4.1 only; unmapped combinations are unspecified. The doc example's `WAVE_FORMAT_EXTENSIBLE` channel-mask bits are from memory of Microsoft's public WAVEFORMATEXTENSIBLE documentation (learn.microsoft.com), not fetched; a general reference. No SACD or DST sources. |
| 2026-10-31 | #68 seek index, corpus and measurements | `scarletbook-seekcost` and a corpus test through indexes. No new sources consulted. |
| 2026-10-31 | #68 seek index on `Disc` | opt-in per area, cursors and continuations through the index, documentation. Specification 4, 4.2 items 6 and 7, at sacd-spec `da6b900`. No new sources consulted. |
| 2026-10-31 | #68 seek index, C interface | `SbSeekIndex` and the `_indexed` openers over the core API; no change to decoding. No new sources consulted. |
| 2026-10-31 | #68 seek index, core (#73) | `AreaIndex`, checkpoints and indexed range cursors. Specification 4, 4.2 item 7, 4.4.5 item 4, 4.5 item 6 and 4.7.1 item 6, and 7.1 items 2 and 3, at sacd-spec `da6b900` (answering sacd-spec#25 to #27); see "Specification version". No new sources consulted. |
| 2026-10-31 | #68 seek index, persistence | saving, loading, merging and checking seek indexes; a fuzz target. Specification 4, 4.2 item 7, and 7.1 item 2, at sacd-spec `da6b900`. FNV-1a (a public-domain hash) written from memory. No new sources consulted. |
| 2026-10-31 | positional reads (#74, part 1) | `ReadAt` and `ReadAtReader`; no change to image reading. Rust standard library documentation of `std::os::unix::fs::FileExt::read_at` and `std::os::windows::fs::FileExt::seek_read`, read in the local `rust-src` of `nightly-2026-10-30` (see Sources consulted). No SACD or DST sources. |
| 2026-10-31 | discs of one image on several threads (#74, part 2) | `Disc::share` and `clone` over a shared TOC; reads unchanged. No new sources consulted. |
| 2026-10-31 | `sb_disc_clone` (#74, part 3) | clones of discs in the C interface over `Disc::share_with_log`. No new sources consulted. |
| 2026-10-31 | positional read callbacks (#74, part 4) | `sb_disc_open_callbacks_at` and `sb_disc_clone_callbacks`. No new sources consulted. |
| 2026-10-31 | C interface for loudspeaker layouts (#75) | `sb_area_loudspeaker_layout`, `sb_area_channel_speaker` and the `SB_LAYOUT_*` and `SB_SPEAKER_*` constants over the core's layouts (specification 3, 3.4.1); no new interpretation. No new sources consulted. |
| 2026-10-31 | CI turnaround (#76) | fuzz targets and Miri slices as parallel jobs; the fast-loop unit test split by frame count; superseded runs on main cancelled. No new sources consulted. |
| 2026-10-31 | change records (#76) | CHANGELOG and CLEANROOM Log entries move to per-change fragments, assembled at release; CI checks them. No new sources consulted. |
| 2026-10-31 | CI: full Miri never cancelled (#84) | the Miri slices become a reusable workflow; the long ones run in a queued workflow on main, nightly and on demand. No new sources consulted. |
| 2026-10-31 | seek index image identity (#90) | a caller-kept seek index is of one image within a process, in Rust and C; documentation. No new sources consulted. |
| 2026-10-31 | release 0.4.0 | version and changelog only: workspace version 0.4.0; the change records of 0.4.0 folded in from changelog.d/ and cleanroom.d/. No new sources consulted. |
| 2026-10-31 | C ABI 2 (#39) | Fixed-width status, flags and sizes, padding-free structs, and `sb_dst_decode` input bounded by the reassembly limit (specification 4, 4.4.3; 7, 7.6); no new interpretation. No new sources consulted. |
| 2026-10-31 | #94 provisional reading, core | `SeekPolicy`, the timecode search and provisional starts, marking. Specification 4, 4.2 item 8, 4.4.5 item 5, 4.7.1 item 6, and 7.1 item 3, at sacd-spec `82eac6c` (see "Specification version"); corpus README updated from it. No new sources consulted. |
| 2026-10-31 | #94 provisional reading, verification | verdicts on provisional readings, the event queue, provisional checkpoints. Specification 4, 4.2 item 8, at sacd-spec `82eac6c`. No new sources consulted. |
| 2026-10-31 | release 0.4.0, fold-in | the change records of ABI 2 (#39, #97) and provisional reading (#94, #98, #99) folded into the 0.4.0 changelog section and this Log. No new sources consulted. |
| 2026-10-31 | seek index: provisional readings bound to their image (#102) | fix in the core. Specification 4, 4.2 item 8, at sacd-spec `82eac6c`. No new sources consulted. |
| 2026-10-31 | #94 provisional reading, C interface | `sb_disc_seek_policy_set`, `SbSpeculation`, verdict queries over the core API. Specification 4, 4.2 item 8, at sacd-spec `82eac6c`. No new sources consulted. |
| 2026-10-31 | #94 cold-seek measurements and C acceptance tests | `scarletbook-seekcost` cold-seek table, `clib-seekcost.py` (C path), C interface tests and smoke test of provisional reading. No new sources consulted. |
| 2026-10-31 | release 0.4.1 | version and changelog only: workspace version 0.4.1; the change records of 0.4.1 folded in from changelog.d/ and cleanroom.d/. No new sources consulted. |
| 2026-10-31 | remaining TOC fields through the C interface (#104, #114) | specification 2, 2.3, 2.4, 2.5.1, 2.6.5 step 1 and 2.7, specification 3, 3.4 and 3.5, at sacd-spec `82eac6c`. No new sources consulted. |
| 2026-10-31 | logical sector reads through the C interface (#105, #113) | specification 1, 1.2 and 1.3 (sector formats, whole-or-nothing reads), already implemented by the core's image reads. No new sources consulted. |
| 2026-10-31 | #106 cancellation and progress, core | `CancelToken`, `Error::Cancelled` and `Progress`; checked and counted where cursors read sectors. API behaviour, no specification change. No new sources consulted. |
| 2026-10-31 | #106 cancellation and progress, C interface | `SbCancel`, `SbProgress`, `SB_STATUS_CANCELLED` over the core API of part 1. API behaviour, no specification change. No new sources consulted. |
| 2026-10-31 | #107 stored and decoded frames in one pass, core | `TrackAudioCursor::keep_stored`, `next_frame_with_stored`, `Stored`; stored content kept next to each decoding in the cursor window. Specification 4, 4.6 and 4.6.1 items 2 and 3 at the current sacd-spec. No new sources consulted. |
| 2026-10-31 | #107 stored and decoded frames in one pass, C interface | `sb_track_audio_keep_stored`, `sb_track_audio_read_with_stored` over the core API of part 1. Specification 4, 4.6.1 items 2 and 3. No new sources consulted. |
| 2026-10-31 | build information through the C interface (#108, #116) | build script of the FFI crate, from Cargo's documented build script environment; no specification involved. No new sources consulted. |
| 2026-10-31 | release 0.5.0 | version and changelog only: workspace version 0.5.0 (a breaking change to the core API, #104); the change records of 0.5.0 folded in from changelog.d/ and cleanroom.d/. No new sources consulted. |
| 2026-10-31 | #123 cheaper provisional seeks (#126) | Provisional start at the sector found, track starts as search knots, reads sized to the range. Specification 4, 4.2 item 8 (with its informative notes), 4.3.3, 4.4.3 item 3.3, 4.4.5 item 5 and 7, 7.1 items 2 and 3 at sacd-spec `82eac6c`. No new sources consulted. |
| 2026-10-31 | #127 TOC timecodes as stored | `Disc::track_timecodes_stored`, `SbTimecode`, `sb_area_playtime_stored`, `sb_track_timecodes_stored`. Specification 3, 3.7.2 and 3.9.2 at sacd-spec `82eac6c`. No new sources consulted. |
| 2026-10-31 | release 0.5.1 | version and changelog only: workspace version 0.5.1 (additions only; `cargo semver-checks` against v0.5.0 as a patch: 229 checks pass); the change records of 0.5.1 folded in from changelog.d/ and cleanroom.d/. No new sources consulted. |
| 2026-11-01 | #123 cold seek last read (#164) | Size and placement of the search's fifth read for provisional starts (no change to what is read from `L`, labels, verification or suspect marks). Specification 4, 4.2 item 8, and 7, 7.1 item 3, at sacd-spec `eafaaba`. No new sources consulted. |
| 2026-11-01 | #123 cold seek tail (#159) | Search and read sizing of provisional starts (no change to what is read from `L`). Specification 4, 4.2 item 8, and 7, 7.1 item 3, at sacd-spec `eafaaba`. No new sources consulted; regula falsi and its Illinois modification are standard numerical methods. |
| 2026-10-31 | #124 saved confirmations (#144) | Confirmed provisional starts saved with the seek index and reported confirmed after a remount. Specification 4, 4.2 items 8 and 9, and 7, 7.1 item 2, at sacd-spec `cb9530b`. No new sources consulted. |
| 2026-10-31 | #124 seek hints (#133) | Locator samples saved with the seek index, TOC fingerprint, sized exact resumes. Specification 4, 4.2 items 7, 8 (locator samples) and 9, and 7, 7.1 items 2 and 3, at sacd-spec `cb9530b` (see "Specification version"). No new sources consulted. |
| 2026-11-01 | #124 seek hints and saved confirmations in the `seek_index` fuzz target | fuzz harness and seeds only: speculative reads, verification and a disc that binds the TOC fingerprint, specification 4, 4.2 items 8 and 9, at sacd-spec `eafaaba`. No new sources consulted. |
| 2026-11-01 | #125 CET shadow stack marking of the x86-64 Linux C libraries (#157) | Build flags and a `.note.gnu.property` added with objcopy; no SACD material. General toolchain documentation consulted 2026-11-01: Linux kernel "Control-flow Enforcement Technology (CET) Shadow Stack" (https://www.kernel.org/doc/html/latest/arch/x86/shstk.html), glibc manual "Hardware Capability Tunables" (https://sourceware.org/glibc/manual/latest/html_node/Hardware-Capability-Tunables.html), GNU ld manual "Options" (https://sourceware.org/binutils/docs/ld/Options.html), The Rust Unstable Book "cf-protection" (https://doc.rust-lang.org/nightly/unstable-book/compiler-flags/cf-protection.html), rust-lang/rust#93754 (https://github.com/rust-lang/rust/issues/93754). |
| 2026-10-31 | #130 DSD-to-PCM conversion, core (`scarletbook-pcm`, #148) | New crate: multistage decimation to 88.2, 176.4 and 352.8 kHz, filter generator and tables, quality and determinism tests, throughput bench. The only specification input is sacd-spec specification 0, 0.6 (DSD byte layout; identical at `82eac6c` and `6db1251`); PCM conversion is outside the specifications, so not a specification gap. Sources (bibliographic records checked on Crossref 2026-10-31, full texts not opened, no code read): Parks & McClellan 1972; Rabiner, McClellan & Parks 1975; Herrmann, Rabiner & Chan 1973; Kaiser 1974 (citation); Crochiere & Rabiner 1981 and 1983 (book, citation); Mintzer 1982; Vaidyanathan & Nguyen 1987; Peled & Liu 1974; White 1989; Hogenauer 1981 (not used); Goldberg 1991; Chao et al. 1990; Schreier & Temes 2005 (citation); Oppenheim & Schafer 1999 (citation). See "General references consulted" and "Patent hygiene". |
| 2026-11-01 | #130 DSD-to-PCM conversion, C ABI (`sb_pcm_*`, #160) | C functions over the `scarletbook-pcm` converter of #148, tests against the Rust converter and a corpus-frame digest in the C library smoke test; no specification involved beyond the DSD frame layout already cited in #148. No new sources consulted. |
| 2026-10-31 | #131 DSDIFF plain DSD layout (#141) | `output::Format::DsdiffDsd`, `PropertyChunk`, the DSDIFF options. Specification 8, 8.1, 8.4.1 to 8.4.5, 8.4.7 item 1, 8.4.8 (absolute start time) and 8.6 rows 14 to 17 at sacd-spec `1517ab7` (unchanged at `6db1251`). No new sources consulted. |
| 2026-10-31 | #131 DSDIFF DST layout (#143) | `output::DstLengths`, `Layout::new_dst`, `DSTF`/`DSTC`/`DSTI`, the frame CRC with the test vectors of 8.4.6.1. Specification 8, 8.4.2, 8.4.6, 8.4.6.1, 8.4.6.2 and 8.6 rows 13 and 18 to 21 at sacd-spec `1517ab7` (unchanged at `6db1251`), with specification 4, 4.4, 4.6.1 items 2 to 5. No new sources consulted. |
| 2026-10-31 | #131 DSDIFF text (#154) | `DIAR`, `DITI`, `COMT`, marker text, `output::Comment`, `InfoChunk`, `MarkerText`. Specification 8, 8.4.1 item 4, 8.4.7 item 5, 8.4.8, 8.4.9 and 8.6 rows 14, 22 to 26 and 31, at sacd-spec `6db1251`. No new sources consulted. |
| 2026-10-31 | #131 DSF layout (#139) | `output::Layout`, `OutputOptions`, `Error::OutputRefused`: DSF header, sound data and sizes. Specification 8, 8.1, 8.2.1 to 8.2.5 and 8.6 rows 1 and 2 at sacd-spec `1517ab7` (unchanged at `6db1251`), with specification 0, 0.6 and 4, 4.6, 4.7.1 and 4.8. No new sources consulted. |
| 2026-10-31 | #131 DSDIFF markers and edited master (#145) | `output::Marker`, `Markers`, `SourceTrack`, `EditedMasterCheck`, `DIIN`/`EMID`/`MARK`. Specification 8, 8.1 item 7, 8.4.2, 8.4.7, 8.4.8, 8.4.10 (items 1 to 4) and 8.4.11, and 8.6 rows 22 to 24 and 27 to 30, at sacd-spec `6db1251`. No new sources consulted. |
| 2026-11-01 | #131 DSF and DSDIFF output, C interface (#161) | `sb_output_*`, `sb_tags_*`, `sb_dst_lengths_*` over `scarletbook::output`; README sections and ctypes bindings; a DSF and a DSDIFF smoke check in `clib-smoke.py`. Specification 8 at sacd-spec `6db1251`. No new sources consulted. |
| 2026-11-01 | #131 output layouts in the `toc` fuzz target | fuzz harness only (layouts, DST lengths and raw ID3 tags of hostile TOCs); no specification involved. No new sources consulted. |
| 2026-10-31 | #131 DSF tags (#149) | `output::TagSet`, `TagItem`, `Tags`, `Id3Options` and the ID3v2.4 writer, with a test reader written from 8.3. Specification 8, 8.1 items 6 and 7, 8.2.1, 8.2.6, 8.3.1 to 8.3.8 and 8.6 rows 3 to 12, at sacd-spec `6db1251`, with specification 2, 2.3.1, 2.3.2, 2.4, 2.5.4 and specification 3, 3.7.3, 3.8. No new sources consulted. |
| 2026-10-31 | #132 no allocation per C call (#140) | the last error message is emptied only when it is not; no specification involved. No new sources consulted. |
| 2026-10-31 | #132 one-pass text decoding (#142) | the same decoding of specification 2, 2.6.2 to 2.6.5 with fewer copies; specification 2 at sacd-spec `6db1251`. No new sources consulted. |
| 2026-10-31 | #132 TOC read benchmark (#135) | `toc_read` criterion benchmark of the C interface and the Rust API over corpus and synthetic images; no specification involved. No new sources consulted. |
| 2026-10-31 | #132 the whole TOC in one call (#147) | `sb_disc_toc` and its layout structs: the values of the existing TOC queries in a binary layout of our own design; no specification involved beyond the fields already read. No new sources consulted. |
| 2026-10-31 | #132 faster TOC digest (#138) | in-process digest of the TOC sectors for `Disc::share`, a four-lane multiply-rotate of our own design; no specification involved. No new sources consulted. |
| 2026-10-31 | #132 `sb_disc_toc` in the `toc` fuzz target (#150) | fuzz harness only; no specification involved. No new sources consulted. |
| 2026-10-31 | #146 decode-on-a-pool benchmark (#152) | `decode_pool` benchmark program of the C interface and the Rust API; no specification involved. No new sources consulted. |
| 2026-11-01 | #146 pool size beside a busy caller (#158) | Documentation of `DecodeOn::Pool` and `sb_pool_new` from measurements of the `decode_pool` benchmark; no specification involved. No new sources consulted. |
| 2026-11-01 | #39 pre-0.5.2 API extensibility (#166) | `#[non_exhaustive]` and builders on unreleased output and PCM types; no behaviour change. No new sources consulted. |
| 2026-11-01 | patent-hygiene inventory for #40 (issue comment, no code) | Inventory of the code's techniques and their earliest dated non-code sources, posted on #40 for the owner's legal review; no patent or patent search was opened, and no SACD/DST source beyond sacd-spec (`eafaaba`, including PROVENANCE.md) was read. Sources consulted on 2026-11-01: Crossref metadata (`api.crossref.org/works/<DOI>`: title, authors, venue and date only, no abstracts) for the candidate prior art listed in the comment (among them Dowell & Jarratt 1971, Brent 1971, Stockham 1966, Sarwate 1988, Halstead 1985, Hoare 1974, Blumofe & Leiserson 1999); the Wayback Machine's CDX index and captures of the FNV hash page (isthe.com/chongo/tech/comp/fnv, captures 2001–2005, read) and of T. Wang's "Integer Hash Function" page (1999-09-03 capture, read). No search used SACD, DSD or DST terms. Nothing was implemented from these sources. |
| 2026-10-31 | seek index of an area without audio sectors completes | fix in the core (#68, #136). Specification 4, 4.2 items 1 and 2, at sacd-spec `cb9530b`. No new sources consulted. |
| 2026-10-31 | provisional checkpoints of one reading only | fix in the core (#94, #134). Specification 4, 4.2 item 8, at sacd-spec `cb9530b`. No new sources consulted. |
| 2026-11-01 | exposure outside the clean room: aclater/sacd, read at the owner's direction (#155) | At about 07:05Z, at the owner's direction, the development session read, through the GitHub API, the top-level listing of `aclater/sacd` (two files), its `DECISIONS.md` in full, the first 40 lines of its `README.md`, and the subject lines of its five latest commits. The owner then ruled these outside the clean room. No linked page was opened: nothing in sacdfs, pysacd, beets-pysacd or elsewhere. The files hold project decisions, priorities, licensing and a project map. They contain no code and no SACD/DST format detail. Their DSD-to-PCM parameters (40 kHz default passband, 30 kHz and ~80 kHz options, ~105–110 dB stopband) were already given as requirements in #130 and are implemented as recorded there. Nothing else in them bears on the code. The work-order ranking they contained was passed to two agents (#125, and the paused #130 C interface); the #125 agent was also given the file's name, and confirmed it did not open the repository. Neither file is to be opened again. |
| 2026-10-31 | sacd-spec#37 suspect provisional readings, C interface | `sb_track_audio_suspect`, `sb_raw_frames_suspect`, a policy that reads suspect readings exactly; over the core API. Specification 4, 4.2 item 8, at sacd-spec `eafaaba`. No new sources consulted. |
| 2026-10-31 | sacd-spec#37 suspect provisional readings (#151) | The suspect mark and the option to read suspect areas exactly. Specification 4, 4.2 item 8, at sacd-spec `eafaaba` (see "Specification version"). No new sources consulted. |
| 2026-11-01 | release 0.5.2 | version and changelog only: workspace version 0.5.2 (a patch: `cargo semver-checks` against v0.5.1 finds no update required); the change records of 0.5.2 folded in from changelog.d/ and cleanroom.d/. No new sources consulted. |
| 2026-11-01 | #130 DSD-to-PCM conversion in the `demux` fuzz target (#168) | fuzz harness only; no specification involved. No new sources consulted. |
| 2026-11-01 | #170 stopband edges and #171 exact silence in `scarletbook-pcm` (#176) | New filter tables from `design.rs`: `Low` to 36 kHz, a separate `Wide` filter for 176.4 kHz to 88.2 kHz, and a first stage with the integer factor `(1 + z^-1)^2 (1 + z^-4)`. The idle patterns are from sacd-spec specification 0, 0.6 at `eafaaba` (0x69 and 0x55, both silence). Issue #171 described how the consumer's own converter reaches exact silence ("making stage 1's eight polyphase sums equal", that is, nulls at every multiple of 352.8 kHz). That approach was not adopted. The implementation was derived independently from the patterns: as +-1 samples, 0x69 satisfies `x[n-4] = -x[n]` (mean 0, spectral lines only at the odd multiples of 352.8 kHz) and 0x55 satisfies `x[n-1] = -x[n]`, so the factor cancels both, exactly, in integer taps. Only those three line frequencies (and 1.4112 MHz) are nulled, not all seven. The cofactor is designed by the Remez exchange with the fixed factor moved into the desired response and the weight, as for types II to IV in [P1] and [P2]; the polyphase and aliasing view is from [P5], and DFTs of periodic sequences and FIR zeros from [P12]. All are already listed under "DSD-to-PCM conversion (#130)". The edge semantics (outside the area is the idle pattern) are #171's requirement. No new sources consulted; no code read. |
| 2026-11-01 | DSD-to-PCM: runs of separate frames, packed 24-bit output, per-frame clip counts (#172, #175) | API and input/output plumbing of `scarletbook-pcm` only; filters and arithmetic unchanged. Requirements from the issue text; the consumer's code was not read. No new sources consulted. |
| 2026-11-01 | #173 run-time AVX2/FMA dispatch in `scarletbook-pcm` (#180) | The same safe kernels built with `#[target_feature(enable = "avx2,fma")]` and chosen by `is_x86_feature_detected!`; output identical to the bit. The rules for calling `#[target_feature]` functions are from rust-lang/rfcs#2396 "target_feature 1.1" (https://github.com/rust-lang/rfcs/pull/2396, merged 2020-02-12), its stabilisation in rust-lang/rust#134090 (https://github.com/rust-lang/rust/pull/134090) and the Rust 1.86.0 release notes (https://blog.rust-lang.org/2025/04/03/Rust-1.86.0/), all consulted 2026-11-01. The status of RFC 3525, struct target features (https://github.com/rust-lang/rfcs/pull/3525, and its tracking issue rust-lang/rust#129107), was checked on the same day: it is not stable. That FMA leaves the exact integer sums unchanged follows from IEEE-754 rounding (Goldberg 1991, already listed). sacdfs is known only by the black-box figures in issue #173. |
| 2026-11-01 | #173 `scarletbook-pcm` kernel speed (#179) | The existing kernels restructured: the same integer stages and tables, output identical to the bit. sacdfs is known only by the black-box figures in issue #173; its code was not read. No new sources consulted. |
| 2026-11-01 | evidence record for #40 (`docs/provenance/EVIDENCE.md`) | Documentation only, no code change. Sources consulted on 2026-11-01 (none is SACD or DST material; no search used SACD, DSD or DST terms): Crossref and OpenAlex metadata for every DOI in EVIDENCE.md's source register, DataCite and arXiv records for arXiv 2302.00819 and 1402.3392, and OpenLibrary records for Crochiere & Rabiner 1983 and Oppenheim & Schafer 1999; Peled & Liu 1973 was found as White 1989's ref. [1] and its Crossref record added. Full texts obtained and read for the passages cited: Brent 1971 (author copy, prose only, the ALGOL listing not read), Dowell & Jarratt 1971 (UBC course copy), Crochiere & Rabiner 1981 and Rabiner, McClellan & Parks 1975 (Rabiner's UCSB reprints), Herrmann, Rabiner & Chan 1973 (BSTJ issue scan, bitsavers), Vaidyanathan & Nguyen 1987 (CaltechAUTHORS), Dongarra & Hinds 1979 (netlib author copy); ACM open-backfile PDFs via Wayback captures of dl.acm.org: Witten, Neal & Cleary 1987, Lam 1988, Sarwate 1988, Stockham 1966, Perl, Itai & Avni 1978, Hoare 1974, Halstead 1985, Collins 1960, Moffat, Neal & Witten 1998, Rau & Glaeser 1981, Goldberg 1991, Blumofe & Leiserson 1999, Wall 1986; Goldberg's Sun/Oracle reprint; Lam 1988 (Stanford copy and the Paderborn copy read for #40); Blumofe & Leiserson (MIT copy via Wayback); Wall WRL 86/3; Said HPL-2004-76 and Giesen 2014 (arXiv); Schmookler & Nowka 2001 (UMBC); Alpha Architecture Reference Manual 2002 (csclub); Mahlke et al. 1992 (author copy); Moffat, Neal & Witten 1998 (course copy); Marpe et al. 2006 (EURASIP); Sze & Budagavi 2013 (MIT); Wayback captures of the FNV page (2000-09-02, 2001-08-23, 2002-08-11) and of T. Wang's "Integer Hash Function" (1999-09-03). The texts in `~/papers` (#40) were hashed and their passages checked again. Index pages opened to find copies: Rabiner's UCSB reprint index, Brent's publication page pub005, the CaltechAUTHORS record czbzh-df707. Wayback CDX lookups and Save Page Now requests for every URL above and for the web sources of "General references consulted" and "Sources consulted"; CDX-index lookups only (the documents not opened) for the three specification 8 documents cited in sacd-spec `PROVENANCE.md`. A general web search engine was used to locate free copies of these papers by title; patent links in its results (USPTO, Google Patents, EPO) were skipped unopened, as were gwern.net, Scribd, Academia.edu, Studocu and forum links. Process records exported (gh api, git log, git bundle). No patent, no SACD or DST implementation and nothing from aclater/sacd was opened. Copies are kept outside the repository (`/home/dev/evidence/scarletbook-rs/`, `MANIFEST.tsv`). |
| 2026-11-01 | release 0.5.3 | version and changelog only: workspace version 0.5.3 (a patch: `cargo semver-checks` against v0.5.2 finds no update required for `scarletbook` or `scarletbook-pcm`); the change records of 0.5.3 folded in from changelog.d/ and cleanroom.d/. No new sources consulted. |
| 2026-11-01 | #169 DSDIFF caller chunks (#183) | `output::CallerChunk`, `OutputOptions::dsdiff_caller_chunks`, `sb_output_options_add_chunk`, `sb_output_options_clear_chunks`. Specification 8, 8.4.1 items 1, 3 and 5, 8.4.2 items 4 and 5, 8.4.11 and 8.6 row 33, at sacd-spec `e069569`. Implemented from the specification text only; no other program's code or output was consulted. No new sources consulted. |
| 2026-11-01 | `DIIN` without `EMID`, and the source track's performer and title in `DIAR` and `DITI` (#169, #184) | specification 8, 8.4.8, 8.4.11 item 1 and 8.6 row 24 at sacd-spec `e069569`, with specification 3, 3.5 (item types 0x01 and 0x02) and 8.1 item 7. Implemented from the specification text only; the issue's description of other programs was not used, and no other program's code or output was consulted. The empty `DIIN` (8.4.8 and 8.6 row 34 at sacd-spec `048aa1a`) answers sacd-spec#41, filed from this work. No new sources consulted. |
| 2026-11-01 | #169 DSDIFF departures D1 and D2 (#187) | specification 8, 8.7 (D1, D2), with 8.4.3, 8.4.7, 8.4.8 and 8.4.9, at sacd-spec `e069569`. Implemented from the specification text only; no other program's code or output was consulted. No new sources consulted. |
| 2026-11-01 | #169 DSF byte ranges of a chain (#186) | specification 8, 8.1 items 2 and 7, 8.2.1 to 8.2.5, 8.2.7 and 8.6 row 32 at sacd-spec `e069569`, with 8.2.7 items 2, 3 and 7 as clarified at sacd-spec `fa558a8`; the checks of specification 4, 4.8 and 4.7.1 items 4 and 5 as before. Implemented from the specification text only; no other program's code or output was consulted. No new sources consulted. |
| 2026-11-01 | caller-supplied ID3v2.3 DSF tag, part of #169 (#185) | specification 8, 8.2.6 items 1 and 4 and 8.6 row 3, at sacd-spec `e069569`. [ID3v2.3] read only through what 8.2.6 states. No new sources consulted. |
| 2026-11-01 | C option for the empty `DIIN` (#169, #192) | specification 8, 8.4.8 and 8.6 row 34 at sacd-spec `048aa1a`; the C side of the Rust option of #184. No other program's code or output was consulted. No new sources consulted. |
| 2026-11-01 | unused sealed accessor removed (#191) | Dead code only: `sealed::Sealed::area_cursor`, which had no caller. No specification text involved. No new sources consulted. |
| 2026-11-01 | track-text section reference 3.8 (#193) | sacd-spec `fa558a8` (see "Update to `fa558a8`"): doc comments only, citing specification 3, 3.8 for item types 0x01 and 0x02. Implemented from the specification text only; no other program's code or output was consulted. No new sources consulted. |
| 2026-11-01 | release 0.5.4 | version and changelog only: workspace version 0.5.4 (a patch: `cargo semver-checks` against v0.5.3 finds no update required for `scarletbook` or `scarletbook-pcm`; `scarletbook-ffi`, which is not published, keeps `SB_ABI_VERSION` 2 with additions only, by `abi-check.py`); the change records of 0.5.4 folded in from changelog.d/ and cleanroom.d/. No new sources consulted. |
| 2026-11-01 | #146 run-time AVX2/BMI dispatch of the DST decoder (#200) | The same safe decoder built with `#[target_feature(enable = "avx2,bmi1,bmi2,lzcnt,movbe")]` and chosen by `is_x86_feature_detected!`, as in #180; the decoder is integer-only, so the output is identical to the bit. The feature set was chosen by profiling this crate's own decoder (perf, and builds with single features). The rules for calling `#[target_feature]` functions are those already cited for #180 (rust-lang/rfcs#2396, rust-lang/rust#134090, the Rust 1.86.0 release notes; [S20], [S21]). Nothing SACD-related beyond specification 5 at sacd-spec. No new sources consulted. |
| 2026-11-01 | x86-64-v3 C library archives (#146) | CI/packaging only; no specification or source consulted. No new sources consulted. |
| 2026-11-01 | release 0.5.5 | version and changelog only: workspace version 0.5.5 (a patch: `cargo semver-checks` against v0.5.4 finds no update required for `scarletbook` or `scarletbook-pcm`; the C ABI is unchanged, `abi-check.py` passes with `SB_ABI_VERSION` 2); the change records of 0.5.5 folded in from changelog.d/ and cleanroom.d/. No new sources consulted. |
| 2026-11-01 | `bench dsf` output-path benchmark (#202, #212) | benchmark program only: writes a DSF file with this crate's own `output::Layout` (specification 8, 8.2, at sacd-spec as already cited); timings come from this repository's own builds and profiles (perf). No new sources consulted. |
| 2026-11-01 | faster DSF channel separation (#202, #214) | specification 8, 8.2.2 and 8.2.3 at sacd-spec as already cited; the loops were chosen by profiling this crate's own builds (perf). Implemented from the specification text only. No new sources consulted. |
| 2026-11-01 | plain DSD frames reserved at their size (#202, #215) | specification 4, 4.4.2 (a plain frame is `C x 4704` bytes) at sacd-spec as already cited; found by profiling this crate's own builds (perf). No new sources consulted. |
| 2026-11-02 | Miri-sized DSF row test (#202, #205, #222) | tests and CI timeout only; no specification or source consulted. No new sources consulted. |
| 2026-11-02 | run-time dispatch of the output path (#202, #225) | the existing `#[target_feature]` dispatch of #146 extended to `Layout::fill` and `Layout::read_at`, with the DST decoder through the same wrapper; the rules for calling `#[target_feature]` functions are those already cited for #180 and #200. Found and checked by profiling and disassembling this crate's own builds (perf, objdump). No new sources consulted. |
| 2026-11-01 | DST inner loop: per-channel probability block, 64-entry tables (#203, #218) | specification 5, 5.5.4 and 5.5.7 at sacd-spec as already cited; the layout was chosen by profiling this crate's own builds (perf, objdump). No new sources consulted. |
| 2026-11-02 | AVX2 builds of the scarletbook-pcm input and output loops (#216, #223) | no SACD or DSP source: `#[target_feature]` builds of existing safe loops in the existing dispatch module, chosen by profiling this crate's own builds (perf). No new sources consulted. |
| 2026-11-02 | release 0.5.6 | version and changelog only: workspace version 0.5.6 (a patch: `cargo semver-checks` against v0.5.5 finds no update required for `scarletbook` or `scarletbook-pcm`; the C ABI is unchanged, `abi-check.py` passes with `SB_ABI_VERSION` 2); the change records of 0.5.6 folded in from changelog.d/ and cleanroom.d/. No new sources consulted. |
| 2026-11-01 | faster S1 table sums in scarletbook-pcm, call-pattern benchmark, corpus PCM digest (#216, #217) | no SACD or DSP source: a loop restructuring of the existing integer sums, chosen by profiling this crate's own builds (perf); the digest test uses the corpus at sacd-spec as already cited. No new sources consulted. |
| 2026-11-01 | scarletbook-pcm scratch buffers not re-zeroed (#216, #219) | no SACD or DSP source: buffer handling found by profiling this crate's own builds (perf). No new sources consulted. |
| 2026-11-02 | `bench dsf --via parallel` prototype (#202, #224, #232) | benchmark program only, on this crate's public `output::Layout` API; `std::os::unix::fs::FileExt::write_all_at` from the Rust standard library documentation. No new sources consulted. |
| 2026-11-02 | `bench` reads images through `BufReader::new` (#202, #239) | found by profiling this crate's own builds (perf); benchmark program only, `std::io::BufReader` from the Rust standard library documentation. No new sources consulted. |
| 2026-11-02 | output-path dispatch test lighter under Miri (#202, #236) | test-only change to this crate's own unit test; no specification involved. No new sources consulted. |
| 2026-11-02 | audio sectors demultiplexed in place (#202, #230) | found by profiling this crate's own builds (perf); no specification change. No new sources consulted. |
| 2026-11-02 | 5-channel DSF rows in 32-bit words (#202, #237) | found by profiling and disassembling this crate's own builds (perf, objdump); integer masks and shifts on this crate's own code; no specification change. No new sources consulted. |
| 2026-11-02 | DST probability-table layout per channel count (#203, #241) | specification 5.5.4 and 5.5.7 at sacd-spec as already cited; the choice was made by timing this crate's own builds on lennon (Ryzen 9 3950X). No new sources consulted. |
| 2026-11-02 | CI benchmark regression check (#226, #233) | CI workflow and script timing this repository's own programs; no specification involved. No new sources consulted. |
| 2026-11-02 | benchmark kit and run list (#226, #231) | release packaging, a run list and a runner of this repository's own programs; no specification involved. No new sources consulted. |
| 2026-11-02 | #228 read-buffer test lighter under Miri (#228, #235) | test-only change to this crate's own unit test; no specification involved. No new sources consulted. |
| 2026-11-02 | raw-image read buffer released after the TOC (#228, #234) | memory use of this crate's own reader, measured with `/proc/self/statm` (proc(5)); no specification change. No new sources consulted. |
| 2026-11-02 | release 0.5.7 | version and changelog only: workspace version 0.5.7 (a patch: `cargo semver-checks` against v0.5.6 finds no update required for `scarletbook` or `scarletbook-pcm`; the C ABI is unchanged, `abi-check.py` passes with `SB_ABI_VERSION` 2); the change records of 0.5.7 folded in from changelog.d/ and cleanroom.d/; the measured table of 0.5.7 added to the notes and to `benchmarks/RESULTS.md`. No new sources consulted. |
| 2026-11-02 | crates.io publishing (#249, #251) | packaging and release workflow only: `scarletbook-ffi` made publishable (metadata, `include`, licences, versioned path dependencies), a `publish` job in release.yml, RELEASING.md and README install lines; the packages ship this repository's own sources plus the sacd-spec CC0-1.0 charset tables already vendored. General (non-SACD) sources consulted 2026-11-02: the crates.io "Trusted Publishing" documentation (https://crates.io/docs/trusted-publishing, read from its source in github.com/rust-lang/crates.io, `svelte/src/routes/docs/trusted-publishing/+page.svelte`) and the README and release tags of https://github.com/rust-lang/crates-io-auth-action (v1.0.6). |
| 2026-11-05 | seek index load check and fuzz harness (#253) | a saved index with more confirmations than its saved maximum of hints is refused; the `seek_index` harness accepts the documented typed error for checkpoints of no reading of the image. The format is this project's own (`seek/persist.rs`); sacd-spec 4.2 item 9 and 7.1 item 2 re-read. No new sources consulted. |
| 2026-11-05 | `bench realtime` (#255) | a measuring mode of the benchmark program, from the public `Disc` API; the library is unchanged. xorshift64 for repeatable seek positions, a general-purpose generator with no bearing on the codec. No new sources consulted. |
