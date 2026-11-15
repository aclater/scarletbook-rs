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
`build` verified (cargo packs reproducibly), and publishes it with `--no-verify`.

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

`preflight` stops a mistaken or stray tag, but not a tag on a commit that also edits
`release.yml`: the workflow that runs is the one in the tagged commit. Only repository and
crates.io settings stop that. The owner makes them, in this order:

1. **Create the environment first, before the pull request that adds `environment: release`
   to `release.yml` merges, and in any case before the next tag.** Settings, Environments, New environment, `release`:
   - Required reviewers: the owner. Prevent self-review stays off, so the owner can approve
     a release they tagged.
   - Deployment branches and tags: Selected branches and tags, add the tag rule `v*`.

   A workflow run that names an environment which does not exist makes GitHub create it,
   with no protection rules, so a tag pushed before this step would sign and publish
   without approval. The `release` and
   `publish` jobs each wait for approval (Review deployments on the run's page), so a
   release is approved twice: once before the assets are signed and uploaded, once before
   crates.io.
2. **Set the trusted publishers' environment to `release`,** once the pull request that
   added `environment: release` to `release.yml` has merged and before the next tag. For
   each of `scarletbook-pcm`, `scarletbook` and `scarletbook-ffi`, at
   `https://crates.io/crates/<crate>/settings`, edit the GitHub trusted publisher (or remove
   it and add it again) with environment `release`, everything else unchanged. crates.io
   accepts a token request from a job in an environment when the trusted publisher names
   that environment or none, and refuses one from a job without an environment when the
   trusted publisher names one. So a publisher with environment `release` fails every
   release from a `release.yml` without `environment: release`, while the workflow change
   works with the old publishers; the workflow change merges first. Until this step is
   done, any job of this workflow file can still get a crates.io token.
3. **Restrict tag creation.** Settings, Rules, Rulesets, `release tags`: add the rule
   "Restrict creations", with Repository admin in the bypass list, so that only the owner
   can create a `v*` tag.

   This rule does not limit our own sessions: they push tags with the owner's admin account,
   which the bypass lets through. It only stops accounts without admin access (and their
   tokens). For every tag our sessions push, the `release` environment's approval is the
   real gate.

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

The trusted publisher names this repository, workflow file and the environment `release`
(above), so the `publish` job stays in `release.yml` itself (not in a reusable workflow it
calls) and keeps `environment: release`. To set one up, once the crate exists:

1. Open `https://crates.io/crates/<crate>/settings` (signed in as an owner).
2. Trusted Publishing, Add, GitHub.
3. Repository owner `aclater`, repository name `scarletbook-rs`, workflow filename
   `release.yml`, environment `release`. Save.
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
any branch, an environment secret only by an approved job in that environment; crates that already exist still go through trusted
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
