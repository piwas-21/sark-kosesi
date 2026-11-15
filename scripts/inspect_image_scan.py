#!/usr/bin/env python3
"""Inspect a trusted unfiltered Trivy image scan and OCI identity offline.

This checks input consistency, not scanner provenance or release approval.
"""

import argparse
import hashlib
import json
import os
import re
import stat
import sys
from collections import Counter
from datetime import UTC, datetime
from pathlib import Path

LIMIT = 64 * 1024 * 1024
DIGEST = re.compile(r"sha256:[0-9a-f]{64}\Z")
REVISION = re.compile(r"[0-9a-f]{40}\Z")
SEVERITIES = ("UNKNOWN", "LOW", "MEDIUM", "HIGH", "CRITICAL")
SCOPE = {
    "scanners": ["vuln"],
    "package_types": ["os", "library"],
    "severities": list(SEVERITIES),
    "include_unfixed": True,
    "include_all_packages": True,
    "ignore_rules": False,
    "vex": False,
}


class InvalidEvidence(ValueError):
    pass


def require(condition, reason):
    if not condition:
        raise InvalidEvidence(reason)


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, "duplicate JSON member")
        result[key] = value
    return result


def read_document(path):
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0)
    require(hasattr(os, "O_NOFOLLOW"), "safe file access unavailable")
    try:
        descriptor = os.open(path, flags)
        with os.fdopen(descriptor, "rb") as file:
            info = os.fstat(file.fileno())
            require(stat.S_ISREG(info.st_mode) and 0 < info.st_size <= LIMIT, "invalid input file")
            raw = file.read(LIMIT + 1)
            require(len(raw) == info.st_size and len(raw) <= LIMIT, "input changed or oversized")
        document = json.loads(raw, object_pairs_hook=unique_object)
    except (OSError, UnicodeError, json.JSONDecodeError, RecursionError):
        raise InvalidEvidence("input unavailable or malformed") from None
    require(isinstance(document, dict), "expected JSON object")
    return raw, document


def digest(raw):
    return "sha256:" + hashlib.sha256(raw).hexdigest()


def timestamp(value):
    require(
        isinstance(value, str)
        and re.fullmatch(
            r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?(?:Z|[+-]\d{2}:\d{2})",
            value,
        )
        is not None,
        "invalid timestamp",
    )
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        raise InvalidEvidence("invalid timestamp") from None
    return parsed


def bounded_string(value):
    return isinstance(value, str) and 0 < len(value) <= 512 and not any(ord(c) < 32 for c in value)


def inspect(
    receipt,
    manifest_raw,
    manifest,
    config_raw,
    config,
    report_raw,
    report,
    *,
    revision,
    image_digest,
    scanner_digest,
    platform,
    now,
):
    require(
        isinstance(revision, str) and REVISION.fullmatch(revision) is not None,
        "invalid expected revision",
    )
    require(
        isinstance(image_digest, str) and DIGEST.fullmatch(image_digest) is not None,
        "invalid expected image digest",
    )
    require(
        isinstance(scanner_digest, str) and DIGEST.fullmatch(scanner_digest) is not None,
        "invalid expected scanner digest",
    )
    require(platform in ("linux/amd64", "linux/arm64"), "unsupported expected platform")
    require(now.tzinfo is not None, "invalid audit clock")
    require(
        set(receipt)
        == {
            "schema_version",
            "source_revision",
            "manifest_digest",
            "scanner_digest",
            "report_digest",
            "scanned_at",
            "exit_code",
            "database",
            "scope",
        },
        "invalid receipt",
    )
    require(
        type(receipt["schema_version"]) is int and receipt["schema_version"] == 1,
        "invalid receipt version",
    )
    require(receipt["source_revision"] == revision, "source revision mismatch")
    require(
        receipt["manifest_digest"] == image_digest == digest(manifest_raw),
        "platform manifest mismatch",
    )
    require(receipt["scanner_digest"] == scanner_digest, "scanner mismatch")
    require(receipt["report_digest"] == digest(report_raw), "report digest mismatch")
    require(
        type(receipt["exit_code"]) is int and receipt["exit_code"] == 0, "scanner did not succeed"
    )
    scope = receipt["scope"]
    require(isinstance(scope, dict) and set(scope) == set(SCOPE), "invalid scan scope")
    require(
        all(
            type(scope[key]) is bool and scope[key] is SCOPE[key]
            for key in ("include_unfixed", "include_all_packages", "ignore_rules", "vex")
        ),
        "scan scope incomplete or filtered",
    )
    require(
        scope["scanners"] == SCOPE["scanners"]
        and scope["package_types"] == SCOPE["package_types"]
        and scope["severities"] == SCOPE["severities"],
        "scan scope incomplete or filtered",
    )
    db = receipt["database"]
    require(
        isinstance(db, dict) and set(db) == {"version", "updated_at", "next_update"},
        "invalid database metadata",
    )
    require(type(db["version"]) is int and db["version"] == 2, "unsupported vulnerability database")
    scanned = timestamp(receipt["scanned_at"])
    updated, expires = timestamp(db["updated_at"]), timestamp(db["next_update"])
    require(updated <= scanned <= now < expires, "stale or future scan database")

    require(
        type(manifest.get("schemaVersion")) is int
        and manifest["schemaVersion"] == 2
        and manifest.get("mediaType")
        in (
            "application/vnd.oci.image.manifest.v1+json",
            "application/vnd.docker.distribution.manifest.v2+json",
        ),
        "invalid OCI platform manifest",
    )
    descriptor, layers = manifest.get("config"), manifest.get("layers")
    require(
        isinstance(descriptor, dict) and isinstance(layers, list) and bool(layers),
        "missing image layers",
    )
    config_hash = descriptor.get("digest")
    require(
        isinstance(config_hash, str) and DIGEST.fullmatch(config_hash) is not None,
        "invalid image config digest",
    )
    require(
        config_hash == digest(config_raw)
        and type(descriptor.get("size")) is int
        and descriptor["size"] == len(config_raw),
        "image config mismatch",
    )
    for layer in layers:
        require(
            isinstance(layer, dict)
            and isinstance(layer.get("digest"), str)
            and DIGEST.fullmatch(layer["digest"]) is not None
            and type(layer.get("size")) is int
            and layer["size"] > 0,
            "invalid layer descriptor",
        )
    os_name, architecture = platform.split("/")
    require(
        config.get("os") == os_name and config.get("architecture") == architecture,
        "image platform mismatch",
    )
    rootfs = config.get("rootfs")
    require(
        isinstance(rootfs, dict)
        and rootfs.get("type") == "layers"
        and isinstance(rootfs.get("diff_ids"), list)
        and len(rootfs["diff_ids"]) == len(layers)
        and all(
            isinstance(value, str) and DIGEST.fullmatch(value) is not None
            for value in rootfs["diff_ids"]
        ),
        "image rootfs mismatch",
    )

    require(
        type(report.get("SchemaVersion")) is int
        and report["SchemaVersion"] == 2
        and report.get("ArtifactType") == "container_image",
        "invalid Trivy report",
    )
    report_created = timestamp(report.get("CreatedAt"))
    require(updated <= report_created <= scanned, "report and database times disagree")
    metadata = report.get("Metadata")
    require(
        isinstance(metadata, dict) and metadata.get("ImageID") == config_hash,
        "scanned image mismatch",
    )
    require(metadata.get("DiffIDs") == rootfs["diff_ids"], "scanned layer identity mismatch")
    image_config, os_info = metadata.get("ImageConfig"), metadata.get("OS")
    require(
        isinstance(image_config, dict)
        and image_config.get("os") == os_name
        and image_config.get("architecture") == architecture,
        "scanned platform mismatch",
    )
    require(
        isinstance(os_info, dict)
        and os_info.get("Family") == "debian"
        and bounded_string(os_info.get("Name")),
        "unsupported or missing OS inventory",
    )
    results = report.get("Results")
    require(isinstance(results, list) and bool(results), "missing scan results")
    counts, packages, os_packages = Counter({severity: 0 for severity in SEVERITIES}), 0, 0
    for target in results:
        require(
            isinstance(target, dict)
            and target.get("Class") in ("os-pkgs", "lang-pkgs")
            and bounded_string(target.get("Target"))
            and bounded_string(target.get("Type")),
            "invalid scan target",
        )
        modified = target.get("ExperimentalModifiedFindings", [])
        require(
            isinstance(modified, list) and not modified,
            "modified findings cannot establish scope",
        )
        inventory = target.get("Packages")
        require(
            isinstance(inventory, list) and bool(inventory), "empty or missing package inventory"
        )
        for package in inventory:
            require(
                isinstance(package, dict)
                and bounded_string(package.get("Name"))
                and bounded_string(package.get("Version")),
                "invalid package inventory",
            )
        packages += len(inventory)
        if target["Class"] == "os-pkgs":
            require(target["Type"] == "debian", "unexpected OS package type")
            os_packages += len(inventory)
        findings = target.get("Vulnerabilities", [])
        require(isinstance(findings, list), "invalid findings")
        for finding in findings:
            require(
                isinstance(finding, dict)
                and bounded_string(finding.get("VulnerabilityID"))
                and bounded_string(finding.get("PkgName"))
                and bounded_string(finding.get("InstalledVersion"))
                and finding.get("Severity") in SEVERITIES,
                "invalid finding",
            )
            counts[finding["Severity"]] += 1
    require(os_packages > 0, "empty Debian package inventory")
    blocked = counts["HIGH"] > 0 or counts["CRITICAL"] > 0
    return {
        "status": "blocked" if blocked else "no_reported_high_or_critical",
        "source_revision": revision,
        "platform": platform,
        "manifest_digest": image_digest,
        "package_count": packages,
        "severity_counts": dict(counts),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in (
        "receipt",
        "manifest",
        "image-config",
        "report",
        "revision",
        "image-digest",
        "scanner-digest",
        "platform",
    ):
        parser.add_argument("--" + flag, required=True)
    args = parser.parse_args()
    try:
        _, receipt = read_document(Path(args.receipt))
        manifest_raw, manifest = read_document(Path(args.manifest))
        config_raw, config = read_document(Path(args.image_config))
        report_raw, report = read_document(Path(args.report))
        result = inspect(
            receipt,
            manifest_raw,
            manifest,
            config_raw,
            config,
            report_raw,
            report,
            revision=args.revision,
            image_digest=args.image_digest,
            scanner_digest=args.scanner_digest,
            platform=args.platform,
            now=datetime.now(UTC),
        )
    except InvalidEvidence as error:
        print(f"Image scan evidence invalid: {error}", file=sys.stderr)
        return 1
    print(json.dumps(result, sort_keys=True))
    return 1 if result["status"] == "blocked" else 0


if __name__ == "__main__":
    sys.exit(main())
