#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Builds the C library (scarletbook-ffi) for one target and packages it as
# dist/scarletbook-ffi-VERSION-TARGET[-VARIANT].tar.gz with a .sha256 file:
#
#   scarletbook-ffi-VERSION-TARGET[-VARIANT]/
#     include/scarletbook.h
#     lib/        shared library (named after the C ABI on Linux and macOS, with an
#                 unversioned link for the linker), static library (and the DLL import
#                 library on Windows)
#     native-static-libs.txt   what to link after the static library
#     README.txt, LICENSE-MIT, LICENSE-APACHE
#
# Usage: clib-package.sh TARGET "MINIMUM PLATFORM" [VARIANT]   (run from the repository root)
# RUSTFLAGS and MACOSX_DEPLOYMENT_TARGET are taken from the environment.
#
# VARIANT is empty for the baseline build of the target, or `v3` (x86_64 Linux only) for a
# build with -C target-cpu=x86-64-v3, the x86-64 psABI's microarchitecture level 3, added
# to RUSTFLAGS so that every crate is compiled for it (issue #146). Its package name ends
# in -v3; the library inside is the same ABI, and sb_build_info() lists the level's
# target features.
set -euo pipefail

target=$1 platform=$2 variant=${3:-}
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
name="scarletbook-ffi-$version-$target"
case "$variant" in
  "") ;;
  v3)
    if [[ "$target" != x86_64-unknown-linux-* ]]; then
      echo "variant v3 is built for x86_64 Linux only, not $target" >&2
      exit 1
    fi
    export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-C target-cpu=x86-64-v3"
    name+=-v3
    platform+=", on an x86-64-v3 CPU (AVX, AVX2, BMI1, BMI2, F16C, FMA, LZCNT, MOVBE, OSXSAVE and the x86-64-v2 features)"
    ;;
  *) echo "unknown variant $variant" >&2; exit 1 ;;
esac
pkg="dist/$name"

# The shared library's name as a program records it, instead of the build path (macOS)
# or the path it was linked by (Linux). It carries the C ABI version, not the crate's
# (issue #39, M8), so a program built for one ABI does not find a library of another, and
# additions, which keep SB_ABI_VERSION, keep the name. dyld compares compatibility
# versions only as "at least", so on macOS the name is what separates ABIs, and the
# version numbers just agree with it. Windows records the DLL's file name in the program's
# import table and has no such field: the DLL keeps its name.
abi=$(sed -n 's/^#define SB_ABI_VERSION //p' crates/scarletbook-ffi/include/scarletbook.h | tr -d '\r')
if ! [[ "$abi" =~ ^[0-9]+$ ]]; then
  echo "no SB_ABI_VERSION in the header (got '$abi')" >&2
  exit 1
fi
built= versioned=
case "$target" in
  *-apple-*)
    built=libscarletbook_ffi.dylib versioned=libscarletbook_ffi.$abi.dylib
    link=(-C "link-arg=-Wl,-install_name,@rpath/$versioned"
      -C "link-arg=-Wl,-compatibility_version,$abi.0.0" -C "link-arg=-Wl,-current_version,$abi.0.0")
    ;;
  *-linux-*)
    built=libscarletbook_ffi.so versioned=libscarletbook_ffi.so.$abi
    link=(-C "link-arg=-Wl,-soname,$versioned")
    ;;
  *) link=() ;;
esac
# x86-64 Linux: mark the libraries compatible with the CET shadow stack (SHSTK), the CET
# feature Linux enforces in user space, so that a SHSTK-marked program keeps it when it
# links them (issue #125). The marking needs no code generation, only that no code
# returns anywhere but to its caller: no dependency switches stacks or longjmps, and
# neither library carries an unwinder of its own (`_Unwind_RaiseException` is undefined in
# both targets' archives, and the .so needs libgcc_s.so.1), so a panic unwinds through the
# consumer's system libgcc_s, which follows the shadow stack where glibc enforces it (GCC
# builds it with --enable-cet there). Indirect branch
# tracking (IBT) is NOT marked: it needs ENDBR64 at every indirect branch target, and
# stable Rust's precompiled std has none, so an IBT mark would crash under enforcement.
shstk=
case "$target" in
  x86_64-unknown-linux-*) shstk=1 link+=(-C link-arg=-Wl,-z,shstk) ;;
esac
# Profile dist: release with one codegen unit, as for the generic `bench` asset.
CARGO_TERM_COLOR=never cargo rustc --profile dist --locked -p scarletbook-ffi --lib --target "$target" \
  -- --print native-static-libs "${link[@]}" 2>&1 | tee build.log
native=$(sed -n 's/^.*native-static-libs: //p' build.log | tail -n 1)
if [[ -z "$native" ]]; then
  echo "no native-static-libs line in the build output" >&2
  exit 1
fi

out="${CARGO_TARGET_DIR:-target}/$target/dist"
rm -rf "$pkg"
mkdir -p "$pkg/include" "$pkg/lib"
cp crates/scarletbook-ffi/include/scarletbook.h "$pkg/include/"
if [[ -n "$versioned" ]]; then
  # The file a program needs at run time, and the link -lscarletbook_ffi finds at link time.
  cp "$out/$built" "$pkg/lib/$versioned"
  ln -s "$versioned" "$pkg/lib/$built"
  shared=("$versioned" "$built")
  shared_use="Shared linking: -Llib -lscarletbook_ffi. The program then needs $versioned at run time, so ship
that file with it ($built only links to it, for the linker)."
else
  shared=(scarletbook_ffi.dll libscarletbook_ffi.dll.a)
  cp "$out/${shared[0]}" "$out/${shared[1]}" "$pkg/lib/"
  shared_use="Shared linking: -Llib -lscarletbook_ffi, and ship the shared library with the program."
fi
cp "$out/libscarletbook_ffi.a" "$pkg/lib/"
if [[ -n "$shstk" ]]; then
  # The linker ANDs the property over every object it pulls from the static library, so
  # each member needs it (rustc cannot emit it on stable).
  "${PYTHON:-python3}" .github/scripts/clib-shstk.py "$pkg/lib/libscarletbook_ffi.a"
fi
printf '%s\n' "$native" > "$pkg/native-static-libs.txt"
cp LICENSE-MIT LICENSE-APACHE "$pkg/"
cat > "$pkg/README.txt" <<EOF
scarletbook C library $version for $target${variant:+ ($variant)}, built from ${GITHUB_SHA:-a local checkout}.
Runs on: $platform.
C ABI version (SB_ABI_VERSION, sb_abi_version()): $abi.

include/scarletbook.h   the header; its top comment states the calling conventions
lib/                    ${shared[*]} (shared), libscarletbook_ffi.a (static)

Static linking: after libscarletbook_ffi.a, link the system libraries in native-static-libs.txt:
  $native
$shared_use

Compatibility policy and verification of this archive (SHA-256, build provenance):
https://github.com/aclater/scarletbook-rs#c-library
EOF
tar -C dist -czf "$pkg.tar.gz" "$name"
# "<hash>  <file>" (two spaces, text mode) on every platform: Git Bash's sha256sum would
# otherwise write binary mode, "<hash> *<file>".
if command -v sha256sum > /dev/null; then
  hash=$(sha256sum "dist/$name.tar.gz" | cut -d ' ' -f 1)
else
  hash=$(shasum -a 256 "dist/$name.tar.gz" | cut -d ' ' -f 1)
fi
printf '%s  %s\n' "$hash" "$name.tar.gz" > "dist/$name.tar.gz.sha256"
grep -Eq "^[0-9a-f]{64}  $name\.tar\.gz\$" "dist/$name.tar.gz.sha256"
ls -l "$pkg/lib" "$pkg.tar.gz"
cat "$pkg.tar.gz.sha256"
