"""Owned restore resources must never be removed while a cluster may run."""

import contextlib
import io
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from rehearse_restore import digest_file, owned_directory, pg_env


class RestoreRehearsalSafety(unittest.TestCase):
    def test_owned_directory_removes_only_stopped_clusters(self):
        with owned_directory() as directory:
            (directory / "source").mkdir()
            self.assertTrue(directory.exists())
        self.assertFalse(directory.exists())

    def test_owned_directory_retains_a_cluster_with_pid_on_error(self):
        with tempfile.TemporaryDirectory() as parent:
            directory = Path(parent) / "owned"
            directory.mkdir()
            with patch("rehearse_restore.tempfile.mkdtemp", return_value=str(directory)):
                with self.assertRaisesRegex(RuntimeError, "synthetic failure"):
                    with contextlib.redirect_stderr(io.StringIO()):
                        with owned_directory() as owned:
                            data = owned / "target" / "data"
                            data.mkdir(parents=True)
                            (data / "postmaster.pid").write_text("synthetic")
                            raise RuntimeError("synthetic failure")
            self.assertTrue((directory / "target" / "data" / "postmaster.pid").exists())

    def test_archive_digest_and_owned_connection_guards(self):
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / "archive"
            archive.write_bytes(b"abc")
            self.assertEqual(
                digest_file(archive),
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            )
            archive.write_bytes(b"abd")
            self.assertNotEqual(
                digest_file(archive),
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            )
        env = pg_env("postgresql://p1_test:synthetic@127.0.0.1:12345/postgres")
        self.assertEqual(env["PGHOST"], "127.0.0.1")
        self.assertEqual(env["PGDATABASE"], "postgres")
        with self.assertRaises(ValueError):
            pg_env("postgresql://p1_test:synthetic@example.test:12345/postgres")
        with self.assertRaises(ValueError):
            pg_env("postgresql://other:synthetic@127.0.0.1:12345/postgres")
