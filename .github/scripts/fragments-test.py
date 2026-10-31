# SPDX-License-Identifier: MIT OR Apache-2.0
"""Tests of fragments.py `check` and `assemble` on a fixture in a temporary directory."""

import subprocess
import sys
import tempfile
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "fragments.py"

CHANGELOG = """# Changelog

Intro.

## [Unreleased]

### Added

- Old addition (#1).

### Fixed

- Old fix (#2).

## [0.1.0] - 2026-01-01

### Added

- First release.

[Unreleased]: https://example.invalid/compare/v0.1.0...HEAD
"""

CLEANROOM = """# Clean room

## Log

| Date | Stage | Notes |
|---|---|---|
| 2026-01-01 | first | done. |
"""


def run(cwd, *args):
    return subprocess.run([sys.executable, "-I", str(SCRIPT), *args], cwd=cwd, capture_output=True, text=True)


def fixture(d):
    (d / "CHANGELOG.md").write_text(CHANGELOG, encoding="utf-8")
    (d / "CLEANROOM.md").write_text(CLEANROOM, encoding="utf-8")
    (d / "changelog.d").mkdir()
    (d / "cleanroom.d").mkdir()
    (d / "changelog.d" / "README.md").write_text("ignored\n", encoding="utf-8")
    (d / "changelog.d" / "a.md").write_text("Added\n- New addition (#3).\n", encoding="utf-8")
    (d / "changelog.d" / "b.md").write_text("Changed\n- New change (#4).\n", encoding="utf-8")
    (d / "cleanroom.d" / "a.md").write_text("| 2026-02-01 | new | none. |\n", encoding="utf-8")


def main():
    failures = []
    total = 0

    def expect(cond, what):
        nonlocal total
        total += 1
        if not cond:
            failures.append(what)

    with tempfile.TemporaryDirectory() as t:
        d = Path(t)
        fixture(d)
        expect(run(d, "check").returncode == 0, "check accepts valid fragments")
        r = run(d, "assemble", "0.2.0", "2026-02-02")
        expect(r.returncode == 0, f"assemble succeeds: {r.stderr}")
        c = (d / "CHANGELOG.md").read_text(encoding="utf-8")
        expect("## [Unreleased]\n\n## [0.2.0] - 2026-02-02\n\n### Added\n\n- Old addition (#1).\n- New addition (#3).\n"
               in c, "Added keeps the old entry and appends the fragment")
        expect("### Changed\n\n- New change (#4).\n" in c, "a new section is created")
        expect(c.index("### Changed") < c.index("### Fixed") < c.index("## [0.1.0]"),
               "sections in Keep a Changelog order, before the earlier release")
        expect("- Old fix (#2)." in c, "Fixed keeps the old entry")
        r_ = (d / "CLEANROOM.md").read_text(encoding="utf-8")
        expect(r_.endswith("| 2026-01-01 | first | done. |\n| 2026-02-01 | new | none. |\n"), "the row is appended")
        expect(sorted(p.name for p in (d / "changelog.d").iterdir()) == ["README.md"], "changelog fragments removed")
        expect(not any((d / "cleanroom.d").iterdir()), "cleanroom fragments removed")

    for name, content, what in [
        ("changelog.d/x.md", "Bogus\n- x\n", "an unknown section"),
        ("changelog.d/x.md", "Added\nno list item\n", "an entry that is not a list item"),
        ("cleanroom.d/x.md", "| not a date | a | b |\n", "a row without a date"),
        ("cleanroom.d/x.md", "| 2026-01-01 | a | b |\n| 2026-01-02 | c | d |\n", "two rows in one fragment"),
    ]:
        with tempfile.TemporaryDirectory() as t:
            d = Path(t)
            fixture(d)
            (d / name).write_text(content, encoding="utf-8")
            expect(run(d, "check").returncode == 1, f"check refuses {what}")

    with tempfile.TemporaryDirectory() as t:
        d = Path(t)
        fixture(d)
        (d / "CHANGELOG.md").write_text(CHANGELOG.replace("## [Unreleased]\n", "## [Unreleased]\n\nstray\n"),
                                        encoding="utf-8")
        expect(run(d, "assemble", "0.2.0", "2026-02-02").returncode == 1,
               "assemble refuses text before the first section")

    for f in failures:
        print(f"FAIL: {f}", file=sys.stderr)
    print(f"fragments-test.py: {total} checks, {len(failures)} failures")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
