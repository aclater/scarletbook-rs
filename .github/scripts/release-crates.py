# SPDX-License-Identifier: MIT OR Apache-2.0
"""The crates.io packages of a release: each crate's own version, and which of them
crates.io already has.

The three published crates need not share one version: from 1.0.0 `scarletbook` and
`scarletbook-ffi` follow the workspace version while `scarletbook-pcm` keeps its own
(0.7.0, outside the 1.0 freeze, #398). So nothing here takes a crate's version from the
tag; each comes from its manifest, through `cargo metadata` (which compiles nothing).

  release-crates.py versions
      Prints `<crate> <version>` for each published crate, in publish order.
  release-crates.py pending [OUT]
      Looks each crate's version up in the crates.io sparse index. A version already there
      (a crate that did not change in this release, a re-run, or a first publish done by
      hand) is skipped and named on stdout; the rest are appended to OUT (default
      $GITHUB_OUTPUT) as `existing=<crates>` (the crate is on crates.io, publish through
      trusted publishing), `new=<crates>` (not on crates.io at all: bootstrap) and
      `pending=<crates>` (both, in publish order).
  release-crates.py lockcheck DIR
      Checks the Cargo.lock inside each .crate file in DIR (packed together by
      `cargo package -p <pending crates>`): every published crate it pins at a version that
      crates.io has must carry crates.io's checksum, and one at a version crates.io does not
      have yet must carry the checksum of its .crate file in DIR (it is published from that
      file first). This is the check that would have stopped 1.0.0 in `build` (RELEASING.md,
      "Pack only what is published").

Exit status 1 on an index error other than 404, a crate missing from the workspace, or a
lockcheck failure.
"""

import hashlib
import json
import os
import subprocess
import sys
import tarfile
import tomllib
import urllib.error
import urllib.request
from pathlib import Path

# Dependency order: scarletbook-ffi depends on the other two at their exact versions.
CRATES = ("scarletbook-pcm", "scarletbook", "scarletbook-ffi")
INDEX = "https://index.crates.io"


def versions(metadata=None):
    """{crate: version} of CRATES, from `cargo metadata` JSON (run here if not given)."""
    if metadata is None:
        metadata = subprocess.run(
            ["cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"],
            check=True, capture_output=True, text=True).stdout
    found = {p["name"]: p["version"] for p in json.loads(metadata)["packages"]}
    missing = [c for c in CRATES if c not in found]
    if missing:
        sys.exit(f"not in the workspace: {' '.join(missing)}")
    return {c: found[c] for c in CRATES}


def index_url(crate):
    """The sparse index file of a crate name of 4 or more characters: first two / next two."""
    name = crate.lower()
    return f"{INDEX}/{name[0:2]}/{name[2:4]}/{name}"


def fetch(url):
    """(HTTP status, body) of a GET."""
    try:
        with urllib.request.urlopen(url, timeout=60) as response:
            return response.status, response.read().decode()
    except urllib.error.HTTPError as error:
        return error.code, ""


def classify(crate_versions, get=fetch):
    """(skipped, existing, new) crate lists; raises RuntimeError on an index error."""
    skipped, existing, new = [], [], []
    for crate, version in crate_versions.items():
        published = index_versions(crate, get)
        if not published:
            new.append(crate)
        elif version in published:
            skipped.append(crate)
        else:
            existing.append(crate)
    return skipped, existing, new


def index_versions(crate, get=fetch):
    """{version: cksum} of a crate in the crates.io index ({} if it is not there)."""
    code, body = get(index_url(crate))
    if code == 404:
        return {}
    if code != 200:
        raise RuntimeError(f"crates.io index returned HTTP {code} for {crate}")
    entries = (json.loads(line) for line in body.splitlines() if line.strip())
    return {e["vers"]: e["cksum"] for e in entries}


def check_lock(packed, lock_text, packed_sums, get=fetch):
    """Problems (strings) with the Cargo.lock `lock_text` of the package `packed`.

    `packed_sums` maps the .crate file names packed together with it to their SHA-256. Every
    entry of a published crate (CRATES) from crates.io must pin the checksum the registry
    will hold when `packed` is uploaded: crates.io's own for a version it has (a skipped
    crate, or one published by an earlier run), the packed file's for one it does not have.
    """
    problems = []
    for entry in tomllib.loads(lock_text).get("package", []):
        name, version = entry["name"], entry["version"]
        if name not in CRATES or not entry.get("source", "").startswith("registry+"):
            continue
        published = index_versions(name, get)
        if version in published:
            want, where = published[version], "crates.io's"
        else:
            want, where = packed_sums.get(f"{name}-{version}.crate"), "the packed file's"
        if want is None:
            problems.append(f"{packed}: pins {name} {version}, which is neither on crates.io "
                            "nor packed with it")
        elif entry.get("checksum") != want:
            problems.append(f"{packed}: pins {name} {version} with checksum "
                            f"{entry.get('checksum')}, not {where} {want}")
    return problems


def lockcheck(directory, get=fetch):
    """Problems with the Cargo.lock of every .crate file in `directory` (see check_lock)."""
    files = sorted(Path(directory).glob("*.crate"))
    sums = {f.name: hashlib.sha256(f.read_bytes()).hexdigest() for f in files}
    problems = []
    for f in files:
        with tarfile.open(f, "r:gz") as tar:
            lock = tar.extractfile(f"{f.name.removesuffix('.crate')}/Cargo.lock")
            if lock is not None:
                problems += check_lock(f.name, lock.read().decode(), sums, get)
    return problems


def main(argv):
    if len(argv) >= 2 and argv[1] == "versions" and len(argv) == 2:
        for crate, version in versions().items():
            print(crate, version)
        return 0
    if len(argv) in (2, 3) and argv[1] == "pending":
        out = argv[2] if len(argv) == 3 else os.environ.get("GITHUB_OUTPUT")
        if not out:
            sys.exit("pending: no OUT file and no GITHUB_OUTPUT")
        crate_versions = versions()
        try:
            skipped, existing, new = classify(crate_versions)
        except RuntimeError as error:
            print(f"::error::{error}")
            return 1
        for crate in skipped:
            print(f"{crate} {crate_versions[crate]} is already on crates.io; skipped")
        pending = [c for c in crate_versions if c in existing or c in new]
        for crate in pending:
            print(f"{crate} {crate_versions[crate]} to publish")
        with open(out, "a", encoding="utf-8") as f:
            f.write(f"existing={' '.join(existing)}\nnew={' '.join(new)}\n"
                    f"pending={' '.join(pending)}\n")
        return 0
    if len(argv) == 3 and argv[1] == "lockcheck":
        try:
            problems = lockcheck(argv[2])
        except RuntimeError as error:
            print(f"::error::{error}")
            return 1
        for problem in problems:
            print(f"::error::{problem}")
        if problems:
            return 1
        print("the packed Cargo.lock files pin the checksums crates.io will hold")
        return 0
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
