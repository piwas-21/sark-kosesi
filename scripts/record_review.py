#!/usr/bin/env python3
"""Record a validated reviewer-authored JSON report for an immutable revision."""

import argparse
import hashlib
import json
import subprocess
from pathlib import Path

from review_policy import validate_review

ROOT = Path(__file__).resolve().parents[1]


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--base", default="origin/develop")
    args = parser.parse_args()
    if git("status", "--porcelain"):
        raise ValueError("Commit reviewed source first; dirty source cannot receive a receipt")
    report = args.report.resolve()
    directory = (ROOT / ".local/reviews").resolve()
    if not report.is_relative_to(directory) or not report.is_file():
        raise ValueError("Reviewer-authored report must be inside .local/reviews")
    head, base = git("rev-parse", "HEAD"), git("rev-parse", args.base)
    validate_review(json.loads(report.read_text()), head, base)
    receipt = {
        "head": head,
        "base": base,
        "report": report.relative_to(ROOT).as_posix(),
        "reportSha256": hashlib.sha256(report.read_bytes()).hexdigest(),
    }
    target = directory / f"{head}.json"
    if target == report:
        raise ValueError("Report filename must differ from the receipt filename")
    target.write_text(json.dumps(receipt, indent=2) + "\n")
    print(f"Validated independent review for {head}")


if __name__ == "__main__":
    main()
