"""Bind human-approved visual baselines to source and image hashes."""

import hashlib
from datetime import UTC, datetime


def source_digest(root):
    paths = []
    for directory in ("apps/web/src", "apps/web/public", "packages/design/src"):
        paths.extend(path for path in (root / directory).rglob("*") if path.is_file())
    paths.extend(
        root / name
        for name in (
            "tests/application/browser.spec.mjs",
            "tests/application/capture.mjs",
            "tests/application/visual-context.mjs",
            "tests/application/serve.mjs",
            "crates/infrastructure/tests/seed_preview.rs",
            "crates/infrastructure/tests/support/browser_catalogue.rs",
            "tests/fixtures/approved_catalogue.rs",
            "playwright.application.config.mjs",
        )
    )
    digest = hashlib.sha256()
    for path in sorted(paths):
        digest.update(str(path.relative_to(root)).encode() + b"\0" + path.read_bytes() + b"\0")
    return digest.hexdigest()


def expected_images():
    return {
        f"tests/application/baselines/{browser}/{surface}-{locale}-{theme}-{width}.png"
        for browser in ("chromium", "firefox", "webkit")
        for surface in (
            "courtyard",
            "gallery",
            "collection",
            "covers",
            "filled",
            "empty",
            "missing",
        )
        for locale in (
            ("nl", "de", "fr", "en", "ar", "tr", "ku")
            if surface == "gallery"
            else ("en", "ku")
            if surface == "empty"
            else ("nl", "ar")
        )
        for theme in ("light", "dark")
        for width in (1280, 390)
    }


def validate(approval, root):
    if approval.get("schemaVersion") != 1 or approval.get("humanApproved") is not True:
        raise ValueError("Manual visual approval required")
    if not isinstance(approval.get("reviewer"), str) or len(approval["reviewer"].strip()) < 3:
        raise ValueError("Human reviewer identity missing")
    if not isinstance(approval.get("evidence"), str) or len(approval["evidence"].strip()) < 30:
        raise ValueError("Human approval evidence missing")
    reviewed = datetime.fromisoformat(approval.get("reviewedAt", ""))
    if reviewed.tzinfo is None or reviewed > datetime.now(UTC):
        raise ValueError("Invalid visual approval timestamp")
    if approval.get("sourceSha256") != source_digest(root):
        raise ValueError("UI source changed after manual approval")
    images = approval.get("images", {})
    if set(images) != expected_images():
        raise ValueError("Visual approval inventory incomplete")
    for name, expected in images.items():
        path = root / name
        if not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != expected:
            raise ValueError(f"Approved screenshot missing/changed: {name}")
