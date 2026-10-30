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

By opening a pull request you confirm that, for the work in it:

- The only SACD/DST material you consulted is `aclater/sacd-spec`: its specifications, the
  charset tables, the conformance corpus and its README, and its `PROVENANCE.md`.
- You did not read, search for, fetch or install the source of any SACD or DST
  implementation, including pysacd, sacd-ripper, `sacd_extract`, FFmpeg's DST decoder,
  sacd-rs, the dst-decoder crate and the Philips/ISO reference decoder, and you made no web
  search for SACD or DST implementations. If you have studied such code in the past, say so
  in the pull request; the owner decides whether the contribution can be taken.
- You did not read patents, live or expired.
- You copied no code from any other project into this repository. Ordinary dependencies
  from crates.io are fine; cargo-deny checks their licences.

General references that are not SACD or DST material (Rust documentation, compiler and
CPU manuals, algorithm books and papers, GitHub documentation) may be used. List each one
you consulted in the pull request, with its URL or citation and the date, so that it can be
recorded in CLEANROOM.md. For a technique adopted for performance, prefer sources published
before 2006 (see "Patent hygiene" in CLEANROOM.md).

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
