#!/usr/bin/env python3
"""Positive and injected-failure controls for lifecycle publication."""

import hashlib
import importlib.util
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("publication", Path(__file__).with_name("publish.py"))
publication = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publication)


class LifecyclePublication(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.staged = self.root / "scratch" / "evidence"
        self.staged.mkdir(parents=True)
        self.target = self.root / "published"
        (self.staged / "status.txt").write_text("stage=complete outcome=success\n")
        (self.staged / "identity.txt").write_text("revision=synthetic\nimage=synthetic\n")
        (self.staged / "results.txt").write_text("synthetic=pass\n")

    def test_complete_publication_checksums_all_files(self):
        publication.publish(self.staged, self.target)
        self.assertFalse(self.staged.exists())
        self.assertEqual(
            (self.target / "status.txt").read_text(), "stage=complete outcome=success\n"
        )
        lines = (self.target / "SHA256SUMS").read_text().splitlines()
        self.assertEqual(len(lines), 3)
        for line in lines:
            digest, name = line.split("  ./")
            self.assertEqual(digest, hashlib.sha256((self.target / name).read_bytes()).hexdigest())

    def test_checksum_generation_failure_is_not_published(self):
        with patch.object(publication.hashlib, "sha256", side_effect=OSError("hash failed")):
            with self.assertRaises(OSError):
                publication.publish(self.staged, self.target)
        self.assertFalse(self.target.exists())

    def test_manifest_write_failure_is_not_published(self):
        original = Path.write_text

        def fail_manifest(path, *args, **kwargs):
            if path.name == "SHA256SUMS":
                raise OSError("manifest write failed")
            return original(path, *args, **kwargs)

        with patch.object(Path, "write_text", fail_manifest):
            with self.assertRaises(OSError):
                publication.publish(self.staged, self.target)
        self.assertFalse(self.target.exists())

    def test_final_rename_failure_is_not_published(self):
        with patch.object(publication.os, "rename", side_effect=OSError("rename failed")):
            with self.assertRaises(OSError):
                publication.publish(self.staged, self.target)
        self.assertFalse(self.target.exists())

    def test_incomplete_success_and_existing_destination_fail(self):
        (self.staged / "results.txt").unlink()
        with self.assertRaises(ValueError):
            publication.publish(self.staged, self.target)
        self.assertFalse(self.target.exists())
        (self.staged / "results.txt").write_text("synthetic=pass\n")
        self.target.mkdir()
        with self.assertRaises(ValueError):
            publication.publish(self.staged, self.target)
        self.assertEqual(list(self.target.iterdir()), [])

    def test_failed_status_publishes_as_failure(self):
        (self.staged / "status.txt").write_text("stage=first_start outcome=failed exit=1\n")
        (self.staged / "results.txt").unlink()
        publication.publish(self.staged, self.target)
        self.assertEqual(
            (self.target / "status.txt").read_text(), "stage=first_start outcome=failed exit=1\n"
        )


if __name__ == "__main__":
    unittest.main()
