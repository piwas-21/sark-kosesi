"""Pure validation shared by merge/release tooling and hostile-fixture tests."""

import hashlib
import json

from review_policy import validate_review


def validate_checks(required, runs, head):
    if not required or len(set(required)) != len(required):
        raise ValueError("Required checks must be nonempty and unique")
    latest = {}
    for run in runs:
        if run.get("head_sha") != head:
            continue
        name = run.get("name")
        if name not in latest or run.get("id", 0) > latest[name].get("id", 0):
            latest[name] = run
    for name in required:
        run = latest.get(name)
        if not run or run.get("status") != "completed" or run.get("conclusion") != "success":
            raise ValueError(f"Required check missing/not successful for current head: {name}")
        if run.get("app", {}).get("slug") != "github-actions":
            raise ValueError(f"Unexpected check publisher: {name}")
    bad = [
        r["name"]
        for r in latest.values()
        if r.get("conclusion") in ("failure", "cancelled", "timed_out", "action_required")
    ]
    if bad:
        raise ValueError(f"Other checks failed: {bad}")


def validate_receipt(receipt, head, base, root):
    root = root.resolve()
    if receipt.get("head") != head or receipt.get("base") != base:
        raise ValueError("Independent review receipt is missing or stale")
    report = (root / receipt.get("report", "")).resolve()
    if not report.is_relative_to(root) or not report.is_file():
        raise ValueError("Review report missing or outside repository")
    if hashlib.sha256(report.read_bytes()).hexdigest() != receipt.get("reportSha256"):
        raise ValueError("Review report changed after receipt")
    validate_review(json.loads(report.read_text()), head, base)


def validate_release(config, evidence, head, root):
    root = root.resolve()
    if config.get("stage") != "application" or config.get("productionEnabled") is not True:
        raise ValueError("Production is disabled or application stage is incomplete")
    if not any(
        p.is_file() and p.suffix in (".ts", ".tsx", ".js") for p in (root / "apps").rglob("*")
    ):
        raise ValueError("Production application is absent")
    if not any((root / "crates").rglob("*.rs")):
        raise ValueError("Rust commerce application is absent")
    gates = config.get("productionGates", {})
    if not gates or any(state != "passed" for state in gates.values()):
        raise ValueError("Production gates are pending")
    if evidence.get("head") != head:
        raise ValueError("Release evidence is not bound to the current head")
    for key in gates:
        item = evidence.get("gates", {}).get(key, {})
        path = (root / item.get("evidence", "")).resolve()
        if item.get("status") != "passed" or not path.is_relative_to(root) or not path.is_file():
            raise ValueError(f"Missing current release evidence: {key}")
        if not path.stat().st_size:
            raise ValueError(f"Empty release evidence: {key}")
