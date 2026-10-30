#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Measures `bench` built in several release configurations (issue #32): single-thread
# decoding time per frame, pinned, best of several alternating rounds.
#
#   perf/build-configs.sh
#
# Configurations (CONFIGS, space-separated):
#   base       the release profile (overflow-checks = true)
#   nocheck    base with overflow-checks = false (for information only)
#   lto        lto = "fat"
#   cgu1       codegen-units = 1
#   lto-cgu1   both
#   pgo-base   base + profile-guided (perf/pgo.sh, trained on corpus + stress images)
#   pgo-cgu1   cgu1 + profile-guided
#   pgo        lto-cgu1 + profile-guided
#   pgo-corpus lto-cgu1 + profile-guided, trained without the stress set (held out)
# CPU levels (LEVELS): generic, x86-64-v3, znver5 (-C target-cpu=...).
#
# Workloads: `bench decode IMAGE --area 2ch|mc --threads 1 --sink null` on two synthetic
# images of MINUTES minutes per area (2 and 6 channels): `stress` (frames of the generated
# stress set) and `corpus` (the corpus's DST frames, repeated).
#
# Environment: CPU (default 2), ROUNDS (default 5), MINUTES (default 1).

source "$(dirname "$0")/lib.sh"

CPU=${CPU:-2}
ROUNDS=${ROUNDS:-5}
MINUTES=${MINUTES:-1}
CONFIGS=${CONFIGS:-base nocheck lto cgu1 lto-cgu1 pgo-base pgo-cgu1 pgo pgo-corpus}
LEVELS=${LEVELS:-generic x86-64-v3 znver5}

work=$ROOT/target/build-configs
mkdir -p "$work/bin" "$work/images"
frames=$(python3 -c "print(round($MINUTES * 60 * 75))")

(cd "$ROOT" && cargo build --release --locked -q -p scarletbook-conformance --bin scarletbook-synth)
synth=$ROOT/target/release/scarletbook-synth
"$synth" "$work/images/stress.iso" --minutes "$MINUTES" --area both --kind dst --mc-channels 6 \
  --dst-frames stress 2> /dev/null
"$synth" "$work/images/corpus.iso" --minutes "$MINUTES" --area both --kind dst --mc-channels 6 2> /dev/null

for level in $LEVELS; do
  flags=
  [[ $level != generic ]] && flags="-C target-cpu=$level"
  for config in $CONFIGS; do
    dir=$work/target-$level-$config
    echo "building $level $config" >&2
    env=(RUSTFLAGS="$flags" CARGO_TARGET_DIR="$dir")
    case $config in
      nocheck) env+=(CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS=false) ;;
      lto) env+=(CARGO_PROFILE_RELEASE_LTO=fat) ;;
      cgu1 | pgo-cgu1) env+=(CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1) ;;
      lto-cgu1 | pgo | pgo-corpus) env+=(CARGO_PROFILE_RELEASE_LTO=fat CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1) ;;
    esac
    case $config in
      pgo | pgo-base | pgo-cgu1) env "${env[@]}" "$ROOT/perf/pgo.sh" "$dir" all 2> /dev/null ;;
      pgo-corpus) env "${env[@]}" "$ROOT/perf/pgo.sh" "$dir" corpus 2> /dev/null ;;
      *) (cd "$ROOT" && env "${env[@]}" cargo build --release --locked -q -p scarletbook-bench) ;;
    esac
    cp "$dir/release/bench" "$work/bin/$level-$config"
  done
done

results=$work/results.tsv
: > "$results"
for round in $(seq "$ROUNDS"); do
  echo "round $round/$ROUNDS" >&2
  for level in $LEVELS; do
    for config in $CONFIGS; do
      for image in stress corpus; do
        for area in 2ch mc; do
          start=$(date +%s.%N)
          taskset -c "$CPU" "$work/bin/$level-$config" decode "$work/images/$image.iso" --area "$area" \
            --threads 1 --sink null
          end=$(date +%s.%N)
          printf '%s\t%s\t%s\t%s\t%s\n' "$round" "$level" "$config" "$image-$area" "$(awk -v a="$start" -v b="$end" 'BEGIN { print b - a }')" >> "$results"
        done
      done
    done
  done
done

python3 -I - "$results" "$frames" <<'PY'
import sys
best = {}
for line in open(sys.argv[1]):
    _, level, config, load, s = line.split("\t")
    k = (level, config, load)
    best[k] = min(best.get(k, 1e9), float(s))
frames = int(sys.argv[2])
loads = ["stress-2ch", "stress-mc", "corpus-2ch", "corpus-mc"]
levels = list(dict.fromkeys(k[0] for k in best))
configs = list(dict.fromkeys(k[1] for k in best))
print(f"ms per frame, one thread, best of the rounds ({frames} frames per area; mc = 6 channels); change against base")
print(f"{'level':<11}{'config':<12}" + "".join(f"{l:>20}" for l in loads))
for level in levels:
    for config in configs:
        cells = []
        for l in loads:
            v, b = best[(level, config, l)], best[(level, "base", l)]
            cells.append(f"{1e3 * v / frames:8.4f} ({100 * (v / b - 1):+5.1f}%)")
        print(f"{level:<11}{config:<12}" + "".join(f"{c:>20}" for c in cells))
PY
