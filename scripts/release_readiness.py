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

if __name__ == "__main__":
    try:
        config = json.loads((ROOT / "config/delivery.json").read_text())
        head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
        validate_release_environment(config, head, ROOT)
        with release_directory(ROOT, head) as directory:
            evidence, manifest_identity = read_release_manifest(directory)
            validate_release(config, evidence, head, ROOT, manifest_identity, directory)
        audit()
        print(
            f"Release evidence and server protection present for {head}; review evidence content before activation."
        )
    except (OSError, UnicodeError, subprocess.CalledProcessError):
        print("Release blocked: configuration, Git, or evidence unavailable", file=sys.stderr)
        sys.exit(1)
    except ValueError as error:
        print(f"Release blocked: {error}", file=sys.stderr)
        sys.exit(1)
