#!/usr/bin/env python3
"""Regenerate Rust OpenAPI and TS contracts in isolation, rejecting drift."""

import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def main():
    actual = subprocess.check_output(
        ["cargo", "run", "--locked", "--quiet", "-p", "commerce-api", "--", "openapi"], cwd=ROOT
    )
    expected = ROOT / "packages/contracts/openapi.json"
    if actual != expected.read_bytes():
        raise ValueError("Rust OpenAPI drift: regenerate and review the contract")
    (ROOT / ".local").mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(dir=ROOT / ".local") as temporary:
        output = Path(temporary) / "generated.ts"
        subprocess.run(
            ["npx", "--no-install", "openapi-typescript", str(expected), "-o", str(output)],
            cwd=ROOT,
            check=True,
        )
        if output.read_bytes() != (ROOT / "packages/contracts/generated.ts").read_bytes():
            raise ValueError("Generated TypeScript contract drift")
    print("Rust OpenAPI and generated TypeScript contracts agree")


if __name__ == "__main__":
    main()
