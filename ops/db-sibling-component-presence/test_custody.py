#!/usr/bin/env python3
"""Reject missing, modified, duplicate and unlisted scan evidence."""

import hashlib
import importlib.util
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location("custody", Path(__file__).with_name("custody.py"))
custody = importlib.util.module_from_spec(spec)
spec.loader.exec_module(custody)


class CustodyControls(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        (self.root / "status.txt").write_text("stage=complete outcome=success\n")
        (self.root / "identity.txt").write_text("image=synthetic\n")
        self.required = {"status.txt", "identity.txt"}
        self.manifest()

    def manifest(self):
        lines = []
        for item in sorted(self.root.iterdir()):
            if item.name != "SHA256SUMS":
                lines.append(f"{hashlib.sha256(item.read_bytes()).hexdigest()}  ./{item.name}\n")
        (self.root / "SHA256SUMS").write_text("".join(lines))

    def test_complete_inventory(self):
        custody.verify(self.root, self.required)

    def test_missing_unlisted_tampered_duplicate_and_failed(self):
        manifest = self.root / "SHA256SUMS"
        original = manifest.read_text()
        for data in ("", original + original.splitlines()[0] + "\n"):
            manifest.write_text(data)
            with self.assertRaises(ValueError):
                custody.verify(self.root, self.required)
        manifest.write_text(original)
        (self.root / "identity.txt").write_text("tampered\n")
        with self.assertRaises(ValueError):
            custody.verify(self.root, self.required)
        (self.root / "identity.txt").write_text("image=synthetic\n")
        (self.root / "extra").write_text("unlisted\n")
        with self.assertRaises(ValueError):
            custody.verify(self.root, self.required)
        (self.root / "extra").unlink()
        (self.root / "status.txt").write_text("stage=scan outcome=failed exit=1\n")
        self.manifest()
        with self.assertRaises(ValueError):
            custody.verify(self.root, self.required)


if __name__ == "__main__":
    unittest.main()
