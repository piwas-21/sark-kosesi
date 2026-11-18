#!/usr/bin/env python3
"""Compare paired full-image PostgreSQL scans and validate report consistency."""

import hashlib
import json
import re
import sys
from collections import Counter, defaultdict
from datetime import UTC, datetime
from pathlib import Path

from compare_db_zlib_scan import BASE_CONFIG as DEBIAN_CONFIG
from compare_db_zlib_scan import BASE_ZLIB, CANDIDATE_ZLIB, FINDINGS, POSITIVE_FINDING
from compare_db_zlib_scan import inspect as inspect_zlib

BASE_CONFIG = "sha256:0f6c51e48b3458e87ab43692e49c9dafc72ffe0a0dbde3fd8cdb6ab92d5c025e"
DIGEST = re.compile(r"sha256:[0-9a-f]{64}\Z")
SEVERITIES = ("UNKNOWN", "LOW", "MEDIUM", "HIGH", "CRITICAL")
LIMIT = 64 * 1024 * 1024
CANDIDATE_MODULES = {
    "github.com/tianon/gosu": None,
    "github.com/moby/sys/user": "v0.4.1",
    "golang.org/x/sys": "v0.1.0",
    "stdlib": "v1.27.4",
}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def bounded(value):
    return isinstance(value, str) and 0 < len(value) <= 512 and not any(ord(c) < 32 for c in value)


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, "duplicate JSON member")
        result[key] = value
    return result


def read_document(path):
    raw = path.read_bytes()
    require(0 < len(raw) <= LIMIT, "empty or oversized evidence")
    document = json.loads(raw, object_pairs_hook=unique_object)
    require(isinstance(document, dict), "expected JSON object")
    return raw, document


def timestamp(value):
    require(
        isinstance(value, str)
        and re.fullmatch(
            r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?(?:Z|[+-]\d{2}:\d{2})", value
        )
        is not None,
        "invalid timestamp",
    )
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        raise ValueError("invalid timestamp") from None


def inspect(
    report,
    config_raw,
    config,
    image_id,
    gosu_version,
    allow_unversioned_main=False,
    *,
    family="alpine",
):
    require(
        "sha256:" + hashlib.sha256(config_raw).hexdigest() == image_id, "config digest mismatch"
    )
    require(
        config.get("os") == "linux" and config.get("architecture") == "amd64",
        "config platform mismatch",
    )
    rootfs = config.get("rootfs")
    require(isinstance(rootfs, dict) and rootfs.get("type") == "layers", "invalid config rootfs")
    diff_ids = rootfs.get("diff_ids")
    require(
        isinstance(diff_ids, list)
        and bool(diff_ids)
        and all(isinstance(item, str) and DIGEST.fullmatch(item) for item in diff_ids),
        "invalid config diff IDs",
    )
    require(
        type(report.get("SchemaVersion")) is int
        and report["SchemaVersion"] == 2
        and report.get("ArtifactType") == "container_image",
        "invalid Trivy report",
    )
    metadata = report.get("Metadata")
    require(
        isinstance(metadata, dict)
        and metadata.get("ImageID") == image_id
        and metadata.get("DiffIDs") == diff_ids,
        "scan image identity mismatch",
    )
    platform, os_info = metadata.get("ImageConfig"), metadata.get("OS")
    require(
        isinstance(platform, dict)
        and platform.get("os") == "linux"
        and platform.get("architecture") == "amd64",
        "scan platform mismatch",
    )
    require(
        isinstance(os_info, dict)
        and os_info.get("Family") == family
        and bounded(os_info.get("Name")),
        "invalid scanner OS identity",
    )
    targets = report.get("Results")
    require(isinstance(targets, list) and bool(targets), "missing scan targets")
    rows, gosu, os_targets = [], [], 0
    for target in targets:
        require(
            isinstance(target, dict)
            and target.get("Class") in ("os-pkgs", "lang-pkgs")
            and bounded(target.get("Target"))
            and bounded(target.get("Type")),
            "invalid scan target",
        )
        require(
            target.get("ExperimentalModifiedFindings", []) == [],
            "modified findings cannot establish scope",
        )
        packages = target.get("Packages")
        require(isinstance(packages, list) and bool(packages), "missing package inventory")
        require(
            all(
                isinstance(p, dict)
                and bounded(p.get("Name"))
                and (
                    bounded(p.get("Version"))
                    or (
                        allow_unversioned_main
                        and target["Class"] == "lang-pkgs"
                        and target["Type"] == "gobinary"
                        and target["Target"].lstrip("/") == "usr/local/bin/gosu"
                        and p.get("Name") == "github.com/tianon/gosu"
                        and p.get("ID") == "github.com/tianon/gosu"
                        and p.get("Relationship") == "root"
                        and "Version" not in p
                    )
                )
                for p in packages
            ),
            "invalid package inventory",
        )
        if target["Class"] == "os-pkgs":
            require(target["Type"] == family, "unexpected OS package type")
            os_targets += 1
        if target["Class"] == "lang-pkgs" and target["Target"].lstrip("/") == "usr/local/bin/gosu":
            require(target["Type"] == "gobinary", "unexpected Gosu target type")
            gosu.append(target)
            require(
                [p["Version"] for p in packages if p["Name"] == "stdlib"] == [gosu_version],
                "unexpected Gosu stdlib version",
            )
        findings = target.get("Vulnerabilities", [])
        require(isinstance(findings, list), "invalid findings")
        for finding in findings:
            require(
                isinstance(finding, dict)
                and all(
                    bounded(finding.get(key))
                    for key in ("VulnerabilityID", "PkgName", "InstalledVersion")
                )
                and finding.get("Severity") in SEVERITIES,
                "invalid finding",
            )
            rows.append(
                {
                    "target": target["Target"],
                    "class": target["Class"],
                    "type": target["Type"],
                    "finding": finding,
                }
            )
    require(os_targets == 1 and len(gosu) == 1, "missing or duplicate OS or Gosu target")
    return rows, gosu[0], os_info["Name"]


def validate_candidate_modules(target):
    packages = target["Packages"]
    require(len(packages) == len(CANDIDATE_MODULES), "unexpected candidate module count")
    seen = set()
    for package in packages:
        name = package["Name"]
        require(name in CANDIDATE_MODULES and name not in seen, "unexpected candidate module")
        seen.add(name)
        version = CANDIDATE_MODULES[name]
        suffix = f"@{version}" if version is not None else ""
        require(package.get("ID") == name + suffix, "candidate module ID mismatch")
        require(package.get("Version") == version, "candidate module version mismatch")
        identifier = package.get("Identifier")
        require(
            isinstance(identifier, dict)
            and identifier.get("PURL") == "pkg:golang/" + name + suffix,
            "candidate module PURL mismatch",
        )
        if version is None:
            require(
                "Version" not in package and package.get("Relationship") == "root",
                "candidate root mismatch",
            )
    require(seen == set(CANDIDATE_MODULES), "missing candidate module")


def comparison_target(row):
    # The image reference in an OS target varies between remote and archive scans.
    return "<os-packages>" if row["class"] == "os-pkgs" else row["target"]


def row_key(row):
    canonical = {**row, "target": comparison_target(row)}
    return json.dumps(canonical, sort_keys=True, separators=(",", ":"), ensure_ascii=True)


def logical_key(row):
    finding = row["finding"]
    return (
        comparison_target(row),
        row["class"],
        row["type"],
        finding["VulnerabilityID"],
        finding["PkgName"],
    )


def difference(left, right):
    available = Counter(map(row_key, right))
    unmatched = []
    for row in left:
        key = row_key(row)
        if available[key]:
            available[key] -= 1
        else:
            unmatched.append(row)
    return sorted(unmatched, key=row_key)


def compare(
    stock,
    candidate,
    stock_raw,
    stock_config,
    candidate_raw,
    candidate_config,
    candidate_id,
    database,
    now=None,
    *,
    debian=False,
):
    stock_id = DEBIAN_CONFIG if debian else BASE_CONFIG
    family = "debian" if debian else "alpine"
    require(
        isinstance(candidate_id, str)
        and DIGEST.fullmatch(candidate_id)
        and candidate_id != stock_id,
        "invalid candidate image ID",
    )
    require(
        isinstance(database, dict)
        and type(database.get("Version")) is int
        and database["Version"] == 2,
        "invalid vulnerability database version",
    )
    updated, expires = timestamp(database.get("UpdatedAt")), timestamp(database.get("NextUpdate"))
    now = now or datetime.now(UTC)
    require(
        all(
            updated <= timestamp(report.get("CreatedAt")) <= now < expires
            for report in (stock, candidate)
        ),
        "stale or future vulnerability database or scan",
    )
    before, stock_gosu, stock_os = inspect(
        stock, stock_raw, stock_config, stock_id, "v1.24.6", family=family
    )
    after, candidate_gosu, candidate_os = inspect(
        candidate,
        candidate_raw,
        candidate_config,
        candidate_id,
        "v1.27.4",
        allow_unversioned_main=True,
        family=family,
    )
    validate_candidate_modules(candidate_gosu)
    require(stock_os == candidate_os, "OS version mismatch")
    require(
        any(
            f["VulnerabilityID"] == "CVE-2025-68121"
            and f["PkgName"] == "stdlib"
            and f["InstalledVersion"] == "v1.24.6"
            and f["Severity"] in ("HIGH", "CRITICAL")
            for f in stock_gosu.get("Vulnerabilities", [])
        ),
        "baseline lacks expected positive Gosu finding",
    )
    if debian:
        _, before_zlib_high = inspect_zlib(stock, stock_id, BASE_ZLIB)
        after_zlib, _ = inspect_zlib(candidate, candidate_id, CANDIDATE_ZLIB)
        other_packages = [
            Counter(
                (p["Name"], p["Version"])
                for target in report["Results"]
                if target["Class"] == "os-pkgs"
                for p in target["Packages"]
                if p["Name"] != "zlib1g"
            )
            for report in (stock, candidate)
        ]
        require(other_packages[0] == other_packages[1], "unexpected Debian package change")
        require(
            POSITIVE_FINDING in before_zlib_high, "baseline lacks expected positive zlib finding"
        )
        require(not FINDINGS & after_zlib, "candidate still reports targeted zlib finding")
        require(
            not any(
                finding["VulnerabilityID"] == "CVE-2025-68121" and finding["PkgName"] == "stdlib"
                for finding in candidate_gosu.get("Vulnerabilities", [])
            ),
            "candidate still reports targeted Gosu finding",
        )
    removed, added = difference(before, after), difference(after, before)
    old_groups, new_groups = defaultdict(list), defaultdict(list)
    for row in removed:
        old_groups[logical_key(row)].append(row)
    for row in added:
        new_groups[logical_key(row)].append(row)
    changed = [
        {"identity": list(key), "stock": old_groups[key], "candidate": new_groups[key]}
        for key in sorted(old_groups.keys() & new_groups.keys())
    ]

    def counts(rows):
        result = Counter(row["finding"]["Severity"] for row in rows)
        return {severity: result[severity] for severity in SEVERITIES}

    return {
        "stock_image_id": stock_id,
        "candidate_image_id": candidate_id,
        "stock_gosu_stdlib": "v1.24.6",
        "candidate_gosu_stdlib": "v1.27.4",
        "severity_counts": {"stock": counts(before), "candidate": counts(after)},
        "finding_counts": {
            "stock": len(before),
            "candidate": len(after),
            "added_rows": len(added),
            "removed_rows": len(removed),
        },
        "stock_findings": before,
        "candidate_findings": after,
        "added_rows": added,
        "removed_rows": removed,
        "changed_groups": changed,
        "meaning": "Reported findings only; capture scope, scanner provenance, reachability and risk acceptance require independent verification.",
    }


def main():
    args = sys.argv[1:]
    debian = args[:1] == ["--debian"]
    if debian:
        args = args[1:]
    if len(args) != 2:
        raise ValueError("expected EVIDENCE_DIR CANDIDATE_CONFIG_SHA256")
    directory = Path(args[0])
    _, stock = read_document(directory / "stock-report.json")
    _, candidate = read_document(directory / "candidate-report.json")
    stock_raw, stock_config = read_document(directory / "stock-config.json")
    candidate_raw, candidate_config = read_document(directory / "candidate-config.json")
    _, database = read_document(directory / "db-metadata.json")
    print(
        json.dumps(
            compare(
                stock,
                candidate,
                stock_raw,
                stock_config,
                candidate_raw,
                candidate_config,
                args[1],
                database,
                debian=debian,
            ),
            indent=2,
        )
    )


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, TypeError, KeyError, RecursionError) as error:
        print(f"PostgreSQL Gosu scan comparison failed: {error}", file=sys.stderr)
        sys.exit(1)
