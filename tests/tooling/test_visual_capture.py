"""Exercise the visual wrapper with local commands and no browser or network."""

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
VISUAL = ROOT / "tests/application/visual.mjs"


class VisualCapture(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.log = self.root / "commands.log"
        self.output = self.root / "github-output"
        self.output.write_text("existing=value\n")
        self.command(
            "python3",
            'printf "approval\\n" >> "$COMMAND_LOG"\n'
            'if [ "$APPROVAL_MODE" = signal ]; then kill -TERM $$; fi\n'
            'exit "$APPROVAL_STATUS"\n',
        )
        self.command(
            "npx",
            'printf "comparison:%s\\n" "$*" >> "$COMMAND_LOG"\n'
            'if [ "$COMPARISON_MODE" = signal ]; then kill -TERM $$; fi\n'
            'exit "$COMPARISON_STATUS"\n',
        )
        capture = self.root / "tests/application/capture.mjs"
        capture.parent.mkdir(parents=True)
        capture.write_text(
            'import { appendFileSync } from "node:fs";\n'
            'appendFileSync(process.env.COMMAND_LOG, "capture\\n");\n'
            'if (process.env.CAPTURE_MODE === "signal") process.kill(process.pid, "SIGTERM");\n'
            "process.exit(Number(process.env.CAPTURE_STATUS));\n"
        )
        self.env = {
            **os.environ,
            "PATH": str(self.bin),
            "COMMAND_LOG": str(self.log),
            "GITHUB_OUTPUT": str(self.output),
            "APPROVAL_MODE": "exit",
            "APPROVAL_STATUS": "1",
            "CAPTURE_MODE": "exit",
            "CAPTURE_STATUS": "0",
            "COMPARISON_MODE": "exit",
            "COMPARISON_STATUS": "0",
        }
        self.node = shutil.which("node")
        self.assertIsNotNone(self.node, "Node is required for visual tooling tests")

    def command(self, name, body):
        path = self.bin / name
        path.write_text("#!/bin/sh\n" + body)
        path.chmod(0o755)

    def run_visual(self):
        return subprocess.run(
            [self.node, str(VISUAL)],
            cwd=self.root,
            env=self.env,
            capture_output=True,
            text=True,
            timeout=15,
            check=False,
        )

    def commands(self):
        return self.log.read_text().splitlines() if self.log.exists() else []

    def assert_no_completion(self):
        self.assertEqual(self.output.read_text(), "existing=value\n")

    def test_missing_or_stale_approval_captures_once_and_still_fails_gate(self):
        result = self.run_visual()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Manual visual-baseline approval required", result.stderr)
        self.assertEqual(self.commands(), ["approval", "capture"])
        self.assertEqual(self.output.read_text(), "existing=value\nvisual_candidates=complete\n")

    def test_successful_local_capture_without_github_output_still_fails_gate(self):
        del self.env["GITHUB_OUTPUT"]
        result = self.run_visual()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Manual visual-baseline approval required", result.stderr)
        self.assertEqual(self.commands(), ["approval", "capture"])
        self.assert_no_completion()

    def test_failed_or_signalled_capture_never_marks_completion(self):
        for mode, status in (("exit", "7"), ("signal", "0")):
            with self.subTest(mode=mode):
                self.log.unlink(missing_ok=True)
                self.env.update(CAPTURE_MODE=mode, CAPTURE_STATUS=status)
                result = self.run_visual()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("capture", result.stderr.lower())
                self.assertIn("SIGTERM" if mode == "signal" else "7", result.stderr)
                self.assertNotIn("Candidates captured", result.stderr)
                self.assertEqual(self.commands(), ["approval", "capture"])
                self.assert_no_completion()

    def test_missing_approval_command_fails_without_capture(self):
        (self.bin / "python3").unlink()
        result = self.run_visual()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("ENOENT", result.stderr)
        self.assertEqual(self.commands(), [])
        self.assert_no_completion()

    def test_signalled_approval_check_fails_without_capture(self):
        self.env["APPROVAL_MODE"] = "signal"
        result = self.run_visual()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("SIGTERM", result.stderr)
        self.assertEqual(self.commands(), ["approval"])
        self.assert_no_completion()

    def test_approved_comparison_never_marks_completion(self):
        self.env["APPROVAL_STATUS"] = "0"
        for mode, status in (("exit", "0"), ("exit", "9"), ("signal", "0")):
            with self.subTest(mode=mode, status=status):
                self.log.unlink(missing_ok=True)
                self.env.update(COMPARISON_MODE=mode, COMPARISON_STATUS=status)
                result = self.run_visual()
                self.assertEqual(result.returncode == 0, mode == "exit" and status == "0")
                self.assertEqual(
                    self.commands(),
                    [
                        "approval",
                        "comparison:playwright test --config playwright.application.config.mjs "
                        "--grep @visual --update-snapshots=none",
                    ],
                )
                self.assert_no_completion()

    def test_missing_comparison_command_never_marks_completion(self):
        self.env["APPROVAL_STATUS"] = "0"
        (self.bin / "npx").unlink()
        result = self.run_visual()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("ENOENT", result.stderr)
        self.assertEqual(self.commands(), ["approval"])
        self.assert_no_completion()

    def test_ci_fallback_requires_browser_failure_and_incomplete_capture(self):
        workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        fallback = workflow.split(
            "- name: Capture visual review candidates after a browser failure", 1
        )[1]
        condition = fallback.splitlines()[1].strip()
        self.assertEqual(
            condition,
            "if: failure() && steps.browser.outcome == 'failure' && "
            "steps.browser.outputs.visual_candidates != 'complete'",
        )


if __name__ == "__main__":
    unittest.main()
