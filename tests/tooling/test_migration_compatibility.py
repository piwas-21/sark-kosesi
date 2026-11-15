"""The offline diagnostic distinguishes migration history from runtime readiness."""

import hashlib
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from check_migration_compatibility import MAX_ARTIFACT_BYTES, compare, read_artifact

SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "check_migration_compatibility.py"


def entry(version, kind="simple", letter="a"):
    return {"version": version, "kind": kind, "checksum_sha384": letter * 96}


def applied(version, letter="a", success=True):
    return {"version": version, "success": success, "checksum_sha384": letter * 96}


def manifest(*entries, ignore_missing=False):
    return {"schema_version": 1, "ignore_missing": ignore_missing, "migrations": list(entries)}


def history(*entries):
    return {"schema_version": 1, "migrations": list(entries)}


class MigrationHistory(unittest.TestCase):
    def test_checks_forward_half_and_sorts_pending_versions_numerically(self):
        embedded = manifest(
            entry("1"),
            entry("2", "reversible_up", "b"),
            entry("2", "reversible_down", "c"),
            entry("8"),
            entry("10"),
        )
        applied_rows = history(applied("2", "b"), applied("1"))
        self.assertEqual(
            compare(embedded, applied_rows),
            {
                "schema_version": 1,
                "applied_count": 2,
                "ignore_missing": False,
                "ignored_applied_versions": [],
                "pending_versions": ["8", "10"],
            },
        )
        self.assertEqual(compare(embedded, history())["applied_count"], 0)
        with self.assertRaisesRegex(ValueError, "Checksum differs for applied version 2"):
            compare(embedded, history(applied("2", "c")))
        with self.assertRaisesRegex(ValueError, "Embedded migration 8 is pending"):
            compare(embedded, applied_rows, require_applied=True)

    def test_rejects_downgrade_and_dirty_history_with_explicit_missing_policy(self):
        newer_history = history(applied("1"), applied("2"))
        with self.assertRaisesRegex(ValueError, "missing applied version 2"):
            compare(manifest(entry("1")), newer_history)
        permissive = manifest(entry("1"), ignore_missing=True)
        self.assertEqual(compare(permissive, newer_history)["ignored_applied_versions"], ["2"])
        exact_history = history(applied("1"))
        self.assertEqual(compare(permissive, exact_history)["pending_versions"], [])
        for retained in (exact_history, newer_history):
            with (
                self.subTest(retained=retained),
                self.assertRaisesRegex(ValueError, "requires a strict"),
            ):
                compare(permissive, retained, require_applied=True)
        with self.assertRaisesRegex(ValueError, "Checksum differs for applied version 1"):
            compare(permissive, history(applied("1", "b"), applied("2")))
        with self.assertRaisesRegex(ValueError, "dirty at version 2"):
            compare(permissive, history(applied("1"), applied("2", success=False)))

    def test_rejects_invalid_artifact_schemas_and_migration_identities(self):
        embedded = manifest(entry("1"))
        for invalid in (
            {},
            {**embedded, "schema_version": True},
            {**embedded, "ignore_missing": 1},
            {**embedded, "extra": False},
            {**embedded, "migrations": {}},
            manifest(),
            manifest(entry("1"), entry("1")),
            manifest(entry("2"), entry("1")),
            manifest(entry("1", "reversible_down"), entry("1", "reversible_up")),
            manifest(entry("1", "reversible_down")),
            manifest(entry("1", "bad_kind")),
            manifest(entry("1", letter="A")),
            manifest(entry("01")),
            manifest(entry("9223372036854775808")),
            manifest(entry(1)),
        ):
            with self.subTest(manifest=invalid), self.assertRaises(ValueError):
                compare(invalid, history())
        for invalid in (
            history(applied("1"), applied("1")),
            history(applied("1", success=1)),
            history({}),
        ):
            with self.subTest(history=invalid), self.assertRaises(ValueError):
                compare(embedded, invalid)
        self.assertEqual(
            compare(manifest(entry("-9223372036854775808")), history())["pending_versions"],
            ["-9223372036854775808"],
        )

    def test_cli_binds_exact_input_bytes_and_emits_no_success_record_after_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            image_file = Path(directory) / "image.json"
            database_file = Path(directory) / "database.json"
            manifest_bytes = json.dumps(manifest(entry("1")), indent=2).encode()
            history_bytes = json.dumps(history(applied("1"))).encode()
            image_file.write_bytes(manifest_bytes)
            database_file.write_bytes(history_bytes)
            args = [sys.executable, SCRIPT, image_file, database_file, "--require-applied"]
            output = subprocess.run(args, capture_output=True, text=True, check=True)
            self.assertEqual(
                json.loads(output.stdout),
                {
                    "schema_version": 1,
                    "applied_count": 1,
                    "ignore_missing": False,
                    "ignored_applied_versions": [],
                    "pending_versions": [],
                    "manifest_sha256": hashlib.sha256(manifest_bytes).hexdigest(),
                    "history_sha256": hashlib.sha256(history_bytes).hexdigest(),
                },
            )
            image_file.write_text(json.dumps(manifest(entry("1"), ignore_missing=True)))
            rejected = subprocess.run(args, capture_output=True, text=True, check=False)
            self.assertNotEqual(rejected.returncode, 0)
            self.assertEqual(rejected.stdout, "")
            self.assertIn("requires a strict", rejected.stderr)
            image_file.write_bytes(manifest_bytes)
            database_file.write_bytes(b'{"schema_version":1,"migrations":[],"migrations":[]}')
            rejected = subprocess.run(args, capture_output=True, text=True, check=False)
            self.assertNotEqual(rejected.returncode, 0)
            self.assertEqual(rejected.stdout, "")
            self.assertIn("duplicate key", rejected.stderr)
            database_file.unlink()
            missing = subprocess.run(args, capture_output=True, text=True, check=False)
            self.assertNotEqual(missing.returncode, 0)
            self.assertEqual(missing.stdout, "")
            self.assertNotIn(directory, missing.stderr)

    def test_file_reader_rejects_invalid_utf8_and_oversized_input(self):
        with tempfile.TemporaryDirectory() as directory:
            file = Path(directory) / "artifact.json"
            for data, message in (
                (b"\xff", "not valid JSON"),
                (b"\x20" * (MAX_ARTIFACT_BYTES + 1), "size limit"),
            ):
                file.write_bytes(data)
                with self.assertRaisesRegex(ValueError, message):
                    read_artifact(file)
