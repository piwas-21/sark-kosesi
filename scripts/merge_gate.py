#!/usr/bin/env python3
"""Read-only by default. Merge only a reviewed head with live policy and CI evidence."""

import argparse
import json
import re
import subprocess
import sys

from audit_github import ROOT, audit, gh_json, pages
from gate_policy import validate_checks, validate_receipt
from sonar_evidence import require_sonar


def unresolved_threads(repo, number):
    owner, name = repo.split("/")
    query = """query($owner:String!,$name:String!,$number:Int!,$cursor:String){
      repository(owner:$owner,name:$name){pullRequest(number:$number){
      reviewThreads(first:100,after:$cursor){nodes{isResolved}pageInfo{hasNextPage endCursor}}}}}"""
    count, cursor = 0, None
    while True:
        args = [
            "api",
            "graphql",
            "-f",
            f"query={query}",
            "-f",
            f"owner={owner}",
            "-f",
            f"name={name}",
            "-F",
            f"number={number}",
        ]
        if cursor:
            args += ["-f", f"cursor={cursor}"]
        result = gh_json(*args)
        if result.get("errors"):
            raise ValueError("Review-thread query returned errors")
        data = result["data"]["repository"]["pullRequest"]["reviewThreads"]
        count += sum(not r["isResolved"] for r in data["nodes"])
        if not data["pageInfo"]["hasNextPage"]:
            return count
        cursor = data["pageInfo"]["endCursor"]


def require_codeql(repo, number, head, merge_sha):
    ref = f"refs/pull/{number}/merge"
    analyses = pages(f"repos/{repo}/code-scanning/analyses?ref={ref}&per_page=100")
    valid = [
        a
        for a in analyses
        if a.get("commit_sha") in (head, merge_sha)
        and a.get("ref") == ref
        and a.get("tool", {}).get("name") == "CodeQL"
        and not a.get("error")
        and a.get("rules_count", 0) > 0
    ]
    for language in ("javascript-typescript", "python"):
        if not any(language in a.get("category", "") for a in valid):
            raise ValueError(f"No populated current CodeQL analysis for {language}")
    alerts = pages(f"repos/{repo}/code-scanning/alerts?state=open&ref={ref}&per_page=100")
    if alerts:
        raise ValueError(
            f"{len(alerts)} open code-scanning alerts in PR scope; resolve or explicitly triage with rationale"
        )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pr", type=int)
    parser.add_argument("--merge", action="store_true")
    args = parser.parse_args()
    config = json.loads((ROOT / "config/delivery.json").read_text())
    repo = config["repository"]
    audit()
    pr = gh_json(
        "pr",
        "view",
        str(args.pr),
        "--repo",
        repo,
        "--json",
        "state,isDraft,headRefOid,baseRefOid,baseRefName,headRefName,reviewDecision,mergeStateStatus",
    )
    if pr["state"] != "OPEN" or pr["isDraft"] or pr["reviewDecision"] == "CHANGES_REQUESTED":
        raise ValueError("PR is not open/ready or has requested changes")
    if pr["baseRefName"] not in ("main", "develop") or (
        pr["baseRefName"] == "main" and pr["headRefName"] != "develop"
    ):
        raise ValueError("Invalid feature/release branch flow")
    head, base = pr["headRefOid"], pr["baseRefOid"]
    if not re.fullmatch("[0-9a-f]{40}", head) or not re.fullmatch("[0-9a-f]{40}", base):
        raise ValueError("Invalid revision data")
    if subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip() != head:
        raise ValueError("Local checkout is not the PR head")
    if subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT, text=True).strip():
        raise ValueError("Local checkout has unreviewed changes")
    subprocess.run(
        ["git", "fetch", "--quiet", "--no-tags", "origin", base, head], cwd=ROOT, check=True
    )
    subprocess.run(["git", "merge-base", "--is-ancestor", base, head], cwd=ROOT, check=True)
    receipt = json.loads((ROOT / ".local/reviews" / f"{head}.json").read_text())
    validate_receipt(receipt, head, base, ROOT)
    check_pages = gh_json(
        "api",
        "--paginate",
        "--slurp",
        f"repos/{repo}/commits/{head}/check-runs?filter=latest&per_page=100",
    )
    runs = [item for page in check_pages for item in page["check_runs"]]
    validate_checks(config["requiredChecks"], runs, head)
    if unresolved_threads(repo, args.pr):
        raise ValueError("Unresolved review conversations remain")
    rest = gh_json("api", f"repos/{repo}/pulls/{args.pr}")
    require_codeql(repo, args.pr, head, rest.get("merge_commit_sha"))
    require_sonar(repo, head)
    # Re-read after network checks; concurrent head/base changes invalidate the decision.
    fresh = gh_json("api", f"repos/{repo}/pulls/{args.pr}")
    if fresh["head"]["sha"] != head or fresh["base"]["sha"] != base:
        raise ValueError("PR head/base changed during verification")
    print(f"Merge gate passed for {repo}#{args.pr} at {head}")
    if args.merge:
        command = ["gh", "pr", "merge", str(args.pr), "--repo", repo, "--match-head-commit", head]
        protected_head = pr["headRefName"] in ("main", "develop")
        command += ["--merge"] if protected_head else ["--squash", "--delete-branch"]
        subprocess.run(command, check=True, cwd=ROOT)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, KeyError, subprocess.CalledProcessError) as error:
        print(f"Merge gate failed closed: {error}", file=sys.stderr)
        sys.exit(1)
