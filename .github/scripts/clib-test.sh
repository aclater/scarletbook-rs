#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Tests a package made by clib-package.sh on the machine it runs on: links the C smoke
# test (crates/scarletbook-ffi/examples/smoke.c) against the static library and against
# the shared library, runs both on corpus images, and runs clib-smoke.py (ctypes) and
# clib-seekcost.py (cold seeks through callbacks) on a corpus image.
#
# Usage: clib-test.sh PACKAGE_DIR   (run from the repository root)
# CC (default cc) is the C compiler, PYTHON (default python3) the Python interpreter.
set -euo pipefail

pkg=$1
cc=${CC:-cc}
python=${PYTHON:-python3}
read -r -a native < "$pkg/native-static-libs.txt"
flags=(-std=c99 -Wall -Wextra -Werror -pedantic -I"$pkg/include")
exe=
case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*) exe=.exe ;;
esac

"$cc" --version | head -n 1
"$cc" "${flags[@]}" crates/scarletbook-ffi/examples/smoke.c "$pkg/lib/libscarletbook_ffi.a" "${native[@]}" \
  -o "smoke-static$exe"
if [[ -n "$exe" ]]; then
  # MinGW: -lscarletbook_ffi finds the import library libscarletbook_ffi.dll.a; the
  # program finds the DLL next to it.
  "$cc" "${flags[@]}" crates/scarletbook-ffi/examples/smoke.c -L"$pkg/lib" -lscarletbook_ffi -o smoke-shared.exe
  cp "$pkg/lib/scarletbook_ffi.dll" .
else
  "$cc" "${flags[@]}" crates/scarletbook-ffi/examples/smoke.c -L"$pkg/lib" -lscarletbook_ffi \
    -Wl,-rpath,"$pkg/lib" -o smoke-shared
  # Linked through the unversioned name, the program records the ABI's (issue #39, M8), and
  # the runs below find the library by that name.
  abi=$(sed -n 's/^#define SB_ABI_VERSION //p' "$pkg/include/scarletbook.h")
  if [[ "$(uname -s)" == Darwin ]]; then
    want=@rpath/libscarletbook_ffi.$abi.dylib
    recorded=$(otool -L smoke-shared | awk '/libscarletbook_ffi/ { print $1 }')
  else
    want=libscarletbook_ffi.so.$abi
    recorded=$(readelf -d smoke-shared | sed -n 's/.*(NEEDED).*\[\(libscarletbook_ffi[^]]*\)\].*/\1/p')
  fi
  echo "smoke-shared needs $recorded"
  if [[ "$recorded" != "$want" ]]; then
    echo "the program linked with the shared library records '$recorded', not $want" >&2
    exit 1
  fi
fi
case "$pkg" in
  *x86_64-unknown-linux-*)
    # Issue #125: the shared library and every member of the static library are marked
    # SHSTK and not IBT (clib-package.sh), so code built with -fcf-protection keeps SHSTK
    # when it links the static library. A relocatable link (-r) merges the property over
    # the objects it pulls from the archive as a program link does, without the build
    # image's C runtime start files, which are older than CET and unmarked.
    lib=$pkg/lib/libscarletbook_ffi
    "$cc" "${flags[@]}" -fcf-protection=full -nostdlib -r crates/scarletbook-ffi/examples/smoke.c "$lib.a" \
      -o smoke-cet.o
    readelf -n "$lib.so" smoke-cet.o | grep 'x86 feature:' || true
    members=$(ar t "$lib.a" | wc -l)
    marked=$(readelf -n "$lib.a" | grep -c 'x86 feature: SHSTK$' || true)
    echo "static library members marked SHSTK: $marked of $members"
    if [[ "$(readelf -n "$lib.so" | grep 'x86 feature:')" != *'x86 feature: SHSTK' ]] \
      || [[ "$(readelf -n smoke-cet.o | grep 'x86 feature:')" != *'x86 feature: SHSTK' ]] \
      || [[ "$marked" != "$members" ]]; then
      echo "the libraries, or the smoke test linked with the static library, are not marked SHSTK (and only SHSTK)" >&2
      exit 1
    fi
    ;;
esac
for id in hostile-base dst-features-2ch-6ch dst-invalid-frames-6ch layout-2054 truncated-in-audio; do
  for kind in static shared; do
    "./smoke-$kind$exe" "corpus/images/$id.iso" > "smoke-$kind.txt"
  done
  # Both programs print the same TOC and decode counts.
  cmp smoke-static.txt smoke-shared.txt
  echo "$id: C smoke test passed (static and shared)"
done

# The sb_* symbols of the shared library's export table.
case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*)
    # The names are the last field of the "[Ordinal/Name Pointer] Table" lines.
    objdump -p "$pkg/lib/scarletbook_ffi.dll" | tr -d '\r' \
      | awk '/\[Ordinal\/Name Pointer\] Table/ { t = 1; next } t && NF == 0 { t = 0 } t { print $NF }' \
      | grep '^sb_' ;;
  Darwin) nm -gU "$pkg/lib/libscarletbook_ffi.dylib" | awk '$2 == "T" { print $3 }' | sed -n 's/^_\(sb_.*\)/\1/p' ;;
  *) nm -D --defined-only "$pkg/lib/libscarletbook_ffi.so" | awk '$2 == "T" { print $3 }' | grep '^sb_' ;;
esac > exports.txt
"$python" .github/scripts/clib-smoke.py "$pkg" exports.txt
"$python" .github/scripts/clib-seekcost.py "$pkg" corpus/images/dst-features-2ch-6ch.iso --repeat 1 --min-gap 0
