#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Times the run list (benchmarks/runlist.json) against two versions and prints the Markdown
# table of benchmarks/RESULTS.md.
#
#   benchmarks/compare.sh build WORK A B   build kits (generic and x86-64-v3, statically linked,
#                                          as benchkit.yml builds them) from A and B into WORK
#   benchmarks/compare.sh run WORK [run.py options]   time them, alternately, and print the table
#
# A and B are git revisions (exported with `git archive` from the repository this script is
# in) or source directories, such as lennon.sh sync copies. Their labels in the table are the
# names given. `run` uses this checkout's run.py, run list and table.py, so both versions run
# the same cases; a case whose program a version lacks (e.g. `bench dsf` before 0.5.6) is
# left blank for it. Build and run are separate so that, on lennon, the build runs under
# `lennon.sh run` and only the timing under `lennon.sh bench`:
#
#   lennon.sh run  NAME -- benchmarks/compare.sh build ../cmp ~/sb/wt/v055 ~/sb/wt/v056
#   lennon.sh bench NAME -- benchmarks/compare.sh run ../cmp > table.md
#
# Needs cargo with the x86_64-unknown-linux-gnu target and a static glibc (libc.a).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
cmd=${1:?usage: see the header}
work=${2:?WORK}
shift 2

case "$cmd" in
  build)
    mkdir -p "$work"
    work=$(cd "$work" && pwd)
    : > "$work/kits"
    for ref in "$@"; do
      label=$(basename "$ref")
      if [ -d "$ref" ]; then
        src=$(cd "$ref" && pwd)
      else
        src="$work/src-$label"
        mkdir -p "$src"
        git -C "$here/.." archive "$ref" | tar -x -C "$src"
      fi
      for build in generic v3; do
        flags="-C target-feature=+crt-static"
        [ "$build" = v3 ] && flags="$flags -C target-cpu=x86-64-v3"
        out="$work/target-$label-$build"
        kit="$work/kit-$label-$build"
        mkdir -p "$kit"
        (cd "$src" && RUSTFLAGS="$flags" CARGO_TARGET_DIR="$out" cargo build -q --profile dist --locked \
          --target x86_64-unknown-linux-gnu -p scarletbook-bench -p scarletbook-conformance \
          --bin bench --bin scarletbook-synth)
        bin="$out/x86_64-unknown-linux-gnu/dist"
        cp "$bin/bench" "$bin/scarletbook-synth" "$kit/"
        # Older versions have no callpattern example; run.py then skips that case for them.
        if (cd "$src" && RUSTFLAGS="$flags" CARGO_TARGET_DIR="$out" cargo build -q --profile dist --locked \
          --target x86_64-unknown-linux-gnu -p scarletbook-pcm --example callpattern 2> /dev/null); then
          cp "$bin/examples/callpattern" "$kit/"
        fi
        echo "$build@$label=$kit" >> "$work/kits"
      done
    done
    echo "kits in $work/kits:" >&2
    cat "$work/kits" >&2
    ;;
  run)
    work=$(cd "$work" && pwd)
    args=()
    # The last version's kits first: they write the images.
    while read -r kit; do args=(--kit "$kit" "${args[@]}"); done < "$work/kits"
    python3 -I "$here/run.py" "${args[@]}" --work "$work/run" "$@" > "$work/results.jsonl" \
      || echo "some runs failed; the table leaves them out" >&2
    labels=$(sed 's/^[^@]*@\([^=]*\)=.*/\1/' "$work/kits" | awk '!seen[$0]++' | paste -sd,)
    python3 -I "$here/table.py" --labels "$labels" "$work/results.jsonl"
    ;;
  *)
    echo "unknown command $cmd" >&2
    exit 2
    ;;
esac
