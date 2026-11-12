#!/usr/bin/env python3
import json
import subprocess
import sys

from audit_github import ROOT, audit
from gate_policy import (
    read_release_manifest,
    release_directory,
    validate_release,
    validate_release_environment,
)


def git(*args):
    return subprocess.run(
        ["git", *args], cwd=ROOT, check=True, text=True, capture_output=True
    ).stdout


def require_clean_revision(head):
    tracked = git("ls-files", "-v", "-z").split("\0")
    if (
        git("rev-parse", "HEAD").strip() != head
        or git("status", "--porcelain=v1", "--untracked-files=all", "--ignore-submodules=none")
        or any(entry and not entry.startswith("H ") for entry in tracked)
    ):
        raise ValueError("Release source is changed or uncommitted")


if __name__ == "__main__":
    try:
        head = git("rev-parse", "HEAD").strip()
        config = json.loads(git("show", f"{head}:config/delivery.json"))
        require_clean_revision(head)
        validate_release_environment(config, head, ROOT)
        with release_directory(ROOT, head) as directory:
            evidence, manifest_identity = read_release_manifest(directory)
            validate_release(config, evidence, head, ROOT, manifest_identity, directory)
        audit(config)
        require_clean_revision(head)
        print(
            f"Release evidence and server protection present for {head}; review evidence content before activation."
        )
    except (OSError, UnicodeError, subprocess.CalledProcessError):
        print("Release blocked: configuration, Git, or evidence unavailable", file=sys.stderr)
        sys.exit(1)
    except ValueError as error:
        print(f"Release blocked: {error}", file=sys.stderr)
        sys.exit(1)
