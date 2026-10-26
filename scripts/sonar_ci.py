#!/usr/bin/env python3
"""Provision and verify a disposable, loopback-only SonarQube CI server."""

import argparse
import base64
import json
import os
import secrets
import time
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.parse import urlencode
from urllib.request import Request, urlopen

from sonar_policy import PROJECT, validate_analysis

HOST = "http://127.0.0.1:9000"


def api(path, credential, data=None, **query):
    if not path.startswith("api/") or ".." in path:
        raise ValueError("Invalid Sonar API path")
    address = f"{HOST}/{path}?{urlencode(query)}"
    request = Request(
        address,
        data=urlencode(data).encode() if data is not None else None,
        headers={"Authorization": "Basic " + base64.b64encode(credential.encode()).decode()},
    )
    try:
        with urlopen(request, timeout=20) as response:
            body = response.read()
    except HTTPError as error:
        detail = error.read(2048).decode("utf-8", errors="replace")
        sensitive = [credential, *credential.split(":"), *(str(v) for v in (data or {}).values())]
        for value in sensitive:
            if value:
                detail = detail.replace(value, "[redacted]")
        error.add_note(f"Sonar API {path}: {detail}")
        raise
    return json.loads(body) if body else {}


def wait_ready():
    deadline = time.monotonic() + 180
    while time.monotonic() < deadline:
        try:
            if api("api/system/status", "").get("status") == "UP":
                return
        except (URLError, TimeoutError, ConnectionError):
            pass
        time.sleep(2)
    raise ValueError("Sonar server did not become ready within three minutes")


def bootstrap():
    wait_ready()
    password = "Aa1!" + secrets.token_urlsafe(36)
    print(f"::add-mask::{password}", flush=True)
    api(
        "api/users/change_password",
        "admin:admin",
        data={"login": "admin", "previousPassword": "admin", "password": password},
    )
    credential = "admin:" + password
    api("api/projects/create", credential, data={"project": PROJECT, "name": "Piwas commerce CI"})
    gate = api("api/qualitygates/create", credential, data={"name": "Piwas owned source"})
    for metric in ("reliability_rating", "security_rating", "sqale_rating"):
        api(
            "api/qualitygates/create_condition",
            credential,
            data={"gateName": gate["name"], "metric": metric, "op": "GT", "error": "1"},
        )
    api(
        "api/qualitygates/select",
        credential,
        data={"gateName": gate["name"], "projectKey": PROJECT},
    )
    token = api("api/user_tokens/generate", credential, data={"name": "disposable-ci"})["token"]
    print(f"::add-mask::{token}", flush=True)
    with Path(os.environ["GITHUB_ENV"]).open("a") as output:
        output.write(f"SONAR_TOKEN={token}\nSONAR_HOST_URL={HOST}\n")
    print("Disposable Sonar server configured; credentials masked")


def completed_analysis(credential):
    properties = dict(
        line.split("=", 1)
        for line in Path(".scannerwork/report-task.txt").read_text().splitlines()
        if "=" in line
    )
    if properties.get("projectKey") != PROJECT or properties.get("serverUrl") != HOST:
        raise ValueError("Scanner report targets the wrong server/project")
    for _ in range(90):
        task = api("api/ce/task", credential, id=properties["ceTaskId"])["task"]
        if task["status"] == "SUCCESS":
            if task.get("componentKey") != PROJECT or not task.get("analysisId"):
                raise ValueError("Sonar completed the wrong project/task")
            return task["analysisId"]
        if task["status"] in ("FAILED", "CANCELED"):
            raise ValueError("Sonar analysis task failed")
        time.sleep(2)
    raise ValueError("Sonar analysis did not complete within three minutes")


def verify():
    credential = os.environ["SONAR_TOKEN"] + ":"
    analysis_id = completed_analysis(credential)
    # Wrong-project component lookup must fail; an empty issues response is not identity proof.
    try:
        api("api/components/show", credential, component="nonexistent-negative-control")
    except HTTPError as error:
        if error.code != 404:
            raise ValueError("Sonar identity negative control failed unexpectedly") from error
    else:
        raise ValueError("Sonar accepted nonexistent project identity")
    component = api("api/components/show", credential, component=PROJECT)
    analyses = api("api/project_analyses/search", credential, project=PROJECT, ps=1)
    gate = api("api/qualitygates/project_status", credential, analysisId=analysis_id)
    measures = api("api/measures/component", credential, component=PROJECT, metricKeys="ncloc")
    issues = api("api/issues/search", credential, componentKeys=PROJECT, resolved="false", ps=100)
    hotspots = api("api/hotspots/search", credential, project=PROJECT, status="TO_REVIEW", ps=100)
    for issue in issues.get("issues", []):
        print(
            json.dumps({k: issue.get(k) for k in ("key", "rule", "component", "line", "message")})
        )
    for hotspot in hotspots.get("hotspots", []):
        print(json.dumps({k: hotspot.get(k) for k in ("key", "component", "line", "message")}))
    print(json.dumps({"qualityGate": gate.get("projectStatus")}))
    evidence = validate_analysis(
        component,
        analyses,
        analysis_id,
        os.environ["ANALYSIS_REVISION"],
        gate,
        measures,
        issues,
        hotspots,
    )
    target = Path(".local/sonar-evidence.json")
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(json.dumps(evidence, indent=2) + "\n")
    print(
        "Sonar analysis verified: current revision, populated gate, zero unresolved issues/hotspots"
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("bootstrap", "verify"))
    action = parser.parse_args().action
    bootstrap() if action == "bootstrap" else verify()
