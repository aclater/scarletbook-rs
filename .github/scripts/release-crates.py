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
      trusted publishing) and `new=<crates>` (not on crates.io at all: bootstrap).

Exit status 1 on an index error other than 404, or a crate missing from the workspace.
"""

import json
import os
import subprocess
import sys
import urllib.error
import urllib.request

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
        code, body = get(index_url(crate))
        if code == 404:
            new.append(crate)
        elif code != 200:
            raise RuntimeError(f"crates.io index returned HTTP {code} for {crate}")
        elif any(json.loads(line)["vers"] == version for line in body.splitlines() if line.strip()):
            skipped.append(crate)
        else:
            existing.append(crate)
    return skipped, existing, new


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
        for crate in existing + new:
            print(f"{crate} {crate_versions[crate]} to publish")
        with open(out, "a", encoding="utf-8") as f:
            f.write(f"existing={' '.join(existing)}\nnew={' '.join(new)}\n")
        return 0
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
