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
| apache.org `LICENSE-2.0.txt` | licence text |
| GitHub API (`gh api repos/<action>/releases`, `.../commits/<tag>`) | resolving action SHAs to pin |
| C library release (#34, #42), all 2026-10-30: the rustc book *Platform Support* (https://doc.rust-lang.org/nightly/rustc/platform-support.html: minimum glibc, musl, macOS and Windows versions per target); GitHub changelog *macOS 13 runner image is closing down* (https://github.blog/changelog/2025-09-19-github-actions-macos-13-runner-image-is-closing-down/: `macos-15-intel`) and a web search on that; actions/runner-images *Windows2025-Readme.md* (https://github.com/actions/runner-images/blob/main/images/windows/Windows2025-Readme.md: GCC and Python on `windows-latest`); the quay.io and Docker Hub registry APIs (image digests); https://static.rust-lang.org/rustup/release-stable.toml and `rustup/archive/1.29.2/<host>/rustup-init.sha256` | build targets, runners and pins of `clib.yml` |
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
| [aclater/sacd-spec#16](https://github.com/aclater/sacd-spec/issues/16) | frame ranges other than tracks (#35): no normative end of the area timeline, the 4.8 checks and per-track counts defined for tracks only, "tracks with pauses" informative | answered in `fa602f9` (sacd-spec PR #20; 4.7.1 frame ranges, play range, tracks with pauses, 4.8 applied to ranges), refined in `e52a42b` | implemented as specified (#44) |
| [aclater/sacd-spec#17](https://github.com/aclater/sacd-spec/issues/17) | raw frames (#37): no normative undecoded view of a position; plain and missing positions in DST passthrough undefined; "invalid" undefined without decoding | answered in `fa602f9` (sacd-spec PR #20; 4.6.1 stored content and DST equivalents), refined in `e52a42b` (stored frame list and per-range stored content) | implemented as specified, both forms and DST equivalents (#49) |
| [aclater/sacd-spec#18](https://github.com/aclater/sacd-spec/issues/18) | 7.1 item 3 described one pass over an area's sectors; range and out-of-order reads rescan | answered in `34f78a3` (4.2 item 6, 7.1 item 3): one pass per track or range is allowed; restated in `e52a42b` | conformant as written; the code cites 4.2 item 6 (#44) |

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
