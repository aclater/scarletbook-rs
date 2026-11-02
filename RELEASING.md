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

1. `preflight`: the tag matches the workspace version and CHANGELOG.md has its section;
2. `clib` and `benchkit`: the C libraries and the benchmark kits, built and tested;
3. `release`: `bench`, the correctness gate, the assets and the GitHub release;
4. `publish`, only if all of the above passed: `scarletbook-pcm`, `scarletbook` and
   `scarletbook-ffi` to crates.io, in that order, with crates.io trusted publishing. A crate
   whose version is already on crates.io is skipped, so the job can be re-run, and a crate
   published by hand (below) is not published twice.

The workflow runs only on tag pushes, never on pull requests or branches.

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

The trusted publisher names this repository and workflow file and **no environment**, so
the `publish` job stays in `release.yml` itself (not in a reusable workflow it calls) and has
no `environment:`. To set one up, once the crate exists:

1. Open `https://crates.io/crates/<crate>/settings` (signed in as an owner).
2. Trusted Publishing, Add, GitHub.
3. Repository owner `aclater`, repository name `scarletbook-rs`, workflow filename
   `release.yml`, environment empty. Save.
4. Optionally, require trusted publishing for the crate, so that no API token can publish it.

| Crate | First version | Published by | Trusted publisher |
|---|---|---|---|
| `scarletbook-pcm` | 0.5.7 | the owner, from the tag v0.5.7 (option A) | configured |
| `scarletbook` | 0.5.7 | the owner, from the tag v0.5.7 (option A) | configured |
| `scarletbook-ffi` | 0.5.8 (pending) | option A or B below, for this crate only | after its first publish |

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

### Option B: a repository secret for one release

The `publish` job publishes a crate that is not on crates.io yet with the repository secret
`CRATES_IO_BOOTSTRAP_TOKEN`, if it is set; crates that already exist still go through trusted
publishing, so requiring trusted publishing on them is no obstacle.

```sh
# Before pushing the tag v0.5.8, or before re-running its failed publish job:
gh secret set CRATES_IO_BOOTSTRAP_TOKEN --repo aclater/scarletbook-rs
# (paste the token at the prompt)

# Push the tag (or re-run the job: gh run rerun <run id> --failed).

# Right after the publish job has succeeded:
gh secret delete CRATES_IO_BOOTSTRAP_TOKEN --repo aclater/scarletbook-rs
```

### After either option

1. Revoke the token at `https://crates.io/settings/tokens`.
2. Set up the trusted publisher for `scarletbook-ffi` (above), and optionally require
   trusted publishing for it.
3. Update the table above. From then on every tag publishes all three crates by itself.
