#!/usr/bin/env python3
"""Require live GitHub protection; stored configuration alone is not proof."""

import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def gh_json(*args):
    result = subprocess.run(["gh", *args], check=True, text=True, capture_output=True)
    return json.loads(result.stdout)


def pages(endpoint):
    values = gh_json("api", "--paginate", "--slurp", endpoint)
    return [item for page in values for item in page]


def audit():
    config = json.loads((ROOT / "config/delivery.json").read_text())
    repo = config["repository"]
    info = gh_json("api", f"repos/{repo}")
    if info.get("default_branch") != "develop":
        raise ValueError("Default branch is not develop")
    candidates = pages(f"repos/{repo}/rulesets?per_page=100")
    candidate = next(
        (r for r in candidates if r["name"] == "main-develop" and r["enforcement"] == "active"),
        None,
    )
    if not candidate:
        raise ValueError(
            "Active main-develop server rules missing; private GitHub Free is insufficient"
        )
    rule = gh_json("api", f"repos/{repo}/rulesets/{candidate['id']}")
    if rule.get("bypass_actors"):
        raise ValueError("Ruleset has bypass actors")
    condition = rule["conditions"]["ref_name"]
    if (
        not {"refs/heads/main", "refs/heads/develop"} <= set(condition["include"])
        or condition["exclude"]
    ):
        raise ValueError("Both protected branches must be covered without exclusions")
    by_type = {r["type"]: r.get("parameters", {}) for r in rule["rules"]}
    if (
        not {"deletion", "non_fast_forward", "pull_request", "required_status_checks"}
        <= by_type.keys()
    ):
        raise ValueError("Missing required branch rule")
    if not by_type["pull_request"].get("required_review_thread_resolution"):
        raise ValueError("Unresolved conversations are not blocked")
    checks = by_type["required_status_checks"]
    contexts = {c["context"] for c in checks["required_status_checks"]}
    if (
        not set(config["requiredChecks"]) <= contexts
        or not checks["strict_required_status_checks_policy"]
    ):
        raise ValueError("Required checks/current-base enforcement incomplete")
    security = info.get("security_and_analysis", {})
    for feature in ("secret_scanning", "secret_scanning_push_protection"):
        if security.get(feature, {}).get("status") != "enabled":
            raise ValueError(f"GitHub security feature unavailable/disabled: {feature}")
    permissions = gh_json("api", f"repos/{repo}/actions/permissions/workflow")
    if permissions.get("default_workflow_permissions") != "read" or permissions.get(
        "can_approve_pull_request_reviews"
    ):
        raise ValueError("Default Actions permissions exceed policy")
    return {
        "repository": repo,
        "visibility": info["visibility"],
        "ruleset": rule["id"],
        "requiredChecks": len(contexts),
    }


if __name__ == "__main__":
    try:
        print(json.dumps(audit(), indent=2))
    except (ValueError, KeyError, subprocess.CalledProcessError) as error:
        print(f"GitHub protection audit failed: {error}", file=sys.stderr)
        sys.exit(1)
