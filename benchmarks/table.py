# SPDX-License-Identifier: MIT OR Apache-2.0
"""Prints the Markdown table of benchmarks/RESULTS.md from run.py's JSON lines.

Usage: python3 -I table.py [--runlist runlist.json] [--labels A,B,...] RESULTS.jsonl...

One row per case and build, one column group per label (version), in `--labels` order or else in
the order the labels first appear: the median wall time, the median CPU time (user + system), and, from the
second label on, the ratio of its median wall time to the first label's. For `callpattern`
the figures are its own: the median over rounds of its CPU milliseconds per second of audio,
at 1 and at 256 frames per call. Failed runs are left out and counted under the table.
"""
import argparse
import json
import os
import re
import statistics
import sys

HERE = os.path.dirname(os.path.abspath(__file__))


def callpattern_ms(stdout):
    """{frames per call: ms CPU per s of audio} from callpattern's table."""
    out = {}
    for m in re.finditer(r"^\| (\d+) \| ([0-9.]+) \| ", stdout or "", re.M):
        out[int(m.group(1))] = float(m.group(2))
    return out


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
                    # ms CPU per s of audio at 1 / 256 frames per call, callpattern's own figure.
                    per = [callpattern_ms(r.get("stdout")) for r in rs]
                    one = [x[1] for x in per if 1 in x]
                    big = [x[256] for x in per if 256 in x]
                    wall = statistics.median(big) if big else None
                    cells += [f"{fmt(statistics.median(one), 2) if one else '-'} / {fmt(wall, 2)} ms per s",
                              "-"]
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
    missing = runlist.get("missing", [])
    if missing:
        print("\nNot measured: " + "; ".join(f"{m['path']} ({m['reason']})" for m in missing) + ".")


if __name__ == "__main__":
    main()
