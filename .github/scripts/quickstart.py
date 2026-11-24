# SPDX-License-Identifier: MIT OR Apache-2.0
"""Runs the README's Quick start as written and compares its output with the README's (#412).

Usage: python3 -I quickstart.py [--readme README.md] [--expect-version X.Y.Z] [--keep]

From the README's "## Quick start" section it takes, under "### Rust" and under "### C", the
first ```sh block (the commands) and the first plain ``` block after it (the output the README
shows). Each command block runs unchanged, as a file, with `bash -euo pipefail`, in a new empty
directory of its own under $RUNNER_TEMP (or the system temporary directory), with stdin from
/dev/null. That directory must have no Cargo.toml, rust-toolchain file or .cargo/config
file above it: cargo would join the block's project to that workspace or take its settings,
and the run would not be a reader's. Cargo's own home ($CARGO_HOME, by default ~/.cargo) is
not counted: cargo reads its config wherever it runs, as it does for a reader (#428).

The comparison, per block: the command block's standard output, decoded as UTF-8, against the
README's output block, line by line, after removing trailing whitespace from every line and
dropping trailing empty lines on both sides. Nothing else is normalised: every other
difference, an extra or a missing line included, fails. Standard error is not compared (it
carries cargo's and gh's progress messages); it goes to this script's standard error, so the
log shows it. A block that exits non-zero fails, whatever its output.

The version is the one the Rust block pins (`cargo add scarletbook@=X.Y.Z`). The sentence
"Every command below was run against vX.Y.Z on <date>." must name the same version, and with
--expect-version so must the pin (on a release, the released version).

Prints a unified diff for each block that differs and exits 1 if any block failed, 2 if the
README could not be read as described.
"""
import argparse
import difflib
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile

BLOCKS = ("Rust", "C")


def section(text, heading, level):
    """The lines under `heading` (a Markdown heading of `level` #s) up to the next heading of the same or a higher level."""
    lines = text.splitlines()
    start = next((i for i, line in enumerate(lines) if line == f"{'#' * level} {heading}"), None)
    if start is None:
        raise ValueError(f"no heading {'#' * level} {heading!r}")
    end = next(
        (i for i in range(start + 1, len(lines)) if re.match(rf"#{{1,{level}}} ", lines[i])),
        len(lines),
    )
    return lines[start + 1 : end]


def fences(lines):
    """The fenced code blocks in `lines`, as (info string, body text) in order."""
    out, i = [], 0
    while i < len(lines):
        m = re.fullmatch(r"```(\S*)\s*", lines[i])
        if not m:
            i += 1
            continue
        j = next((k for k in range(i + 1, len(lines)) if lines[k].rstrip() == "```"), None)
        if j is None:
            raise ValueError(f"unclosed code fence: {lines[i]!r}")
        out.append((m.group(1), "".join(line + "\n" for line in lines[i + 1 : j])))
        i = j + 1
    return out


def extract(text):
    """{block name: (commands, expected output)}, and the version the Quick start names."""
    quick = section(text, "Quick start", 2)
    blocks = {}
    for name in BLOCKS:
        found = fences(section("\n".join(quick), name, 3))
        sh = next((k for k, (info, _) in enumerate(found) if info == "sh"), None)
        if sh is None or sh + 1 >= len(found) or found[sh + 1][0] != "":
            raise ValueError(f"### {name}: expected a ```sh block followed by a plain ``` output block")
        blocks[name] = (found[sh][1], found[sh + 1][1])
    pin = re.search(r"^cargo add scarletbook@=(\S+)$", blocks["Rust"][0], flags=re.M)
    if pin is None:
        raise ValueError("### Rust: no `cargo add scarletbook@=X.Y.Z` line")
    said = re.search(r"Every command below was run against v(\S+) on \d{4}-\d{2}-\d{2}\.", "\n".join(quick))
    if said is None:
        raise ValueError("no sentence 'Every command below was run against vX.Y.Z on YYYY-MM-DD.'")
    return blocks, pin.group(1), said.group(1)


def normalise(text):
    lines = [line.rstrip() for line in text.splitlines()]
    while lines and not lines[-1]:
        lines.pop()
    return lines


PROJECT_FILES = ("Cargo.toml", "rust-toolchain", "rust-toolchain.toml")
CONFIG_FILES = ("config", "config.toml")


def cargo_home():
    """Cargo's home directory: $CARGO_HOME, or ~/.cargo."""
    home = os.environ.get("CARGO_HOME")
    return pathlib.Path(home).resolve() if home else (pathlib.Path.home() / ".cargo").resolve()


def cargo_ancestor(path, home):
    """The first project file, or .cargo/config file outside `home`, in `path` or above it, or None.

    A .cargo directory that is cargo's home (`home`) is not counted: cargo reads its config
    from there wherever it runs, so it doesn't make the run differ from a reader's (#428).
    """
    for d in (path, *path.parents):
        for f in PROJECT_FILES:
            if (d / f).exists():
                return d / f
        dot = d / ".cargo"
        if dot.resolve() != home:
            for f in CONFIG_FILES:
                if (dot / f).exists():
                    return dot / f
    return None


def run(name, commands, base, keep):
    """Runs `commands` in a new empty directory under `base`; returns (exit status, stdout)."""
    script_dir = pathlib.Path(tempfile.mkdtemp(prefix=f"quickstart-{name}-script-", dir=base))
    work = pathlib.Path(tempfile.mkdtemp(prefix=f"quickstart-{name}-", dir=base))
    try:
        script = script_dir / "block.sh"
        script.write_text(commands, encoding="utf-8")
        print(f"::group::{name} block in {work}" if os.environ.get("GITHUB_ACTIONS") else f"== {name} block in {work}",
              file=sys.stderr, flush=True)
        done = subprocess.run(["bash", "-euo", "pipefail", str(script)], cwd=work,
                              stdin=subprocess.DEVNULL, stdout=subprocess.PIPE)
        if os.environ.get("GITHUB_ACTIONS"):
            print("::endgroup::", file=sys.stderr, flush=True)
        return done.returncode, done.stdout.decode("utf-8", errors="replace")
    finally:
        shutil.rmtree(script_dir, ignore_errors=True)
        if not keep:
            shutil.rmtree(work, ignore_errors=True)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--readme", default="README.md", type=pathlib.Path)
    ap.add_argument("--expect-version", help="fail unless the Quick start pins this version")
    ap.add_argument("--keep", action="store_true", help="keep the work directories")
    args = ap.parse_args()

    base = pathlib.Path(os.environ.get("RUNNER_TEMP") or tempfile.gettempdir()).resolve()
    if found := cargo_ancestor(base, cargo_home()):
        print(f"quickstart: {found} is above the work directory {base}; set RUNNER_TEMP elsewhere", file=sys.stderr)
        return 2
    try:
        blocks, version, said = extract(args.readme.read_text(encoding="utf-8"))
    except (OSError, ValueError) as e:
        print(f"quickstart: {args.readme}: {e}", file=sys.stderr)
        return 2

    failures = []
    if said != version:
        failures.append(f"the README says the commands were run against v{said}, but they pin {version}")
    if args.expect_version and args.expect_version != version:
        failures.append(f"the Quick start pins {version}, expected {args.expect_version}")
    for name, (commands, expected) in blocks.items():
        status, stdout = run(name, commands, base, args.keep)
        got, want = normalise(stdout), normalise(expected)
        if status != 0:
            failures.append(f"{name} block: exited with status {status}")
        if got != want:
            diff = difflib.unified_diff(want, got, f"{args.readme} ({name} output)", f"actual ({name} output)", lineterm="")
            failures.append(f"{name} block: output differs from the README\n" + "\n".join(diff))
        if status == 0 and got == want:
            print(f"{name} block: ok ({len(got)} lines as in the README)")

    for failure in failures:
        print(f"FAIL: {failure}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
