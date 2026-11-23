#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Packs and publishes the crates.io packages of a release without compiling anything
# (issue #345). Run from the tagged checkout.
#
#   release-publish.sh [publish]   (release.yml `publish`, with CARGO_REGISTRY_TOKEN)
#       Publishes the crates named in $CRATES, in that order.
#   release-publish.sh precheck    (release.yml `publish`, before the first upload)
#       Packs all of $CRATES together and checks each against the verified package.
#   release-publish.sh dry-run     (ci.yml, no token, no upload)
#       Looks up which crates are not on crates.io yet, packs them together as `build`
#       does (with --no-verify) and checks their Cargo.lock pins (release-crates.py
#       lockcheck). It would have stopped the 1.0.0 packing on every pull request.
#
# `cargo publish` cannot upload a given .crate file; it always packs from the sources. So
# each crate is packed again with --no-verify (no build), and published only if that file
# is byte-identical to the one `build` packed and verified, listed in
# release-files/crates-io/SHA256SUMS (cargo packs reproducibly). The publish then packs the
# same sources once more, with --no-verify.
#
# `build` packs only the crates that are not on crates.io yet, together; every other
# published crate resolves from crates.io (RELEASING.md, "Pack only what is published").
# A repack here gives the same file: a crate published earlier in this job (or an earlier
# run) resolves from crates.io, which holds exactly the file `build` packed beside it, since
# it was published only after matching that file byte for byte.
#
# Each crate's version comes from its manifest (release-crates.py), not from the tag: the
# crates need not share one version (scarletbook-pcm stays at 0.7.0 from 1.0.0, #398).
set -euo pipefail

mode=${1:-publish}
sums=release-files/crates-io/SHA256SUMS
crates_py=.github/scripts/release-crates.py
versions=$(python3 -I "$crates_py" versions)

file_of() {
  local version
  version=$(awk -v c="$1" '$1 == c { print $2 }' <<< "$versions")
  if [[ -z "$version" ]]; then
    echo "::error::$1 is not one of the published crates" >&2
    exit 1
  fi
  echo "$1-$version.crate"
}

# match DIR CRATE: the packed file in DIR is the one `build` verified.
match() {
  local file want got
  file=$(file_of "$2")
  want=$(awk -v f="$file" '$2 == f { print $1 }' "$sums")
  got=$(sha256sum "$1/$file" | cut -d' ' -f1)
  if [[ -z "$want" || "$got" != "$want" ]]; then
    echo "::error::$file packed here ($got) is not the package verified by the build job (${want:-missing}); not publishing"
    exit 1
  fi
  echo "$file matches the verified package ($got)"
}

pack_together() { # TARGET_DIR CRATE...
  local dir=$1 args=() crate
  shift
  for crate in "$@"; do args+=(-p "$crate"); done
  cargo package --no-verify --locked "${args[@]}" --target-dir "$dir"
}

case $mode in
  publish)
    for crate in $CRATES; do
      cargo package --no-verify --locked -p "$crate"
      match target/package "$crate"
      # Waits until the version is in the index, so the next crate resolves it.
      cargo publish --no-verify --locked -p "$crate"
    done
    ;;
  precheck)
    # shellcheck disable=SC2086 # $CRATES is a space-separated list of crate names
    pack_together target/precheck $CRATES
    for crate in $CRATES; do match target/precheck/package "$crate"; done
    ;;
  dry-run)
    out=$(mktemp)
    python3 -I "$crates_py" pending "$out"
    pending=$(sed -n 's/^pending=//p' "$out")
    rm -f "$out"
    if [[ -z "$pending" ]]; then
      echo "every published crate's version is on crates.io; nothing to pack"
      exit 0
    fi
    dir=target/release-dry-run
    rm -rf "$dir/package" "$dir/packed"
    # shellcheck disable=SC2086
    pack_together "$dir" $pending
    mkdir "$dir/packed"
    for crate in $pending; do cp "$dir/package/$(file_of "$crate")" "$dir/packed/"; done
    python3 -I "$crates_py" lockcheck "$dir/packed"
    ;;
  *)
    echo "usage: $0 [publish|precheck|dry-run]" >&2
    exit 2
    ;;
esac
