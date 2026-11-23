# SPDX-License-Identifier: MIT OR Apache-2.0
"""Tests of linkcheck.py on fixtures in a temporary directory."""

import os
import subprocess
import sys
import tempfile
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "linkcheck.py"

# Headings and anchors the links below point at.
TARGET = """# Target

## Install & build `Result<(), Error>` (#12)

## Repeat

## Repeat

## A [linked](https://example.invalid) heading

Setext heading
--------------

<a id="explicit"></a>
<a name="named"></a>

```
## Not a heading
```
"""

GOOD = """# Good

[file](target.md), [dir](sub/), [dir](sub), [anchor](target.md#target),
[punctuation](target.md#install--build-result-error-12),
[repeat](target.md#repeat) [repeat](target.md#repeat-1), [link text](target.md#a-linked-heading),
[setext](target.md#setext-heading), [id](target.md#explicit), [name](target.md#named),
[self](#good), [titled](target.md "Title"), [angle](<sub/with space.md>),
[encoded](sub/with%20space.md), [root](/target.md#target), [up](sub/../target.md),
[line](code.rs#L2), [lines](code.rs#L1-L3), ![image](img.png), [`a[b]`](target.md),
<a href="target.md#repeat">html</a> <img src="img.png">
[https](https://example.invalid/missing) [mail](mailto:x@example.invalid)
`[code span](missing.md)` and <!-- [comment](missing.md) -->

```
[fenced](missing.md)
```

[ref]: target.md#repeat-1
"""

BROKEN = [
    ("[x](missing.md)", "no such file"),
    ("[x](target.md#missing)", "no such anchor"),
    ("[x](target.md#not-a-heading)", "no such anchor"),
    ("[x](target.md#repeat-2)", "no such anchor"),
    ("[x](#missing)", "no such anchor"),
    ("[x](sub/#a)", "anchor into a directory"),
    ("[x](code.rs#main)", "is not a line"),
    ("[x](code.rs#L9)", "no such line"),
    ("[x](../outside.md)", "points outside the repository"),
    ("![x](missing.png)", "no such file"),
    ('<a href="missing.md">x</a>', "no such file"),
    ("[ref]: missing.md", "no such file"),
]


def run(cwd, *args):
    env = {k: v for k, v in os.environ.items() if k != "GITHUB_ACTIONS"}
    return subprocess.run([sys.executable, "-I", str(SCRIPT), *args], cwd=cwd, capture_output=True, text=True, env=env)


def fixture(d):
    (d / "target.md").write_text(TARGET, encoding="utf-8")
    (d / "good.md").write_text(GOOD, encoding="utf-8")
    (d / "sub").mkdir()
    (d / "sub" / "with space.md").write_text("# Space\n", encoding="utf-8")
    (d / "code.rs").write_text("fn main() {\n}\n", encoding="utf-8")
    (d / "img.png").write_bytes(b"\x89PNG")


def main():
    failures = []
    total = 0

    def expect(cond, what):
        nonlocal total
        total += 1
        if not cond:
            failures.append(what)

    with tempfile.TemporaryDirectory() as t:
        d = Path(t) / "repo"
        d.mkdir()
        fixture(d)
        r = run(d, "good.md", "target.md")
        expect(r.returncode == 0, f"good links pass: {r.stdout.strip()}")
        expect("0 broken" in r.stdout, f"good links report none broken: {r.stdout.strip()}")

        for link, problem in BROKEN:
            (d / "bad.md").write_text(f"# Bad\n\n{link}\n", encoding="utf-8")
            r = run(d, "bad.md")
            expect(r.returncode == 1 and problem in r.stdout and "bad.md:3:" in r.stdout,
                   f"{link} is reported as '{problem}': {r.stdout.strip()}")

        # --root from another directory.
        (d / "bad.md").write_text("[x](missing.md)\n", encoding="utf-8")
        r = run(Path(t), "--root", "repo", "good.md")
        expect(r.returncode == 0, f"--root resolves files under the root: {r.stdout.strip()}")

    # Without file arguments: every tracked Markdown file but the excluded ones.
    with tempfile.TemporaryDirectory() as t:
        d = Path(t)
        fixture(d)
        (d / "corpus").mkdir()
        (d / "corpus" / "README.md").write_text("[x](../spec/missing.md)\n", encoding="utf-8")
        (d / "untracked.md").write_text("[x](missing.md)\n", encoding="utf-8")
        subprocess.run(["git", "init", "-q", str(d)], check=True)
        subprocess.run(["git", "-C", str(d), "add", "good.md", "target.md", "corpus/README.md",
                        "sub", "code.rs", "img.png"], check=True)
        r = run(d)
        expect(r.returncode == 0 and r.stdout.startswith("3 files,"),
               f"the tracked files less the excluded are checked: {r.stdout.strip()}")
        (d / "tracked.md").write_text("[x](missing.md)\n", encoding="utf-8")
        subprocess.run(["git", "-C", str(d), "add", "tracked.md"], check=True)
        r = run(d)
        expect(r.returncode == 1 and "tracked.md:1:" in r.stdout,
               f"a newly tracked file is checked: {r.stdout.strip()}")

    for f in failures:
        print(f"FAIL: {f}", file=sys.stderr)
    print(f"linkcheck-test.py: {total} checks, {len(failures)} failures")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
