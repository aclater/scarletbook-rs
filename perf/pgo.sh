#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Profile-guided build of `bench`, deterministic: an instrumented build is trained on fixed
# inputs, the profiles are merged with llvm-profdata, and `bench` is rebuilt with them.
#
#   perf/pgo.sh TARGET_DIR [all|corpus]
#
# Training inputs, each decoded by the instrumented `bench` with one thread (several
# threads would race on the profile counters and make the profile vary from run to run):
#
# - every corpus image, both areas, `--sink hash` (all frame formats and error paths);
# - synthetic images written by `scarletbook-synth`: 2- and 6-channel DST from corpus
#   frames, 5-channel DST, plain DSD, and (with `all`, the default) 2- and 6-channel DST
#   from the generated stress set of the `dst_frames` benchmark; `--sink null`.
#
# `corpus` leaves the stress-set image out, so that the stress set can measure the result
# on frames the profile has not seen.
#
# Needs the rustup component llvm-tools (`rustup component add llvm-tools-preview`).
# Environment: RUSTFLAGS (e.g. "-C target-cpu=znver5"), PROFILE (cargo profile, default
# release), TARGET (a target triple for --target, default none). The profile is written
# to TARGET_DIR/pgo/merged.profdata and the final `bench` to the usual place in
# TARGET_DIR; `cargo test` with the same RUSTFLAGS (printed at the end), PROFILE, TARGET
# and CARGO_TARGET_DIR reuses that binary.

source "$(dirname "$0")/lib.sh"

out=${1:?usage: perf/pgo.sh TARGET_DIR [all|corpus]}
training=${2:-all}
PROFILE=${PROFILE:-release}
mkdir -p "$out"
out=$(cd "$out" && pwd)
work=$out/pgo
rm -rf "$work"
mkdir -p "$work/raw" "$work/images"

target_args=()
profile_dir=$PROFILE
[[ $PROFILE == dev ]] && profile_dir=debug
bin_dir=$out/$profile_dir
if [[ -n ${TARGET:-} ]]; then
  target_args=(--target "$TARGET")
  bin_dir=$out/$TARGET/$profile_dir
fi

profdata=$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin/llvm-profdata
[[ -x $profdata ]] || { echo "llvm-profdata not found: rustup component add llvm-tools-preview" >&2; exit 1; }

# Training images (an ordinary build of the generator, with the caller's RUSTFLAGS unset).
(cd "$ROOT" && RUSTFLAGS= CARGO_TARGET_DIR=$work/target-tools cargo build --release --locked -q \
  -p scarletbook-conformance --bin scarletbook-synth)
synth=$work/target-tools/release/scarletbook-synth
"$synth" "$work/images/dst-2ch-6ch.iso" --minutes 0.4 --area both --kind dst --mc-channels 6 --track-minutes 0.1 2> /dev/null
"$synth" "$work/images/dst-5ch.iso" --minutes 0.2 --area mc --kind dst --mc-channels 5 --track-minutes 0.1 2> /dev/null
"$synth" "$work/images/dsd.iso" --minutes 0.2 --area both --kind dsd --mc-channels 6 --track-minutes 0.1 2> /dev/null
if [[ $training == all ]]; then
  "$synth" "$work/images/stress.iso" --minutes 0.4 --area both --kind dst --mc-channels 6 \
    --dst-frames stress --track-minutes 0.1 2> /dev/null
fi

# Instrumented build and training. RUSTFLAGS is split on spaces, so the profile directory
# must not contain any.
if [[ $work =~ [[:space:]] ]]; then
  echo "pgo.sh: the target directory ($work) must not contain spaces" >&2
  exit 1
fi
(cd "$ROOT" && RUSTFLAGS="${RUSTFLAGS:-} -Cprofile-generate=$work/raw" CARGO_TARGET_DIR=$work/target-gen \
  cargo build --profile "$PROFILE" --locked -q -p scarletbook-bench "${target_args[@]}")
gen_dir=$work/target-gen/$profile_dir
[[ -n ${TARGET:-} ]] && gen_dir=$work/target-gen/$TARGET/$profile_dir
# Corpus images include rejection cases and images without one of the areas, on which
# `bench` exits with an error by design; at least one run must succeed.
ok=0
for img in "$ROOT"/corpus/images/*.iso; do
  for area in 2ch mc; do
    "$gen_dir/bench" decode "$img" --area "$area" --threads 1 --sink hash > /dev/null 2>&1 && ok=1
  done
done
if (( ok == 0 )); then
  echo "pgo.sh: no corpus training run succeeded" >&2
  exit 1
fi
# The synthetic images hold the areas they were made with, so a failure here is a real one.
for img in "$work"/images/*.iso; do
  areas=(2ch mc)
  [[ $(basename "$img") == dst-5ch.iso ]] && areas=(mc)
  for area in "${areas[@]}"; do
    "$gen_dir/bench" decode "$img" --area "$area" --threads 1 --sink null > /dev/null
  done
done
"$profdata" merge -o "$work/merged.profdata" "$work"/raw/*.profraw

# Optimised build.
use_flags="${RUSTFLAGS:-} -Cprofile-use=$work/merged.profdata"
(cd "$ROOT" && RUSTFLAGS="$use_flags" CARGO_TARGET_DIR=$out \
  cargo build --profile "$PROFILE" --locked -q -p scarletbook-bench "${target_args[@]}")
echo "bench: $bin_dir/bench" >&2
echo "RUSTFLAGS=$use_flags" >&2
