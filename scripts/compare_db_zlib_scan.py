#!/usr/bin/env python3
"""Compare same-database Trivy inventories for one isolated zlib candidate."""

import json
import re
import sys
from datetime import UTC, datetime
from pathlib import Path

BASE_CONFIG = "sha256:3cccc725163a95947178f4da5e9bf16add75cb286170ac5a2cd2f6513534955d"
BASE_ZLIB = "zlib1g@1:1.3.dfsg+really1.3.1-1+b1"
CANDIDATE_ZLIB = "zlib1g@1:1.3.dfsg+really1.3.1-1+deb13u1"
FINDINGS = {"CVE-2026-76844", "CVE-2026-85091"}


def inspect(report, image_id, package_id):
    if not isinstance(report, dict) or report.get("SchemaVersion") != 2:
        raise ValueError("invalid Trivy report schema")
    if report.get("ArtifactType") != "container_image":
        raise ValueError("report is not a container image")
    metadata = report.get("Metadata", {})
    if metadata.get("ImageID") != image_id:
        raise ValueError("scan image ID mismatch")
    os_info = metadata.get("OS", {})
    if os_info.get("Family") != "debian" or not os_info.get("Name"):
        raise ValueError("missing Debian OS identity")
    results = report.get("Results")
    if not isinstance(results, list):
        raise ValueError("missing scan targets")
    targets = [target for target in results if target.get("Class") == "os-pkgs"]
    if len(targets) != 1 or targets[0].get("Type") != "debian":
        raise ValueError("missing Debian package target")
    packages = targets[0].get("Packages")
    if not isinstance(packages, list) or not packages:
        raise ValueError("missing package inventory")
    zlib = [p for p in packages if p.get("Name") == "zlib1g"]
    if len(zlib) != 1 or zlib[0].get("ID") != package_id:
        raise ValueError("unexpected zlib package identity")
    vulnerabilities = targets[0].get("Vulnerabilities", [])
    if not isinstance(vulnerabilities, list):
        raise ValueError("malformed vulnerabilities")
    zlib_findings = [finding for finding in vulnerabilities if finding.get("PkgName") == "zlib1g"]
    return (
        {finding.get("VulnerabilityID") for finding in zlib_findings},
        {
            finding.get("VulnerabilityID")
            for finding in zlib_findings
            if finding.get("Severity") == "HIGH"
        },
    )


def compare(base, candidate, candidate_id, database):
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", candidate_id) or candidate_id == BASE_CONFIG:
        raise ValueError("invalid or unchanged candidate image ID")
    if not isinstance(database, dict) or database.get("Version") != 2:
        raise ValueError("invalid vulnerability database version")
    try:
        updated = datetime.fromisoformat(database["UpdatedAt"].replace("Z", "+00:00"))
        expires = datetime.fromisoformat(database["NextUpdate"].replace("Z", "+00:00"))
        created = [
            datetime.fromisoformat(r["CreatedAt"].replace("Z", "+00:00")) for r in (base, candidate)
        ]
        if not all(updated <= time <= datetime.now(UTC) < expires for time in created):
            raise ValueError("vulnerability database expired or timestamps disagree")
    except (KeyError, TypeError, AttributeError):
        raise ValueError("invalid vulnerability database or report timestamps") from None
    _, before_high = inspect(base, BASE_CONFIG, BASE_ZLIB)
    after, after_high = inspect(candidate, candidate_id, CANDIDATE_ZLIB)
    if not FINDINGS <= before_high:
        raise ValueError("baseline lacks the expected positive findings")
    if FINDINGS & after:
        raise ValueError("candidate still reports a targeted finding")
    return {
        "baseline_image_id": BASE_CONFIG,
        "candidate_image_id": candidate_id,
        "baseline_zlib": BASE_ZLIB,
        "candidate_zlib": CANDIDATE_ZLIB,
        "targeted_cves_reported_before": sorted(FINDINGS),
        "targeted_cves_reported_after": sorted(FINDINGS & after),
        "remaining_zlib_high": sorted(after_high),
        "meaning": "scanner comparison only; no reachability or risk acceptance",
    }


def main():
    if len(sys.argv) != 5:
        raise ValueError(
            "expected base report, candidate report, candidate image ID and DB metadata"
        )
    base = json.loads(Path(sys.argv[1]).read_bytes())
    candidate = json.loads(Path(sys.argv[2]).read_bytes())
    database = json.loads(Path(sys.argv[4]).read_bytes())
    print(json.dumps(compare(base, candidate, sys.argv[3], database), indent=2))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, TypeError, KeyError) as error:
        print(f"zlib scan comparison failed: {error}", file=sys.stderr)
        sys.exit(1)
