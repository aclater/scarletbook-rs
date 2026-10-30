# Contributing

Thank you for helping. Bug reports, corpus failures, fuzzing finds and pull requests are
welcome. Security problems go through [SECURITY.md](SECURITY.md), not public issues.

## The clean-room rule

scarletbook-rs is a clean-room implementation: every behaviour comes from the
specifications and conformance corpus of
[aclater/sacd-spec](https://github.com/aclater/sacd-spec), and nothing else about SACD or
DST. [CLEANROOM.md](CLEANROOM.md) is the record of how it was written. A contribution that
breaks the rule cannot be accepted, however good it is, so please read this section before
you start.

By opening a pull request you confirm that, for the work in it, you followed these rules.
They summarise CLEANROOM.md ("Rules followed", "General references consulted" with the
owner's decision B, and "Patent hygiene"); if the two ever disagree, CLEANROOM.md governs.

- **SACD/DST sources.** The only SACD/DST material you consulted is `aclater/sacd-spec`:
  its specifications, the charset tables, the conformance corpus and its README, and its
  `PROVENANCE.md`.
- **No implementations.** You did not read, search for, fetch or install the source of any
  SACD or DST implementation, including pysacd, sacd-ripper, `sacd_extract`, FFmpeg's DST
  decoder, sacd-rs, the dst-decoder crate and the Philips/ISO reference decoder. If you
  have studied such code in the past, say so in the pull request; the owner decides whether
  the contribution can be taken.
- **No code from repositories that contain one (decision B).** You opened no source file,
  of any kind, from a repository that also contains an SACD or DST implementation. FFmpeg
  is one: nothing from it may be read, not even code unrelated to SACD, nor its commit
  pages or mailing-list threads.
- **No SACD, DSD or DST search terms.** You made no search, for any purpose, with terms
  that name SACD, DSD or DST. If a search on another subject returns such a result, report
  it in the pull request without opening it.
- **No patents.** You read no patent, live or expired, and opened no patent page.
- **No copied code.** You copied no code from any other project into this repository.
  Ordinary dependencies from crates.io are fine; cargo-deny checks their licences.

General references that are neither SACD nor DST material may be used: Rust
documentation, compiler and CPU manuals, profilers, algorithm books and papers, GitHub
documentation, and general techniques of unrelated projects. Papers, articles, books and
documentation are preferred to code. **No technique may be adopted on the strength of code
alone**: every technique you adopt must also be supported by a paper, article, book or
documentation. List every source you consulted in the pull request, with its URL or
citation and the date you read it, so that it can be recorded in CLEANROOM.md. For each
technique adopted for performance, give the earliest dated non-code source you found and
read that describes it; sources published before 2006 are preferred, and a technique whose
earliest source is from 2006 or later is flagged in "Patent hygiene" for review.

If the specification is ambiguous or incomplete, do not guess and do not look at how
another reader does it. Open an issue on `aclater/sacd-spec`, mark the affected code with
a `TODO` that names the issue, and carry on with the rest.

## Code

- The core crate is `#![forbid(unsafe_code)]`; `unsafe` lives only in `scarletbook-ffi`,
  where every unsafe operation has its own `SAFETY:` comment.
- Images are hostile input: bound every allocation (specification 7), check every position,
  return typed errors, and never panic across the C ABI.
- Every new source file starts with an SPDX header:
  `// SPDX-License-Identifier: MIT OR Apache-2.0` (or `#` for YAML and shell).
- Match the surrounding code's style, comments and naming.
- After changing the C ABI, regenerate the header with
  `cbindgen --config cbindgen.toml --crate scarletbook-ffi --output include/scarletbook.h`
  in `crates/scarletbook-ffi`; CI checks it is up to date.
- Add a line under "Unreleased" in [CHANGELOG.md](CHANGELOG.md) for a user-visible change.

## Checks

CI runs these; please run them before you push:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo test --workspace --all-features
```

If you changed `scarletbook` or `scarletbook-ffi`, also run Miri
(`cargo +nightly miri test -p scarletbook -p scarletbook-ffi`), and if you changed
dependencies, `cargo deny check`. The minimum supported Rust version is 1.85.

GitHub Actions are pinned by full commit SHA with the version in a comment, and checkouts
use `persist-credentials: false`; zizmor checks the workflows.

## Licence

Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed
under MIT OR Apache-2.0, without any additional terms or conditions.
