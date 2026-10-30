#!/usr/bin/env python3
"""Prove that independent money oracles reject an actual executable arithmetic defect."""

import os
import shutil
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def main():
    with tempfile.TemporaryDirectory(prefix="money-mutation-", dir=ROOT / ".local") as temporary:
        directory = Path(temporary)
        shutil.copytree(ROOT / "crates/domain/src", directory / "src")
        shutil.copytree(ROOT / "crates/domain/tests", directory / "tests")
        (directory / "Cargo.toml").write_text("""[workspace]
[package]
name="commerce-domain"
version="0.1.0"
edition="2024"
[dependencies]
serde={version="=1.0.229",features=["derive"]}
""")
        subprocess.run(
            [
                "cargo",
                "generate-lockfile",
                "--offline",
                "--manifest-path",
                str(directory / "Cargo.toml"),
            ],
            check=True,
            capture_output=True,
        )
        for relative, original_expression, mutant, expected in (
            (
                "lib.rs",
                ".checked_mul(",
                ".checked_add(",
                "test exact_minor_units_with_independent_oracles ... FAILED",
            ),
            (
                "quotes/totals.rs",
                "+ denominator / 2",
                "+ 0",
                "test half_cent_mixed_rates_zero_and_per_extended_line_rounding_are_explicit ... FAILED",
            ),
        ):
            source = directory / "src" / relative
            original = source.read_text()
            if original.count(original_expression) != 1:
                raise ValueError("Mutation target changed; review the intended arithmetic defect")
            source.write_text(original.replace(original_expression, mutant))
            try:
                result = subprocess.run(
                    [
                        "cargo",
                        "test",
                        "--locked",
                        "--offline",
                        "--manifest-path",
                        str(directory / "Cargo.toml"),
                    ],
                    env={**os.environ, "CARGO_TARGET_DIR": str(ROOT / "target/money-mutation")},
                    text=True,
                    capture_output=True,
                )
                if (
                    result.returncode == 0
                    or expected not in result.stdout
                    or "error: could not compile" in result.stderr
                ):
                    raise ValueError("Money oracle did not reject the intended executable mutant")
            finally:
                source.write_text(original)
    print("Independent money oracles killed multiplication and VAT-rounding mutants")


if __name__ == "__main__":
    main()
