<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Releasing

A release is a tag `vX.Y.Z` on the merge commit of the pull request that prepares it. That
pull request sets the version in three places (the build fails if they disagree):

- `version` under `[workspace.package]` in `Cargo.toml`;
- the `version = "=X.Y.Z"` of `scarletbook` and `scarletbook-pcm` under `[dependencies]` in
  `crates/scarletbook-ffi/Cargo.toml` (crates.io needs a version beside each path);

and renames the Unreleased section of CHANGELOG.md for the version
(`.github/scripts/fragments.py assemble X.Y.Z DATE`).

Pushing the tag runs `.github/workflows/release.yml`:

1. `preflight`: the tagged commit is on `main` and passed `main`'s required checks (it
   waits up to 45 minutes for checks still running on a just-merged commit), the tag
   matches the workspace version and CHANGELOG.md has its section;
2. `clib` and `benchkit`: the C libraries and the benchmark kits, built and tested;
3. `build`, with a read-only token: `bench`, the correctness gates, all assets with their
   checksums, the release notes, and the three crates.io packages, packed and verified
   (`cargo package`);
4. `release`, in the environment `release`: the build provenance attestation and
   the GitHub release, from `build`'s files;
5. `publish`, in the environment `release`, only if all of the above passed:
   `scarletbook-pcm`, `scarletbook` and `scarletbook-ffi` to crates.io, in that order, with
   crates.io trusted publishing. A crate whose version is already on crates.io is skipped, so
   the job can be re-run, and a crate published by hand (below) is not published twice.

The jobs that sign and publish compile nothing (issue #345): dependencies' build scripts and
proc macros run only in `build`, which holds no write, signing or publishing permission.
`cargo publish` always packs from the sources, so `publish` packs each crate again with
`--no-verify`, which builds nothing, checks that the file is byte-identical to the package
`build` verified (cargo packs reproducibly), and publishes it with `--no-verify`. Before the
first upload it repacks all three together and compares them, so a mismatch stops the job
before any crate is published.

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
