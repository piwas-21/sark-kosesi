#!/usr/bin/env python3
import json
import subprocess
import sys
from pathlib import Path

from gate_policy import validate_receipt

root = Path(__file__).resolve().parents[1]
head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
for line in sys.stdin:
    local_ref, local_sha, remote_ref, remote_sha = line.split()
    if remote_ref in ("refs/heads/main", "refs/heads/develop"):
        raise SystemExit(
            "Direct pushes/deletions to main or develop are forbidden; use a reviewed PR."
        )
    if local_sha != "0" * 40 and local_sha != head:
        raise SystemExit("Push only the reviewed current HEAD from this checkout.")
if subprocess.check_output(["git", "status", "--porcelain"], cwd=root, text=True).strip():
    raise SystemExit("Source changes remain uncommitted/unreviewed.")
subprocess.run(["git", "fetch", "--quiet", "origin", "develop"], cwd=root, check=True)
base = subprocess.check_output(["git", "rev-parse", "origin/develop"], cwd=root, text=True).strip()
try:
    receipt = json.loads((root / ".local/reviews" / f"{head}.json").read_text())
    validate_receipt(receipt, head, base, root)
except (ValueError, OSError) as error:
    raise SystemExit(f"Push blocked: {error}") from error
subprocess.run(["npm", "run", "check"], cwd=root, check=True)
subprocess.run(["python3", "scripts/scan_secrets.py"], cwd=root, check=True)
print("Pre-push checks and current-head review receipt passed.")
