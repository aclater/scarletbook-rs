# SPDX-License-Identifier: MIT OR Apache-2.0
"""Change fragments: one file per pull request, folded into CHANGELOG.md and CLEANROOM.md
when a release is prepared, so that independent pull requests never edit the same lines.

    fragments.py check
    fragments.py require BASE_REF
    fragments.py assemble VERSION DATE

changelog.d/<name>.md   a CHANGELOG entry; the first line is the section (Added, Changed,
                        Deprecated, Removed, Fixed or Security), the rest is the entry's
                        Markdown list item(s).
cleanroom.d/<name>.md   one row of the CLEANROOM.md Log table, starting with `| <date> |`.

`check` validates every fragment. `require BASE_REF` fails if the change since BASE_REF
touches library or program code (crates/*/src, Cargo.toml or Cargo.lock) without adding a
fragment to both directories. `assemble` moves the CHANGELOG.md `## [Unreleased]` entries and
every changelog fragment under `## [VERSION] - DATE`, appends the cleanroom rows to the Log,
and deletes the fragments.
"""

import re
import subprocess
import sys
from pathlib import Path

SECTIONS = ["Added", "Changed", "Deprecated", "Removed", "Fixed", "Security"]
ROW = re.compile(r"^\| \d{4}-\d{2}-\d{2} \| .+ \| .+ \|$")
CHANGELOG_D = Path("changelog.d")
CLEANROOM_D = Path("cleanroom.d")


def fragments(directory):
    return sorted(p for p in directory.glob("*.md") if p.name != "README.md")


def read_changelog(path):
    lines = path.read_text(encoding="utf-8").strip("\n").split("\n")
    section = lines[0].strip()
    body = "\n".join(lines[1:]).strip("\n")
    if section not in SECTIONS:
        raise ValueError(f"{path}: first line must be one of {', '.join(SECTIONS)}, not {section!r}")
    if not body.lstrip().startswith("- "):
        raise ValueError(f"{path}: the entry must be a Markdown list item starting with '- '")
    return section, body


def read_cleanroom(path):
    text = path.read_text(encoding="utf-8").strip("\n")
    if "\n" in text or not ROW.match(text):
        raise ValueError(f"{path}: must be one Log row, `| YYYY-MM-DD | stage | notes |`")
    return text


def check():
    for p in fragments(CHANGELOG_D):
        read_changelog(p)
    for p in fragments(CLEANROOM_D):
        read_cleanroom(p)


def require(base):
    out = subprocess.run(
        ["git", "diff", "--name-status", "--no-renames", f"{base}...HEAD"],
        check=True, capture_output=True, text=True,
    ).stdout
    changed, added = [], set()
    for line in out.splitlines():
        status, path = line.split("\t", 1)
        changed.append(path)
        if status == "A":
            added.add(path)
    code = [p for p in changed if re.match(r"^crates/[^/]+/src/", p) or p in ("Cargo.toml", "Cargo.lock")]
    if not code:
        return
    missing = [
        d for d in (CHANGELOG_D, CLEANROOM_D)
        if not any(p.startswith(f"{d}/") and p.endswith(".md") and not p.endswith("README.md") for p in added)
    ]
    if missing:
        raise ValueError(
            "this change touches code (" + ", ".join(code[:3]) + ("..." if len(code) > 3 else "")
            + ") but adds no fragment to " + " and ".join(f"{d}/" for d in missing)
            + "; see CONTRIBUTING.md, \"Change records\""
        )


def assemble(version, date):
    check()
    changelog = Path("CHANGELOG.md")
    text = changelog.read_text(encoding="utf-8")
    head = "## [Unreleased]\n"
    start = text.index(head) + len(head)
    end = text.index("\n## [", start)
    # The entries already under Unreleased, by section.
    entries = {s: [] for s in SECTIONS}
    current = None
    for line in text[start:end].strip("\n").split("\n"):
        if line.startswith("### "):
            current = line[4:].strip()
            entries.setdefault(current, [])
        elif current is not None:
            entries[current].append(line)
    blocks = {s: ["\n".join(v).strip("\n")] if "".join(v).strip() else [] for s, v in entries.items()}
    for p in fragments(CHANGELOG_D):
        section, body = read_changelog(p)
        blocks[section].append(body)
    out = []
    for s in SECTIONS + [s for s in blocks if s not in SECTIONS]:
        if blocks.get(s):
            out.append(f"### {s}\n\n" + "\n".join(blocks[s]) + "\n")
    new = f"\n## [{version}] - {date}\n\n" + "\n".join(out)
    text = text[:start] + new + text[end:]
    changelog.write_text(text, encoding="utf-8")

    cleanroom = Path("CLEANROOM.md")
    rows = [read_cleanroom(p) for p in fragments(CLEANROOM_D)]
    c = cleanroom.read_text(encoding="utf-8").rstrip("\n").split("\n")
    if not c[-1].startswith("| "):
        raise ValueError("CLEANROOM.md must end with the Log table")
    cleanroom.write_text("\n".join(c + rows) + "\n", encoding="utf-8")
    for p in fragments(CHANGELOG_D) + fragments(CLEANROOM_D):
        p.unlink()


def main():
    try:
        if sys.argv[1:2] == ["check"] and len(sys.argv) == 2:
            check()
        elif sys.argv[1:2] == ["require"] and len(sys.argv) == 3:
            check()
            require(sys.argv[2])
        elif sys.argv[1:2] == ["assemble"] and len(sys.argv) == 4:
            assemble(sys.argv[2], sys.argv[3])
        else:
            print(__doc__, file=sys.stderr)
            return 2
    except (OSError, ValueError, subprocess.CalledProcessError) as e:
        print(f"fragments.py: {e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
