# SPDX-License-Identifier: MIT OR Apache-2.0
"""Tests of release-crates.py: per-crate versions and the crates.io skip, against a fake index."""

import hashlib
import importlib.util
import io
import json
import sys
import tarfile
import tempfile
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
    """A fake sparse index: crate -> versions on crates.io (absent: 404). A version is a
    string (its checksum is then made up) or a (version, checksum) pair."""
    def get(url):
        crate = url.rsplit("/", 1)[1]
        if crate not in published:
            return 404, ""
        entries = (v if isinstance(v, tuple) else (v, f"sum-{crate}-{v}") for v in published[crate])
        return 200, "".join(json.dumps({"name": crate, "vers": v, "cksum": c}) + "\n"
                            for v, c in entries)
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

# The 1.0.0 incident (release run 45158302304): `build` packed all three crates together,
# so scarletbook-ffi's Cargo.lock pinned the scarletbook-pcm 0.7.0 packed in that run
# (95dbb558...) instead of the one on crates.io (ffa22529...): a crate's repack never matches
# its published file, since .cargo_vcs_info.json names the commit. These are the real sums.
PCM_INDEX = "ffa2252919d7515180a8a9280314801806436b22804f8da972c3f6762b657abf"
PCM_REPACK = "95dbb5588685b187b7004cf1ea9da883af3bc5674a695d024df58f4839aa708f"
REGISTRY = "registry+https://github.com/rust-lang/crates.io-index"


def lock(**pins):
    """A packaged Cargo.lock: crate -> (version, checksum); plus a path-free third party."""
    entries = [("libc", "0.2.0", "sum-libc")] + [(n, v, c) for n, (v, c) in pins.items()]
    return "version = 4\n" + "".join(
        f'\n[[package]]\nname = "{n}"\nversion = "{v}"\nsource = "{REGISTRY}"\nchecksum = "{c}"\n'
        for n, v, c in entries)


def crate_file(directory, name, version, lock_text, extra=b""):
    """Writes <name>-<version>.crate holding the Cargo.lock; returns its SHA-256."""
    path = Path(directory) / f"{name}-{version}.crate"
    with tarfile.open(path, "w:gz") as tar:
        for member, data in (("Cargo.lock", lock_text.encode()), ("src/lib.rs", extra)):
            info = tarfile.TarInfo(f"{name}-{version}/{member}")
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
    return hashlib.sha256(path.read_bytes()).hexdigest()


registry = index(**{"scarletbook-pcm": ["0.6.2", ("0.7.0", PCM_INDEX)],
                    "scarletbook": ["0.7.0"], "scarletbook-ffi": ["0.7.0"]})

with tempfile.TemporaryDirectory() as old, tempfile.TemporaryDirectory() as fixed:
    # As 1.0.0's `build` packed them: pcm too, and ffi pinned that repack.
    crate_file(old, "scarletbook-pcm", "0.7.0", lock(), b"repack")
    sb = crate_file(old, "scarletbook", "1.0.0", lock())
    crate_file(old, "scarletbook-ffi", "1.0.0", lock(**{"scarletbook-pcm": ("0.7.0", PCM_REPACK),
                                                        "scarletbook": ("1.0.0", sb)}))
    check("1.0.0's packing: ffi pins a repacked pcm crates.io does not have",
          rc.lockcheck(old, registry),
          [f"scarletbook-ffi-1.0.0.crate: pins scarletbook-pcm 0.7.0 with checksum {PCM_REPACK}, "
           f"not crates.io's {PCM_INDEX}"])
    # Only the pending crates, pcm resolved from crates.io; scarletbook is not on crates.io
    # yet, so ffi must pin the file packed beside it (from which scarletbook is published).
    sb = crate_file(fixed, "scarletbook", "1.0.1", lock())
    crate_file(fixed, "scarletbook-ffi", "1.0.1", lock(**{"scarletbook-pcm": ("0.7.0", PCM_INDEX),
                                                          "scarletbook": ("1.0.1", sb)}))
    check("pending crates only: every pin is what crates.io will hold",
          rc.lockcheck(fixed, registry), [])

check("a pin of an unpublished version must match its packed file",
      rc.check_lock("scarletbook-ffi-1.0.1.crate",
                    lock(**{"scarletbook": ("1.0.1", "other")}),
                    {"scarletbook-1.0.1.crate": "sum"}, registry),
      ["scarletbook-ffi-1.0.1.crate: pins scarletbook 1.0.1 with checksum other, "
       "not the packed file's sum"])
check("a pin neither on crates.io nor packed",
      rc.check_lock("scarletbook-ffi-1.0.1.crate", lock(**{"scarletbook": ("1.0.1", "x")}),
                    {}, registry),
      ["scarletbook-ffi-1.0.1.crate: pins scarletbook 1.0.1, which is neither on crates.io "
       "nor packed with it"])
# A re-run after scarletbook 1.0.1 was published: ffi alone, scarletbook from crates.io.
check("a pin of a version published earlier matches crates.io",
      rc.check_lock("scarletbook-ffi-1.0.1.crate",
                    lock(**{"scarletbook": ("1.0.1", "sum-scarletbook-1.0.1")}), {},
                    index(**{"scarletbook": ["1.0.1"]})), [])

print(f"{failures} failures")
sys.exit(1 if failures else 0)
