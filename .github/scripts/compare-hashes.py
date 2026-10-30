# SPDX-License-Identifier: MIT OR Apache-2.0
"""Compares `bench ... --sink hash` output with the conformance manifest.

Usage: compare-hashes.py CASE_ID AREA(2ch|mc) OUTPUT_FILE
"""
import json
import sys

case_id, area, path = sys.argv[1:4]
got = open(path).read().splitlines()
cases = json.load(open("corpus/manifest.json"))["cases"]
case = next(c for c in cases if c["id"] == case_id)
slot = {"2ch": "2ch", "mc": "multichannel"}[area]
tracks = next(a for a in case["expected"]["audio"] if a["area"] == slot)["tracks"]
want = [
    "\t".join([str(t["number"])] + (t["sha256"] if t["outcome"] == "ok" else [t["error"]]))
    for t in tracks
]
if got != want:
    sys.exit(f"{case_id} {area}: bench output differs from the manifest\ngot:  {got}\nwant: {want}")
print(f"{case_id} {area}: {len(got)} tracks match the manifest")
