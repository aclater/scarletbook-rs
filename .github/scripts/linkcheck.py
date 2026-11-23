# SPDX-License-Identifier: MIT OR Apache-2.0
"""Checks the links inside the repository: relative paths and #anchors in Markdown (#412).

usage: python3 -I linkcheck.py [--root DIR] [FILE.md...]

Without files, every Markdown file git tracks under the root is checked, less EXCLUDED.
Each inline link or image, reference definition and HTML href/src outside code must name
an existing file or directory; an anchor must name a heading or an explicit
<a id="..."> / <a name="..."> of the Markdown file it points into, or a line (#L12,
#L12-L20) of another text file. Anchors are GitHub's heading slugs: the heading's text
lowercased, characters other than letters, digits, spaces, '-' and '_' dropped, spaces
turned into '-', and repeats suffixed -1, -2... External links (anything with a scheme,
such as https: or mailto:) are left to the weekly external-links.yml; this check reads no
network and is deterministic.
"""

import os
import re
import subprocess
import sys
from pathlib import Path
from urllib.parse import unquote

# Tracked Markdown files that are not checked, each with its reason.
EXCLUDED = {
    # A verbatim copy of sacd-spec's corpus README (README.md, "Conformance"); its relative
    # links point into the sacd-spec repository (../spec/..., ../PROVENANCE.md).
    "corpus/README.md",
}

# An inline link or image: [text](dest "title") or ![alt](<dest>). The text may hold one
# level of brackets ([`a[b]`](x)).
INLINE = re.compile(r"!?\[(?:[^\[\]]|\[[^\]]*\])*\]\(\s*(<[^>]*>|[^\s)]+)(?:\s+(?:\"[^\"]*\"|'[^']*'|\([^)]*\)))?\s*\)")
# A reference definition: [label]: dest "title".
REFDEF = re.compile(r"^ {0,3}\[[^\]]+\]:\s*(<[^>]*>|\S+)")
# An HTML link or image source.
HTML = re.compile(r"<(?:a|img)\b[^>]*?\b(?:href|src)\s*=\s*([\"'])(.*?)\1", re.IGNORECASE)
# An explicit HTML anchor.
HTML_ANCHOR = re.compile(r"<a\b[^>]*?\b(?:id|name)\s*=\s*([\"'])(.*?)\1", re.IGNORECASE)
SCHEME = re.compile(r"^[A-Za-z][A-Za-z0-9+.-]*:")
FENCE = re.compile(r"^ {0,3}(```+|~~~+)")
LINE_ANCHOR = re.compile(r"^L(\d+)(?:-L(\d+))?$")


def strip_code(lines):
    """The lines with fenced code blocks, inline code spans and HTML comments blanked."""
    out, fence, comment = [], None, False
    for line in lines:
        m = FENCE.match(line)
        if fence:
            if m and m.group(1)[0] == fence[0] and len(m.group(1)) >= len(fence) and not line[m.end():].strip():
                fence = None
            out.append("")
            continue
        if m and not comment:
            fence = m.group(1)
            out.append("")
            continue
        # Code spans of any backtick count: `a`, ``a`b``.
        line = re.sub(r"(`+)(?:(?!\1).)+?\1", "``", line)
        kept = ""
        while line:
            if comment:
                end = line.find("-->")
                if end < 0:
                    line = ""
                else:
                    line, comment = line[end + 3:], False
            else:
                start = line.find("<!--")
                if start < 0:
                    kept, line = kept + line, ""
                else:
                    kept, line, comment = kept + line[:start], line[start + 4:], True
        out.append(kept)
    return out


def slug(title):
    """GitHub's anchor for a heading's text, before the repeat suffix."""
    title = re.sub(r"!?\[([^\]]*)\]\([^)]*\)", r"\1", title)  # links and images: their text
    # HTML tags, outside code spans (`Result<(), Error>` is text).
    parts = re.split(r"((`+)(?:(?!\2).)+?\2)", title)
    title = "".join(p if i % 3 else re.sub(r"<[^>]+>", "", p) for i, p in enumerate(parts) if i % 3 != 2)
    return re.sub(r"[^\w\- ]", "", title.strip().lower()).replace(" ", "-")


def anchors(path):
    """Every anchor a Markdown file defines: its headings' slugs and explicit HTML anchors."""
    text = path.read_text(encoding="utf-8").split("\n")
    blanked = strip_code(text)
    seen, result = {}, set()

    def add(s):
        n = seen.get(s, 0)
        seen[s] = n + 1
        result.add(s if n == 0 else f"{s}-{n}")

    for i, (orig, line) in enumerate(zip(text, blanked)):
        for m in HTML_ANCHOR.finditer(line):
            result.add(m.group(2))
        if re.match(r"^ {0,3}#{1,6}(\s|$)", line):
            add(slug(re.match(r"^ {0,3}#{1,6}\s*(.*?)(?:\s+#+)?\s*$", orig).group(1)))
        elif (
            i > 0
            and re.match(r"^ {0,3}(=+|-+)\s*$", line)
            and blanked[i - 1].strip()
            and not re.match(r"^ {0,3}(#|[-*+>|]|\d+[.)]\s)", blanked[i - 1])
            and not re.match(r"^ {0,3}(=+|-+)\s*$", blanked[i - 1])
            and (i < 2 or not blanked[i - 2].strip())
        ):
            # A setext heading: one paragraph line underlined with = or -.
            add(slug(text[i - 1]))
    return result


def targets(line):
    """The link destinations of one blanked line."""
    for m in INLINE.finditer(line):
        yield m.group(1)
    m = REFDEF.match(line)
    if m:
        yield m.group(1)
    for m in HTML.finditer(line):
        yield m.group(2)


def check_file(root, path, cache):
    """(line number, destination, problem) for each broken link of one Markdown file."""
    problems, count = [], 0
    for lineno, line in enumerate(strip_code(path.read_text(encoding="utf-8").split("\n")), 1):
        for target in targets(line):
            if target.startswith("<") and target.endswith(">"):
                target = target[1:-1]
            if not target or SCHEME.match(target) or target.startswith("//"):
                continue
            count += 1
            file_part, _, anchor = target.partition("#")
            file_part, anchor = unquote(file_part.split("?")[0]), unquote(anchor)
            if not file_part:
                dest = path
            elif file_part.startswith("/"):
                dest = root / file_part.lstrip("/")
            else:
                dest = path.parent / file_part
            dest = Path(_normalise(dest))
            problem = None
            if not str(dest).startswith(str(root) + "/") and dest != root:
                problem = "points outside the repository"
            elif not dest.exists():
                problem = "no such file"
            elif anchor:
                if dest.is_dir():
                    problem = "anchor into a directory"
                elif dest.suffix.lower() in (".md", ".markdown"):
                    if dest not in cache:
                        cache[dest] = anchors(dest)
                    if anchor not in cache[dest]:
                        problem = "no such anchor"
                else:
                    m = LINE_ANCHOR.match(anchor)
                    lines = dest.read_bytes().count(b"\n") + 1
                    if not m:
                        problem = "anchor into a non-Markdown file is not a line (#L12)"
                    elif max(int(m.group(1)), int(m.group(2) or 0)) > lines:
                        problem = f"no such line (the file has {lines})"
            if problem:
                problems.append((lineno, target, problem))
    return problems, count


def _normalise(p):
    """The path with . and .. resolved lexically (symbolic links are not followed)."""
    parts = []
    for part in Path(p).parts:
        if part == "..":
            if len(parts) > 1:
                parts.pop()
        elif part != ".":
            parts.append(part)
    return str(Path(*parts))


def tracked_markdown(root):
    out = subprocess.run(
        ["git", "-C", str(root), "ls-files", "-z", "--", "*.md", "*.markdown"],
        check=True, capture_output=True, text=True,
    ).stdout
    return sorted(n for n in out.split("\0") if n and n not in EXCLUDED)


def main(argv):
    args = list(argv)
    root = Path(".")
    if args[:1] == ["--root"]:
        root, args = Path(args[1]), args[2:]
    root = root.resolve()
    names = args or tracked_markdown(root)
    cache, links, broken = {}, 0, 0
    for name in names:
        path = Path(_normalise(root / name))
        problems, count = check_file(root, path, cache)
        links += count
        for lineno, target, problem in problems:
            broken += 1
            rel = path.relative_to(root)
            if os.environ.get("GITHUB_ACTIONS") == "true":
                print(f"::error file={rel},line={lineno}::{target}: {problem}")
            else:
                print(f"{rel}:{lineno}: {target}: {problem}")
    print(f"{len(names)} files, {links} relative links and anchors, {broken} broken")
    return 1 if broken else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
