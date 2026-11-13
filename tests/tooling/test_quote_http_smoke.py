"""Safe launcher controls use fakes; they never start PostgreSQL or the API."""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import quote_http_smoke as smoke


class QuoteHttpSmoke(unittest.TestCase):
    def test_only_owned_native_database_reaches_fixed_command(self):
        database = (
            "postgresql://p1_test:syntheticfixturepassword0123456789@127.0.0.1:12345/postgres"
        )
        environment = {
            "P1_POSTGRES_BIN": "/fixture/bin",
            "PATH": "/fixture/path",
            "API_URL": "https://must-not-be-used.example",
            "DATABASE_URL": "postgresql://must-not-be-used.example/db",
            "HTTPS_PROXY": "https://must-not-be-used.example",
            "CALLBACK_INTAKE_PROFILE_ID": "must-not-be-used",
        }
        calls = []

        def fake_run(*args, **kwargs):
            calls.append(args)
            if "stop" in args:
                (Path(args[2]) / "postmaster.pid").unlink()
            return subprocess.CompletedProcess(args, 0, "postgres (PostgreSQL) 18.1\n", "")

        def fake_cluster(directory, binaries):
            self.assertEqual(binaries, Path("/fixture/bin"))
            self.assertNotIn("API_URL", os.environ)
            self.assertNotIn("HTTPS_PROXY", os.environ)
            (directory / "data").mkdir()
            (directory / "data/postmaster.pid").write_text("fake")
            return database

        def fake_subprocess(command, **kwargs):
            child_env = kwargs["env"]
            self.assertEqual(child_env["DATABASE_URL"], database)
            self.assertEqual(child_env["APP_ENV"], "test")
            self.assertNotIn("API_URL", child_env)
            self.assertNotIn("HTTPS_PROXY", child_env)
            self.assertNotIn("CALLBACK_INTAKE_PROFILE_ID", child_env)
            proof = Path(child_env["SARK_QUOTE_OWNED_DB_PROOF"])
            self.assertEqual(proof.stat().st_mode & 0o777, 0o600)
            self.assertEqual(json.loads(proof.read_text()), {"database_url": database})
            self.assertEqual(command[-3:], ["--ignored", "--exact", "owned_quote_http_smoke"])

            class Child:
                def wait(self, timeout=None):
                    self_timeout = timeout
                    assert self_timeout == 1800
                    return 7

            return Child()

        with (
            patch.dict(os.environ, environment, clear=True),
            patch.object(sys, "argv", ["quote_http_smoke.py"]),
            patch.object(smoke, "run", side_effect=fake_run),
            patch.object(smoke, "native_cluster", side_effect=fake_cluster),
            patch.object(smoke.subprocess, "Popen", side_effect=fake_subprocess),
        ):
            self.assertEqual(smoke.main(), 7)
        self.assertEqual(len(calls), 2)
        self.assertEqual(calls[1][-2:], ("-m", "fast"))

    def test_rejects_arguments_missing_path_and_wrong_version_before_cluster(self):
        for environment, argv, version in [
            ({}, ["quote_http_smoke.py"], None),
            ({"P1_POSTGRES_BIN": "relative"}, ["quote_http_smoke.py"], None),
            ({"P1_POSTGRES_BIN": "/fixture/bin"}, ["quote_http_smoke.py", "target"], None),
            (
                {"P1_POSTGRES_BIN": "/fixture/bin"},
                ["quote_http_smoke.py"],
                "postgres (PostgreSQL) 17.1",
            ),
        ]:
            with (
                self.subTest(argv=argv, version=version),
                patch.dict(os.environ, environment, clear=True),
                patch.object(sys, "argv", argv),
                patch.object(smoke, "native_cluster") as cluster,
                patch.object(
                    smoke, "run", return_value=subprocess.CompletedProcess([], 0, version or "", "")
                ),
            ):
                with self.assertRaises(SystemExit):
                    smoke.main()
                cluster.assert_not_called()

    def test_failed_stop_preserves_owned_directory(self):
        with tempfile.TemporaryDirectory(prefix="quote-smoke-unit-") as parent:
            directory = Path(parent) / "owned"
            directory.mkdir()

            def fake_cluster(path, _binaries):
                (path / "data").mkdir()
                (path / "data/postmaster.pid").write_text("fake")
                return "postgresql://fixture"

            class Child:
                def wait(self, timeout=None):
                    return 0

            with (
                patch.dict(os.environ, {"P1_POSTGRES_BIN": "/fixture/bin"}, clear=True),
                patch.object(sys, "argv", ["quote_http_smoke.py"]),
                patch.object(smoke.tempfile, "mkdtemp", return_value=str(directory)),
                patch.object(smoke, "native_cluster", side_effect=fake_cluster),
                patch.object(smoke.subprocess, "Popen", return_value=Child()),
                patch.object(
                    smoke,
                    "run",
                    side_effect=[
                        subprocess.CompletedProcess([], 0, "postgres (PostgreSQL) 18.1", ""),
                        RuntimeError("stop failed"),
                    ],
                ),
            ):
                with self.assertRaisesRegex(RuntimeError, "retained"):
                    smoke.main()
            self.assertTrue((directory / "data/postmaster.pid").exists())

    def test_timeout_terminates_only_owned_child_group(self):
        class Child:
            pid = 12345
            calls = 0

            def wait(self, timeout=None):
                self.calls += 1
                if self.calls == 1:
                    raise subprocess.TimeoutExpired("fixture", timeout)
                return 143

        child = Child()
        with (
            patch.dict(os.environ, {"P1_POSTGRES_BIN": "/fixture/bin"}, clear=True),
            patch.object(sys, "argv", ["quote_http_smoke.py"]),
            patch.object(smoke, "native_cluster", return_value="postgresql://fixture"),
            patch.object(smoke.subprocess, "Popen", return_value=child),
            patch.object(smoke.os, "killpg") as terminate,
            patch.object(
                smoke,
                "run",
                return_value=subprocess.CompletedProcess([], 0, "postgres (PostgreSQL) 18.1", ""),
            ),
        ):
            with self.assertRaisesRegex(RuntimeError, "deadline"):
                smoke.main()
        terminate.assert_called_once_with(12345, smoke.signal.SIGTERM)


if __name__ == "__main__":
    unittest.main()
