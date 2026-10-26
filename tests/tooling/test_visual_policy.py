import copy
import hashlib
import tempfile
import unittest
from datetime import UTC, datetime
from pathlib import Path

from visual_policy import expected_images, source_digest, validate


class VisualApproval(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for name in (
            "tests/application/browser.spec.mjs",
            "playwright.application.config.mjs",
            "apps/web/src/page.tsx",
        ):
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("independent fixture")
        images = {}
        for name in expected_images():
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"fixture image bytes, not an actual application approval")
            images[name] = hashlib.sha256(path.read_bytes()).hexdigest()
        self.approval = {
            "schemaVersion": 1,
            "humanApproved": True,
            "reviewer": "Fixture reviewer",
            "evidence": "Test-only attestation exercising validator, not real approval.",
            "reviewedAt": datetime.now(UTC).isoformat(),
            "sourceSha256": source_digest(self.root),
            "images": images,
        }

    def test_exact_fixture_accepted_but_changes_invalidate_it(self):
        validate(self.approval, self.root)
        (self.root / "apps/web/src/page.tsx").write_text("changed")
        with self.assertRaisesRegex(ValueError, "source changed"):
            validate(self.approval, self.root)

    def test_missing_inventory_bad_approval_and_changed_images_fail(self):
        for key, value in (
            ("schemaVersion", 2),
            ("humanApproved", False),
            ("reviewer", ""),
            ("evidence", ""),
            ("reviewedAt", "2099-01-01T00:00:00+00:00"),
            ("reviewedAt", "2026-01-01T00:00:00"),
            ("images", {}),
        ):
            invalid = copy.deepcopy(self.approval)
            invalid[key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                validate(invalid, self.root)
        image = self.root / next(iter(expected_images()))
        image.write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "screenshot missing/changed"):
            validate(self.approval, self.root)
