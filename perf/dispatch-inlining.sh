#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Checks that the run-time dispatch of the core crate (src/dispatch.rs) still builds its hot
# loops with the dispatched features. A function that stops being inlined into the
# `#[target_feature]` wrapper `avx2::run` is built without them, silently: the output stays
# the same, only slower. This builds `bench` for generic x86-64 (or takes BINARY, such as
# the release libscarletbook_ffi.so, as CI does) and disassembles the wrapper's instances, matched by demangled path, never by hash:
#
# - `avx2::run::<..output::Fill>` and `avx2::run::<..output::ReadAt<..>>`, the output path:
#   the DSF channel separation must be there, as AVX2 shuffles (`vpshufb` on `ymm`);
# - `avx2::run::<..dispatch::Group>`, the DST decoder (also on rayon pool threads, which
#   call `decode_group` themselves): LZCNT or the BMI shifts must be there;
# - the generic instances, `Layout::fill` and `dispatch::decode_group`: neither, since
#   they run on any x86-64.
#
# Only instruction classes are checked, not counts, so that other toolchains pass.
#
#   perf/dispatch-inlining.sh            # exit status 1, with a message per failed check
#
# Environment: BINARY (a built executable or library of the core crate, for generic
# x86-64; default: build `bench` into TARGET_DIR),
# TARGET_DIR (default target/perf-dispatch). Needs objdump (binutils).

source "$(dirname "$0")/lib.sh"

if [[ -n ${BINARY:-} ]]; then
  exe=$BINARY
else
  TARGET_DIR=${TARGET_DIR:-$ROOT/target/perf-dispatch}
  (cd "$ROOT" && CARGO_TARGET_DIR=$TARGET_DIR RUSTFLAGS='' cargo build --locked --release -q -p scarletbook-bench)
  exe=$TARGET_DIR/release/bench
fi
dis=$(mktemp)
trap 'rm -f "$dis"' EXIT
objdump -d --no-show-raw-insn -C "$exe" > "$dis"

# body REGEX: the disassembly of the functions whose demangled name matches REGEX (an
# extended regular expression on the `<name>:` line).
body() {
  awk -v re="$1" '/^[0-9a-f]+ <.*>:$/ { p = ($0 ~ re); next } p' "$dis"
}

AVX='vpshufb.*%ymm'
BMI='(lzcnt|shlx|shrx|sarx|bzhi|andn)[[:space:]]'
fail=0
# exists NAME REGEX: some function matches REGEX; has and lacks NAME REGEX CLASS: their
# instructions include, or don't include, one matching CLASS.
exists() {
  if [[ -z $(body "$2") ]]; then
    echo "FAIL $1: no function matches /$2/ (renamed, or no longer instantiated?)"
    fail=1
    return 1
  fi
}
has() {
  exists "$1" "$2" || return 0
  # Not `body | grep -q`: with pipefail, grep's early exit fails the pipeline.
  if grep -qE "$3" <<< "$(body "$2")"; then
    echo "ok   $1: has /$3/"
  else
    echo "FAIL $1: no instruction matches /$3/: its hot loop is no longer inlined into the dispatch wrapper"
    fail=1
  fi
}
lacks() {
  exists "$1" "$2" || return 0
  if grep -qE "$3" <<< "$(body "$2")"; then
    echo "FAIL $1: has /$3/, which a generic x86-64 function must not"
    fail=1
  else
    echo "ok   $1: no /$3/"
  fi
}
has 'dispatched Layout::fill' '<scarletbook::dispatch::avx2::run::<scarletbook::output::Fill>>:' "$AVX"
has 'dispatched Layout::read_at' '<scarletbook::dispatch::avx2::run::<scarletbook::output::ReadAt<' "$AVX"
has 'dispatched DST group' '<scarletbook::dispatch::avx2::run::<scarletbook::dispatch::Group>>:' "$BMI"
lacks 'generic Layout::fill' '<<scarletbook::output::Layout>::fill>:' "$AVX"
lacks 'generic DST group' '<scarletbook::dispatch::decode_group>:' "$BMI"
lacks 'generic DST group' '<scarletbook::dispatch::decode_group>:' '%ymm'
exit $fail
