#!/usr/bin/env python3
"""Require complete same-run scan and composition checksum inventories."""

import hashlib
import re
import sys
from pathlib import Path

REQUIRED = {
    "db-sibling-scan-evidence": {
        "status.txt",
        "revision.txt",
        "identity.txt",
        "comparison.json",
        "source-component.json",
        "stock-report.json",
        "composed-report.json",
        "composed-config.json",
    },
    "db-sibling-postgres-evidence": {
        "status.txt",
        "revision.txt",
        "image-ids.txt",
        "validation.txt",
    },
    "db-sibling-root-evidence": {"status.txt", "revision.txt", "image-ids.txt", "validation.txt"},
    "db-source-feature-evidence": {"status.txt", "identity.txt", "target-fixture.txt"},
}


def verify(directory, required):
    if directory.is_symlink() or not directory.is_dir():
        raise ValueError("missing evidence directory")
    manifest = directory / "SHA256SUMS"
    if manifest.is_symlink() or not manifest.is_file():
        raise ValueError("missing checksum inventory")
    records = {}
    for line in manifest.read_text().splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  \./([^\n]+)", line)
        if not match:
            raise ValueError("invalid checksum record")
        name = match.group(2)
        parts = Path(name).parts
        if (
            name in records
            or not parts
            or Path(name).is_absolute()
            or any(p in (".", "..") for p in parts)
        ):
            raise ValueError("invalid checksum path")
        file = directory / name
        if (
            any(directory.joinpath(*parts[:i]).is_symlink() for i in range(1, len(parts) + 1))
            or not file.is_file()
        ):
            raise ValueError("invalid evidence file")
        h = hashlib.sha256()
        with file.open("rb") as stream:
            while block := stream.read(1048576):
                h.update(block)
        if h.hexdigest() != match.group(1):
            raise ValueError("evidence checksum mismatch")
        records[name] = match.group(1)
    actual = set()
    for file in directory.rglob("*"):
        if file.is_symlink():
            raise ValueError("symlink in evidence")
        if file.is_file() and file != manifest:
            actual.add(file.relative_to(directory).as_posix())
    if not records or actual != set(records) or not required <= actual:
        raise ValueError("incomplete evidence inventory")
    if (directory / "status.txt").read_text() != "stage=complete outcome=success\n":
        raise ValueError("unsuccessful prerequisite")
    if any(not (directory / name).stat().st_size for name in required):
        raise ValueError("empty required evidence")


def main():
    root = Path(sys.argv[1])
    for name, required in REQUIRED.items():
        verify(root / name, required)


if __name__ == "__main__":
    main()
