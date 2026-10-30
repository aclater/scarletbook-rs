# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Helpers sourced by the scripts in perf/.

set -euo pipefail

# The repository root.
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

# bench_exe DIR TARGET_DIR [BENCH] [PACKAGE]: builds a criterion bench of the checkout at
# DIR (default: dst_frames of scarletbook) into TARGET_DIR and prints the executable's
# path. RUSTFLAGS and CARGO_PROFILE_* from the environment apply.
bench_exe() {
  local dir=$1 target=$2 bench=${3:-dst_frames} package=${4:-scarletbook}
  (cd "$dir" && CARGO_TARGET_DIR=$target cargo bench --locked -p "$package" --bench "$bench" --no-run \
    --message-format=json-render-diagnostics) |
    python3 -I -c '
import json, sys
exe = [m["executable"] for m in map(json.loads, sys.stdin)
       if m.get("reason") == "compiler-artifact" and m.get("executable") and m["target"]["name"] == sys.argv[1]]
if not exe:
    sys.exit("no bench executable " + sys.argv[1])
print(exe[-1])' "$bench"
}
