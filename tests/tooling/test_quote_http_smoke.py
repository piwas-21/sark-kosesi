"""Safe launcher controls use fakes; they never start PostgreSQL or the API."""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import call, patch

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
                    assert timeout == 0.2
                    return 7

            return Child()

        with (
            patch.dict(os.environ, environment, clear=True),
            patch.object(sys, "argv", ["quote_http_smoke.py"]),
            patch.object(smoke, "run", side_effect=fake_run),
            patch.object(smoke, "native_cluster", side_effect=fake_cluster),
            patch.object(smoke.subprocess, "Popen", side_effect=fake_subprocess),
            patch.object(smoke, "drain_group", return_value=True),
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
                patch.object(smoke, "drain_group", return_value=True),
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

    def test_signal_during_child_run_requests_owned_group_drain(self):
        class Child:
            pid = 12345
            calls = 0

            def wait(self, timeout=None):
                self.calls += 1
                if self.calls == 1:
                    os.kill(os.getpid(), smoke.signal.SIGTERM)
                    raise subprocess.TimeoutExpired("fixture", timeout)
                return 143

        child = Child()
        with (
            patch.dict(os.environ, {"P1_POSTGRES_BIN": "/fixture/bin"}, clear=True),
            patch.object(sys, "argv", ["quote_http_smoke.py"]),
            patch.object(smoke, "native_cluster", return_value="postgresql://fixture"),
            patch.object(smoke.subprocess, "Popen", return_value=child),
            patch.object(smoke, "drain_group", return_value=True) as drain,
            patch.object(
                smoke,
                "run",
                return_value=subprocess.CompletedProcess([], 0, "postgres (PostgreSQL) 18.1", ""),
            ),
        ):
            with self.assertRaisesRegex(InterruptedError, "interrupted"):
                smoke.main()
        drain.assert_called_once_with(child)

    def test_startup_signal_waits_for_cluster_then_stops_it(self):
        calls = []

        def fake_cluster(directory, _binaries):
            (directory / "data").mkdir()
            (directory / "data/postmaster.pid").write_text("fake")
            os.kill(os.getpid(), smoke.signal.SIGTERM)
            return "postgresql://fixture"

        def fake_run(*args, **_kwargs):
            calls.append(args)
            if "stop" in args:
                (Path(args[2]) / "postmaster.pid").unlink()
            return subprocess.CompletedProcess(args, 0, "postgres (PostgreSQL) 18.1", "")

        with (
            patch.dict(os.environ, {"P1_POSTGRES_BIN": "/fixture/bin"}, clear=True),
            patch.object(sys, "argv", ["quote_http_smoke.py"]),
            patch.object(smoke, "native_cluster", side_effect=fake_cluster),
            patch.object(smoke.subprocess, "Popen") as popen,
            patch.object(smoke, "run", side_effect=fake_run),
        ):
            with self.assertRaisesRegex(InterruptedError, "interrupted"):
                smoke.main()
        popen.assert_not_called()
        self.assertEqual(calls[-1][-2:], ("-m", "fast"))

    def test_signal_during_child_creation_still_drains_created_group(self):
        class Child:
            def wait(self, timeout=None):
                raise AssertionError("signal must be checked before waiting")

        child = Child()

        def create(*_args, **_kwargs):
            os.kill(os.getpid(), smoke.signal.SIGTERM)
            return child

        with (
            patch.dict(os.environ, {"P1_POSTGRES_BIN": "/fixture/bin"}, clear=True),
            patch.object(sys, "argv", ["quote_http_smoke.py"]),
            patch.object(smoke, "native_cluster", return_value="postgresql://fixture"),
            patch.object(smoke.subprocess, "Popen", side_effect=create),
            patch.object(smoke, "drain_group", return_value=True) as drain,
            patch.object(
                smoke,
                "run",
                return_value=subprocess.CompletedProcess([], 0, "postgres (PostgreSQL) 18.1", ""),
            ),
        ):
            with self.assertRaisesRegex(InterruptedError, "interrupted"):
                smoke.main()
        drain.assert_called_once_with(child)

    def test_leader_exit_does_not_hide_stubborn_group(self):
        class Child:
            pid = 12345

            def poll(self):
                return 0

        with (
            patch.object(smoke, "group_present", side_effect=[True, True, False]),
            patch.object(smoke.os, "killpg") as kill,
        ):
            self.assertTrue(smoke.drain_group(Child(), grace=0))
        self.assertEqual(
            kill.call_args_list,
            [call(12345, smoke.signal.SIGTERM), call(12345, smoke.signal.SIGKILL)],
        )

    def test_group_status_rejects_unknown_permissions(self):
        with patch.object(smoke.os, "killpg", side_effect=PermissionError):
            self.assertIsNone(smoke.group_present(12345))
        with patch.object(smoke.os, "killpg", side_effect=ProcessLookupError):
            self.assertFalse(smoke.group_present(12345))

    def test_failed_native_startup_retains_directory_even_without_pidfile(self):
        with tempfile.TemporaryDirectory(prefix="quote-smoke-unit-") as parent:
            directory = Path(parent) / "owned"
            directory.mkdir()
            with (
                patch.dict(os.environ, {"P1_POSTGRES_BIN": "/fixture/bin"}, clear=True),
                patch.object(sys, "argv", ["quote_http_smoke.py"]),
                patch.object(smoke.tempfile, "mkdtemp", return_value=str(directory)),
                patch.object(smoke, "native_cluster", side_effect=RuntimeError("uncertain")),
                patch.object(
                    smoke,
                    "run",
                    return_value=subprocess.CompletedProcess(
                        [], 0, "postgres (PostgreSQL) 18.1", ""
                    ),
                ),
            ):
                with self.assertRaisesRegex(RuntimeError, "startup unconfirmed"):
                    smoke.main()
            self.assertTrue(directory.exists())

    def test_unknown_group_retains_cluster_without_stopping_it(self):
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
                patch.object(smoke, "drain_group", return_value=False),
                patch.object(
                    smoke,
                    "run",
                    return_value=subprocess.CompletedProcess(
                        [], 0, "postgres (PostgreSQL) 18.1", ""
                    ),
                ) as run,
            ):
                with self.assertRaisesRegex(RuntimeError, "not drained"):
                    smoke.main()
            self.assertTrue((directory / "data/postmaster.pid").exists())
            self.assertEqual(run.call_count, 1)


if __name__ == "__main__":
    unittest.main()
