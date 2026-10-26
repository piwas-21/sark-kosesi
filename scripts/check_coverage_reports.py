#!/usr/bin/env python3
"""Require real per-package Rust/TS coverage and populated Clippy imports."""

import json
from pathlib import Path

from check_app_coverage import check
from rust_coverage import validate

ROOT = Path(__file__).resolve().parents[1]


def main():
    validate(json.loads((ROOT / "coverage/rust-stable.json").read_text()), ROOT, branches=False)
    rust = ROOT / "coverage/rust.json"
    results = validate(json.loads(rust.read_text()), ROOT)
    for result in results:
        print(json.dumps(result))
    typescript = ROOT / "coverage/typescript/lcov.info"
    for package in ("apps/web/src", "packages/design/src"):
        critical = ["apps/web/src/proxy.ts"] if package == "apps/web/src" else []
        check(typescript, package, critical, root=ROOT)
    messages = [
        json.loads(line)
        for line in (ROOT / "coverage/clippy.json").read_text().splitlines()
        if line.strip()
    ]
    artifacts = [item for item in messages if item.get("reason") == "compiler-artifact"]
    if not artifacts or not any(
        item.get("reason") == "build-finished" and item.get("success") is True for item in messages
    ):
        raise ValueError("Missing/populated Clippy evidence")
    print(
        "Every maintained package measured; line/branch/region thresholds met; Clippy report populated"
    )


if __name__ == "__main__":
    main()
