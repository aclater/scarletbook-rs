# SPDX-License-Identifier: MIT OR Apache-2.0
"""Tests of quickstart.py's work-directory guard, `cargo_ancestor` (#428)."""

import importlib.util
import pathlib
import sys
import tempfile

SCRIPT = pathlib.Path(__file__).resolve().parent / "quickstart.py"
spec = importlib.util.spec_from_file_location("quickstart", SCRIPT)
quickstart = importlib.util.module_from_spec(spec)
spec.loader.exec_module(quickstart)


def main():
    failures = []
    total = 0

    def expect(cond, what):
        nonlocal total
        total += 1
        if not cond:
            failures.append(what)

    with tempfile.TemporaryDirectory() as t:
        root = pathlib.Path(t).resolve()
        home = root / "home"
        cargo_home = home / ".cargo"
        cargo_home.mkdir(parents=True)
        (cargo_home / "config.toml").write_text("[net]\n", encoding="utf-8")
        work = home / "work" / "_temp"
        work.mkdir(parents=True)
        guard = quickstart.cargo_ancestor

        expect(guard(work, cargo_home.resolve()) is None,
               "a runner home whose .cargo (with a config) is CARGO_HOME passes")

        (home / "work" / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
        expect(guard(work, cargo_home.resolve()) == home / "work" / "Cargo.toml",
               "a parent Cargo.toml fails")
        (home / "work" / "Cargo.toml").unlink()

        (home / "work" / "rust-toolchain.toml").write_text("[toolchain]\n", encoding="utf-8")
        expect(guard(work, cargo_home.resolve()) == home / "work" / "rust-toolchain.toml",
               "a parent rust-toolchain.toml fails")
        (home / "work" / "rust-toolchain.toml").unlink()

        other = home / "work" / ".cargo"
        other.mkdir()
        expect(guard(work, cargo_home.resolve()) is None, "a parent .cargo with no config passes")
        (other / "config.toml").write_text("[build]\n", encoding="utf-8")
        expect(guard(work, cargo_home.resolve()) == other / "config.toml",
               "a parent .cargo/config.toml that isn't CARGO_HOME fails")
        expect(guard(work, other.resolve()) == cargo_home / "config.toml",
               "with CARGO_HOME elsewhere, the home's .cargo/config.toml fails")
        (cargo_home / "config.toml").unlink()
        expect(guard(work, other.resolve()) is None,
               "the same .cargo passes when it is CARGO_HOME")

    for f in failures:
        print(f"FAIL: {f}", file=sys.stderr)
    print(f"quickstart-test.py: {total} checks, {len(failures)} failures")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
