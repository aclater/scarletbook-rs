# SPDX-License-Identifier: MIT OR Apache-2.0
"""Reference runner for benchmarks/runlist.json (benchmarks/README.md).

Usage:
    python3 -I run.py --kit generic=DIR [--kit v3=DIR] --work DIR [--case ID]... [--smoke]
        > results.jsonl

Each DIR is an unpacked scarletbook-benchkit archive. The runner writes the run list's images
into WORK with the first kit's scarletbook-synth (the images do not depend on the build),
runs every case's rounds, alternating the builds, and prints one JSON line per timed run on
stdout. Progress goes to stderr. It needs Python 3.8 or later and nothing else: no network,
no privileges. `--smoke` runs every case once on tiny images, to check the run list itself.
"""
import argparse
import json
import os
import platform
import shlex
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))


def cpu_flags():
    try:
        with open("/proc/cpuinfo") as f:
            for line in f:
                if line.startswith("flags"):
                    return set(line.split(":", 1)[1].split())
    except OSError:
        pass
    return set()


def read(path):
    try:
        with open(path) as f:
            return f.read().strip()
    except OSError:
        return None


def host_facts():
    model = None
    try:
        with open("/proc/cpuinfo") as f:
            for line in f:
                if line.startswith("model name"):
                    model = line.split(":", 1)[1].strip()
                    break
    except OSError:
        pass
    mem = None
    try:
        with open("/proc/meminfo") as f:
            mem = int(f.readline().split()[1]) * 1024
    except (OSError, ValueError, IndexError):
        pass
    return {
        "cpu": model,
        "logical_cpus": os.cpu_count(),
        "usable_cpus": len(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else None,
        "smt": read("/sys/devices/system/cpu/smt/active"),
        "governor": read("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor"),
        "mem_bytes": mem,
        "kernel": platform.release(),
    }


def run_child(argv, keep_stdout):
    """Runs argv; returns wall seconds, exit code, rusage, stdout (or None) and stderr."""
    # stdout and stderr go to files, not pipes, so that neither can block the child; the
    # child is reaped with wait4, which returns its own rusage (user and system CPU).
    with tempfile.TemporaryFile() as out, tempfile.TemporaryFile() as err:
        start = time.perf_counter()
        proc = subprocess.Popen(argv, stdout=out if keep_stdout else subprocess.DEVNULL, stderr=err)
        _, status, usage = os.wait4(proc.pid, 0)
        wall = time.perf_counter() - start
        proc.returncode = os.WEXITSTATUS(status) if os.WIFEXITED(status) else -os.WTERMSIG(status)
        out.seek(0)
        err.seek(0)
        return wall, proc.returncode, usage, out.read() if keep_stdout else None, err.read()


def expand(argv, kit, image, scratch):
    return [a.replace("{kit}", kit).replace("{image}", image or "").replace("{scratch}", scratch) for a in argv]


def smoke_synth(args):
    """The image recipe with a tiny duration, for --smoke."""
    args = list(args)
    i = args.index("--minutes")
    args[i + 1] = "0.2"
    return args + ["--track-minutes", "0.1"]


def warm(path):
    with open(path, "rb") as f:
        while f.read(1 << 24):
            pass


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--runlist", default=os.path.join(HERE, "runlist.json"))
    p.add_argument("--kit", action="append", required=True, metavar="BUILD=DIR")
    p.add_argument("--work", required=True)
    p.add_argument("--case", action="append", default=[])
    p.add_argument("--smoke", action="store_true")
    a = p.parse_args()

    runlist = json.load(open(a.runlist))
    if runlist.get("format_version") != 1:
        sys.exit("unsupported run list format_version")
    flags = cpu_flags()
    known = {b["id"]: b for b in runlist["builds"]}
    kits = []
    for spec in a.kit:
        build, _, path = spec.partition("=")
        if build not in known or not path:
            sys.exit(f"--kit {spec}: expected BUILD=DIR with BUILD one of {sorted(known)}")
        missing = [f for f in known[build]["cpu_flags"] if f not in flags]
        if missing:
            print(f"skipping build {build}: the CPU lacks {' '.join(missing)}", file=sys.stderr)
            continue
        kits.append((build, os.path.abspath(path)))
    if not kits:
        sys.exit("no build can run on this CPU")
    os.makedirs(a.work, exist_ok=True)
    scratch = os.path.join(os.path.abspath(a.work), "scratch")
    os.makedirs(scratch, exist_ok=True)

    host = host_facts()
    version = subprocess.run([os.path.join(kits[0][1], "bench"), "--version"], capture_output=True,
                             text=True, check=True).stdout.split()[-1]
    defaults = runlist["defaults"]
    cases = [c for c in runlist["cases"] if not a.case or c["id"] in a.case]
    images = {i["id"]: i for i in runlist["images"]}

    paths = {}
    for image_id in sorted({c["image"] for c in cases if c["image"]}):
        name = image_id + ("-smoke" if a.smoke else "") + ".iso"
        path = os.path.join(os.path.abspath(a.work), name)
        if not os.path.exists(path):
            recipe = images[image_id]["synth"]
            argv = [os.path.join(kits[0][1], "scarletbook-synth"), path + ".part"]
            argv += smoke_synth(recipe) if a.smoke else recipe
            print("+ " + shlex.join(argv), file=sys.stderr)
            subprocess.run(argv, check=True)
            os.replace(path + ".part", path)
        paths[image_id] = path

    failed = False
    for case in cases:
        c = dict(defaults, **case)
        rounds = 1 if a.smoke else c["rounds"]
        warmups = 0 if a.smoke else c["warmup_runs"]
        keep = c["stdout"] == "keep"
        image = paths.get(c["image"])
        if image and c["cache"] == "warm":
            warm(image)
        for build, kit in kits:
            for _ in range(warmups):
                run_child(expand(c["command"], kit, image, scratch), False)
        for r in range(rounds):
            # abba: the builds in order on even rounds, reversed on odd ones.
            order = kits if c["alternation"] != "abba" or r % 2 == 0 else kits[::-1]
            for build, kit in order:
                argv = expand(c["command"], kit, image, scratch)
                print(f"+ [{c['id']} {build} round {r}] " + shlex.join(argv), file=sys.stderr)
                wall, code, ru, stdout, stderr = run_child(argv, keep)
                line = {
                    "case": c["id"],
                    "build": build,
                    "version": version,
                    "round": r,
                    "exit": code,
                    "wall_s": round(wall, 6),
                    "user_s": round(ru.ru_utime, 6),
                    "sys_s": round(ru.ru_stime, 6),
                    "cpu_s": round(ru.ru_utime + ru.ru_stime, 6),
                    "max_rss_kib": ru.ru_maxrss,
                    "host": host,
                }
                if keep:
                    line["stdout"] = stdout.decode("utf-8", "replace")
                if code != 0:
                    line["stderr"] = stderr.decode("utf-8", "replace")[:4000]
                    failed = True
                print(json.dumps(line, sort_keys=True), flush=True)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
