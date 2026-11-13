"""Owned restore resources must never be removed while a cluster may run."""

import contextlib
import io
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from rehearse_restore import digest_file, owned_directory, pg_env, source_state


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

    def test_source_revision_uses_owned_git_context_and_detects_hidden_changes(self):
        environment = {
            key: value for key, value in os.environ.items() if not key.startswith("GIT_")
        }
        with tempfile.TemporaryDirectory() as directory:
            owned = Path(directory) / "owned"
            foreign = Path(directory) / "foreign"

            def git(root, *args):
                return subprocess.run(
                    ["git", "-C", str(root), *args],
                    env=environment,
                    check=True,
                    text=True,
                    capture_output=True,
                ).stdout.strip()

            for root, contents in ((owned, "owned source"), (foreign, "foreign source")):
                root.mkdir()
                git(root, "init", "-q")
                (root / "fixture.txt").write_text(contents)
                git(root, "add", "fixture.txt")
                git(
                    root,
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "commit",
                    "-qm",
                    "fixture",
                )
            revision = git(owned, "rev-parse", "HEAD")
            self.assertNotEqual(revision, git(foreign, "rev-parse", "HEAD"))
            with patch.dict(
                os.environ,
                {
                    "GIT_DIR": str(foreign / ".git"),
                    "GIT_WORK_TREE": str(foreign),
                    "GIT_INDEX_FILE": str(foreign / ".git" / "index"),
                },
            ):
                self.assertEqual(source_state(owned), (revision, True))
                (owned / "untracked.txt").write_text("new source")
                self.assertEqual(source_state(owned), (revision, False))
                (owned / "untracked.txt").unlink()
                (owned / "fixture.txt").write_text("changed owned source")
                self.assertEqual(source_state(owned), (revision, False))
                for flag in ("assume-unchanged", "skip-worktree"):
                    git(owned, "update-index", f"--{flag}", "fixture.txt")
                    self.assertEqual(git(owned, "status", "--porcelain"), "")
                    self.assertEqual(source_state(owned), (revision, False))
                    git(owned, "update-index", f"--no-{flag}", "fixture.txt")
                git(owned, "add", "fixture.txt")
                self.assertEqual(source_state(owned), (revision, False))

    def test_replacement_refs_cannot_change_reported_source_state(self):
        environment = {
            key: value for key, value in os.environ.items() if not key.startswith("GIT_")
        }
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)

            def git(*args):
                return subprocess.run(
                    ["git", "-C", str(root), *args],
                    env=environment,
                    check=True,
                    text=True,
                    capture_output=True,
                ).stdout.strip()

            git("init", "-q")
            fixture = root / "fixture.txt"
            for contents in ("original", "replacement"):
                fixture.write_text(contents)
                git("add", "fixture.txt")
                git(
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "commit",
                    "-qm",
                    contents,
                )
            replacement = git("rev-parse", "HEAD")
            original = git("rev-parse", "HEAD^")
            git("--no-replace-objects", "checkout", "-q", "--detach", original)
            git("replace", original, replacement)
            self.assertNotEqual(git("status", "--porcelain"), "")
            self.assertEqual(git("--no-replace-objects", "status", "--porcelain"), "")
            self.assertEqual(source_state(root), (original, True))
