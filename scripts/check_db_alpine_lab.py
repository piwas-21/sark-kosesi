#!/usr/bin/env python3
"""Capture pinned registry identity and compare scoped Debian/Alpine scan reports."""

import json
import sys
from collections import Counter
from datetime import UTC, datetime
from pathlib import Path

from capture_db_image_scan import (
    capture_identity,
    capture_registry_identity,
    sha,
)
from inspect_image_scan import bounded_string

ALPINE_INDEX = "sha256:b6972e1115a4ff8eba5d7f7d19184926d1e16d7d1136ae1e1a7ebf286da0f015"
ALPINE_MANIFEST = "sha256:fa1b61f5a4844f4d4b73c9b15716cfc75fe70c931f2d32723630f3b900056366"
DEBIAN_CONFIG = "sha256:3cccc725163a95947178f4da5e9bf16add75cb286170ac5a2cd2f6513534955d"
ALPINE_CONFIG = "sha256:0f6c51e48b3458e87ab43692e49c9dafc72ffe0a0dbde3fd8cdb6ab92d5c025e"
TARGET_CVE = "CVE-2026-6653"


def capture_identities(output):
    capture_identity(output)
    capture_registry_identity(output, "library/postgres", ALPINE_INDEX, ALPINE_MANIFEST, "alpine-")
    for prefix, expected in (("", DEBIAN_CONFIG), ("alpine-", ALPINE_CONFIG)):
        manifest = json.loads((output / f"{prefix}manifest.json").read_bytes())
        config_raw = (output / f"{prefix}config.json").read_bytes()
        if manifest["config"]["digest"] != expected or sha(config_raw) != expected:
            raise ValueError("pinned image configuration mismatch")


def inspect(report, config, image_id, family, package_id):
    if not isinstance(report, dict) or report.get("SchemaVersion") != 2:
        raise ValueError("invalid report schema")
    meta = report.get("Metadata")
    if report.get("ArtifactType") != "container_image" or not isinstance(meta, dict):
        raise ValueError("invalid image report")
    if meta.get("ImageID") != image_id or meta.get("DiffIDs") != config["rootfs"]["diff_ids"]:
        raise ValueError("report image identity mismatch")
    image_config = meta.get("ImageConfig", {})
    if image_config.get("os") != "linux" or image_config.get("architecture") != "amd64":
        raise ValueError("report platform mismatch")
    os_info = meta.get("OS", {})
    if os_info.get("Family") != family or not os_info.get("Name"):
        raise ValueError("report operating system mismatch")
    targets = report.get("Results")
    if not isinstance(targets, list) or not targets:
        raise ValueError("missing report targets")
    os_targets = [t for t in targets if isinstance(t, dict) and t.get("Class") == "os-pkgs"]
    if len(os_targets) != 1 or os_targets[0].get("Type") != family:
        raise ValueError("missing operating system target")
    packages = os_targets[0].get("Packages")
    if not isinstance(packages, list) or not packages:
        raise ValueError("missing package inventory")
    matched = [p for p in packages if isinstance(p, dict) and p.get("Name") == "libxml2"]
    if len(matched) != 1 or matched[0].get("ID") != package_id:
        raise ValueError("unexpected libxml2 package identity")
    counts = Counter({key: 0 for key in ("UNKNOWN", "LOW", "MEDIUM", "HIGH", "CRITICAL")})
    hits = []
    for target in targets:
        if not isinstance(target, dict) or target.get("Class") not in ("os-pkgs", "lang-pkgs"):
            raise ValueError("invalid report target")
        modified = target.get("ExperimentalModifiedFindings", [])
        if not isinstance(modified, list) or modified:
            raise ValueError("modified findings cannot establish scope")
        inventory = target.get("Packages")
        if not isinstance(inventory, list) or not inventory:
            raise ValueError("empty target package inventory")
        if any(
            not isinstance(package, dict)
            or not bounded_string(package.get("Name"))
            or not bounded_string(package.get("Version"))
            for package in inventory
        ):
            raise ValueError("invalid target package inventory")
        findings = target.get("Vulnerabilities", [])
        if not isinstance(findings, list):
            raise ValueError("invalid findings")
        for finding in findings:
            if (
                not isinstance(finding, dict)
                or finding.get("Severity") not in counts
                or not bounded_string(finding.get("VulnerabilityID"))
                or not bounded_string(finding.get("PkgName"))
                or not bounded_string(finding.get("InstalledVersion"))
            ):
                raise ValueError("invalid finding")
            counts[finding["Severity"]] += 1
            if finding.get("VulnerabilityID") == TARGET_CVE:
                hits.append({"package": finding.get("PkgName"), "severity": finding["Severity"]})
    return {
        "image_id": image_id,
        "os": os_info["Name"],
        "libxml2": package_id,
        "severity_counts": dict(counts),
        "target_cve_findings": hits,
    }


def compare(base, alpine, base_config, alpine_config, database, now):
    if not isinstance(database, dict) or database.get("Version") != 2 or now.tzinfo is None:
        raise ValueError("invalid scan database or clock")
    try:
        updated = datetime.fromisoformat(database["UpdatedAt"].replace("Z", "+00:00"))
        expires = datetime.fromisoformat(database["NextUpdate"].replace("Z", "+00:00"))
        created = [
            datetime.fromisoformat(r["CreatedAt"].replace("Z", "+00:00")) for r in (base, alpine)
        ]
        if not all(updated <= time <= now < expires for time in created):
            raise ValueError("stale database or report timestamps")
    except (KeyError, TypeError, AttributeError):
        raise ValueError("invalid scan database or report timestamps") from None
    before = inspect(
        base, base_config, DEBIAN_CONFIG, "debian", "libxml2@2.12.7+dfsg+really2.9.14-2.1+deb13u3"
    )
    after = inspect(alpine, alpine_config, ALPINE_CONFIG, "alpine", "libxml2@2.13.9-r2")
    if not any(hit["package"] == "libxml2" for hit in before["target_cve_findings"]):
        raise ValueError("baseline positive CVE control missing")
    return {
        "target_cve": TARGET_CVE,
        "baseline": before,
        "alpine": after,
        "interpretation": "reported findings only; neither image is accepted or proven safe",
    }


def main():
    if len(sys.argv) == 3 and sys.argv[1] == "identity":
        capture_identities(Path(sys.argv[2]))
    elif len(sys.argv) == 3 and sys.argv[1] == "compare":
        root = Path(sys.argv[2])

        def load(name):
            return json.loads((root / name).read_bytes())

        for name, expected in (
            ("config.json", DEBIAN_CONFIG),
            ("alpine-config.json", ALPINE_CONFIG),
        ):
            if sha((root / name).read_bytes()) != expected:
                raise ValueError("image configuration digest changed")
        result = compare(
            load("base-report.json"),
            load("alpine-report.json"),
            load("config.json"),
            load("alpine-config.json"),
            load("db-metadata.json"),
            datetime.now(UTC),
        )
        print(json.dumps(result, indent=2))
    else:
        raise ValueError("expected identity or compare and an evidence directory")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, TypeError, KeyError) as error:
        print(f"alpine lab failed: {type(error).__name__}", file=sys.stderr)
        sys.exit(1)
