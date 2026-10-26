import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from check_foundation import (
    APPLICATION_SCRIPTS,
    check_stage,
    check_workflow,
    local_target,
    placeholder_command,
)


class Foundation(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.patcher = patch("check_foundation.ROOT", self.root)
        self.patcher.start()
        self.addCleanup(self.patcher.stop)

    def test_placeholder_commands_cannot_claim_application_success(self):
        for command in ("", "echo success", "vitest --passWithNoTests", "cargo test; exit 0"):
            with self.subTest(command=command):
                self.assertTrue(placeholder_command(command))
        for command in ("cargo clippy --locked", "node tests/checks.mjs", "vitest run"):
            with self.subTest(command=command):
                self.assertFalse(placeholder_command(command))

    def test_crates_cannot_sneak_through_foundation_stage(self):
        config = {"stage": "foundation"}
        check_stage(config, {})
        crate = self.root / "crates/domain/lib.rs"
        crate.parent.mkdir(parents=True)
        crate.write_text("pub struct Stock(u32);")
        with self.assertRaisesRegex(ValueError, "application CI"):
            check_stage(config, {})

    def test_application_requires_rust_commands_and_real_workflow(self):
        config = {"stage": "application", "requiredChecks": ["Application gate"]}
        package = {
            "scripts": {
                name: "python3 scripts/application_checks.py" for name in APPLICATION_SCRIPTS
            }
        }
        with self.assertRaisesRegex(ValueError, "Rust source"):
            check_stage(config, package)
        crate = self.root / "crates/domain/lib.rs"
        crate.parent.mkdir(parents=True)
        crate.write_text("pub struct Stock(u32);")
        workflow = self.root / ".github/workflows/ci.yml"
        workflow.parent.mkdir(parents=True)
        workflow.write_text("name: Application gate")
        check_stage(config, package)
        package["scripts"]["check:rust"] = "echo passed"
        with self.assertRaisesRegex(ValueError, "real application command"):
            check_stage(config, package)
        package["scripts"]["check:rust"] = "cargo clippy --locked"
        workflow.write_text("name: Foundation")
        with self.assertRaisesRegex(ValueError, "job missing"):
            check_stage(config, package)

    def test_link_parser_accepts_local_and_rejects_escape_or_missing(self):
        source = self.root / "README.md"
        source.write_text("Fixture")
        local_target(source, "./README.md#anchor")
        local_target(source, "https://example.invalid/page")
        for link in ("../outside.md", "absent.md"):
            with self.subTest(link=link), self.assertRaises(ValueError):
                local_target(source, link)

    def test_workflow_controls_discriminate_pins_and_unsafe_options(self):
        package = {"devDependencies": {"@playwright/test": "1.64.1"}}
        pinned = "uses: actions/checkout@" + "a" * 40
        check_workflow(pinned, package)
        for workflow in (
            "uses: actions/checkout@main",
            "image: postgres:latest",
            pinned + "\ncontinue-on-error: true",
            pinned + "\npull_request_target:",
        ):
            with self.subTest(workflow=workflow), self.assertRaises(ValueError):
                check_workflow(workflow, package)
