#!/usr/bin/env python3
"""Publish complete lifecycle evidence with an atomic directory rename."""

import hashlib
import os
import re
import sys
from pathlib import Path


def publish(staged, destination):
    if (
        staged.is_symlink()
        or not staged.is_dir()
        or destination.exists()
        or destination.is_symlink()
    ):
        raise ValueError("invalid evidence publication paths")
    status_path = staged / "status.txt"
    if status_path.is_symlink() or not status_path.is_file():
        raise ValueError("missing status")
    status = status_path.read_text(encoding="utf-8")
    if status == "stage=complete outcome=success\n":
        for name in ("identity.txt", "results.txt"):
            item = staged / name
            if item.is_symlink() or not item.is_file() or not item.stat().st_size:
                raise ValueError("incomplete successful evidence")
    elif not re.fullmatch(r"stage=[a-z_]+ outcome=failed exit=[1-9][0-9]*\n", status):
        raise ValueError("invalid status")
    records = []
    for item in sorted(staged.iterdir()):
        if item.is_symlink() or not item.is_file() or item.name == "SHA256SUMS":
            raise ValueError("unexpected evidence entry")
        records.append(f"{hashlib.sha256(item.read_bytes()).hexdigest()}  ./{item.name}\n")
    (staged / "SHA256SUMS").write_text("".join(records), encoding="utf-8")
    # Both directories are under runner temp; rename fails across filesystems.
    os.rename(staged, destination)


def main():
    source, destination = sys.argv[1:]
    publish(Path(source), Path(destination))


if __name__ == "__main__":
    main()
