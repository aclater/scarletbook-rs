#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Cycles and instructions per decoded symbol for the dst_frames benchmarks, from perf stat
# (user-space counters).
#
#   perf/cycles-per-symbol.sh [NAME...]     # default: every benchmark of dst_frames
#
# NAME is a dst_frames benchmark (`corpus/2ch`, `stress/6ch-o128`, ...;
# `SCARLETBOOK_PERF_BENCH=list` lists them). Each is run twice under perf stat, decoding 0
# and FRAMES frames, and the difference is divided by the symbols decoded, so the start-up
# (reading the corpus, generating the stress set) cancels out.
#
# Environment: CPU (default 2) pins the runs with taskset; FRAMES (default 3000);
# TARGET_DIR (default target/perf); RUSTFLAGS, e.g. "-C target-cpu=znver5".

source "$(dirname "$0")/lib.sh"

CPU=${CPU:-2}
FRAMES=${FRAMES:-3000}
TARGET_DIR=${TARGET_DIR:-$ROOT/target/perf}

exe=$(bench_exe "$ROOT" "$TARGET_DIR")
if [[ $# -eq 0 ]]; then
  mapfile -t names < <(SCARLETBOOK_PERF_BENCH=list "$exe" | cut -f1)
else
  names=("$@")
fi

# counts NAME N: prints "cycles instructions task-clock-ms symbols".
counts() {
  local out
  out=$(mktemp)
  local symbols
  symbols=$(SCARLETBOOK_PERF_BENCH=$1 SCARLETBOOK_PERF_FRAMES=$2 \
    perf stat -x, -o "$out" -e cycles:u,instructions:u,task-clock taskset -c "$CPU" "$exe")
  awk -F, -v s="$symbols" '
    $3 ~ /^cycles/ { c = $1 } $3 ~ /^instructions/ { i = $1 } $3 ~ /^task-clock/ { t = $1 }
    END { print c, i, t, s }' "$out"
  rm -f "$out"
}

printf '%-24s %10s %10s %6s %12s\n' benchmark cycles/sym instr/sym IPC ms/frame
for name in "${names[@]}"; do
  read -r c0 i0 t0 _ < <(counts "$name" 0)
  read -r c1 i1 t1 s1 < <(counts "$name" "$FRAMES")
  awk -v n="$name" -v c="$((c1 - c0))" -v i="$((i1 - i0))" -v t0="$t0" -v t1="$t1" -v s="$s1" -v f="$FRAMES" \
    'BEGIN { printf "%-24s %10.2f %10.2f %6.2f %12.4f\n", n, c / s, i / s, i / c, (t1 - t0) / f }'
done
