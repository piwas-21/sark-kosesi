"""Pure validation shared by merge/release tooling and hostile-fixture tests."""

import hashlib
import json
import os
import re
import stat
from contextlib import ExitStack, contextmanager
from pathlib import PurePosixPath

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


def valid_head(head):
    return isinstance(head, str) and re.fullmatch(r"[0-9a-f]{40}", head) is not None


@contextmanager
def release_directory(root, head):
    """Pin the revision directory for the entire manifest and artifact validation."""
    if not valid_head(head):
        raise ValueError("Invalid release head")
    if not hasattr(os, "O_NOFOLLOW") or not hasattr(os, "O_DIRECTORY"):
        raise ValueError("Safe release evidence access is unavailable")
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
    with ExitStack() as stack:
        try:
            directory = os.open(root, flags)
            stack.callback(os.close, directory)
            for part in (".local", "releases", head):
                directory = os.open(part, flags, dir_fd=directory)
                stack.callback(os.close, directory)
        except OSError:
            raise ValueError("Release evidence directory unavailable") from None
        yield directory


@contextmanager
def open_release_file(directory, name):
    if not isinstance(name, str) or not name or "\\" in name or "\x00" in name:
        raise ValueError("Invalid release evidence path")
    parts = name.split("/")
    if any(part in ("", ".", "..") for part in parts) or PurePosixPath(name).is_absolute():
        raise ValueError("Invalid release evidence path")
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
    with ExitStack() as stack:
        try:
            for part in parts[:-1]:
                directory = os.open(part, flags, dir_fd=directory)
                stack.callback(os.close, directory)
            # Nonblocking prevents a FIFO from hanging before its type can be checked.
            fd = os.open(parts[-1], os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory)
            artifact = stack.enter_context(os.fdopen(fd, "rb"))
            file_stat = os.fstat(artifact.fileno())
        except (OSError, UnicodeError):
            raise ValueError("Release evidence file unavailable") from None
        if not stat.S_ISREG(file_stat.st_mode):
            raise ValueError("Release evidence must be a regular file")
        yield artifact, (file_stat.st_dev, file_stat.st_ino)


def read_release_manifest(directory):
    def unique_object(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError("Duplicate manifest key")
            result[key] = value
        return result

    with open_release_file(directory, "evidence.json") as (manifest, identity):
        try:
            return json.load(manifest, object_pairs_hook=unique_object), identity
        except (OSError, UnicodeError, json.JSONDecodeError):
            raise ValueError("Release evidence manifest is unreadable or malformed") from None


def evidence_digest(artifact):
    digest = hashlib.sha256()
    nonblank = False
    try:
        while chunk := artifact.read(1024 * 1024):
            digest.update(chunk)
            nonblank |= bool(chunk.strip())
    except OSError:
        raise ValueError("Release evidence file unreadable") from None
    if not nonblank:
        raise ValueError("Release evidence is empty or blank")
    return digest.hexdigest()


def validate_release_environment(config, head, sources):
    if config.get("stage") != "application" or config.get("productionEnabled") is not True:
        raise ValueError("Production is disabled or application stage is incomplete")
    if not any(
        p.startswith("apps/") and PurePosixPath(p).suffix in (".ts", ".tsx", ".js") for p in sources
    ):
        raise ValueError("Production application is absent")
    if not any(p.startswith("crates/") and PurePosixPath(p).suffix == ".rs" for p in sources):
        raise ValueError("Rust commerce application is absent")
    gates = config.get("productionGates")
    if (
        not isinstance(gates, dict)
        or not gates
        or any(state != "passed" for state in gates.values())
    ):
        raise ValueError("Production gates are pending")
    if not valid_head(head):
        raise ValueError("Invalid release head")
    return gates


def validate_release(config, evidence, head, sources, manifest_identity, directory):
    gates = validate_release_environment(config, head, sources)
    if (
        not isinstance(evidence, dict)
        or set(evidence) != {"version", "head", "gates"}
        or type(evidence["version"]) is not int
        or evidence["version"] != 1
        or evidence["head"] != head
        or not isinstance(evidence["gates"], dict)
        or set(evidence["gates"]) != set(gates)
    ):
        raise ValueError("Invalid or stale release evidence manifest")
    identities, digests = {manifest_identity}, set()
    for key in gates:
        item = evidence["gates"][key]
        if (
            not isinstance(item, dict)
            or set(item) != {"gate", "head", "status", "evidence", "sha256"}
            or item["gate"] != key
            or item["head"] != head
            or item["status"] != "passed"
            or not isinstance(item["sha256"], str)
            or not re.fullmatch(r"[0-9a-f]{64}", item["sha256"])
            or item["evidence"] == "evidence.json"
        ):
            raise ValueError("Invalid current release evidence")
        with open_release_file(directory, item["evidence"]) as (artifact, identity):
            actual = evidence_digest(artifact)
        if identity in identities or actual in digests or actual != item["sha256"]:
            raise ValueError("Reused or changed release evidence")
        identities.add(identity)
        digests.add(actual)
