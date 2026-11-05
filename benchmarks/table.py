# SPDX-License-Identifier: MIT OR Apache-2.0
"""Prints the Markdown table of benchmarks/RESULTS.md from run.py's JSON lines.

Usage: python3 -I table.py [--runlist runlist.json] [--labels A,B,...] RESULTS.jsonl...

One row per case and build, one column group per label (version), in `--labels` order or else in
the order the labels first appear: the median wall time, the median CPU time (user + system), and, from the
second label on, the ratio of its median wall time to the first label's. For `callpattern`
the figures are its own: the median over rounds of its CPU milliseconds per second of audio,
at 1, 8 and 256 frames per call. A case with a `figure` shows that line of its stdout instead of
wall and CPU time, the median over rounds, in microseconds: what it times is far shorter than
starting a process. Failed runs are left out and counted under the table. So is any
cell whose rounds differ by more than SPREAD_LISTED in wall time, with its range: each round reads
its own copy of the image, and byte-identical copies can differ by more than a median of a few
rounds evens out (README.md, "Fresh copies"). The builds of a round read the same copy; the printed
ratio is of the medians over the rounds, which on such cases can differ slightly from a per-round ratio.
"""
import argparse
import json
import os
import re
import statistics
import sys

HERE = os.path.dirname(os.path.abspath(__file__))

# A cell's rounds differing by more than this (fastest to slowest) are listed under the table.
SPREAD_LISTED = 1.05


def callpattern_ms(stdout):
    """{frames per call: ms CPU per s of audio} from callpattern's table."""
    out = {}
    for m in re.finditer(r"^\| (\d+) \| ([0-9.]+) \| ", stdout or "", re.M):
        out[int(m.group(1))] = float(m.group(2))
    return out


def figure_of(line, name):
    """The value of the `name<TAB>value` line of a run's stdout, or None."""
    m = re.search(rf"^{re.escape(name)}\t([0-9.]+)$", line.get("stdout") or "", re.M)
    return float(m.group(1)) if m else None


def timed(line, figures):
    """What a run measured: its case's figure, or else its wall time."""
    name = figures.get(line["case"])
    return figure_of(line, name) if name else line["wall_s"]


def fmt(x, digits=3):
    return "-" if x is None else f"{x:.{digits}f}"


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--runlist", default=os.path.join(HERE, "runlist.json"))
    p.add_argument("--labels", default="")
    p.add_argument("results", nargs="+")
    a = p.parse_args()
    runlist = json.load(open(a.runlist))
    paths = {c["id"]: c["path"] for c in runlist["cases"]}
    order = [c["id"] for c in runlist["cases"]]
    figures = {c["id"]: c["figure"] for c in runlist["cases"] if "figure" in c}

    lines, failed = [], 0
    for path in a.results:
        with open(path) as f:
            for text in f:
                if text.strip():
                    line = json.loads(text)
                    if line["exit"] != 0:
                        failed += 1
                    else:
                        lines.append(line)
    if not lines:
        sys.exit("no successful runs")
    labels, builds = [x for x in a.labels.split(",") if x], []
    for line in lines:
        label = line.get("label", line["version"])
        if label not in labels:
            labels.append(label)
        if line["build"] not in builds:
            builds.append(line["build"])

    known = [b["id"] for b in runlist["builds"]]
    builds.sort(key=lambda b: known.index(b) if b in known else len(known))
    runs = {}
    for line in lines:
        key = (line["case"], line["build"], line.get("label", line["version"]))
        runs.setdefault(key, []).append(line)

    head = ["Path", "Build"]
    for i, label in enumerate(labels):
        head += [f"{label} wall s", f"{label} CPU s"] + ([f"{label} / {labels[0]}"] if i else [])
    rows = ["| " + " | ".join(head) + " |", "|" + "---|" * len(head)]
    cases = sorted({k[0] for k in runs}, key=lambda c: order.index(c) if c in order else len(order))
    for case in cases:
        for build in builds:
            cells = [paths.get(case, case), build]
            base = None
            for i, label in enumerate(labels):
                rs = runs.get((case, build, label), [])
                if case == "pcm-callpattern":
                    # ms CPU per s of audio at 1 / 8 / 256 frames per call, callpattern's own
                    # figure; the ratio column compares the 8-frame figures.
                    per = [callpattern_ms(r.get("stdout")) for r in rs]
                    med = {n: statistics.median([x[n] for x in per if n in x]) if any(n in x for x in per)
                           else None for n in (1, 8, 256)}
                    wall = med[8]
                    cells += [" / ".join(fmt(med[n], 2) for n in (1, 8, 256)) + " ms per s", "-"]
                elif case in figures:
                    values = [v for v in (timed(r, figures) for r in rs) if v is not None]
                    wall = statistics.median(values) if values else None
                    cells += [fmt(wall, 1) + " µs" if wall is not None else "-", "-"]
                else:
                    wall = statistics.median(r["wall_s"] for r in rs) if rs else None
                    cpu = statistics.median(r["cpu_s"] for r in rs) if rs else None
                    cells += [fmt(wall), fmt(cpu, 2)]
                if i == 0:
                    base = wall
                else:
                    cells.append(fmt(wall / base) if wall and base else "-")
            rows.append("| " + " | ".join(cells) + " |")
    print("\n".join(rows))
    rounds = sorted({len(v) for v in runs.values()})
    host = lines[0]["host"]
    print(f"\nMedians of {'/'.join(map(str, rounds))} rounds per cell; {len(lines)} runs"
          + (f", {failed} failed and left out" if failed else "") + ".")
    print(f"Host: {host.get('cpu')}, {host.get('logical_cpus')} logical CPUs, SMT {host.get('smt')}, "
          f"governor {host.get('governor')}, kernel {host.get('kernel')}.")
    spread = []
    for case in cases:
        for build in builds:
            for label in labels:
                walls = [timed(r, figures) for r in runs.get((case, build, label), [])]
                walls = [w for w in walls if w is not None]
                if walls and min(walls) > 0 and max(walls) / min(walls) > SPREAD_LISTED:
                    unit = f"{min(walls):.1f} to {max(walls):.1f} µs" if case in figures else f"{min(walls):.3f} to {max(walls):.3f} s"
                    spread.append(f"{paths.get(case, case)}, {build}, {label}: {unit}")
    if spread:
        print("\nSpread between rounds above "
              f"{(SPREAD_LISTED - 1) * 100:.0f} %, so read these seconds as a range: " + "; ".join(spread) + ".")
    missing = runlist.get("missing", [])
    if missing:
        print("\nNot measured: " + "; ".join(f"{m['path']} ({m['reason']})" for m in missing) + ".")


if __name__ == "__main__":
    main()
