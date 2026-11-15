#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Publishes the crates named in $CRATES (in that order) to crates.io without compiling
# anything (issue #345). Run by release.yml's `publish` job from the tagged checkout, with
# CARGO_REGISTRY_TOKEN set and the `build` job's packages in release-files/crates/.
#
# `cargo publish` cannot upload a given .crate file; it always packs from the sources. So
# each crate is packed again with --no-verify (no build), and published only if that file
# is byte-identical to the one `build` packed and verified (cargo packs reproducibly). The
# publish then packs the same sources once more, with --no-verify.
set -euo pipefail

version=${GITHUB_REF_NAME#v}
sums=release-files/crates/SHA256SUMS
for crate in $CRATES; do
  file="$crate-$version.crate"
  cargo package --no-verify --locked -p "$crate"
  want=$(awk -v f="$file" '$2 == f { print $1 }' "$sums")
  got=$(sha256sum "target/package/$file" | cut -d' ' -f1)
  if [[ -z "$want" || "$got" != "$want" ]]; then
    echo "::error::$file packed here ($got) is not the package verified by the build job (${want:-missing}); not publishing"
    exit 1
  fi
  echo "$file matches the verified package ($got)"
  # Waits until the version is in the index, so the next crate resolves it.
  cargo publish --no-verify --locked -p "$crate"
done
