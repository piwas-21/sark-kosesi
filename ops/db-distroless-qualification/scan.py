"""Pair both exact Debian layouts with the same current unfiltered Trivy database."""

import argparse
import json
import os
import shutil
import subprocess
import sys
from collections import Counter
from datetime import UTC, datetime
from pathlib import Path

from assembly import read_json
from capture_db_image_scan import SCANNER_IMAGE, TARGET, capture_identity
from compare import (
    POSITIVE,
    SEVERITIES,
    STOCK_ID,
    check_dpkg,
    check_sbom,
    difference,
    inspect,
    load_dpkg,
    timestamp,
)
from run_lab import docker, failure_sites
from runtime import instance_id

from packages import HERE, digest, image_inventory, owned_directory, require

VERSIONS = {"stock": "13.7", "server": "13.8"}
REPO = HERE.parents[1]


def compared(stock, server, cfg_stock, cfg_server, server_id, inventories, sboms, db, now):
    require(db.get("Version") == 2 and now.tzinfo is not None, "Invalid Trivy database evidence")
    refreshed, expires = timestamp(db["UpdatedAt"]), timestamp(db["NextUpdate"])
    for report in (stock, server):
        require(
            refreshed <= timestamp(report["CreatedAt"]) <= now < expires, "Stale Trivy evidence"
        )
    comparison = {}
    findings = {}
    for role, report, cfg, image_id in (
        ("stock", stock, cfg_stock, STOCK_ID),
        ("server", server, cfg_server, server_id),
    ):
        rows, packages, languages, version = inspect(report, cfg, image_id)
        require(version == VERSIONS[role], "Trivy Debian version differs from the pinned profile")
        check_dpkg(packages, inventories[role])
        sbom_count = check_sbom(sboms[role], packages, languages)
        counts = Counter(row["finding"]["Severity"] for row in rows)
        findings[role] = rows
        comparison[role] = {
            "image_id": image_id,
            "debian_version": version,
            "package_count": len(packages),
            "sbom_count": sbom_count,
            "severity_counts": {level: counts[level] for level in SEVERITIES},
        }
    require(
        any(
            row["finding"]["Severity"] in ("HIGH", "CRITICAL")
            and (row["finding"]["VulnerabilityID"], row["finding"]["PkgName"]) == POSITIVE
            for row in findings["stock"]
        ),
        "Stock finding control absent",
    )
    return {
        **comparison,
        "stock_only_exact_rows": difference(findings["stock"], findings["server"]),
        "server_only_exact_rows": difference(findings["server"], findings["stock"]),
        "findings_status": "review_required",
    }


def run(folder, evidence, server_id):
    owned_directory(folder)
    owned_directory(evidence)
    scratch = folder / "scan"
    scratch.mkdir(mode=0o700)
    os.chmod(scratch, 0o700)
    temporary = scratch / "tmp"
    temporary.mkdir(mode=0o700)
    owned_directory(temporary)
    capture_identity(scratch)
    cfg_stock = read_json(scratch / "config.json")
    config = folder / "server-config.json"
    require("sha256:" + digest(config) == server_id, "Verified server config changed")
    cfg_server = read_json(config)
    scanner_id = read_json(scratch / "scanner-manifest.json")["config"]["digest"]
    subprocess.run(
        [
            "sh",
            str(REPO / "ops/db-zlib-candidate/pull_verified_base.sh"),
            "--image",
            SCANNER_IMAGE,
            scanner_id,
            str(scratch / "scanner-pull.log"),
        ],
        check=True,
        timeout=300,
    )
    archive = folder / "server.tar"
    sealed = read_json(folder / "server-image-seal.json")
    require(
        sealed.keys() == {"server_image", "server_image_archive_sha256"}
        and sealed["server_image"] == server_id,
        "Server image archive seal differs",
    )
    archive_sha = sealed["server_image_archive_sha256"]
    require(digest(archive) == archive_sha, "Server archive changed since verified import")
    (scratch / "empty.yml").write_text("{}\n")
    (scratch / "empty.ignore").write_text("")
    inventory = HERE / "inputs.json"
    locks = read_json(inventory)
    locks["postgres"] = read_json(HERE / "source-lock.json")
    root_export = folder / "server-rootfs.tar"
    exported = read_json(folder / "rootfs-export-seal.json")
    require(
        exported.keys() == {"server_image", "export_container_id", "server_rootfs_export_sha256"}
        and exported["server_image"] == server_id
        and instance_id(exported["export_container_id"])
        and digest(root_export) == exported["server_rootfs_export_sha256"],
        "Server rootfs export seal differs",
    )
    candidate = image_inventory(root_export, locks)
    require(
        digest(root_export) == exported["server_rootfs_export_sha256"],
        "Server rootfs export changed",
    )
    root_export.unlink()
    stock = docker(
        "run",
        "--rm",
        "--pull=never",
        "--network",
        "none",
        "--read-only",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--entrypoint",
        "sh",
        "-i",
        STOCK_ID,
        "-s",
        "--",
        "packages",
        data=(REPO / "ops/db-sibling-runtime-root/probe.sh").read_bytes(),
    ).stdout.decode()
    inventories = {"stock": load_dpkg(stock), "server": load_dpkg("\n".join(candidate))}
    args = [
        "--config",
        "/scan/empty.yml",
        "--cache-dir",
        "/scan/cache",
    ]

    def trivy(network, *command, local_input=False):
        mount = (
            ["--mount", f"type=bind,src={archive},dst=/inputs/server.tar,readonly"]
            if local_input
            else []
        )
        docker(
            "run",
            "--rm",
            "--pull=never",
            "--read-only",
            "--network",
            network,
            "--cap-drop",
            "ALL",
            "--security-opt",
            "no-new-privileges",
            "--memory",
            "1g",
            "--cpus",
            "1",
            "--pids-limit",
            "128",
            "--user",
            f"{os.getuid()}:{os.getgid()}",
            "--tmpfs",
            "/tmp:rw,nosuid,nodev,size=64m",
            # Trivy stages the complete compressed DB before extracting it.
            "--env",
            "TMPDIR=/scan/tmp",
            "--mount",
            f"type=bind,src={scratch},dst=/scan",
            *mount,
            scanner_id,
            *args,
            *command,
            timeout=360,
        )

    scan_args = [
        "image",
        "--format",
        "json",
        "--scanners",
        "vuln",
        "--pkg-types",
        "os,library",
        "--severity",
        ",".join(SEVERITIES),
        "--pkg-relationships",
        "unknown,root,workspace,direct,indirect",
        "--list-all-pkgs",
        "--show-suppressed",
        "--exit-code",
        "0",
        "--ignorefile",
        "/scan/empty.ignore",
        "--platform",
        "linux/amd64",
    ]
    trivy(
        "bridge", *scan_args, "--image-src", "remote", "--output", "/scan/stock-report.json", TARGET
    )
    db = scratch / "cache/db/trivy.db"
    metadata = scratch / "cache/db/metadata.json"
    reference_db = digest(db), digest(metadata)
    require(digest(archive) == archive_sha, "Server archive changed before scanner launch")
    trivy(
        "none",
        *scan_args,
        "--skip-db-update",
        "--input",
        "/inputs/server.tar",
        "--output",
        "/scan/server-report.json",
        local_input=True,
    )
    require(digest(archive) == archive_sha, "Server archive changed during scanner run")
    for role in ("stock", "server"):
        trivy(
            "none",
            "convert",
            "--format",
            "cyclonedx",
            "--ignorefile",
            "/scan/empty.ignore",
            "--show-suppressed",
            "--list-all-pkgs",
            "--severity",
            ",".join(SEVERITIES),
            "--output",
            f"/scan/{role}-sbom.json",
            f"/scan/{role}-report.json",
        )
    require(
        reference_db == (digest(db), digest(metadata)) and archive_sha == digest(archive),
        "Trivy database or server archive changed",
    )
    for role, records in inventories.items():
        (evidence / f"{role}-inventory.json").write_text(json.dumps(records, sort_keys=True) + "\n")
    for name in ("stock-report.json", "server-report.json", "stock-sbom.json", "server-sbom.json"):
        shutil.copyfile(scratch / name, evidence / name)
    shutil.copyfile(metadata, evidence / "db-metadata.json")
    result = compared(
        *(
            read_json(scratch / f"{role}-report.json", 128 * 1024**2)
            for role in ("stock", "server")
        ),
        cfg_stock,
        cfg_server,
        server_id,
        inventories,
        {
            role: read_json(scratch / f"{role}-sbom.json", 128 * 1024**2)
            for role in ("stock", "server")
        },
        read_json(metadata),
        datetime.now(UTC),
    )
    result.update(
        {
            "database_sha256": reference_db[0],
            "server_image_archive_sha256": archive_sha,
            "server_rootfs_export_sha256": exported["server_rootfs_export_sha256"],
            "scanner_id": scanner_id,
        }
    )
    (evidence / "scan-comparison.json").write_text(json.dumps(result, sort_keys=True) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("folder", type=Path)
    parser.add_argument("evidence", type=Path)
    parser.add_argument("server_id")
    opts = parser.parse_args()
    try:
        run(opts.folder, opts.evidence, opts.server_id)
    except Exception as error:
        for name, line in failure_sites(error):
            print(f"scan_failure={name}:{line}", file=sys.stderr, flush=True)
        raise
