#!/usr/bin/env python3
"""Check one target without suppressing or accepting other scanner findings."""

import hashlib
import json
import re
import sys
from pathlib import Path

from check_db_alpine_gosu import read_document
from compare_db_zlib_scan import BASE_CONFIG, BASE_ZLIB, CANDIDATE_ZLIB, inspect

TARGET = "CVE-2026-85091"


def check(stock, candidate, comparison, source_component, candidate_id):
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", candidate_id) or candidate_id == BASE_CONFIG:
        raise ValueError("invalid candidate identity")
    if (
        comparison.get("stock_image_id") != BASE_CONFIG
        or comparison.get("composed_image_id") != candidate_id
    ):
        raise ValueError("paired comparison identity mismatch")
    if source_component.get("composed_image_id") != candidate_id:
        raise ValueError("source component identity mismatch")
    before, before_high = inspect(stock, BASE_CONFIG, BASE_ZLIB)
    after, _ = inspect(candidate, candidate_id, CANDIDATE_ZLIB)
    if TARGET not in before or TARGET not in before_high:
        raise ValueError("stock positive zlib finding missing")
    if TARGET in after:
        raise ValueError("target zlib finding remains at any severity")
    return {
        "stock_image_id": BASE_CONFIG,
        "candidate_image_id": candidate_id,
        "stock_zlib_package": BASE_ZLIB,
        "candidate_zlib_package": CANDIDATE_ZLIB,
        "target": TARGET,
        "stock_target_high": True,
        "candidate_target_reported": False,
        "candidate_other_zlib_findings": sorted(after),
        "interpretation": "Scanner observation for one package advisory only. All other findings remain undisposed, and source-built PostgreSQL advisories require separate review.",
    }


def main():
    directory, candidate_id = Path(sys.argv[1]), sys.argv[2]
    documents = {}
    hashes = {}
    for name in ("stock-report", "composed-report", "comparison", "source-component"):
        raw, documents[name] = read_document(directory / f"{name}.json")
        hashes[name] = hashlib.sha256(raw).hexdigest()
    result = check(
        documents["stock-report"],
        documents["composed-report"],
        documents["comparison"],
        documents["source-component"],
        candidate_id,
    )
    result["input_sha256"] = hashes
    print(json.dumps(result, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
