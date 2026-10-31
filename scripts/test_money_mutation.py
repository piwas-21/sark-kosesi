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
        # Keep the actual package/dependency/edition/lint graph and locked closure.
        # A hand-written miniature manifest drifts when domain dependencies change.
        for name in ("Cargo.toml", "Cargo.lock"):
            shutil.copy2(ROOT / name, directory / name)
        shutil.copytree(ROOT / "crates", directory / "crates")
        shutil.copytree(ROOT / "tests/fixtures", directory / "tests/fixtures")
        command = [
            "cargo",
            "test",
            "--locked",
            "--offline",
            "--manifest-path",
            str(directory / "Cargo.toml"),
            "-p",
            "commerce-domain",
        ]
        environment = {**os.environ, "CARGO_TARGET_DIR": str(ROOT / "target/money-mutation")}
        baseline = subprocess.run(
            [*command, "--test", "money", "--test", "quotes", "--test", "promotions"],
            env=environment,
            text=True,
            capture_output=True,
        )
        if baseline.returncode != 0:
            (ROOT / ".local/money-mutation-baseline-failure.log").write_text(
                baseline.stdout + baseline.stderr
            )
            raise ValueError("Isolated money/quote/promotion baseline did not compile and pass")
        for relative, original_expression, mutant, test_binary, test_name in (
            (
                "lib.rs",
                ".checked_mul(",
                ".checked_add(",
                "money",
                "exact_minor_units_with_independent_oracles",
            ),
            (
                "quotes/totals.rs",
                "+ denominator / 2",
                "+ 0",
                "quotes",
                "half_cent_mixed_rates_zero_and_per_extended_line_rounding_are_explicit",
            ),
            (
                "promotions/allocation.rs",
                ".checked_add(5000)",
                ".checked_add(0)",
                "promotions",
                "percentage_half_cent_caps_and_full_discounts_are_explicit",
            ),
            (
                "promotions/allocation.rs",
                ".then(a.0.cmp(&b.0))",
                ".then(b.0.cmp(&a.0))",
                "promotions",
                "largest_remainder_has_exact_cent_oracles_and_canonical_sku_ties",
            ),
            (
                "promotions/refund.rs",
                ".checked_add(denominator / 2)",
                ".checked_add(0)",
                "promotions",
                "cumulative_refunds_keep_captured_vat_instead_of_repeating_rounding",
            ),
        ):
            expected = f"test {test_name} ... FAILED"
            source = directory / "crates/domain/src" / relative
            original = source.read_text()
            if original.count(original_expression) != 1:
                raise ValueError("Mutation target changed; review the intended arithmetic defect")
            source.write_text(original.replace(original_expression, mutant))
            try:
                result = subprocess.run(
                    [*command, "--test", test_binary, "--", "--exact", test_name],
                    env=environment,
                    text=True,
                    capture_output=True,
                )
                if (
                    result.returncode == 0
                    or expected not in result.stdout
                    or "error: could not compile" in result.stderr
                ):
                    (ROOT / ".local/money-mutation-failure.log").write_text(
                        result.stdout + result.stderr
                    )
                    raise ValueError(
                        f"Money oracle did not reject intended mutant in {relative}: {expected}"
                    )
            finally:
                source.write_text(original)
    print("Independent money oracles killed multiplication, VAT, promotion and refund mutants")


if __name__ == "__main__":
    main()
