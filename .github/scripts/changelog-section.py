# SPDX-License-Identifier: MIT OR Apache-2.0
"""Print the body of one version's section of a Keep a Changelog file.

    changelog-section.py CHANGELOG.md 0.3.0

The section is the text after the heading `## [0.3.0]` (optionally followed by ` - <date>`)
up to the next `## ` heading or the link references at the end. Exits with status 1, and a
message on stderr, if the section is missing or empty, so that a release cannot be published
without its notes.
"""

import re
import sys


def section(text: str, version: str) -> str:
    heading = re.compile(r"^## \[" + re.escape(version) + r"\](?: - \d{4}-\d{2}-\d{2})?\s*$")
    lines = text.splitlines()
    start = next((i for i, line in enumerate(lines) if heading.match(line)), None)
    if start is None:
        raise ValueError(f"no `## [{version}]` section")
    body = []
    for line in lines[start + 1 :]:
        if line.startswith("## ") or re.match(r"^\[[^\]]+\]: ", line):
            break
        body.append(line)
    out = "\n".join(body).strip()
    if not out:
        raise ValueError(f"the `## [{version}]` section is empty")
    return out + "\n"


def main() -> int:
    if len(sys.argv) != 3:
        print("usage: changelog-section.py CHANGELOG.md VERSION", file=sys.stderr)
        return 2
    path, version = sys.argv[1], sys.argv[2]
    try:
        with open(path, encoding="utf-8") as f:
            sys.stdout.write(section(f.read(), version))
    except (OSError, ValueError) as e:
        print(f"changelog-section.py: {path}: {e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
