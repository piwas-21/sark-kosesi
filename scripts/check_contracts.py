#!/usr/bin/env python3
"""Regenerate Rust OpenAPI and TS contracts in isolation, rejecting drift."""

import json
import subprocess
import tempfile
from pathlib import Path

from generate_contract_adapter import generate

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
        output = Path(temporary) / "models"
        subprocess.run(
            [
                "npx",
                "--no-install",
                "openapi-ts",
                "-i",
                "packages/contracts/openapi.json",
                "-o",
                str(output),
                "-c",
                "@hey-api/typescript",
            ],
            cwd=ROOT,
            check=True,
        )
        if (output / "types.gen.ts").read_bytes() != (
            ROOT / "packages/contracts/models.gen.ts"
        ).read_bytes():
            raise ValueError("Generated TypeScript model drift")
        adapter = generate(json.loads(expected.read_text()))
        if adapter.encode() != (ROOT / "packages/contracts/generated.ts").read_bytes():
            raise ValueError("Generated TypeScript adapter drift")
    print("Rust OpenAPI and generated TypeScript contracts agree")


if __name__ == "__main__":
    main()
