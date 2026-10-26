import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scan_secrets import export_sources


class SecretSourceInventory(unittest.TestCase):
    def setUp(self):
        self.environment = {
            key: value for key, value in os.environ.items() if not key.startswith("GIT_")
        }
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "repo"
        self.root.mkdir()
        self.output = Path(self.temp.name) / "export"
        subprocess.run(["git", "init", "--quiet", str(self.root)], check=True, env=self.environment)
        (self.root / ".gitignore").write_text("build/\n")
        (self.root / "app.rs").write_text("owned source")
        subprocess.run(
            ["git", "add", ".gitignore", "app.rs"], cwd=self.root, check=True, env=self.environment
        )

    def test_tracked_and_new_source_scanned_ignored_runtime_excluded(self):
        (self.root / "new.ts").write_text("new source")
        (self.root / "build").mkdir()
        (self.root / "build/preview-key.json").write_text("generated runtime data")
        self.assertEqual(export_sources(self.root, self.output), 3)
        self.assertEqual((self.output / "app.rs").read_text(), "owned source")
        self.assertTrue((self.output / "new.ts").is_file())
        self.assertFalse((self.output / "build").exists())
        subprocess.run(
            ["git", "add", "--force", "build/preview-key.json"],
            cwd=self.root,
            check=True,
            env=self.environment,
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
            ["git", "rm", "--cached", "-r", "."],
            cwd=self.root,
            check=True,
            capture_output=True,
            env=self.environment,
        )
        (self.root / "app.rs").unlink()
        (self.root / ".gitignore").unlink()
        with self.assertRaises(ValueError):
            export_sources(self.root, self.output)

    def test_inherited_hook_environment_cannot_redirect_source_inventory(self):
        foreign = Path(self.temp.name) / "foreign"
        foreign.mkdir()
        subprocess.run(["git", "init", "--quiet", str(foreign)], check=True, env=self.environment)
        with patch.dict(
            os.environ,
            {
                "GIT_DIR": str(foreign / ".git"),
                "GIT_WORK_TREE": str(foreign),
                "GIT_INDEX_FILE": str(foreign / ".git/index"),
            },
        ):
            self.assertEqual(export_sources(self.root, self.output), 2)
        self.assertTrue((self.output / "app.rs").is_file())
        self.assertEqual(
            subprocess.check_output(
                ["git", "ls-files"], cwd=foreign, env=self.environment, text=True
            ),
            "",
        )
