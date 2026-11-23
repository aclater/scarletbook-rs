<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Releasing

A release is a tag `vX.Y.Z` on the merge commit of the pull request that prepares it. That
pull request sets the version in two places (the build fails if they disagree):

- `version` under `[workspace.package]` in `Cargo.toml`, which `scarletbook`,
  `scarletbook-ffi` and the unpublished crates follow, and which the tag must match;
- the `version = "=X.Y.Z"` of `scarletbook` under `[dependencies]` in
  `crates/scarletbook-ffi/Cargo.toml` (crates.io needs a version beside each path);

and renames the Unreleased section of CHANGELOG.md for the version
(`.github/scripts/fragments.py assemble X.Y.Z DATE`).

`scarletbook-pcm` has its own `version` in `crates/scarletbook-pcm/Cargo.toml`, not the
workspace's. It is decommissioned (#398): frozen at 0.7.0 on crates.io, no further releases,
outside the 1.0 freeze. Its pin in `crates/scarletbook-ffi/Cargo.toml` stays `=0.7.0`, and
nothing under `crates/scarletbook-pcm/` changes, since such a change could never ship. No
release packs or publishes it: its 0.7.0 is on crates.io, so `build` packs only the other two
and verifies `scarletbook-ffi` against the crates.io 0.7.0, and `publish` skips it (below,
"Pack only what is published").

**From 1.0.0, check the semver of every release** (#39, R3) against the previous tag, with the
release type of the bump, for the two frozen crates:

```sh
# --release-type: patch for x.y.Z, minor for x.Y.0 (major only for a planned 2.0.0).
cargo semver-checks -p scarletbook --baseline-rev vX.Y.Z --release-type patch --all-features
cargo semver-checks -p scarletbook-ffi --baseline-rev vX.Y.Z --release-type patch --all-features
python3 .github/scripts/abi-check.py   # the C ABI; SB_ABI_VERSION stays 2 throughout 1.x
```

Both must report "no semver update required"; give the result in the pull request. A check
that fails, or any change to `abi/abi-2.txt` other than an addition, stops the release. `scarletbook-ffi`'s contract is the C ABI, so `abi-check.py` is its real check; the
Rust check of it is for completeness. `scarletbook-pcm` is not checked: it is decommissioned and
frozen at 0.7.0 (#398).

It also moves the README's Quick start (#364) to the new version: the `cargo add` pin, the
tag in the image URL, the release asset name, and any API the examples use that changed. The
Quick start fetches the published release, so it can only run once the tag is out. After the
release, run both of its command blocks exactly as written in an empty directory, and if they
fail, or their output differs from what the README shows, fix the README in a follow-up pull
request.

Pushing the tag runs `.github/workflows/release.yml`:

1. `preflight`: the tagged commit is on `main` and passed `main`'s required checks (it
   waits up to 45 minutes for checks still running on a just-merged commit), the tag
   matches the workspace version and CHANGELOG.md has its section;
2. `clib` and `benchkit`: the C libraries and the benchmark kits, built and tested;
3. `build`, with a read-only token: `bench`, the correctness gates, all assets with their
   checksums, the release notes, and the crates.io packages of the crates whose version is
   not on crates.io yet, packed together and verified (`cargo package`);
4. `release`, in the environment `release`: the build provenance attestation and
   the GitHub release, from `build`'s files;
5. `publish`, in the environment `release`, only if all of the above passed:
   `scarletbook-pcm`, `scarletbook` and `scarletbook-ffi` to crates.io, in that order, with
   crates.io trusted publishing. Each crate is published at its own manifest version, and one
   whose version is already in the crates.io index is skipped
   (`.github/scripts/release-crates.py`, tested by `release-crates-test.py` in CI): an unchanged
   crate (`scarletbook-pcm`, frozen at 0.7.0, #398) is not published again, the job can be re-run,
   and a crate published by hand (below) is not published twice.

The jobs that sign and publish compile nothing (issue #345): dependencies' build scripts and
proc macros run only in `build`, which holds no write, signing or publishing permission.
`cargo publish` always packs from the sources, so `publish` packs each crate again with
`--no-verify`, which builds nothing, checks that the file is byte-identical to the package
`build` verified (cargo packs reproducibly), and publishes it with `--no-verify`. Before the
first upload it repacks all the crates it is about to publish together and compares them, so
a mismatch stops the job before any crate is published.

### Pack only what is published

`build` looks each crate's version up in the crates.io index (`release-crates.py pending`,
read-only, no token) and packs only the crates crates.io does not have yet, together:
`cargo package --locked -p <pending crates>`. A crate whose version is already on crates.io
(skipped) is not packed; the crates that depend on it resolve it from crates.io, as they will
for every user. It must not be packed: a repack never matches the published file
(`.cargo_vcs_info.json` names the commit), and a crate packed beside it would pin the
repack's checksum in its packaged `Cargo.lock`, a checksum crates.io does not have.

`publish` repacks the same way. Its crates resolve every crate outside the set from
crates.io, including one it uploaded a moment earlier (or in an earlier run of the job):
that crate was uploaded only after its repack matched `build`'s file byte for byte, so
crates.io's checksum of it is the one `build`'s packages pin. The repack of each later crate
is then the file `build` verified.

`release-crates.py lockcheck` checks this on the packed files: every published crate a
packaged `Cargo.lock` pins must carry crates.io's checksum if crates.io has that version, and
the checksum of the file packed beside it if not. `build` runs it, and CI runs a dry run of
the packing on every pull request and push (`release-publish.sh dry-run`: `--no-verify`, no
token, no upload). `release-crates-test.py` covers the 1.0.0 case.

**The 1.0.0 incident.** `build` packed all three crates together, `scarletbook-pcm` 0.7.0
included, although that version was already on crates.io. `scarletbook-ffi`'s packaged
`Cargo.lock` therefore pinned the repacked `scarletbook-pcm` (`95dbb558...`) instead of the
published one (`ffa22529...`). `publish` skipped `scarletbook-pcm`, published `scarletbook`
1.0.0, then repacked `scarletbook-ffi` against the crates.io `scarletbook-pcm`, and the
byte-identity check refused it (`6ce1c36...` against `build`'s `c94e18b...`), rightly: the
package `build` verified pinned a checksum crates.io does not have. crates.io has
`scarletbook` 1.0.0 but no `scarletbook-ffi` 1.0.0; 1.0.1 publishes both.

**A release whose `publish` stopped partway is finished by the next version.** A tag runs the
workflow of its own commit, and a re-run of `publish` takes `build`'s packages of the same
run, so a fault in either cannot be fixed for that tag. Do not move or re-push the tag: fix
the workflow on `main` and release the next patch version, which publishes every crate whose
version is not on crates.io.

The split keeps the publishing token and the signing out of every step that runs dependency
code. It does not make `build`'s output trustworthy: a compromised dependency in `build` (or
in the `clib` and `benchkit` jobs) could alter the archives or the packed crates, and the
attestation would then sign them and `publish` would publish them as they are.

The workflow runs only on tag pushes, never on pull requests or branches.

The required checks are read from `main`'s branch ruleset, and matched against the GitHub
Actions check runs of the tagged commit. `dependency-review` is the exception: it runs only
on pull requests, so a commit on `main` never has it; the ruleset required it on the pull
request that merged. If `preflight` fails because a check failed or was still running after
45 minutes, fix that on `main`, or re-run the release workflow once the checks have passed.

**main's CI was cancelled on the tagged commit.** `ci.yml` cancels a running push run on
`main` when the next push starts one, so if another pull request merges before the
release-preparation commit's CI has finished, that commit's checks end `cancelled` and
`preflight` refuses the tag. The tag is fine; do not move or re-push it. Instead:

```sh
# 1. Re-run main's cancelled CI run for the tagged commit and wait for it to pass.
gh run list --repo aclater/scarletbook-rs --workflow ci.yml --commit "$(git rev-parse vX.Y.Z^{commit})"
gh run rerun <ci run id> --repo aclater/scarletbook-rs
gh run watch <ci run id> --repo aclater/scarletbook-rs --exit-status
# 2. Re-run the failed release run; it runs again on the same tag and commit.
gh run rerun <release run id> --repo aclater/scarletbook-rs
```

## Repository and crates.io settings (issue #341)

Releases are fully automated (owner, 2026-11-15): a pushed tag that passes `preflight`
publishes with no approval step. The agents manage the code and the releases. The owner
decided on 2026-11-16 to keep one setting, the environment below, and not to restrict the
crates.io trusted publishers to an environment or to restrict who can create a tag.

**The environment `release`** (Settings, Environments): no required reviewers; deployment
branches and tags: Selected branches and tags, with one rule of **Ref type: Tag**, pattern
`v*`. It must be a tag rule: a branch rule `v*` matches no tag, so the `release` and
`publish` jobs of a tag run would be refused by the environment and the release would stop
there. A workflow run that names an environment which does not exist makes GitHub create it
with no deployment rule, which lets a job on any branch use it.

### What this stops, and what it doesn't

- The environment's secrets, such as the bootstrap secret (below), are available only to
  the `release` and `publish` jobs of a run on a `v*` tag.
- `preflight` refuses a tag whose commit is not on `main` or has not passed `main`'s
  required checks.

Together these stop honest mistakes and stray tags. They do not stop a compromised token
with write access to the repository. The crates.io trusted publishers name this repository
and `release.yml`, but no environment, so crates.io issues a token to any job of a workflow
file named `release.yml` in this repository, whatever the ref; and anyone who can push to
the repository can create a `v*` tag, since tag creation is not restricted. Such a token can
therefore publish from any commit it can tag and get past `preflight`, for example by
tagging a commit that edits `release.yml` (the workflow that runs is the tagged commit's
copy, `preflight` included), or from a branch whose `release.yml` it has changed. Nothing in
the workflow guards against that: protecting the tokens and accounts with write access is
what does.

## Before the release: check the packages

```sh
cargo package --list -p scarletbook-pcm
cargo package --list -p scarletbook
cargo package --list -p scarletbook-ffi
# All three together: cargo verifies scarletbook-ffi against the other two as packaged.
cargo publish --dry-run --locked -p scarletbook-pcm -p scarletbook -p scarletbook-ffi
```

Each crate's `include` in its `Cargo.toml` decides what ships: the sources, what the build
reads (`build.rs`, `build/`, the header, the CC0-1.0 charset tables of `scarletbook`, each
with its licence line), the README and the licences. Tests, benches, examples, the corpus,
fuzz data and the ABI records stay in the repository.

## Trusted publishing (once per crate)

crates.io issues a 30-minute token to this repository's release workflow through GitHub
OIDC (`rust-lang/crates-io-auth-action`); no token is stored. It can only be configured for
a crate that already exists on crates.io ("Your crate must already be published to crates.io
(initial publish requires an API token)", crates.io documentation, "Trusted Publishing"), so
trusted publishing cannot create a crate: each crate's first version is published with an
API token (below), and trusted publishing is set up right after.

The trusted publisher names this repository and workflow file, with no environment (owner,
2026-11-16), so the `publish` job stays in `release.yml` itself (not in a reusable workflow
it calls). To set one up, once the crate exists:

1. Open `https://crates.io/crates/<crate>/settings` (signed in as an owner).
2. Trusted Publishing, Add, GitHub.
3. Repository owner `aclater`, repository name `scarletbook-rs`, workflow filename
   `release.yml`, environment left empty. Save.
4. Optionally, require trusted publishing for the crate, so that no API token can publish it.

| Crate | First version | Published by | Trusted publisher |
|---|---|---|---|
| `scarletbook-pcm` | 0.5.7 | the owner, from the tag v0.5.7 (option A) | configured |
| `scarletbook` | 0.5.7 | the owner, from the tag v0.5.7 (option A) | configured |
| `scarletbook-ffi` | 0.5.8 | the owner, on the tag v0.5.8 | configured |

At v0.5.7, `scarletbook-ffi` had `publish = false` and versionless path dependencies, so it
could not be published from that tag; it starts at 0.5.8.

## The first publish of a crate (`scarletbook-ffi` at 0.5.8)

On the tag v0.5.8 the `publish` job publishes `scarletbook-pcm` and `scarletbook` through
trusted publishing, then reaches `scarletbook-ffi`, which is not on crates.io yet. Without
a bootstrap token (option B) it stops there with an error that names the crate; the release
itself and the other two crates are already done. Then use option A, or set the token and
re-run the job (option B).

Both options use a short-lived API token. Create it at `https://crates.io/settings/tokens/new`:

- name: `scarletbook-ffi bootstrap`;
- expiration: 1 day;
- scopes: `publish-new` only;
- crates: `scarletbook-ffi`.

Copy the token; crates.io shows it once. The crates.io account needs a verified e-mail
address (`https://crates.io/settings/profile`).

### Option A: publish from the tag on your own machine

Needs Rust 1.85 or later (`rustup update stable`) and git. Run it after the tag's `publish`
job has published (or skipped) `scarletbook-pcm` and `scarletbook` 0.5.8, since
`scarletbook-ffi` depends on exactly those versions.

```sh
git clone https://github.com/aclater/scarletbook-rs.git
cd scarletbook-rs
git checkout v0.5.8
git status                                   # must be clean: cargo refuses dirty trees
cargo login                                  # paste the token
cargo publish --locked -p scarletbook-ffi
cargo logout                                 # removes the token from ~/.cargo/credentials.toml
```

### Option B: an environment secret for one release

The `publish` job publishes a crate that is not on crates.io yet with the secret
`CRATES_IO_BOOTSTRAP_TOKEN` of the environment `release`, if it is set. Set it on the
environment, not the repository: a repository secret can be read by a job of any workflow on
any branch, an environment secret only by a job in that environment, which is limited to
`v*` tags; crates that already exist still go through trusted
publishing, so requiring trusted publishing on them is no obstacle.

```sh
# Before pushing the tag v0.5.8, or before re-running its failed publish job:
gh secret set CRATES_IO_BOOTSTRAP_TOKEN --env release --repo aclater/scarletbook-rs
# (paste the token at the prompt)

# Push the tag (or re-run the job: gh run rerun <run id> --failed).

# Right after the publish job has succeeded:
gh secret delete CRATES_IO_BOOTSTRAP_TOKEN --env release --repo aclater/scarletbook-rs
```

### After either option

1. Revoke the token at `https://crates.io/settings/tokens`.
2. Set up the trusted publisher for `scarletbook-ffi` (above), and optionally require
   trusted publishing for it.
3. Update the table above. From then on every tag publishes all three crates by itself.
