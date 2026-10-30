#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# A flame graph of DstDecoder::decode on one dst_frames benchmark, with perf record and
# inferno.
#
#   perf/flamegraph.sh NAME [OUT.svg]       # e.g. perf/flamegraph.sh stress/2ch-o128
#
# Needs perf and inferno (`cargo install --locked inferno --version 0.12.8`). The bench is
# built with debug info and frame pointers into its own target directory
# (target/perf-fp). Only stacks through `<DstDecoder>::decode` are kept (FOCUS= keeps all,
# including the start-up that reads the corpus and generates the stress set).
#
# Environment: CPU (default 2), FRAMES (default 10000), FREQ (sampling Hz, default 4000),
# CALL_GRAPH (fp or dwarf, default fp), FOCUS (default `DstDecoder>::decode`), RUSTFLAGS
# (appended to the frame-pointer flag).

source "$(dirname "$0")/lib.sh"

name=${1:?usage: perf/flamegraph.sh NAME [OUT.svg]}
out=${2:-flamegraph-${name//\//-}.svg}
CPU=${CPU:-2}
FRAMES=${FRAMES:-10000}
FREQ=${FREQ:-4000}
CALL_GRAPH=${CALL_GRAPH:-fp}
FOCUS=${FOCUS-DstDecoder>::decode}

for tool in perf inferno-collapse-perf inferno-flamegraph; do
  command -v "$tool" > /dev/null || { echo "$tool not found (see the header of $0)" >&2; exit 1; }
done

exe=$(RUSTFLAGS="-C force-frame-pointers=yes ${RUSTFLAGS:-}" CARGO_PROFILE_BENCH_DEBUG=true \
  bench_exe "$ROOT" "$ROOT/target/perf-fp")
data=$(mktemp --suffix=.perf.data)
trap 'rm -f "$data"' EXIT
SCARLETBOOK_PERF_BENCH=$name SCARLETBOOK_PERF_FRAMES=$FRAMES \
  perf record -q -o "$data" -F "$FREQ" --call-graph "$CALL_GRAPH" -- taskset -c "$CPU" "$exe" > /dev/null
perf script -i "$data" 2> /dev/null | inferno-collapse-perf |
  { if [[ -n $FOCUS ]]; then grep -F -- "$FOCUS"; else cat; fi; } |
  inferno-flamegraph --title "dst_frames $name" > "$out"
echo "$out"
