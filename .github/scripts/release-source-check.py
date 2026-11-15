# SPDX-License-Identifier: MIT OR Apache-2.0
"""Refuse to release a commit that is not on the default branch or has not passed its checks.

    release-source-check.py OWNER/REPO COMMIT DEFAULT_BRANCH

Run by release.yml's `preflight` job before anything is built (issue #341). The commit must:

1. be reachable from the default branch (the compare API says `identical` or `behind`);
2. have passed the default branch's required status checks: the `required_status_checks`
   rules that apply to the branch (`GET /repos/.../rules/branches/BRANCH`), each matched by
   name against the commit's latest check runs from GitHub Actions. A run that concluded
   success, neutral or skipped passes, as for a pull request.

Checks still queued or running (the push to the default branch starts them, so a tag pushed
right after a merge finds them unfinished) are waited for, up to `--wait` minutes.

`dependency-review` is not expected on the commit: dependency-review.yml runs only on pull
requests, so a commit on the default branch never has that check run itself; the ruleset
made it pass on the pull request that brought the commit in.

Uses the workflow's token from GH_TOKEN (contents: read, checks: read). Exits with status 1
and a `::error::` line if the commit fails either condition.
"""

import argparse
import json
import os
import sys
import time
import urllib.request

# GitHub Actions' app id: only its check runs count, not a same-named one from another app.
GITHUB_ACTIONS_APP_ID = 15368
# Required checks whose workflow runs only on pull requests (see above).
PULL_REQUEST_ONLY = {"dependency-review"}
PASSED = {"success", "neutral", "skipped"}


def api(path: str):
    base = os.environ.get("GITHUB_API_URL", "https://api.github.com")
    req = urllib.request.Request(
        base + path,
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": "Bearer " + os.environ["GH_TOKEN"],
            "X-GitHub-Api-Version": "2022-11-28",
        },
    )
    with urllib.request.urlopen(req, timeout=60) as resp:
        return json.load(resp)


def required_checks(repo: str, branch: str) -> set[str]:
    names = set()
    for rule in api(f"/repos/{repo}/rules/branches/{branch}"):
        if rule["type"] == "required_status_checks":
            names.update(c["context"] for c in rule["parameters"]["required_status_checks"])
    return names


def check_runs(repo: str, sha: str) -> list[dict]:
    runs, page = [], 1
    while True:
        got = api(
            f"/repos/{repo}/commits/{sha}/check-runs"
            f"?filter=latest&app_id={GITHUB_ACTIONS_APP_ID}&per_page=100&page={page}"
        )
        runs += got["check_runs"]
        if len(runs) >= got["total_count"] or not got["check_runs"]:
            return runs
        page += 1


def fail(msg: str):
    print(f"::error::{msg}")
    sys.exit(1)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("repo")
    p.add_argument("sha")
    p.add_argument("branch")
    p.add_argument("--wait", type=float, default=45, help="minutes to wait for pending checks")
    args = p.parse_args()

    status = api(f"/repos/{args.repo}/compare/{args.branch}...{args.sha}")["status"]
    if status not in ("identical", "behind"):
        fail(f"commit {args.sha} is not on {args.branch} (compare: {status}); release only from {args.branch}")
    print(f"{args.sha} is on {args.branch} ({status})")

    required = required_checks(args.repo, args.branch)
    if not required:
        fail(f"no required status checks found for {args.branch}; cannot confirm the commit passed them")
    expected = sorted(required - PULL_REQUEST_ONLY)
    print("required checks:", ", ".join(sorted(required)))
    print("not expected on a pushed commit:", ", ".join(sorted(required & PULL_REQUEST_ONLY)) or "none")

    deadline = time.monotonic() + args.wait * 60
    while True:
        runs = check_runs(args.repo, args.sha)
        failed, pending = [], []
        for name in expected:
            mine = [r for r in runs if r["name"] == name]
            if not mine or any(r["status"] != "completed" for r in mine):
                pending.append(name)
            elif any(r["conclusion"] not in PASSED for r in mine):
                failed.append(f"{name} ({', '.join(r['conclusion'] for r in mine)})")
        if failed:
            fail(f"required checks did not pass on {args.sha}: {'; '.join(failed)}")
        if not pending:
            print("all required checks passed:", ", ".join(expected))
            return
        if time.monotonic() >= deadline:
            fail(f"required checks missing or unfinished on {args.sha} after {args.wait:g} min: {', '.join(pending)}")
        print("waiting for:", ", ".join(pending), flush=True)
        time.sleep(60)


if __name__ == "__main__":
    main()
