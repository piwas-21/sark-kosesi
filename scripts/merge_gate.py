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


def observe_json(endpoint):
    """One bounded, read-only observation; never retry a merge operation."""
    response = subprocess.run(
        ["gh", "api", endpoint],
        check=True,
        cwd=ROOT,
        text=True,
        capture_output=True,
        timeout=5,
    )
    return json.loads(response.stdout)


def reviewed_tree(head):
    """Read the immutable tree of the exact locally verified head."""
    response = subprocess.run(
        ["git", "rev-parse", "--verify", f"{head}^{{tree}}"],
        check=True,
        cwd=ROOT,
        text=True,
        capture_output=True,
        timeout=5,
    )
    tree = response.stdout.strip()
    if not re.fullmatch("[0-9a-f]{40}", tree):
        raise ValueError("Invalid reviewed tree")
    return tree


def observed_exact_merge(repo, number, head, base, base_ref, head_ref, protected_head):
    """Confirm only a recorded merge descended from the exact reviewed base."""
    pr = observe_json(f"repos/{repo}/pulls/{number}")
    if (
        pr.get("number") != number
        or pr.get("state") != "closed"
        or pr.get("merged") is not True
        or not isinstance(pr.get("merged_at"), str)
        or not pr["merged_at"]
        or pr.get("head", {}).get("sha") != head
        or pr.get("head", {}).get("ref") != head_ref
        or pr.get("base", {}).get("ref") != base_ref
    ):
        return False
    merged = pr.get("merge_commit_sha")
    if not isinstance(merged, str) or not re.fullmatch("[0-9a-f]{40}", merged):
        return False
    commit = observe_json(f"repos/{repo}/commits/{merged}")
    parents = commit.get("parents")
    expected = [base, head] if protected_head else [base]
    if (
        commit.get("sha") != merged
        or not isinstance(parents, list)
        or any(not isinstance(parent, dict) for parent in parents)
        or [parent.get("sha") for parent in parents] != expected
        or commit.get("commit", {}).get("tree", {}).get("sha") != reviewed_tree(head)
    ):
        return False
    ref = observe_json(f"repos/{repo}/git/ref/heads/{base_ref}")
    target = ref.get("object", {}).get("sha")
    if (
        ref.get("ref") != f"refs/heads/{base_ref}"
        or ref.get("object", {}).get("type") != "commit"
        or not isinstance(target, str)
        or not re.fullmatch("[0-9a-f]{40}", target)
    ):
        return False
    if target == merged:
        return True
    comparison = observe_json(f"repos/{repo}/compare/{merged}...{target}")
    return (
        comparison.get("status") == "ahead"
        and comparison.get("base_commit", {}).get("sha") == merged
        and comparison.get("merge_base_commit", {}).get("sha") == merged
    )


def merge_once(command, repo, number, head, base, base_ref, head_ref, protected_head):
    try:
        subprocess.run(command, check=True, cwd=ROOT)
    except (subprocess.CalledProcessError, OSError) as error:
        try:
            confirmed = observed_exact_merge(
                repo, number, head, base, base_ref, head_ref, protected_head
            )
        except (
            ValueError,
            OSError,
            KeyError,
            TypeError,
            AttributeError,
            subprocess.SubprocessError,
        ):
            confirmed = False
        if not confirmed:
            raise ValueError("Merge outcome unresolved; no retry was attempted") from error
        print("Exact merge observed on the target branch; branch deletion was not checked")


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
        merge_once(
            command, repo, args.pr, head, base, pr["baseRefName"], pr["headRefName"], protected_head
        )


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, KeyError, subprocess.CalledProcessError) as error:
        print(f"Merge gate failed closed: {error}", file=sys.stderr)
        sys.exit(1)
