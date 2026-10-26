"""Read Sonar evidence from the successful CI run for the exact candidate SHA."""

import io
import json
import subprocess
from zipfile import ZipFile

from audit_github import gh_json
from sonar_policy import validate_evidence


def require_sonar(repo, head):
    result = gh_json(
        "api", f"repos/{repo}/actions/workflows/ci.yml/runs?head_sha={head}&per_page=100"
    )
    runs = [
        r
        for r in result.get("workflow_runs", [])
        if r.get("head_sha") == head and r.get("event") in ("pull_request", "push")
    ]
    if not runs:
        raise ValueError("No CI workflow run for Sonar evidence")
    run = max(runs, key=lambda r: r["id"])
    if run.get("conclusion") != "success":
        raise ValueError("Latest CI workflow did not succeed")
    artifacts = gh_json("api", f"repos/{repo}/actions/runs/{run['id']}/artifacts?per_page=100")
    matching = [
        a
        for a in artifacts.get("artifacts", [])
        if a["name"] == f"sonar-{head}" and not a["expired"]
    ]
    if len(matching) != 1:
        raise ValueError("Sonar artifact missing, ambiguous or expired")
    payload = subprocess.check_output(
        ["gh", "api", f"repos/{repo}/actions/artifacts/{matching[0]['id']}/zip"]
    )
    with ZipFile(io.BytesIO(payload)) as archive:
        if archive.namelist() != ["sonar-evidence.json"]:
            raise ValueError("Unexpected Sonar artifact contents")
        if archive.getinfo("sonar-evidence.json").file_size > 100_000:
            raise ValueError("Oversized Sonar evidence")
        evidence = json.loads(archive.read("sonar-evidence.json"))
    validate_evidence(evidence, head)
