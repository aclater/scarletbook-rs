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
(initial publish requires an API token)", crates.io documentation, "Trusted Publishing").
So each crate's first version is published with an API token, by one of the two ways below,
and trusted publishing is set up right after.

For each of `scarletbook-pcm`, `scarletbook` and `scarletbook-ffi`, once it exists:

1. Open `https://crates.io/crates/<crate>/settings` (signed in as an owner).
2. Trusted Publishing, Add, GitHub.
3. Repository owner `aclater`, repository name `scarletbook-rs`, workflow filename
   `release.yml`, environment `crates-io`. Save.

## The first publish of a crate

The crates.io account needs a verified e-mail address before it can publish
(`https://crates.io/settings/profile`).

Both ways use a short-lived API token. Create it at `https://crates.io/settings/tokens/new`:

- name: `scarletbook bootstrap`;
- expiration: 1 day for option A, 7 days for option B;
- scopes: `publish-new` and `publish-update` only;
- crates: `scarletbook`, `scarletbook-pcm`, `scarletbook-ffi` (or the pattern `scarletbook*`).

Copy the token; crates.io shows it once.

### Option A: publish from the tag on your own machine

Needs Rust 1.85 or later (`rustup update stable`) and git.

```sh
git clone https://github.com/aclater/scarletbook-rs.git
cd scarletbook-rs
git checkout vX.Y.Z                          # the release tag, e.g. v0.5.7
git status                                   # must be clean: cargo refuses dirty trees
cargo login                                  # paste the token
cargo publish --locked -p scarletbook-pcm
cargo publish --locked -p scarletbook
cargo publish --locked -p scarletbook-ffi    # not for v0.5.7: see below
cargo logout                                 # removes the token from ~/.cargo/credentials.toml
```

Then revoke the token at `https://crates.io/settings/tokens` and set up trusted publishing
for each crate published (above). If the tag's `publish` job ran before or fails for a
crate without a trusted publisher, re-run it after setting one up; it skips what is
already there.

### Option B: a token secret for one release

The `publish` job uses the environment secret `CRATES_IO_BOOTSTRAP_TOKEN`, when it is set,
instead of trusted publishing. It only exists in the workflow from 0.5.8 on, so it cannot
publish v0.5.7.

```sh
# Before pushing the tag: create the environment and store the token in it.
gh api -X PUT repos/aclater/scarletbook-rs/environments/crates-io
gh secret set CRATES_IO_BOOTSTRAP_TOKEN --env crates-io --repo aclater/scarletbook-rs
# (paste the token at the prompt)

# Push the tag as usual; the release workflow publishes with the token and warns that it did.

# Right after the publish job has succeeded:
gh secret delete CRATES_IO_BOOTSTRAP_TOKEN --env crates-io --repo aclater/scarletbook-rs
```

Then revoke the token at `https://crates.io/settings/tokens` and set up trusted publishing
for every crate (above), so that the next release publishes without a token.

### v0.5.7

At the tag v0.5.7, `scarletbook-pcm` and `scarletbook` can be published as they are
(`cargo publish --dry-run` passes for both), but `scarletbook-ffi` cannot: it has
`publish = false` and its path dependencies have no version. The tag's `release.yml` has no
`publish` job either. So 0.5.7 is published by option A, for `scarletbook-pcm` and
`scarletbook` only; `scarletbook-ffi` starts at 0.5.8, by option A or B for that one crate
(the other two then publish through trusted publishing).
