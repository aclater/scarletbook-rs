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

## Specification version

| Item | Value |
|---|---|
| Repository | https://github.com/aclater/sacd-spec |
| Branch | `main` (the specifications and the corpus were both merged there) |
| Commit | `37a16a37511e721eac8f119073d56702a1c591da` ("Merge pull request #2 from aclater/corpus", 2026-10-30) |
| Corpus | `corpus/` of that commit, copied unchanged into this repository's `corpus/` (CC0-1.0); manifest `format_version` 1, `spec_version` `1.0-draft`, 129 cases |
| Charset tables | `spec/charsets/*.txt` of that commit, copied unchanged into `crates/scarletbook/data/charsets/` (CC0-1.0) |

The clone was first on the `corpus` branch (commit `8544f6b`); it was switched to `main`
before any specification was read, and all work uses `main` at the commit above.

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
/home/dev/src/sacd-spec
```

The only match is the sacd-spec clone. No SACD or DST implementation code was found on the
machine.

### Tools installed during the work

| Tool | Source | Why |
|---|---|---|
| Rust 1.85.0 (MSRV) and nightly-2026-10-30 (Miri, cargo-fuzz) | rustup | CI parity |
| `gcc-c++` 16.2.1 | Fedora 44 repositories (`dnf`) | libFuzzer needs a C++ compiler |
| crates: thiserror, proptest, serde_json, sha2, libfuzzer-sys and their dependencies | crates.io | dependencies; none is SACD- or DST-specific |

## Sources consulted

| Source | What for |
|---|---|
| sacd-spec `spec/00` to `spec/05`, `spec/07`, `spec/charsets/` | the implementation |
| sacd-spec `spec/06-ps3-protocol.md` | not read (the network protocol is out of scope) |
| sacd-spec `corpus/README.md`, `corpus/manifest.json`, `corpus/images/` | conformance tests |
| sacd-spec `README.md`, `PROVENANCE.md`, `LICENSE` | process and licensing |
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

## Statement

None of the forbidden sources listed under "Rules followed" was read, searched for, fetched
or installed while writing this repository. Every behaviour implemented here comes from the
sacd-spec specifications at the commit above, checked against its corpus.

## Specification issues filed

None yet.

## Log

| Date | Stage | Notes |
|---|---|---|
| 2026-10-30 | environment | checks above |
| 2026-10-30 | 1: TOC and sector detection | specifications 0 to 3 and 7; all 129 corpus cases pass the TOC, warning and error comparison |
| 2026-10-30 | 2: audio sector multiplexing | specification 4; every corpus track without DST frames (245 of 437) matches its per-channel SHA-256 and frame counts; the 192 tracks with DST frames await stage 3. No new sources consulted. |
| 2026-10-30 | 3: DST decoding | specification 5 (including informative annex 5.A, used only to write a test encoder); all 129 corpus cases pass in full: TOC, warnings, rejections and every track's per-channel SHA-256 and frame counts (437 tracks). The prediction sum is evaluated with per-filter lookup tables over history bytes, a regrouping of the sum in 5.5.3 derived here, not taken from any decoder. No new sources consulted. |
