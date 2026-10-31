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
fi
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
