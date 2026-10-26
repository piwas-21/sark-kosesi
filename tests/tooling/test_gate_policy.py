import hashlib
import json
import sys
import tempfile
import unittest
from datetime import UTC, datetime
from pathlib import Path

from test_review_policy import report

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "scripts"))
from gate_policy import validate_checks, validate_receipt, validate_release

HEAD, BASE = "a" * 40, "b" * 40


def run(name="Foundation", **changes):
    result = {
        "id": 1,
        "name": name,
        "head_sha": HEAD,
        "status": "completed",
        "conclusion": "success",
        "app": {"slug": "github-actions"},
    }
    return {**result, **changes}


class CheckPolicy(unittest.TestCase):
    def test_populated_success_is_accepted(self):
        validate_checks(["Foundation"], [run()], HEAD)

    def test_empty_policy_and_missing_runs_are_rejected(self):
        for required, runs in [([], []), (["Foundation"], [])]:
            with self.subTest(required=required), self.assertRaises(ValueError):
                validate_checks(required, runs, HEAD)

    def test_skipped_failed_pending_and_stale_are_rejected(self):
        for change in [
            {"conclusion": "skipped"},
            {"conclusion": "failure"},
            {"status": "in_progress"},
            {"head_sha": BASE},
            {"app": {"slug": "untrusted"}},
        ]:
            with self.subTest(change=change), self.assertRaises(ValueError):
                validate_checks(["Foundation"], [run(**change)], HEAD)

    def test_latest_failure_cannot_hide_behind_old_success(self):
        with self.assertRaises(ValueError):
            validate_checks(["Foundation"], [run(), run(id=2, conclusion="failure")], HEAD)

    def test_nonrequired_failed_check_blocks(self):
        with self.assertRaises(ValueError):
            validate_checks(["Foundation"], [run(), run("Extra", conclusion="failure")], HEAD)


class EvidencePolicy(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "review.md").write_text(json.dumps(report(datetime.now(UTC))))
        self.receipt = {
            "head": HEAD,
            "base": BASE,
            "verdict": "approved",
            "reviewer": "independent-reviewer",
            "report": "review.md",
            "reportSha256": hashlib.sha256((self.root / "review.md").read_bytes()).hexdigest(),
        }

    def test_current_review_accepted(self):
        validate_receipt(self.receipt, HEAD, BASE, self.root)

    def test_stale_or_changed_report_rejected(self):
        with self.assertRaises(ValueError):
            validate_receipt(self.receipt, BASE, BASE, self.root)
        (self.root / "review.md").write_text("Changed after approval")
        with self.assertRaises(ValueError):
            validate_receipt(self.receipt, HEAD, BASE, self.root)

    def test_release_disabled_fails_even_with_success_claim(self):
        with self.assertRaises(ValueError):
            validate_release(
                {"stage": "foundation", "productionEnabled": False}, {"head": HEAD}, HEAD, self.root
            )

    def test_application_and_real_evidence_required(self):
        app = self.root / "apps/storefront"
        app.mkdir(parents=True)
        (app / "page.tsx").write_text("export default function Page() {}")
        crates = self.root / "crates/domain"
        crates.mkdir(parents=True)
        (crates / "lib.rs").write_text("pub struct Money(i64);")
        config = {
            "stage": "application",
            "productionEnabled": True,
            "productionGates": {"security": "passed"},
        }
        evidence = {
            "head": HEAD,
            "gates": {"security": {"status": "passed", "evidence": "review.md"}},
        }
        validate_release(config, evidence, HEAD, self.root)
        evidence["gates"]["security"]["evidence"] = "missing.md"
        with self.assertRaises(ValueError):
            validate_release(config, evidence, HEAD, self.root)

    def test_release_requires_real_apps_and_each_nonempty_gate(self):
        config = {
            "stage": "application",
            "productionEnabled": True,
            "productionGates": {"security": "passed"},
        }
        evidence = {
            "head": HEAD,
            "gates": {"security": {"status": "passed", "evidence": "review.md"}},
        }
        with self.assertRaises(ValueError):
            validate_release(config, evidence, HEAD, self.root)
        app = self.root / "apps"
        app.mkdir()
        (app / "page.tsx").write_text("export const Page = () => null")
        with self.assertRaises(ValueError):
            validate_release(config, evidence, HEAD, self.root)
        crates = self.root / "crates"
        crates.mkdir()
        (crates / "lib.rs").write_text("pub struct Money(i64);")
        for gates in ({}, {"security": "pending"}):
            with self.assertRaises(ValueError):
                validate_release({**config, "productionGates": gates}, evidence, HEAD, self.root)
        (self.root / "review.md").write_text("")
        with self.assertRaises(ValueError):
            validate_release(config, evidence, HEAD, self.root)

    def test_path_escape_and_wrong_revision_rejected(self):
        with self.assertRaises(ValueError):
            validate_receipt({**self.receipt, "report": "../outside.md"}, HEAD, BASE, self.root)
        app = self.root / "apps"
        app.mkdir()
        (app / "app.ts").write_text("export const x = 1")
        crates = self.root / "crates/domain"
        crates.mkdir(parents=True)
        (crates / "lib.rs").write_text("pub struct Money(i64);")
        config = {
            "stage": "application",
            "productionEnabled": True,
            "productionGates": {"security": "passed"},
        }
        with self.assertRaises(ValueError):
            validate_release(config, {"head": BASE}, HEAD, self.root)


if __name__ == "__main__":
    unittest.main()
