import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from datetime import UTC, datetime
from pathlib import Path

from test_review_policy import report

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "scripts"))
from gate_policy import (
    read_release_manifest as read_manifest_from_directory,
)
from gate_policy import (
    release_directory,
    validate_checks,
    validate_receipt,
    validate_release_environment,
)
from gate_policy import (
    validate_release as validate_with_directory,
)


def read_release_manifest(root, head):
    with release_directory(root, head) as directory:
        return read_manifest_from_directory(directory)


def validate_release(config, evidence, head, root, manifest_identity=None):
    validate_release_environment(config, head, root)
    with release_directory(root, head) as directory:
        if manifest_identity is None:
            _, manifest_identity = read_manifest_from_directory(directory)
        validate_with_directory(config, evidence, head, root, manifest_identity, directory)


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

    def release_fixture(self):
        (self.root / "apps").mkdir()
        (self.root / "apps/page.tsx").write_text("export default function Page() {}")
        (self.root / "crates").mkdir()
        (self.root / "crates/lib.rs").write_text("pub struct Money(i64);")
        self.release_dir = self.root / ".local/releases" / HEAD
        self.release_dir.mkdir(parents=True)
        (self.release_dir / "security.txt").write_bytes(b"security evidence\n")
        (self.release_dir / "restore.txt").write_bytes(b"restore evidence\n")
        self.config = {
            "stage": "application",
            "productionEnabled": True,
            "productionGates": {"security": "passed", "restore": "passed"},
        }
        self.evidence = {
            "version": 1,
            "head": HEAD,
            "gates": {
                "security": {
                    "gate": "security",
                    "head": HEAD,
                    "status": "passed",
                    "evidence": "security.txt",
                    "sha256": hashlib.sha256(b"security evidence\n").hexdigest(),
                },
                "restore": {
                    "gate": "restore",
                    "head": HEAD,
                    "status": "passed",
                    "evidence": "restore.txt",
                    "sha256": hashlib.sha256(b"restore evidence\n").hexdigest(),
                },
            },
        }
        (self.release_dir / "evidence.json").write_text(json.dumps(self.evidence))

    def test_manifest_and_real_evidence_accepted(self):
        self.release_fixture()
        evidence, identity = read_release_manifest(self.root, HEAD)
        validate_release(self.config, evidence, HEAD, self.root, identity)

    def test_requires_application_and_passed_gates(self):
        self.release_fixture()
        for config in [
            {**self.config, "productionEnabled": False},
            {**self.config, "stage": "foundation"},
            {**self.config, "productionGates": {}},
            {**self.config, "productionGates": {"security": "pending"}},
        ]:
            with self.subTest(config=config), self.assertRaises(ValueError):
                validate_release(config, self.evidence, HEAD, self.root)
        (self.root / "apps/page.tsx").unlink()
        with self.assertRaises(ValueError):
            validate_release(self.config, self.evidence, HEAD, self.root)
        (self.root / "apps/page.tsx").write_text("app")
        (self.root / "crates/lib.rs").unlink()
        with self.assertRaises(ValueError):
            validate_release(self.config, self.evidence, HEAD, self.root)

    def test_malformed_stale_and_gate_swapped_manifests_rejected(self):
        self.release_fixture()
        for change in [
            {"version": 2},
            {"version": True},
            {"head": BASE},
            {"extra": 1},
            {"gates": {}},
            {"gates": {**self.evidence["gates"], "extra": {}}},
            {
                "gates": {
                    "security": self.evidence["gates"]["restore"],
                    "restore": self.evidence["gates"]["security"],
                }
            },
        ]:
            with self.subTest(change=change), self.assertRaises(ValueError):
                validate_release(self.config, {**self.evidence, **change}, HEAD, self.root)
        item = self.evidence["gates"]["security"]
        for change in [
            {"gate": "restore"},
            {"head": BASE},
            {"status": "pending"},
            {"sha256": "bad"},
            {"extra": "unexpected"},
        ]:
            candidate = {
                **self.evidence,
                "gates": {**self.evidence["gates"], "security": {**item, **change}},
            }
            with self.subTest(change=change), self.assertRaises(ValueError):
                validate_release(self.config, candidate, HEAD, self.root)
        (self.release_dir / "evidence.json").write_text('{"version":1,"version":1}')
        with self.assertRaises((ValueError, OSError)):
            read_release_manifest(self.root, HEAD)

    def test_paths_content_and_aliases_rejected(self):
        self.release_fixture()
        item = self.evidence["gates"]["security"]
        (self.release_dir / "blank.txt").write_text("  \n")
        (self.release_dir / "link.txt").symlink_to("security.txt")
        os.link(self.release_dir / "restore.txt", self.release_dir / "alias.txt")
        (self.release_dir / "copy.txt").write_bytes(b"restore evidence\n")
        os.link(self.release_dir / "evidence.json", self.release_dir / "manifest-alias.txt")
        for name in [
            "../security.txt",
            "/tmp/evidence",
            "./security.txt",
            "x//y",
            "x\\y",
            "evidence.json",
            "missing.txt",
            "link.txt",
            "blank.txt",
            "alias.txt",
            "manifest-alias.txt",
            "restore.txt",
            "copy.txt",
        ]:
            candidate_item = {**item, "evidence": name}
            if name in ("alias.txt", "restore.txt", "copy.txt", "manifest-alias.txt", "blank.txt"):
                candidate_item["sha256"] = hashlib.sha256(
                    (self.release_dir / name).read_bytes()
                ).hexdigest()
            candidate = {
                **self.evidence,
                "gates": {**self.evidence["gates"], "security": candidate_item},
            }
            with self.subTest(name=name), self.assertRaises((ValueError, OSError)):
                validate_release(self.config, candidate, HEAD, self.root)
        (self.release_dir / "security.txt").write_text("changed content")
        with self.assertRaises(ValueError):
            validate_release(self.config, self.evidence, HEAD, self.root)

    def test_manifest_and_parent_symlinks_rejected(self):
        self.release_fixture()
        manifest = self.release_dir / "evidence.json"
        manifest.unlink()
        manifest.symlink_to("security.txt")
        with self.assertRaises((ValueError, OSError)):
            read_release_manifest(self.root, HEAD)
        manifest.unlink()
        manifest.write_text(json.dumps(self.evidence))
        (self.release_dir / "nested").symlink_to(self.release_dir, target_is_directory=True)
        item = self.evidence["gates"]["security"]
        candidate = {
            **self.evidence,
            "gates": {
                **self.evidence["gates"],
                "security": {**item, "evidence": "nested/security.txt"},
            },
        }
        with self.assertRaises((ValueError, OSError)):
            validate_release(self.config, candidate, HEAD, self.root)

    def test_release_directory_symlink_and_swapped_contents_rejected(self):
        self.release_fixture()
        security = self.release_dir / "security.txt"
        restore = self.release_dir / "restore.txt"
        security_bytes, restore_bytes = security.read_bytes(), restore.read_bytes()
        security.write_bytes(restore_bytes)
        restore.write_bytes(security_bytes)
        with self.assertRaises(ValueError):
            validate_release(self.config, self.evidence, HEAD, self.root)
        releases = self.root / ".local/releases"
        moved = self.root / ".local/moved-releases"
        releases.rename(moved)
        releases.symlink_to(moved, target_is_directory=True)
        with self.assertRaises((ValueError, OSError)):
            read_release_manifest(self.root, HEAD)

    def test_invalid_head_and_fifo_are_rejected(self):
        self.release_fixture()
        with self.assertRaises(ValueError):
            read_release_manifest(self.root, "../outside")
        os.mkfifo(self.release_dir / "pipe")
        item = self.evidence["gates"]["security"]
        candidate = {
            **self.evidence,
            "gates": {**self.evidence["gates"], "security": {**item, "evidence": "pipe"}},
        }
        with self.assertRaises(ValueError):
            validate_release(self.config, candidate, HEAD, self.root)

    def test_revision_directory_is_pinned_across_manifest_and_artifacts(self):
        self.release_fixture()
        replacement = self.release_dir.parent / "replacement"
        replacement.mkdir()
        (replacement / "evidence.json").write_bytes(b"alternate manifest\n")
        os.link(replacement / "evidence.json", replacement / "security.txt")
        (replacement / "restore.txt").write_bytes(b"restore evidence\n")
        self.evidence["gates"]["security"]["sha256"] = hashlib.sha256(
            b"alternate manifest\n"
        ).hexdigest()
        (self.release_dir / "evidence.json").write_text(json.dumps(self.evidence))
        with release_directory(self.root, HEAD) as directory:
            evidence, identity = read_manifest_from_directory(directory)
            self.release_dir.rename(self.release_dir.parent / "original")
            replacement.rename(self.release_dir)
            # The replacement's artifact is its own manifest and matches the claimed digest.
            self.assertTrue(
                os.path.samefile(
                    self.release_dir / "security.txt", self.release_dir / "evidence.json"
                )
            )
            with self.assertRaisesRegex(ValueError, "Reused or changed"):
                validate_with_directory(self.config, evidence, HEAD, self.root, identity, directory)

    def test_cli_redacts_hostile_paths_and_invalid_manifest_bytes(self):
        self.release_fixture()
        repo = Path(__file__).resolve().parents[2]
        scripts = self.root / "scripts"
        scripts.mkdir()
        for name in (
            "release_readiness.py",
            "gate_policy.py",
            "audit_github.py",
            "review_policy.py",
        ):
            shutil.copy(repo / "scripts" / name, scripts / name)
        config_dir = self.root / "config"
        config_dir.mkdir()
        (config_dir / "delivery.json").write_text(json.dumps(self.config))
        fake_bin = self.root / "bin"
        fake_bin.mkdir()
        git = fake_bin / "git"
        git.write_text(f"#!/bin/sh\nprintf '%s\\n' {HEAD}\n")
        git.chmod(0o755)
        env = {**os.environ, "PATH": f"{fake_bin}:{os.environ['PATH']}"}
        manifest = self.release_dir / "evidence.json"
        sentinel = "HOSTILE_EVIDENCE_SENTINEL"
        bad_path = json.loads(json.dumps(self.evidence))
        bad_path["gates"]["security"]["evidence"] = sentinel
        bad_encoding = json.loads(json.dumps(self.evidence))
        bad_encoding["gates"]["security"]["evidence"] = sentinel + "\ud800"
        for contents in (
            json.dumps(bad_path).encode(),
            json.dumps(bad_encoding).encode(),
            b'"' + sentinel.encode() + b'\xff"',
        ):
            with self.subTest(contents=contents):
                manifest.write_bytes(contents)
                result = subprocess.run(
                    [sys.executable, str(scripts / "release_readiness.py")],
                    cwd=self.root,
                    env=env,
                    text=True,
                    capture_output=True,
                    check=False,
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("Release blocked:", result.stderr)
                self.assertNotIn(sentinel, result.stderr + result.stdout)

    def test_release_cli_blocked_before_audit_when_disabled(self):
        repo = Path(__file__).resolve().parents[2]
        result = subprocess.run(
            [sys.executable, str(repo / "scripts/release_readiness.py")],
            cwd=repo,
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Production is disabled", result.stderr)
        self.assertNotIn("GitHub", result.stderr)

    def test_review_path_escape_rejected(self):
        with self.assertRaises(ValueError):
            validate_receipt({**self.receipt, "report": "../outside.md"}, HEAD, BASE, self.root)


if __name__ == "__main__":
    unittest.main()
