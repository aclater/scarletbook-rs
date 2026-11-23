# SPDX-License-Identifier: MIT OR Apache-2.0
"""Tests of release-crates.py: per-crate versions and the crates.io skip, against a fake index."""

import importlib.util
import json
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "release_crates", Path(__file__).resolve().parent / "release-crates.py")
rc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(rc)

failures = 0


def check(name, got, want):
    global failures
    if got != want:
        failures += 1
        print(f"FAIL {name}: got {got!r}, want {want!r}")
    else:
        print(f"ok   {name}")


def metadata(**crate_versions):
    return json.dumps({"packages": [{"name": n, "version": v} for n, v in crate_versions.items()]})


def index(**published):
    """A fake sparse index: crate -> list of versions on crates.io (absent: 404)."""
    def get(url):
        crate = url.rsplit("/", 1)[1]
        if crate not in published:
            return 404, ""
        return 200, "".join(json.dumps({"name": crate, "vers": v}) + "\n" for v in published[crate])
    return get


# The 1.0.0 layout: scarletbook-pcm keeps 0.7.0, the other two move to 1.0.0.
v = rc.versions(metadata(**{"scarletbook-ffi": "1.0.0", "scarletbook": "1.0.0",
                            "scarletbook-pcm": "0.7.0", "scarletbook-bench": "1.0.0"}))
check("versions in publish order", list(v.items()),
      [("scarletbook-pcm", "0.7.0"), ("scarletbook", "1.0.0"), ("scarletbook-ffi", "1.0.0")])

check("index path", rc.index_url("scarletbook-pcm"), "https://index.crates.io/sc/ar/scarletbook-pcm")

seen = {"scarletbook-pcm": ["0.6.2", "0.7.0"], "scarletbook": ["0.6.2", "0.7.0"],
        "scarletbook-ffi": ["0.6.2", "0.7.0"]}
check("unchanged pcm skipped, the others published", rc.classify(v, index(**seen)),
      (["scarletbook-pcm"], ["scarletbook", "scarletbook-ffi"], []))

done = {c: vs + ([] if c == "scarletbook-pcm" else ["1.0.0"]) for c, vs in seen.items()}
check("re-run after a full publish skips all three", rc.classify(v, index(**done)),
      (["scarletbook-pcm", "scarletbook", "scarletbook-ffi"], [], []))

part = dict(seen, scarletbook=seen["scarletbook"] + ["1.0.0"])
check("re-run after a partial publish", rc.classify(v, index(**part)),
      (["scarletbook-pcm", "scarletbook"], ["scarletbook-ffi"], []))

no_ffi = {c: vs for c, vs in seen.items() if c != "scarletbook-ffi"}
check("a crate not on crates.io is new", rc.classify(v, index(**no_ffi)),
      (["scarletbook-pcm"], ["scarletbook"], ["scarletbook-ffi"]))

same = rc.versions(metadata(**{"scarletbook-pcm": "0.7.1", "scarletbook": "0.7.1",
                               "scarletbook-ffi": "0.7.1"}))
check("a shared version publishes all three", rc.classify(same, index(**seen)),
      ([], ["scarletbook-pcm", "scarletbook", "scarletbook-ffi"], []))

try:
    rc.classify(v, lambda url: (503, ""))
    check("index error raises", "no error", "RuntimeError")
except RuntimeError:
    check("index error raises", "RuntimeError", "RuntimeError")

try:
    rc.versions(metadata(scarletbook="1.0.0"))
    check("missing crate exits", "no exit", "SystemExit")
except SystemExit:
    check("missing crate exits", "SystemExit", "SystemExit")

print(f"{failures} failures")
sys.exit(1 if failures else 0)
