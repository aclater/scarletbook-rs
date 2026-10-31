# SPDX-License-Identifier: MIT OR Apache-2.0
"""Tests abi-check.py on edited copies of the header and its baseline.

Usage: abi-check-test.py   (run from anywhere; exits non-zero on a failed case)

Each case copies abi-check.py, include/scarletbook.h and abi/ into a temporary tree,
edits the copy, runs the check there and compares the first line of its output.
"""
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
FFI = pathlib.Path("crates") / "scarletbook-ffi"
SCRIPT = pathlib.Path(".github") / "scripts" / "abi-check.py"
VERSION = int(re.search(r"^#define SB_ABI_VERSION (\d+)$",
                        (ROOT / FFI / "include" / "scarletbook.h").read_text(encoding="utf-8"), re.M).group(1))
BASELINE = f"abi-{VERSION}.txt"
NEXT = f"#define SB_ABI_VERSION {VERSION + 1}"
CURRENT = f"#define SB_ABI_VERSION {VERSION}"

ADDITIVE = "additive change"
BREAKING = "breaking change"
OK = "ABI matches"


def tree():
    tmp = pathlib.Path(tempfile.mkdtemp())
    for rel in (SCRIPT, FFI / "include" / "scarletbook.h"):
        (tmp / rel).parent.mkdir(parents=True, exist_ok=True)
        shutil.copy(ROOT / rel, tmp / rel)
    shutil.copytree(ROOT / FFI / "abi", tmp / FFI / "abi")
    return tmp


def edit(tmp, old, new):
    header = tmp / FFI / "include" / "scarletbook.h"
    text = header.read_text(encoding="utf-8")
    if not re.search(old, text, flags=re.M):
        raise SystemExit(f"test setup: {old!r} not in the header")
    header.write_text(re.sub(old, new, text, count=1, flags=re.M), encoding="utf-8")


def run(tmp, *args):
    out = subprocess.run([sys.executable, str(tmp / SCRIPT), *args], capture_output=True, text=True)
    return (out.stdout + out.stderr).splitlines()[0], out.returncode


ADD_FUNCTION = (r"^void sb_disc_free\(struct SbDisc \*disc\);$", r"\g<0>\nint32_t sb_new(void);")
FIELD_IN_DISC_INFO = (r"^  uint32_t warning_count;\n", r"\g<0>  uint32_t extra;\n")

# (name, edits to the header, extra setup, arguments, expected start of output, expected exit)
CASES = [
    ("unchanged", [], None, [], OK, 0),
    ("comment only", [(r"^ \* Conventions, for every function:", " * Conventions, for each function:")],
     None, [], OK, 0),
    ("new function", [ADD_FUNCTION], None, [], ADDITIVE, 1),
    ("new status code", [(r"^  SB_STATUS_PANIC = -14,$", r"\g<0>\n  SB_STATUS_NEW = -15,")],
     None, [], ADDITIVE, 1),
    ("new macro", [(rf"^{CURRENT}$", r"\g<0>\n#define SB_NEW 3")], None, [], ADDITIVE, 1),
    ("new struct", [(r"^typedef struct SbWarning \{", "typedef struct SbNew {\n  int32_t a;\n} SbNew;\n\n\\g<0>")],
     None, [], ADDITIVE, 1),
    ("new union", [(r"^typedef struct SbWarning \{", "typedef union SbNew {\n  int32_t a;\n} SbNew;\n\n\\g<0>")],
     None, [], ADDITIVE, 1),
    ("field at the end of a struct", [FIELD_IN_DISC_INFO], None, [], BREAKING, 1),
    ("field in the middle of a struct", [(r"^  uint32_t start_lsn;\n", r"  uint32_t x;\n\g<0>")],
     None, [], BREAKING, 1),
    ("changed signature", [(r"^uint32_t sb_abi_version\(void\);", "uint64_t sb_abi_version(void);")],
     None, [], BREAKING, 1),
    ("renamed parameter", [(r"^void sb_disc_free\(struct SbDisc \*disc\);", "void sb_disc_free(struct SbDisc *d);")],
     None, [], BREAKING, 1),
    ("removed function", [(r"^void sb_disc_free\(struct SbDisc \*disc\);\n", "")], None, [], BREAKING, 1),
    ("changed enum value", [(r"^  SB_STATUS_PANIC = -14,", "  SB_STATUS_PANIC = -15,")], None, [], BREAKING, 1),
    ("version skipped", [(rf"^{CURRENT}$", f"#define SB_ABI_VERSION {VERSION + 2}")],
     None, ["--write"], "no baseline", 1),
    ("bump without a baseline", [(rf"^{CURRENT}$", NEXT)], None, [], "no baseline", 1),
    ("breaking change with a bump, --write", [FIELD_IN_DISC_INFO, (rf"^{CURRENT}$", NEXT)], None, ["--write"],
     "wrote", 0),
    ("addition, --write", [ADD_FUNCTION], None, ["--write"], "additive change: rewrote", 0),
    ("breaking change, --write", [FIELD_IN_DISC_INFO], None, ["--write"], BREAKING, 1),
    ("stray file in abi/", [], "stray", [], OK, 0),
    # A breaking change hidden by rewriting the baseline by hand: caught against the base
    # branch's baseline (#42 review).
    ("breaking change, baseline rewritten", [FIELD_IN_DISC_INFO], "rewrite", [], OK, 0),
    ("breaking change, baseline rewritten, --base", [FIELD_IN_DISC_INFO], "rewrite", ["--base"], BREAKING, 1),
    ("addition, baseline regenerated, --base", [ADD_FUNCTION], "rewrite", ["--base"], OK, 0),
]


def main():
    failed = 0
    for name, edits, setup, args, want, code in CASES:
        tmp = tree()
        try:
            base = tmp / "base.txt"
            shutil.copy(tmp / FFI / "abi" / BASELINE, base)
            for old, new in edits:
                edit(tmp, old, new)
            if setup == "stray":
                (tmp / FFI / "abi" / f"{BASELINE}.orig").write_text("x", encoding="utf-8")
            if setup == "rewrite":
                # What --write would refuse, done by hand: the header's normalised form.
                spec = {"__name__": "abi_check", "__file__": str(tmp / SCRIPT)}
                exec((tmp / SCRIPT).read_text(encoding="utf-8"), spec)
                header = (tmp / FFI / "include" / "scarletbook.h").read_text(encoding="utf-8")
                (tmp / FFI / "abi" / BASELINE).write_text(spec["normalise"](header), encoding="utf-8")
            args = [x for a in args for x in (["--base", str(base)] if a == "--base" else [a])]
            first, rc = run(tmp, *args)
            ok = first.startswith(want) and rc == code
            failed += not ok
            print(f"{'ok  ' if ok else 'FAIL'} {name}: exit {rc}, {first[:100]}")
        finally:
            shutil.rmtree(tmp)
    if failed:
        sys.exit(f"{failed} case(s) failed")
    print(f"all {len(CASES)} cases passed")


main()
