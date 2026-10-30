# SPDX-License-Identifier: MIT OR Apache-2.0
"""Checks that the C ABI did not change without an SB_ABI_VERSION bump.

Usage: abi-check.py [--write]

crates/scarletbook-ffi/abi/ holds one snapshot per ABI version, abi-N.txt: the
declarations of include/scarletbook.h with comments and layout removed, so that only a
change to the ABI itself (a function, a struct field, an enum value, a macro) shows. The
check fails unless the snapshot of the header's SB_ABI_VERSION exists, is the newest one
and matches the header. A changed ABI therefore needs a new SB_ABI_VERSION and a new
snapshot; --write creates that snapshot, and refuses to overwrite an existing one
(snapshots of earlier versions are the ABI history and are never edited).
"""
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


def main():
    write = sys.argv[1:] == ["--write"]
    if sys.argv[1:] not in ([], ["--write"]):
        sys.exit(__doc__)
    abi = normalise(HEADER.read_text(encoding="utf-8"))
    found = re.search(r"^#define SB_ABI_VERSION (\d+)$", abi, flags=re.M)
    if not found:
        sys.exit(f"{HEADER}: no #define SB_ABI_VERSION")
    version = int(found.group(1))
    snapshot = SNAPSHOTS / f"abi-{version}.txt"
    versions = sorted(int(p.stem[4:]) for p in SNAPSHOTS.glob("abi-*.txt"))
    if write:
        if snapshot.exists():
            sys.exit(f"{snapshot.name} exists; snapshots are never rewritten. Bump SB_ABI_VERSION.")
        snapshot.write_text(abi, encoding="utf-8", newline="\n")
        print(f"wrote {snapshot.relative_to(FFI)}")
        return
    if not snapshot.exists():
        sys.exit(
            f"no snapshot for SB_ABI_VERSION {version}: run `python3 .github/scripts/abi-check.py --write` "
            "after bumping SB_ABI_VERSION for an ABI change"
        )
    if versions[-1] != version:
        sys.exit(f"SB_ABI_VERSION is {version} but the newest snapshot is abi-{versions[-1]}.txt")
    if snapshot.read_text(encoding="utf-8") != abi:
        sys.exit(
            f"the ABI in include/scarletbook.h differs from {snapshot.relative_to(FFI)}, but SB_ABI_VERSION "
            f"is still {version}. Every ABI change increments SB_ABI_VERSION (README, \"ABI "
            "compatibility\"): bump it in src/lib.rs, regenerate the header and run "
            "`python3 .github/scripts/abi-check.py --write`.\n"
            + "".join(
                difflib.unified_diff(
                    snapshot.read_text(encoding="utf-8").splitlines(True),
                    abi.splitlines(True),
                    snapshot.name,
                    "include/scarletbook.h",
                )
            )
        )
    print(f"ABI matches {snapshot.relative_to(FFI)} (SB_ABI_VERSION {version})")


main()
