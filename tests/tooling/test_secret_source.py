import subprocess
import tempfile
import unittest
from pathlib import Path

from scan_secrets import export_sources


class SecretSourceInventory(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "repo"
        self.root.mkdir()
        self.output = Path(self.temp.name) / "export"
        subprocess.run(["git", "init", "--quiet", str(self.root)], check=True)
        (self.root / ".gitignore").write_text("build/\n")
        (self.root / "app.rs").write_text("owned source")
        subprocess.run(["git", "add", ".gitignore", "app.rs"], cwd=self.root, check=True)

    def test_tracked_and_new_source_scanned_ignored_runtime_excluded(self):
        (self.root / "new.ts").write_text("new source")
        (self.root / "build").mkdir()
        (self.root / "build/preview-key.json").write_text("generated runtime data")
        self.assertEqual(export_sources(self.root, self.output), 3)
        self.assertEqual((self.output / "app.rs").read_text(), "owned source")
        self.assertTrue((self.output / "new.ts").is_file())
        self.assertFalse((self.output / "build").exists())
        subprocess.run(
            ["git", "add", "--force", "build/preview-key.json"], cwd=self.root, check=True
        )
        export_sources(self.root, self.output)
        self.assertTrue((self.output / "build/preview-key.json").is_file())

    def test_missing_or_symlink_source_cannot_silently_disappear(self):
        (self.root / "app.rs").unlink()
        with self.assertRaises(ValueError):
            export_sources(self.root, self.output)
        (self.root / "app.rs").symlink_to(self.root / ".gitignore")
        with self.assertRaises(ValueError):
            export_sources(self.root, self.output)

    def test_empty_inventory_is_not_a_scan(self):
        subprocess.run(
            ["git", "rm", "--cached", "-r", "."], cwd=self.root, check=True, capture_output=True
        )
        (self.root / "app.rs").unlink()
        (self.root / ".gitignore").unlink()
        with self.assertRaises(ValueError):
            export_sources(self.root, self.output)
