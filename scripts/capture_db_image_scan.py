#!/usr/bin/env python3
"""Capture a remote-only, unfiltered scan of the pinned staging PostgreSQL image."""

import hashlib
import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request
from datetime import UTC, datetime
from pathlib import Path

INDEX = "sha256:9f4fd4aa918f52ae3d6f3b687f1de90d8f867cd4d520021545a9a50c120eae3b"
PLATFORM = "sha256:c7fedce47eff6a78cb35dad6b627f0d684107f75ab15c0870e40c093766ba452"
SCANNER = "sha256:85e87be1a96459c38a4eea47dc64eb2d342bb14cd4b4cef96adcf6ff03378b7c"
SCANNER_INDEX = "sha256:be1190afcb28352bfddc4ddeb71470835d16462af68d310f9f4bca710961a41e"
REGISTRY = "mirror.gcr.io"
SCANNER_IMAGE = f"{REGISTRY}/aquasec/trivy@{SCANNER}"
TARGET = f"{REGISTRY}/library/postgres@{PLATFORM}"
ACCEPT = ", ".join(
    (
        "application/vnd.oci.image.index.v1+json",
        "application/vnd.oci.image.manifest.v1+json",
        "application/vnd.docker.distribution.manifest.list.v2+json",
        "application/vnd.docker.distribution.manifest.v2+json",
    )
)


def sha(raw):
    return "sha256:" + hashlib.sha256(raw).hexdigest()


class RegistryFailure(RuntimeError):
    def __init__(self, repo, kind, reference, status):
        self.endpoint = f"{REGISTRY}/v2/{repo}/{kind}/{reference}"
        self.status = status
        super().__init__(f"{self.endpoint}: HTTP {status}")


def get_registry(repo, kind, reference):
    request = urllib.request.Request(
        f"https://{REGISTRY}/v2/{repo}/{kind}/{reference}", headers={"Accept": ACCEPT}
    )
    for delay in (15, 30, 0):
        try:
            with urllib.request.urlopen(request, timeout=60) as response:
                return response.read()
        except urllib.error.HTTPError as error:
            error.close()
            if error.code != 429 or not delay:
                raise RegistryFailure(repo, kind, reference, error.code) from None
            time.sleep(delay)


def capture_registry_identity(output, repo, index_digest, platform_digest, prefix):
    index_raw = get_registry(repo, "manifests", index_digest)
    if sha(index_raw) != index_digest:
        raise ValueError("registry index digest mismatch")
    index = json.loads(index_raw)
    matches = [
        entry
        for entry in index["manifests"]
        if entry.get("platform") == {"architecture": "amd64", "os": "linux"}
    ]
    if len(matches) != 1 or matches[0]["digest"] != platform_digest:
        raise ValueError("pinned index does not select expected AMD64 manifest")
    manifest_raw = get_registry(repo, "manifests", platform_digest)
    if sha(manifest_raw) != platform_digest:
        raise ValueError("platform digest mismatch")
    manifest = json.loads(manifest_raw)
    config_digest = manifest["config"]["digest"]
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", config_digest):
        raise ValueError("invalid image config digest")
    config_raw = get_registry(repo, "blobs", config_digest)
    if sha(config_raw) != config_digest:
        raise ValueError("image config digest mismatch")
    config = json.loads(config_raw)
    if config.get("os") != "linux" or config.get("architecture") != "amd64":
        raise ValueError("image config platform mismatch")
    for name, raw in (
        ("index.json", index_raw),
        ("manifest.json", manifest_raw),
        ("config.json", config_raw),
    ):
        (output / f"{prefix}{name}").write_bytes(raw)
    return config


def capture_identity(output):
    capture_registry_identity(output, "library/postgres", INDEX, PLATFORM, "")
    config = capture_registry_identity(output, "aquasec/trivy", SCANNER_INDEX, SCANNER, "scanner-")
    environment = config.get("config", {}).get("Env", [])
    if any(value.startswith("TRIVY_") for value in environment):
        raise ValueError("scanner image has a TRIVY environment override")


def make_receipt(revision, metadata, report, scanned_at):
    return {
        "schema_version": 1,
        "source_revision": revision,
        "manifest_digest": PLATFORM,
        "scanner_digest": SCANNER,
        "report_digest": sha(report),
        "scanned_at": scanned_at,
        "exit_code": 0,
        "database": {
            "version": metadata["Version"],
            "updated_at": metadata["UpdatedAt"],
            "next_update": metadata["NextUpdate"],
        },
        "scope": {
            "scanners": ["vuln"],
            "package_types": ["os", "library"],
            "severities": ["UNKNOWN", "LOW", "MEDIUM", "HIGH", "CRITICAL"],
            "include_unfixed": True,
            "include_all_packages": True,
            "show_suppressed": True,
            "ignore_rules": False,
            "vex": False,
        },
    }


def verify_staging_image(path):
    result = subprocess.run(
        [
            "docker",
            "compose",
            "--env-file",
            "/dev/null",
            "-f",
            str(path),
            "config",
            "--no-interpolate",
            "--format",
            "json",
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    document = json.loads(result.stdout)
    services = document.get("services") if isinstance(document, dict) else None
    db = services.get("db") if isinstance(services, dict) else None
    if not isinstance(db, dict) or db.get("image") != f"postgres:18.7-trixie@{INDEX}":
        raise ValueError("staging db image pin changed; review capture pins")


def write_status(output, stage, outcome, **details):
    status = {"stage": stage, "outcome": outcome, **details}
    (output / "capture-status.json").write_text(json.dumps(status, indent=2) + "\n")


def capture(output):
    write_status(output, "source", "running")
    uid, gid = os.geteuid(), os.getegid()
    if uid == 0 or gid == 0:
        raise ValueError("capture requires an unprivileged runner identity")
    revision = os.environ["CAPTURE_REVISION"]
    if not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError("invalid capture revision")
    actual = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    if actual != revision:
        raise ValueError("checkout revision mismatch")
    verify_staging_image(Path("deploy/staging/compose.yml"))
    write_status(output, "registry_identity", "running")
    capture_identity(output)
    (output / "empty.yml").write_text("{}\n")
    (output / "empty.ignore").write_text("")
    scratch = output / "scratch"
    scratch.mkdir(mode=0o700)
    command = [
        "docker",
        "run",
        "--rm",
        "--read-only",
        "--cap-drop=ALL",
        "--security-opt=no-new-privileges",
        "--user",
        f"{uid}:{gid}",
        "--tmpfs",
        "/tmp:rw,nosuid,nodev",
        "--mount",
        f"type=bind,src={output},dst=/inputs,readonly",
        "--mount",
        f"type=bind,src={scratch},dst=/scratch",
        SCANNER_IMAGE,
        "--config",
        "/inputs/empty.yml",
        "--cache-dir",
        "/scratch/cache",
        "image",
        "--image-src",
        "remote",
        "--format",
        "json",
        "--scanners",
        "vuln",
        "--pkg-types",
        "os,library",
        "--severity",
        "UNKNOWN,LOW,MEDIUM,HIGH,CRITICAL",
        "--pkg-relationships",
        "unknown,root,workspace,direct,indirect",
        "--list-all-pkgs",
        "--show-suppressed",
        "--exit-code",
        "0",
        "--ignorefile",
        "/inputs/empty.ignore",
        "--platform",
        "linux/amd64",
        "--output",
        "/scratch/report.json",
        TARGET,
    ]
    (output / "command.json").write_text(json.dumps(command, indent=2) + "\n")
    write_status(output, "scanner", "running")
    with (output / "scanner.log").open("w") as log:
        result = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, check=False)
    if result.returncode != 0:
        raise RuntimeError(f"scanner failed with exit code {result.returncode}")
    scanned_at = datetime.now(UTC).isoformat().replace("+00:00", "Z")
    report = (scratch / "report.json").read_bytes()
    json.loads(report)
    (output / "report.json").write_bytes(report)
    metadata_raw = (scratch / "cache" / "db" / "metadata.json").read_bytes()
    metadata = json.loads(metadata_raw)
    (output / "db-metadata.json").write_bytes(metadata_raw)
    receipt = make_receipt(revision, metadata, report, scanned_at)
    (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    write_status(output, "offline_audit", "running")
    audit = subprocess.run(
        [
            sys.executable,
            "scripts/inspect_image_scan.py",
            "--receipt",
            str(output / "receipt.json"),
            "--manifest",
            str(output / "manifest.json"),
            "--image-config",
            str(output / "config.json"),
            "--report",
            str(output / "report.json"),
            "--revision",
            revision,
            "--image-digest",
            PLATFORM,
            "--scanner-digest",
            SCANNER,
            "--platform",
            "linux/amd64",
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    (output / "audit.log").write_text(audit.stdout + audit.stderr)
    try:
        audit_result = json.loads(audit.stdout)
    except json.JSONDecodeError:
        raise ValueError("offline audit rejected capture; see audit.log") from None
    if (audit.returncode, audit_result.get("status")) not in (
        (0, "no_reported_high_or_critical"),
        (1, "blocked"),
    ):
        raise ValueError("offline audit did not validate capture; see audit.log")
    files = (
        "index.json",
        "manifest.json",
        "config.json",
        "scanner-index.json",
        "scanner-manifest.json",
        "scanner-config.json",
        "empty.yml",
        "empty.ignore",
        "report.json",
        "db-metadata.json",
        "receipt.json",
        "command.json",
        "scanner.log",
        "audit.log",
        "capture-status.json",
    )
    write_status(output, "complete", "success")
    (output / "SHA256SUMS").write_text(
        "".join(f"{sha((output / name).read_bytes())[7:]}  {name}\n" for name in files)
    )


def main():
    output = Path(os.environ["CAPTURE_OUTPUT"]).resolve()
    output.mkdir(mode=0o755, parents=True, exist_ok=True)
    if any(path.name != "capture-status.json" for path in output.iterdir()):
        raise ValueError("capture output must be empty")
    write_status(output, "initializing", "running")
    try:
        capture(output)
    except Exception as error:
        status = json.loads((output / "capture-status.json").read_text())
        details = {"error_type": type(error).__name__}
        if isinstance(error, RegistryFailure):
            details.update(endpoint=error.endpoint, http_status=error.status)
        write_status(output, status["stage"], "failed", **details)
        raise


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"capture failed: {error}", file=sys.stderr)
        sys.exit(1)
