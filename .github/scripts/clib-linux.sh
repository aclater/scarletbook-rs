#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Builds, packages and tests the C library for a Linux target inside a PyPA build image
# (.github/workflows/clib.yml), as root, with the repository mounted as the working
# directory:
#
# - *-linux-gnu in manylinux2014 (CentOS 7, glibc 2.17): the shared library is linked
#   against glibc 2.17, so it needs no newer symbol version; checked below.
# - *-linux-musl in musllinux_1_2 (Alpine, musl 1.2). Rust's musl targets link the C
#   runtime statically by default and then cannot build a shared library;
#   -crt-static links musl dynamically, as a Python extension on musl must.
#
# Both images carry the C compiler and the Python the tests use, so the libraries are
# tested on the oldest C library they claim to support.
#
# Usage: clib-linux.sh TARGET "MINIMUM PLATFORM" [VARIANT]; RUST_STABLE is the toolchain.
# VARIANT `v3` builds for x86-64-v3 (clib-package.sh); it is tested only on a CPU that has
# the level's features, and stops otherwise rather than skip its tests.
set -euo pipefail

target=$1 platform=$2 variant=${3:-}
if [[ "$variant" == v3 ]]; then
  # The x86-64-v3 features as /proc/cpuinfo names them: LZCNT is "abm", and the kernel
  # lists XSAVE, not OSXSAVE (the bit it sets once it has enabled XSAVE).
  for flag in avx avx2 bmi1 bmi2 f16c fma abm movbe xsave cx16 lahf_lm popcnt sse4_1 sse4_2 ssse3; do
    if ! grep -qw "$flag" /proc/cpuinfo; then
      echo "this CPU lacks $flag, so it is not x86-64-v3 and cannot test the v3 build" >&2
      exit 1
    fi
  done
fi
# rustup-init of a pinned rustup release, checked against its pinned SHA-256
# (static.rust-lang.org/rustup/archive/<version>/<host>/rustup-init.sha256).
rustup_version=1.29.2
host="$(uname -m)-unknown-linux-${target##*-}"
case "$host" in
  x86_64-unknown-linux-gnu) sha=5be191e8c70a5007074254cb74d78dbfdd25cd5a61c27de7b8e5410122ec24f4 ;;
  aarch64-unknown-linux-gnu) sha=a9c168dbea81a84671c6b7a5c1795cb0eb699020110d107ddfca427bd797e697 ;;
  x86_64-unknown-linux-musl) sha=2ba97d668c06dc4b1884f2a3fa6f4cf14b9bd0c1db379af7e4f87a705ff19edd ;;
  aarch64-unknown-linux-musl) sha=7fdbb23acf6495efee2e371580c52b365917551b4835090a225c44ac30915235 ;;
  *) echo "no pinned rustup-init for $host" >&2; exit 1 ;;
esac
curl -sSfo /tmp/rustup-init "https://static.rust-lang.org/rustup/archive/$rustup_version/$host/rustup-init"
echo "$sha  /tmp/rustup-init" | sha256sum -c -
chmod +x /tmp/rustup-init
/tmp/rustup-init -y --quiet --profile minimal --default-toolchain "$RUST_STABLE"
# shellcheck source=/dev/null
source "$HOME/.cargo/env"
rustup target add "$target"
export PYTHON=/opt/python/cp312-cp312/bin/python
case "$target" in
  *-musl) export RUSTFLAGS="-C target-feature=-crt-static" ;;
esac

bash .github/scripts/clib-package.sh "$target" "$platform" "$variant"
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
pkg="dist/scarletbook-ffi-$version-$target${variant:+-$variant}"
abi=$(sed -n 's/^#define SB_ABI_VERSION //p' "$pkg/include/scarletbook.h")
so="$pkg/lib/libscarletbook_ffi.so.$abi"

echo "Shared library dependencies:"
dynamic=$(readelf -d "$so")
grep -E 'NEEDED|SONAME' <<< "$dynamic"
if ! grep -q "SONAME.*\[libscarletbook_ffi\.so\.$abi\]" <<< "$dynamic"; then
  echo "the shared library's SONAME is not libscarletbook_ffi.so.$abi (SB_ABI_VERSION is $abi)" >&2
  exit 1
fi
# The versioned name is the file and the unversioned one a link to it, in the archive too.
listing=$(tar -tzvf "$pkg.tar.gz")
if [[ -L "$so" || "$(readlink "$pkg/lib/libscarletbook_ffi.so")" != "libscarletbook_ffi.so.$abi" ]] \
  || ! grep -Eq "^-.*/lib/libscarletbook_ffi\.so\.$abi\$" <<< "$listing" \
  || ! grep -Eq "^l.*/lib/libscarletbook_ffi\.so -> libscarletbook_ffi\.so\.$abi\$" <<< "$listing"; then
  echo "lib/libscarletbook_ffi.so must be a link to the file libscarletbook_ffi.so.$abi, in the archive too:" >&2
  grep 'libscarletbook_ffi\.so' <<< "$listing" >&2
  exit 1
fi
case "$target" in
  *-gnu)
    newest=$(objdump -T "$so" | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/GLIBC_//' | sort -uV | tail -n 1)
    echo "Newest glibc symbol version required: $newest"
    if [[ "$(printf '%s\n' "$newest" 2.17 | sort -V | tail -n 1)" != 2.17 ]]; then
      echo "the shared library requires glibc $newest, newer than 2.17 (manylinux2014)" >&2
      exit 1
    fi
    ;;
esac

bash .github/scripts/clib-test.sh "$pkg"
