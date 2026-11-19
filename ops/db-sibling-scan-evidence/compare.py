#!/usr/bin/env python3
"""Fail-closed comparison of two unsuppressed, same-database Trivy reports."""

import importlib.util
import json
import re
import sys
from collections import Counter
from datetime import UTC, datetime
from pathlib import Path

from check_db_alpine_gosu import bounded, difference, read_document, timestamp

SEVERITIES = ("UNKNOWN", "LOW", "MEDIUM", "HIGH", "CRITICAL")
STATUSES = {
    "unknown",
    "not_affected",
    "affected",
    "fixed",
    "under_investigation",
    "will_not_fix",
    "fix_deferred",
    "end_of_life",
}
STOCK_ID = "sha256:3cccc725163a95947178f4da5e9bf16add75cb286170ac5a2cd2f6513534955d"
POSITIVE = ("CVE-2026-6653", "libxml2")


def require(condition, message):
    if not condition:
        raise ValueError(message)


def inspect(report, config, image_id):
    require(
        isinstance(report, dict)
        and type(report.get("SchemaVersion")) is int
        and report["SchemaVersion"] == 2
        and report.get("ArtifactType") == "container_image",
        "invalid scan report",
    )
    root = config.get("rootfs", {})
    diffs = root.get("diff_ids")
    require(
        config.get("os") == "linux"
        and config.get("architecture") == "amd64"
        and root.get("type") == "layers"
        and isinstance(diffs, list)
        and bool(diffs),
        "invalid image config",
    )
    metadata = report.get("Metadata")
    require(
        isinstance(metadata, dict)
        and metadata.get("ImageID") == image_id
        and metadata.get("DiffIDs") == diffs,
        "scan identity mismatch",
    )
    platform = metadata.get("ImageConfig", {})
    os_info = metadata.get("OS", {})
    require(
        platform.get("os") == "linux"
        and platform.get("architecture") == "amd64"
        and os_info.get("Family") == "debian"
        and bounded(os_info.get("Name")),
        "invalid scan platform",
    )
    targets = report.get("Results")
    require(isinstance(targets, list) and bool(targets), "empty scan targets")
    rows, os_count, packages, language_packages = [], 0, [], []
    for target in targets:
        require(
            isinstance(target, dict)
            and target.get("Class") in ("os-pkgs", "lang-pkgs")
            and bounded(target.get("Target"))
            and bounded(target.get("Type")),
            "invalid scan target",
        )
        for key, value in target.items():
            if "suppressed" in key.lower() or "modifiedfindings" in key.lower():
                require(value == [], "suppressed or modified findings")
        inventory = target.get("Packages")
        require(isinstance(inventory, list) and bool(inventory), "missing package inventory")
        for package in inventory:
            require(
                isinstance(package, dict) and bounded(package.get("Name")), "invalid package record"
            )
            version = package.get("Version")
            require(
                bounded(version)
                or (
                    target["Class"] == "lang-pkgs"
                    and package.get("Relationship") == "root"
                    and package.get("ID") == package.get("Name")
                    and version is None
                ),
                "invalid package version",
            )
        if target["Class"] == "os-pkgs":
            require(target["Type"] == "debian", "unexpected OS target")
            os_count += 1
            packages = inventory
        else:
            require(target["Type"] == "gobinary", "unsupported language package target")
            language_packages.extend(inventory)
        findings = target.get("Vulnerabilities", [])
        require(isinstance(findings, list), "invalid vulnerability records")
        for finding in findings:
            require(
                isinstance(finding, dict)
                and all(
                    bounded(finding.get(key))
                    for key in ("VulnerabilityID", "PkgName", "InstalledVersion")
                )
                and finding.get("Severity") in SEVERITIES
                and finding.get("Status") in STATUSES,
                "invalid vulnerability record",
            )
            require(
                all(
                    value is False or value == []
                    for key, value in finding.items()
                    if "suppress" in key.lower()
                ),
                "suppressed or unknown suppression state",
            )
            rows.append(
                {
                    "target": target["Target"],
                    "class": target["Class"],
                    "type": target["Type"],
                    "finding": finding,
                }
            )
    require(os_count == 1, "missing or duplicate OS package inventory")
    return rows, packages, language_packages, os_info["Name"]


def check_dpkg(packages, dpkg):
    actual = {}
    for package in packages:
        name = package["Name"]
        require(
            name not in actual
            and bounded(package.get("ID"))
            and package["ID"].startswith(name + "@"),
            "invalid Debian package identity",
        )
        actual[name] = package["ID"][len(name) + 1 :]
    require(actual == dpkg, "scanner and installed dpkg inventories differ")


def load_dpkg(text):
    path = Path("ops/db-sibling-runtime-root/validate.py")
    spec = importlib.util.spec_from_file_location("sibling_root_validate", path)
    require(spec is not None and spec.loader is not None, "dpkg validator unavailable")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.parse_live_packages(text)


def check_sbom(sbom, packages, language_packages):
    require(
        isinstance(sbom, dict)
        and sbom.get("bomFormat") == "CycloneDX"
        and bounded(sbom.get("specVersion")),
        "invalid CycloneDX SBOM",
    )
    components = sbom.get("components")
    require(
        isinstance(components, list)
        and bool(components)
        and all(isinstance(c, dict) for c in components),
        "empty SBOM",
    )
    pairs = {(c.get("name"), c.get("version")) for c in components}
    for package in packages:
        name = package["Name"]
        require(
            (name, package["ID"][len(name) + 1 :]) in pairs,
            "SBOM is missing an installed OS package",
        )
    for package in language_packages:
        require(
            (package["Name"], package.get("Version")) in pairs,
            "SBOM is missing a detected Go package",
        )
    return len(components)


def compare(
    stock,
    candidate,
    stock_config,
    candidate_config,
    candidate_id,
    db,
    stock_sbom,
    candidate_sbom,
    stock_dpkg,
    candidate_dpkg,
    now,
):
    require(
        re.fullmatch(r"sha256:[0-9a-f]{64}", candidate_id) is not None and candidate_id != STOCK_ID,
        "invalid candidate identity",
    )
    require(
        isinstance(db, dict)
        and type(db.get("Version")) is int
        and db["Version"] == 2
        and now.tzinfo is not None,
        "invalid database metadata",
    )
    updated, expires = timestamp(db.get("UpdatedAt")), timestamp(db.get("NextUpdate"))
    require(
        all(updated <= timestamp(r.get("CreatedAt")) <= now < expires for r in (stock, candidate)),
        "stale database or report",
    )
    before, stock_packages, stock_languages, stock_os = inspect(stock, stock_config, STOCK_ID)
    after, candidate_packages, candidate_languages, candidate_os = inspect(
        candidate, candidate_config, candidate_id
    )
    require(stock_os == candidate_os, "OS version mismatch")
    check_dpkg(stock_packages, stock_dpkg)
    check_dpkg(candidate_packages, candidate_dpkg)
    require(
        any(
            (r["finding"]["VulnerabilityID"], r["finding"]["PkgName"]) == POSITIVE
            and r["finding"]["Severity"] in ("HIGH", "CRITICAL")
            for r in before
        ),
        "stock positive control absent",
    )
    sbom_counts = {
        "stock": check_sbom(stock_sbom, stock_packages, stock_languages),
        "composed": check_sbom(candidate_sbom, candidate_packages, candidate_languages),
    }

    def counts(rows):
        return {
            severity: sum(row["finding"]["Severity"] == severity for row in rows)
            for severity in SEVERITIES
        }

    def statuses(rows):
        return dict(sorted(Counter(row["finding"]["Status"] for row in rows).items()))

    source_gap = [
        row
        for row in before
        if re.match(r"^(postgresql(?:-|$)|libpq[0-9])", row["finding"]["PkgName"])
    ]
    return {
        "stock_image_id": STOCK_ID,
        "composed_image_id": candidate_id,
        "severity_counts": {"stock": counts(before), "composed": counts(after)},
        "status_counts": {"stock": statuses(before), "composed": statuses(after)},
        "package_counts": {"stock": len(stock_packages), "composed": len(candidate_packages)},
        "sbom_component_counts": sbom_counts,
        "stock_rows": before,
        "composed_rows": after,
        "stock_only_exact_rows": difference(before, after),
        "composed_only_exact_rows": difference(after, before),
        "stock_postgresql_package_rows_with_source_coverage_gap": source_gap,
        "interpretation": "Exact unmatched scanner records are not resolved vulnerabilities. Source-built PostgreSQL and libpq may be absent from the package scan; separate source-aware advisory review is required. Findings are not accepted or waived.",
    }


def main():
    directory = Path(sys.argv[1])
    candidate_id = sys.argv[2]
    documents = {}
    for name in (
        "stock-report",
        "composed-report",
        "stock-config",
        "composed-config",
        "db-metadata",
        "stock-sbom",
        "composed-sbom",
    ):
        _, documents[name] = read_document(directory / f"{name}.json")
    from hashlib import sha256

    for name, expected in (("stock-config", STOCK_ID), ("composed-config", candidate_id)):
        require(
            "sha256:" + sha256((directory / f"{name}.json").read_bytes()).hexdigest() == expected,
            "config digest mismatch",
        )
    result = compare(
        documents["stock-report"],
        documents["composed-report"],
        documents["stock-config"],
        documents["composed-config"],
        candidate_id,
        documents["db-metadata"],
        documents["stock-sbom"],
        documents["composed-sbom"],
        load_dpkg((directory / "stock-dpkg.txt").read_text()),
        load_dpkg((directory / "composed-dpkg.txt").read_text()),
        datetime.now(UTC),
    )
    (directory / "comparison.json").write_text(json.dumps(result, sort_keys=True, indent=2) + "\n")


if __name__ == "__main__":
    main()
