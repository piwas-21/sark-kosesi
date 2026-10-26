"""Validate reviewer-authored evidence; never infer approval from a filename."""

import re
from datetime import UTC, datetime, timedelta

REPOSITORY = "piwas-21/sark-kosesi"


def validate_review(report, head, base, now=None):
    if not isinstance(report, dict) or report.get("schemaVersion") != 1:
        raise ValueError("Unsupported independent review schema")
    if report.get("repository") != REPOSITORY:
        raise ValueError("Review belongs to another repository")
    for key, expected in (("head", head), ("base", base)):
        if not re.fullmatch(r"[0-9a-f]{40}", str(expected)) or report.get(key) != expected:
            raise ValueError("Review does not match the exact head/base")
    if report.get("independent") is not True:
        raise ValueError("Reviewer must attest independent review")
    if not isinstance(report.get("reviewer"), str) or len(report["reviewer"].strip()) < 3:
        raise ValueError("Reviewer identity is missing")
    if report.get("verdict") != "approved" or report.get("issues") != []:
        raise ValueError("Review is not approved with zero unresolved findings")
    if not isinstance(report.get("summary"), str) or len(report["summary"].strip()) < 40:
        raise ValueError("Substantive reviewer summary is missing")
    try:
        reviewed = datetime.fromisoformat(report["reviewedAt"])
    except (KeyError, TypeError, ValueError) as error:
        raise ValueError("Review timestamp is invalid") from error
    if reviewed.tzinfo is None:
        raise ValueError("Review timestamp must include timezone")
    now = now or datetime.now(UTC)
    age = now - reviewed
    if age < timedelta(0) or age > timedelta(hours=24):
        raise ValueError("Review is future-dated or expired; obtain fresh review")
