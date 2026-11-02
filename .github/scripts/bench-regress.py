# SPDX-License-Identifier: MIT OR Apache-2.0
"""The CI benchmark regression subset (issue #226): a base and a head build, timed alternately
on the same machine, compared as ratios.

Usage: bench-regress.py BASE_BIN_DIR HEAD_BIN_DIR WORK_DIR [--rounds N] [--tolerance T]

Each BIN_DIR holds `bench` and `callpattern` (the scarletbook-pcm example) of one build; HEAD_BIN_DIR
also holds `scarletbook-synth`, which writes the small images into WORK_DIR. Every case runs
`--rounds` times per build, in ABBA order (base, head; head, base; ...), after one untimed run
of each. Every timed run starts a fresh copy of its program (below). The figure per run is the
child's CPU time (user + system, from wait4), which a
descheduled process does not accumulate; every case is single-threaded. A case regresses when
median(head) / median(base) exceeds 1 + tolerance. A case the base build cannot run (a
command it does not have yet) is skipped with a notice.

Fresh copies: on lennon (3950X), one file of `bench` decoded DST 42 % slower than byte-identical
copies of it (1.77 s against 1.24 s of CPU, the same instruction count), every time, until its
page cache was dropped: the physical pages that hold a program's code can cost that much. A
copy per run turns that into noise that the median absorbs, instead of a fixed offset of one
build.

Absolute times vary several-fold between runner machines; the ratio of two builds timed in
turns on one machine does not (benchmarks/README.md of #226 has the noise measured).
"""
import argparse
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

# id, image (scarletbook-synth arguments after the output path, or None), command; `{bin}`
# and `{image}` are filled in.
IMAGES = {
    "dst-2ch": ["--minutes", "1", "--area", "2ch", "--kind", "dst", "--dst-frames", "stress"],
    "dsd-2ch": ["--minutes", "20", "--area", "2ch", "--kind", "dsd"],
}
CASES = [
    ("dst-decode-2ch-t1", "dst-2ch",
     ["{bin}/bench", "decode", "{image}", "--area", "2ch", "--threads", "1", "--sink", "null"]),
    ("dsf-plain-2ch-t1", "dsd-2ch",
     ["{bin}/bench", "dsf", "{image}", "--area", "2ch", "--threads", "1", "--out", "/dev/null"]),
    ("pcm-callpattern", None, ["{bin}/callpattern", "60", "3", "1,256"]),
]


def run(argv):
    """CPU seconds of one run of argv, or None if it failed."""
    with tempfile.TemporaryFile() as err:
        try:
            proc = subprocess.Popen(argv, stdout=subprocess.DEVNULL, stderr=err)
        except OSError as e:  # the build has no such program
            sys.stderr.write(f"{argv[0]}: {e}\n")
            return None
        _, status, usage = os.wait4(proc.pid, 0)
        proc.returncode = os.WEXITSTATUS(status) if os.WIFEXITED(status) else -1
        if proc.returncode != 0:
            err.seek(0)
            sys.stderr.write(err.read().decode("utf-8", "replace")[:2000])
            return None
    return usage.ru_utime + usage.ru_stime


def main():
    p = argparse.ArgumentParser()
    p.add_argument("base")
    p.add_argument("head")
    p.add_argument("work")
    p.add_argument("--rounds", type=int, default=7)
    p.add_argument("--tolerance", type=float, default=0.15)
    a = p.parse_args()
    builds = [("base", os.path.abspath(a.base)), ("head", os.path.abspath(a.head))]
    os.makedirs(a.work, exist_ok=True)
    images = {}
    for name, args in IMAGES.items():
        path = os.path.join(os.path.abspath(a.work), name + ".iso")
        if not os.path.exists(path):
            subprocess.run([os.path.join(builds[1][1], "scarletbook-synth"), path] + args, check=True,
                           stderr=subprocess.DEVNULL)
        with open(path, "rb") as f:  # into the page cache
            while f.read(1 << 24):
                pass
        images[name] = path

    rows, failed = [], False
    start = time.monotonic()
    for case, image, command in CASES:
        argv = {b: [s.replace("{bin}", d).replace("{image}", images.get(image, "")) for s in command]
                for b, d in builds}
        if run(argv["base"]) is None:
            print(f"::notice::{case}: the base build cannot run it; skipped")
            rows.append((case, "-", "-", "-", "skipped (base)"))
            continue
        if run(argv["head"]) is None:
            print(f"::error::{case}: the head build fails")
            rows.append((case, "-", "-", "-", "head fails"))
            failed = True
            continue
        times = {"base": [], "head": []}
        copies = os.path.join(os.path.abspath(a.work), "copies")
        os.makedirs(copies, exist_ok=True)
        for r in range(a.rounds):
            for b, _ in (builds if r % 2 == 0 else builds[::-1]):
                # A fresh copy of the program, in fresh page-cache pages (see above).
                prog = os.path.join(copies, f"{b}-{r}-{os.path.basename(argv[b][0])}")
                shutil.copy2(argv[b][0], prog)
                times[b].append(run([prog] + argv[b][1:]))
        # Removed only now, so that a copy does not get the pages its predecessor just freed.
        shutil.rmtree(copies)
        if None in times["base"] + times["head"]:
            print(f"::error::{case}: a timed run failed")
            failed = True
            continue
        base, head = statistics.median(times["base"]), statistics.median(times["head"])
        ratio = head / base
        verdict = "ok"
        if ratio > 1 + a.tolerance:
            verdict = "REGRESSION"
            failed = True
            print(f"::error::{case}: head takes {ratio:.3f} x the base's CPU time "
                  f"(tolerance {1 + a.tolerance:.2f})")
        # Slowest over fastest run of each build: the noise on this runner.
        spread = "/".join(f"{max(t) / min(t) - 1:.1%}" for t in times.values())
        rows.append((case, f"{base:.3f}", f"{head:.3f}", f"{ratio:.3f}", f"{verdict} (spread {spread})"))

    table = ["| Case | Base CPU s (median) | Head CPU s (median) | Head / base | Verdict |",
             "|---|---|---|---|---|"] + ["| " + " | ".join(r) + " |" for r in rows]
    table.append(f"\n{a.rounds} rounds per build, ABBA order, tolerance {a.tolerance:.0%}; "
                 f"{time.monotonic() - start:.0f} s of timing.")
    print("\n".join(table))
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a") as f:
            f.write("\n".join(table) + "\n")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
