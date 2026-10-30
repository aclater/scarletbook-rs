# SPDX-License-Identifier: MIT OR Apache-2.0
"""Checks the C ABI against its baseline, so that a breaking change bumps SB_ABI_VERSION.

Usage: abi-check.py [--write] [--base FILE]

crates/scarletbook-ffi/abi/ holds one baseline per ABI version, abi-N.txt: the
declarations of include/scarletbook.h with comments and layout removed, so that only a
change to the ABI itself (a function, a struct field, an enum value, a macro) shows. The
baseline of the header's SB_ABI_VERSION must exist, be the newest, and match the header
(README, "ABI compatibility"):

- an addition (new functions, types, enum values or macros; no existing declaration
  removed or changed, no struct field added) keeps SB_ABI_VERSION: the same change
  regenerates abi-N.txt with --write;
- any other change (a declaration removed or changed, a struct field added, an enum
  value changed) is breaking: it bumps SB_ABI_VERSION, and --write adds abi-(N+1).txt.

--write writes the baseline of the header's SB_ABI_VERSION. It overwrites an existing
baseline only for an addition; baselines of earlier versions are history.

--base FILE also classifies the header against FILE, the same version's baseline on the
base branch (CI passes it on pull requests), so that rewriting the baseline by hand cannot
hide a breaking change.
"""
import argparse
import difflib
import pathlib
import re
import sys

FFI = pathlib.Path(__file__).resolve().parents[2] / "crates" / "scarletbook-ffi"
HEADER = FFI / "include" / "scarletbook.h"
SNAPSHOTS = FFI / "abi"


def normalise(text):
    """The header's declarations without comments: one statement, enumerator or
    preprocessor line per line, tokens separated by single spaces."""
    text = re.sub(r"/\*.*?\*/", " ", text, flags=re.S)
    text = re.sub(r"//[^\n]*", " ", text)
    out, tokens = [], []

    def flush():
        line, depth = "", 0
        for i, tok in enumerate(tokens):
            glue = i == 0 or tok in ",;)" or tok == "(" or line.endswith(("(", "*", "\n", "= -"))
            line += tok if glue else " " + tok
            depth += {"{": 1, "}": -1}.get(tok, 0)
            if tok in ";{" or (tok == "," and depth > 0):
                line += "\n"
        out.extend(part.strip() for part in line.split("\n"))
        tokens.clear()

    for line in text.splitlines():
        if line.strip().startswith("#"):
            flush()
            out.append(" ".join(line.split()))
        else:
            tokens.extend(re.findall(r"\w+|\S", line))
    flush()
    return "\n".join(line for line in out if line) + "\n"


def classify(old, new):
    """None if `new` equals `old`, "additive" if it only adds to it, else "breaking"."""
    old, new = old.splitlines(), new.splitlines()
    if old == new:
        return None
    # The kind of block each baseline line sits in, to place insertions.
    context, stack = [], []
    for line in old:
        # context[i]: the block holding the gap before line i.
        context.append(stack[-1] if stack else "top")
        if line.startswith("}"):
            stack.pop()
        if line.endswith("{"):
            # Struct and union bodies are layout; enum bodies take new enumerators.
            stack.append(
                "layout" if re.search(r"\b(struct|union)\b", line) else "enum" if "enum" in line else "top"
            )
    context.append("top")
    for op, i1, _, _, _ in difflib.SequenceMatcher(None, old, new, autojunk=False).get_opcodes():
        # Inserted lines are new declarations, or new enumerators; a new struct or union
        # field changes the layout.
        if op in ("replace", "delete") or (op == "insert" and context[i1] == "layout"):
            return "breaking"
    return "additive"


def diff(old, new, name):
    return "".join(difflib.unified_diff(old.splitlines(True), new.splitlines(True), name, "include/scarletbook.h"))


def breaking(old, abi, version, name):
    sys.exit(
        f"breaking change: bump SB_ABI_VERSION and add abi-{version + 1}.txt (bump it in "
        "crates/scarletbook-ffi/src/lib.rs, regenerate the header with cbindgen, then run "
        f"`python3 .github/scripts/abi-check.py --write`)\n{diff(old, abi, name)}"
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--write", action="store_true", help="write the baseline (see above)")
    parser.add_argument("--base", type=pathlib.Path, help="the base branch's baseline of the same version")
    args = parser.parse_args()
    abi = normalise(HEADER.read_text(encoding="utf-8"))
    found = re.search(r"^#define SB_ABI_VERSION (\d+)$", abi, flags=re.M)
    if not found:
        sys.exit(f"{HEADER}: no #define SB_ABI_VERSION")
    version = int(found.group(1))
    if args.base is not None and classify(args.base.read_text(encoding="utf-8"), abi) == "breaking":
        breaking(args.base.read_text(encoding="utf-8"), abi, version, f"{args.base.name} (base branch)")
    baseline = SNAPSHOTS / f"abi-{version}.txt"
    versions = sorted(
        int(m.group(1)) for p in SNAPSHOTS.iterdir() if (m := re.fullmatch(r"abi-(\d+)\.txt", p.name))
    )
    newest = versions[-1] if versions else None
    name = baseline.relative_to(FFI)
    if not baseline.exists():
        if args.write and (newest is None or version == newest + 1):
            baseline.write_text(abi, encoding="utf-8", newline="\n")
            print(f"wrote {name}")
            return
        sys.exit(
            f"no baseline for SB_ABI_VERSION {version}"
            + (f" (the newest is abi-{newest}.txt; SB_ABI_VERSION goes up by one)" if newest else "")
            + ": after a breaking change, bump SB_ABI_VERSION by one and run "
            "`python3 .github/scripts/abi-check.py --write`"
        )
    if version != newest:
        sys.exit(f"SB_ABI_VERSION is {version} but the newest baseline is abi-{newest}.txt")
    old = baseline.read_text(encoding="utf-8")
    change = classify(old, abi)
    if change is None:
        print(f"ABI matches {name} (SB_ABI_VERSION {version})")
    elif change == "additive" and args.write:
        baseline.write_text(abi, encoding="utf-8", newline="\n")
        print(f"additive change: rewrote {name}")
    elif change == "additive":
        sys.exit(
            f"additive change: regenerate the baseline with --write "
            f"(`python3 .github/scripts/abi-check.py --write`) in the same commit\n{diff(old, abi, baseline.name)}"
        )
    else:
        breaking(old, abi, version, baseline.name)


if __name__ == "__main__":
    main()
