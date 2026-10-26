import copy
import unittest
from datetime import UTC, datetime, timedelta

from review_policy import validate_review

HEAD, BASE = "a" * 40, "b" * 40
NOW = datetime(2026, 10, 26, 12, tzinfo=UTC)


def report(now=NOW):
    return {
        "schemaVersion": 1,
        "repository": "piwas-21/sark-kosesi",
        "head": HEAD,
        "base": BASE,
        "reviewer": "independent-test-reviewer",
        "independent": True,
        "reviewedAt": now.isoformat(),
        "verdict": "approved",
        "issues": [],
        "summary": "Reviewed authorization, evidence freshness and hostile input handling.",
    }


class ReviewPolicy(unittest.TestCase):
    def test_current_independent_review(self):
        validate_review(report(), HEAD, BASE, NOW)

    def test_reject_untrustworthy_claims(self):
        changes = [
            {"schemaVersion": 2},
            {"repository": "other"},
            {"head": BASE},
            {"base": HEAD},
            {"independent": False},
            {"reviewer": ""},
            {"reviewer": 123},
            {"verdict": "changes_requested"},
            {"issues": [{"severity": "high", "message": "Authorization bypass"}]},
            {"issues": None},
            {"summary": "short"},
            {"summary": 123},
            {"reviewedAt": "invalid"},
            {"reviewedAt": None},
            {"reviewedAt": "2026-10-26T12:00:00"},
            {"reviewedAt": (NOW + timedelta(seconds=1)).isoformat()},
            {"reviewedAt": (NOW - timedelta(hours=24, seconds=1)).isoformat()},
        ]
        for change in changes:
            with self.subTest(change=change), self.assertRaises(ValueError):
                validate_review({**report(), **change}, HEAD, BASE, NOW)

    def test_required_fields_cannot_be_omitted(self):
        for key in report():
            value = copy.deepcopy(report())
            del value[key]
            with self.subTest(key=key), self.assertRaises(ValueError):
                validate_review(value, HEAD, BASE, NOW)

    def test_malformed_head_and_nondocument(self):
        for document, head in (([], HEAD), (report(), "main")):
            with self.assertRaises(ValueError):
                validate_review(document, head, BASE, NOW)
