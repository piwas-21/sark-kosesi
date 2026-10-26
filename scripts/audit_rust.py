#!/usr/bin/env python3
"""Audit with fresh RustSec data and reject unknown dependency licences/sources."""

import json
import subprocess
import tempfile
from datetime import UTC, datetime, timedelta
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def main():
    database = ROOT / ".local/advisory-db"
    if not database.exists():
        subprocess.run(
            ["git", "clone", "--quiet", "https://github.com/RustSec/advisory-db", str(database)],
            check=True,
        )
    subprocess.run(["git", "-C", str(database), "fetch", "--quiet", "origin", "main"], check=True)
    subprocess.run(["git", "-C", str(database), "merge", "--ff-only", "origin/main"], check=True)
    timestamp = int(
        subprocess.check_output(
            ["git", "-C", str(database), "show", "-s", "--format=%ct", "HEAD"], text=True
        )
    )
    age = datetime.now(UTC) - datetime.fromtimestamp(timestamp, UTC)
    if age < timedelta(0) or age > timedelta(days=7):
        raise ValueError("RustSec advisory database is stale or future-dated")
    report = subprocess.run(
        ["cargo", "audit", "--json", "--db", str(database), "--no-fetch"],
        check=True,
        capture_output=True,
        text=True,
    )
    evidence = json.loads(report.stdout)
    if evidence.get("vulnerabilities", {}).get("found") is not False or not evidence.get(
        "lockfile", {}
    ).get("dependency-count"):
        raise ValueError("Missing/populated-vulnerability audit evidence")
    if not evidence.get("database", {}).get("advisory-count"):
        raise ValueError("RustSec database is empty")
    with tempfile.TemporaryDirectory(prefix="audit-control-", dir=ROOT / ".local") as temporary:
        lock = Path(temporary) / "Cargo.lock"
        lock.write_text(
            'version = 4\n[[package]]\nname = "time"\nversion = "0.1.44"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n'
        )
        hostile = subprocess.run(
            ["cargo", "audit", "--json", "--db", str(database), "--no-fetch", "--file", str(lock)],
            capture_output=True,
            text=True,
        )
        findings = json.loads(hostile.stdout).get("vulnerabilities", {})
        if (
            hostile.returncode == 0
            or findings.get("found") is not True
            or not any(
                item.get("advisory", {}).get("id") == "RUSTSEC-2020-0071"
                for item in findings.get("list", [])
            )
        ):
            raise ValueError("RustSec negative control did not detect the known vulnerable fixture")
    (ROOT / ".local/evidence").mkdir(parents=True, exist_ok=True)
    (ROOT / ".local/evidence/rust-audit.json").write_text(report.stdout)
    subprocess.run(
        ["cargo", "deny", "--locked", "check", "licenses", "bans", "sources"], check=True
    )
    print("Current RustSec audit and reviewed licence/source policy passed")


if __name__ == "__main__":
    main()
