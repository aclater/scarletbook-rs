#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Builds the C library (scarletbook-ffi) for one target and packages it as
# dist/scarletbook-ffi-VERSION-TARGET.tar.gz with a .sha256 file:
#
#   scarletbook-ffi-VERSION-TARGET/
#     include/scarletbook.h
#     lib/        shared library, static library (and the DLL import library on Windows)
#     native-static-libs.txt   what to link after the static library
#     README.txt, LICENSE-MIT, LICENSE-APACHE
#
# Usage: clib-package.sh TARGET "MINIMUM PLATFORM"   (run from the repository root)
# RUSTFLAGS and MACOSX_DEPLOYMENT_TARGET are taken from the environment.
set -euo pipefail

target=$1 platform=$2
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
name="scarletbook-ffi-$version-$target"
pkg="dist/$name"

# Profile dist: release with one codegen unit, as for the generic `bench` asset.
CARGO_TERM_COLOR=never cargo rustc --profile dist --locked -p scarletbook-ffi --lib --target "$target" \
  -- --print native-static-libs 2>&1 | tee build.log
native=$(sed -n 's/^.*native-static-libs: //p' build.log | tail -n 1)
if [[ -z "$native" ]]; then
  echo "no native-static-libs line in the build output" >&2
  exit 1
fi

out="${CARGO_TARGET_DIR:-target}/$target/dist"
rm -rf "$pkg"
mkdir -p "$pkg/include" "$pkg/lib"
cp crates/scarletbook-ffi/include/scarletbook.h "$pkg/include/"
case "$target" in
  *-windows-*) shared=(scarletbook_ffi.dll libscarletbook_ffi.dll.a) ;;
  *-apple-*) shared=(libscarletbook_ffi.dylib) ;;
  *) shared=(libscarletbook_ffi.so) ;;
esac
for f in "${shared[@]}" libscarletbook_ffi.a; do
  cp "$out/$f" "$pkg/lib/"
done
printf '%s\n' "$native" > "$pkg/native-static-libs.txt"
cp LICENSE-MIT LICENSE-APACHE "$pkg/"
cat > "$pkg/README.txt" <<EOF
scarletbook C library $version for $target, built from ${GITHUB_SHA:-a local checkout}.
Runs on: $platform.
C ABI version (SB_ABI_VERSION, sb_abi_version()): $(sed -n 's/^#define SB_ABI_VERSION //p' crates/scarletbook-ffi/include/scarletbook.h).

include/scarletbook.h   the header; its top comment states the calling conventions
lib/                    ${shared[*]} (shared), libscarletbook_ffi.a (static)

Static linking: after libscarletbook_ffi.a, link the system libraries in native-static-libs.txt:
  $native
Shared linking: -Llib -lscarletbook_ffi, and ship the shared library with the program.

Compatibility policy and verification of this archive (SHA-256, build provenance):
https://github.com/aclater/scarletbook-rs#c-library
EOF
tar -C dist -czf "$pkg.tar.gz" "$name"
if command -v sha256sum > /dev/null; then
  (cd dist && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")
else
  (cd dist && shasum -a 256 "$name.tar.gz" > "$name.tar.gz.sha256")
fi
ls -l "$pkg/lib" "$pkg.tar.gz"
cat "$pkg.tar.gz.sha256"
