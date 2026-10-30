#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Compares the dst_frames benchmarks of two revisions on one pinned CPU.
#
#   perf/compare.sh REV_A REV_B [FILTER]    # e.g. perf/compare.sh main HEAD stress/
#
# Each revision is checked out in a temporary git worktree (`.` means the current working
# tree, uncommitted changes included) and built into its own target directory under
# target/compare/. The two benchmark programs then run alternately (A, B, A, B, ...), each
# run pinned to the same CPU with taskset. For each benchmark the best time of the rounds
# is reported for A and B, with B's change relative to A, and per group (corpus, stress)
# the geometric mean of the ratios. Both revisions must contain the dst_frames benchmark.
#
# Environment: ROUNDS (default 3), CPU (default 2), WARMUP and MEASURE (criterion
# seconds, default 1 and 3), RUSTFLAGS and CARGO_PROFILE_* (apply to both builds).

source "$(dirname "$0")/lib.sh"

rev_a=${1:?usage: perf/compare.sh REV_A REV_B [FILTER]}
rev_b=${2:?usage: perf/compare.sh REV_A REV_B [FILTER]}
filter=${3:-}
ROUNDS=${ROUNDS:-3}
CPU=${CPU:-2}
WARMUP=${WARMUP:-1}
MEASURE=${MEASURE:-3}

work=$ROOT/target/compare
rm -rf "$work/crit" "$work/a" "$work/b"
mkdir -p "$work/crit"
git -C "$ROOT" worktree prune

cleanup() {
  for side in a b; do
    [[ -d $work/$side ]] && git -C "$ROOT" worktree remove --force "$work/$side"
  done
  return 0
}
trap cleanup EXIT

checkout() {
  local side=$1 rev=$2
  if [[ $rev == . ]]; then
    echo "$ROOT"
  else
    git -C "$ROOT" worktree add --quiet --detach "$work/$side" "$rev" >&2
    echo "$work/$side"
  fi
}

declare -A exe
for side in a b; do
  rev=$rev_a
  [[ $side == b ]] && rev=$rev_b
  dir=$(checkout "$side" "$rev")
  echo "building $side: $rev ($(git -C "$dir" rev-parse --short HEAD))" >&2
  exe[$side]=$(bench_exe "$dir" "$work/target-$side")
done

for round in $(seq "$ROUNDS"); do
  for side in a b; do
    echo "round $round/$ROUNDS: $side" >&2
    CRITERION_HOME=$work/crit/$side-$round taskset -c "$CPU" "${exe[$side]}" --bench --noplot --quiet \
      --warm-up-time "$WARMUP" --measurement-time "$MEASURE" $filter > /dev/null 2> "$work/run.log" ||
      { cat "$work/run.log" >&2; exit 1; }
  done
done

python3 -I - "$work/crit" "$rev_a" "$rev_b" <<'PY'
import json, math, pathlib, sys

root, rev_a, rev_b = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3]
best = {}
for est in root.glob("*/**/new/estimates.json"):
    run = est.relative_to(root).parts[0]
    side = run.split("-")[0]
    name = "/".join(est.relative_to(root).parts[1:-2])
    e = json.loads(est.read_text())
    ns = (e.get("slope") or e["mean"])["point_estimate"]
    key = (name, side)
    best[key] = min(best.get(key, math.inf), ns)

names = sorted({n for n, _ in best})
print(f"A = {rev_a}, B = {rev_b}; best of the rounds, microseconds per frame")
print(f"{'benchmark':<26}{'A':>10}{'B':>10}{'B vs A':>9}")
groups = {}
for n in names:
    a, b = best.get((n, "a")), best.get((n, "b"))
    if a is None or b is None:
        print(f"{n:<26} only in {'A' if b is None else 'B'}")
        continue
    groups.setdefault(n.split("/")[0], []).append(b / a)
    print(f"{n:<26}{a / 1e3:>10.1f}{b / 1e3:>10.1f}{100 * (b / a - 1):>+8.1f}%")
for g, ratios in groups.items():
    gm = math.exp(sum(map(math.log, ratios)) / len(ratios))
    print(f"{g + ' (geometric mean)':<46}{100 * (gm - 1):>+8.1f}%")
PY
