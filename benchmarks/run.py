# SPDX-License-Identifier: MIT OR Apache-2.0
"""Reference runner for benchmarks/runlist.json (benchmarks/README.md).

Usage:
    python3 -I run.py --kit generic=DIR [--kit v3=DIR] --work DIR [--case ID]... [--smoke]
        > results.jsonl

Each DIR is an unpacked scarletbook-benchkit archive. `--kit BUILD@LABEL=DIR` times kits of
several versions in one alternation (benchmarks/compare.sh); LABEL, by default the kit's
version, goes into each result line. A case whose program or `bench` command a kit lacks (an
older version) is skipped for that kit, and fails the run only if no kit has it, so that a failed
run is always a real failure. The runner writes the run list's images
into WORK with the first kit's scarletbook-synth (the images do not depend on the build),
runs every case's rounds, alternating the builds, and prints one JSON line per timed run on
stdout. Progress goes to stderr. It needs Python 3.8 or later and nothing else: no network,
no privileges. `--smoke` runs every case once on tiny images, to check the run list itself.

Every timed run starts a fresh copy of its program: on a Zen 2 machine one file of `bench`
decoded DST 42 % slower than byte-identical copies of it, every time, until its page cache was
dropped (benchmarks/README.md, "Fresh copies"). Image files show the same effect, so every round
reads a fresh copy of the case's image too.

Before the cases, a self-check (`--no-selftest` skips it) times plain-DSD-to-DSF on a 10-minute
image both through this runner and as a plain shell command on an independent copy, and stops
with exit status 1 if they disagree by more than `--selftest-tolerance` percent (default 5).
"""
import argparse
import json
import os
import platform
import re
import shlex
import shutil
import statistics
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
        "page_bytes": os.sysconf("SC_PAGE_SIZE"),
        "kernel": platform.release(),
    }


def free_by_order():
    """Free memory in KiB by block size, summed over the zones of /proc/buddyinfo: index n is
    blocks of 2^n pages. None where that file is missing. Which free blocks a fresh image copy
    takes may move plain-DSD seconds (README, "Fresh copies"), so each round records this before
    and after writing its copy."""
    kib = os.sysconf("SC_PAGE_SIZE") // 1024
    total = []
    try:
        with open("/proc/buddyinfo") as f:
            for line in f:
                for order, count in enumerate(line.split()[4:]):
                    if order == len(total):
                        total.append(0)
                    total[order] += int(count) * (kib << order)
    except (OSError, ValueError):
        return None
    return total


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
    return [a.replace("{kit}", kit).replace("{image}", image or "").replace("{scratch}", scratch)
            .replace("{evict}", os.path.join(HERE, "evict.py")) for a in argv]


def bench_commands(program):
    """The commands in the usage message of a `bench`, or None if it lists none."""
    run = subprocess.run([program], capture_output=True, text=True)
    return set(re.findall(r"^(?:usage:)?\s*bench\s+(\S+)", run.stderr, re.M)) or None


def kit_lacks(argv, known_commands):
    """What of a case's expanded command the kit does not have, or None if it has it all.

    known_commands caches bench_commands per program. A `bench` that lists no commands is run
    anyway, so that a usage message this cannot read shows up as failed runs, not as blank cells.
    """
    program = argv[0]
    if not os.path.exists(program):
        return os.path.basename(program)
    if os.path.basename(program) != "bench":
        return None
    if program not in known_commands:
        known_commands[program] = bench_commands(program)
    commands = known_commands[program]
    return f"bench {argv[1]}" if commands is not None and argv[1] not in commands else None


def smoke_synth(args):
    """The image recipe with a tiny duration, for --smoke."""
    args = with_minutes(args, "0.2")
    return args + ["--track-minutes", "0.1"]


def with_minutes(args, minutes):
    args = list(args)
    args[args.index("--minutes") + 1] = minutes
    return args


# The run list is code: it names the programs the runner starts and the files it writes.
PROGRAM = re.compile(r"\{kit\}/[A-Za-z0-9_-]+")
ID = re.compile(r"[a-z0-9-]+")


def check(runlist):
    """Rejects a run list whose commands start anything but a program of the kit, or whose
    image or case ids are not plain names (they become file names in the work directory)."""
    errors = []
    for i in runlist["images"]:
        if not ID.fullmatch(str(i["id"])):
            errors.append(f"image id {i['id']!r} is not of the form [a-z0-9-]+")
    for c in runlist["cases"]:
        if not ID.fullmatch(str(c["id"])):
            errors.append(f"case id {c['id']!r} is not of the form [a-z0-9-]+")
        command = c.get("command") or [""]
        if not PROGRAM.fullmatch(command[0]):
            errors.append(f"case {c['id']!r}: the program {command[0]!r} is not {{kit}}/NAME")
        # bench starts what --evict names: only the helper beside this runner.
        for flag, value in zip(command, command[1:] + [""]):
            if flag == "--evict" and value != "{evict}":
                errors.append(f"case {c['id']!r}: --evict {value!r} is not {{evict}}")
    if errors:
        sys.exit("run list rejected:\n  " + "\n  ".join(errors))


def warm(path):
    with open(path, "rb") as f:
        while f.read(1 << 24):
            pass


def stage(image, path):
    """Writes a fresh copy of image at path (new page-cache pages), flushed to disk so that no
    write-back runs while it is timed, and reads it once. A plain read/write copy: a reflink
    would be just as fresh in the page cache, but the copy must not depend on the filesystem."""
    with open(image, "rb") as src, open(path, "wb") as dst:
        while True:
            block = src.read(1 << 24)
            if not block:
                break
            dst.write(block)
        dst.flush()
        os.fsync(dst.fileno())
    warm(path)
    return path


def make_image(path, recipe, kit, scratch):
    if not os.path.exists(path):
        argv = [os.path.join(kit, "scarletbook-synth"), path + ".part"]
        argv += expand(recipe, kit, None, scratch)
        print("+ " + shlex.join(argv), file=sys.stderr)
        subprocess.run(argv, check=True)
        os.replace(path + ".part", path)
    return path


# The self-check's case and image: plain DSD to DSF is the shortest case, where a fixed cost in
# the runner shows most; 10 minutes keeps a run near 0.1 s on a 3950X, long enough to time.
SELFTEST_CASE = "dsf-plain-2ch"
SELFTEST_MINUTES = "10"
SELFTEST_ROUNDS = 8


def selftest(runlist, kit, work, scratch, tolerance):
    """Times SELFTEST_CASE through this runner (a fresh program copy, run_child) against a plain
    shell command (`sh -c 'exec ... >/dev/null 2>&1'`, timed around the shell, also on a fresh
    program copy), on one staged copy of the image, alternating ABBA. Both read the same image
    file, so this checks what the runner adds to a run, not the image's pages. Returns False if
    the median of the per-round ratios is off 1 by more than tolerance percent."""
    case = next(c for c in runlist["cases"] if c["id"] == SELFTEST_CASE)
    recipe = next(i for i in runlist["images"] if i["id"] == case["image"])["synth"]
    image = make_image(os.path.join(work, f"selftest-{SELFTEST_CASE}-{SELFTEST_MINUTES}m.iso"),
                       with_minutes(recipe, SELFTEST_MINUTES), kit, scratch)
    copies = os.path.join(scratch, "selftest")
    os.makedirs(copies, exist_ok=True)
    staged = stage(image, os.path.join(copies, "image.iso"))

    def argv_for(r, side):
        argv = expand(case["command"], kit, staged, scratch)
        argv[0] = shutil.copy2(argv[0], os.path.join(copies, f"{side}-{r}-bench"))
        return argv

    def runner(r):
        wall, code, *_ = run_child(argv_for(r, "runner"), False)
        return wall, code

    def direct(r):
        argv = argv_for(r, "direct")
        start = time.perf_counter()
        code = subprocess.run(["sh", "-c", 'exec "$@" >/dev/null 2>&1', "sh"] + argv).returncode
        return time.perf_counter() - start, code

    ratios = []
    for r in range(SELFTEST_ROUNDS):
        order = (runner, direct) if r % 2 == 0 else (direct, runner)
        got = {f: f(r) for f in order}
        if any(code != 0 for _, code in got.values()):
            print(f"selftest: {SELFTEST_CASE} failed to run", file=sys.stderr)
            return False
        ratios.append(got[runner][0] / got[direct][0])
        print(f"selftest round {r}: runner {got[runner][0]:.4f} s, direct {got[direct][0]:.4f} s",
              file=sys.stderr)
    shutil.rmtree(copies)
    ratio = statistics.median(ratios)
    ok = abs(ratio - 1) * 100 <= tolerance
    print(f"selftest: runner / direct = {ratio:.3f} (median of {len(ratios)} rounds), tolerance "
          f"{tolerance:g} %: {'ok' if ok else 'FAILED, the timings of this run would not be reliable'}",
          file=sys.stderr)
    return ok


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    # A developer override: consumers use the run list inside the verified kit.
    p.add_argument("--runlist", default=os.path.join(HERE, "runlist.json"))
    p.add_argument("--kit", action="append", required=True, metavar="BUILD[@LABEL]=DIR")
    p.add_argument("--work", required=True)
    p.add_argument("--case", action="append", default=[])
    p.add_argument("--smoke", action="store_true")
    p.add_argument("--no-selftest", action="store_true", help="skip the self-check")
    p.add_argument("--selftest-only", action="store_true", help="run only the self-check")
    p.add_argument("--selftest-tolerance", type=float, default=5.0, metavar="PERCENT")
    a = p.parse_args()

    runlist = json.load(open(a.runlist))
    if runlist.get("format_version") != 1:
        sys.exit("unsupported run list format_version")
    check(runlist)
    flags = cpu_flags()
    known = {b["id"]: b for b in runlist["builds"]}
    kits = []
    for spec in a.kit:
        name, _, path = spec.partition("=")
        build, _, label = name.partition("@")
        if build not in known or not path:
            sys.exit(f"--kit {spec}: expected BUILD[@LABEL]=DIR with BUILD one of {sorted(known)}")
        if label and not re.fullmatch(r"[A-Za-z0-9._-]+", label):
            sys.exit(f"--kit {spec}: LABEL must be letters, digits, '.', '_' or '-'")
        missing = [f for f in known[build]["cpu_flags"] if f not in flags]
        if missing:
            print(f"skipping build {build}: the CPU lacks {' '.join(missing)}", file=sys.stderr)
            continue
        path = os.path.abspath(path)
        version = subprocess.run([os.path.join(path, "bench"), "--version"], capture_output=True,
                                 text=True, check=True).stdout.split()[-1]
        kits.append((build, label or version, path, version))
    if not kits:
        sys.exit("no build can run on this CPU")
    os.makedirs(a.work, exist_ok=True)
    scratch = os.path.join(os.path.abspath(a.work), "scratch")
    os.makedirs(scratch, exist_ok=True)

    host = host_facts()
    defaults = runlist["defaults"]
    cases = [c for c in runlist["cases"] if not a.case or c["id"] in a.case]
    images = {i["id"]: i for i in runlist["images"]}

    if not a.no_selftest and not selftest(runlist, kits[0][2], os.path.abspath(a.work), scratch,
                                          a.selftest_tolerance):
        sys.exit(1)
    if a.selftest_only:
        sys.exit(0)

    paths = {}
    for image_id in sorted({c["image"] for c in cases if c["image"]}):
        name = image_id + ("-smoke" if a.smoke else "") + ".iso"
        recipe = images[image_id]["synth"]
        paths[image_id] = make_image(os.path.join(os.path.abspath(a.work), name),
                                     smoke_synth(recipe) if a.smoke else recipe, kits[0][2], scratch)

    failed = False
    known_commands = {}
    for case in cases:
        c = dict(defaults, **case)
        rounds = 1 if a.smoke else c["rounds"]
        warmups = 0 if a.smoke else c["warmup_runs"]
        keep = c["stdout"] == "keep"
        source = paths.get(c["image"])
        copies = os.path.join(scratch, "copies")
        os.makedirs(copies, exist_ok=True)
        # Kits that have the case's program and command (an older version may not).
        able = []
        for k in kits:
            lacks = kit_lacks(expand(c["command"], k[2], source, scratch), known_commands)
            if lacks:
                print(f"skipping {c['id']} for {k[0]}@{k[1]}: the kit has no {lacks}", file=sys.stderr)
            else:
                able.append(k)
        if not able:
            # Every kit skipping a case is a wrong run list or wrong kits, not an older version.
            print(f"error: no kit can run {c['id']}", file=sys.stderr)
            failed = True
            continue
        image = None
        for r in range(rounds):
            free_before = free_by_order()
            if source:
                # A fresh copy of the image for each round, shared by the builds of the round
                # (README, "Fresh copies"): the page-cache pages that hold one image file can
                # make every read of it slower, for as long as that file stays cached. The
                # copies are kept until the case ends, so that none reuses pages another
                # just freed, which were the slow ones on lennon (#248).
                image = stage(source, os.path.join(copies, f"{c['id']}-{r}.iso"))
            free_after = free_by_order()
            if r == 0:
                for build, label, kit, version in able:
                    for _ in range(warmups):
                        run_child(expand(c["command"], kit, image, scratch), False)
            # abba: the builds in order on even rounds, reversed on odd ones.
            order = able if c["alternation"] != "abba" or r % 2 == 0 else able[::-1]
            for build, label, kit, version in order:
                argv = expand(c["command"], kit, image, scratch)
                copy = os.path.join(copies, f"{c['id']}-{build}-{label}-{r}-{os.path.basename(argv[0])}")
                shutil.copy2(argv[0], copy)
                argv[0] = copy
                print(f"+ [{c['id']} {build}@{label} round {r}] " + shlex.join(argv), file=sys.stderr)
                wall, code, ru, stdout, stderr = run_child(argv, keep)
                line = {
                    "case": c["id"],
                    "build": build,
                    "label": label,
                    "version": version,
                    "round": r,
                    "exit": code,
                    "wall_s": round(wall, 6),
                    "user_s": round(ru.ru_utime, 6),
                    "sys_s": round(ru.ru_stime, 6),
                    "cpu_s": round(ru.ru_utime + ru.ru_stime, 6),
                    "max_rss_kib": ru.ru_maxrss,
                    "free_kib_by_order_before": free_before,
                    "free_kib_by_order_after": free_after,
                    "host": host,
                }
                if keep:
                    line["stdout"] = stdout.decode("utf-8", "replace")
                if code != 0:
                    line["stderr"] = stderr.decode("utf-8", "replace")[:4000]
                    failed = True
                print(json.dumps(line, sort_keys=True), flush=True)
        # Removed only after the case, so that no copy gets the pages another just freed.
        shutil.rmtree(copies)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
