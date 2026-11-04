"""Isolated prototype controls; requires a pristine local npm install as input."""

import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "patch_minimatch_10", ROOT / "scripts/patch_minimatch_10.py"
)
PATCH = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PATCH)

PROBE = r"""
const loaded = await import(process.argv[1]);
const M = loaded.Minimatch ?? loaded.default.Minimatch;
const pattern = Array(8).fill('**/../a/b').join('/');
const limited = new M(pattern, {optimizationLevel: 2, braceExpandMax: 64});
const ordinary = new M('**/../a/b/**/../c/d', {optimizationLevel: 2});
const large = new M('{x,*}/{0..999}', {optimizationLevel: 2});
const js = new M('src/**/*.ts');
console.log(JSON.stringify({
  limited: limited.set.length, ordinary: ordinary.set.length, large: large.set.length,
  matches: [ordinary.match('../a/b/../c/d'), ordinary.match('x/a/b/y/c/d'),
    large.match('x/5'), large.match('y/999'), large.match('.y/5'),
    js.match('src/app/index.ts'), js.match('src/app/index.js')]
}));
"""


class MinimatchBackport(unittest.TestCase):
    def setUp(self):
        source = Path(os.environ["SARK_MINIMATCH_SOURCE_DIR"])
        self.node = os.environ["SARK_NODE"]
        self.assertEqual(
            subprocess.check_output([self.node, "--version"], text=True).strip(), "v24.22.1"
        )
        self.temp = tempfile.TemporaryDirectory(prefix="sark-minimatch-control-")
        self.addCleanup(self.temp.cleanup)
        self.project = Path(self.temp.name)
        modules = self.project / "node_modules"
        modules.mkdir()
        for name in ("minimatch", "brace-expansion", "balanced-match"):
            shutil.copytree(source / name, modules / name)
        self.package = modules / "minimatch"

    def probe(self, mode):
        path = self.package / "dist" / mode / "index.js"
        result = subprocess.check_output(
            [self.node, "--input-type=module", "-e", PROBE, path.as_uri()],
            text=True,
            timeout=15,
        )
        return json.loads(result)

    def test_exploit_control_and_healthy_matching_for_both_module_formats(self):
        expected_matches = [True, True, True, True, False, True, False]
        for mode in ("commonjs", "esm"):
            self.assertEqual(
                self.probe(mode),
                {
                    "limited": 256,
                    "ordinary": 4,
                    "large": 1000,
                    "matches": expected_matches,
                },
            )
        with self.assertRaisesRegex(ValueError, "not installed"):
            PATCH.check(self.project)
        PATCH.check(self.project, apply=True)
        PATCH.check(self.project)
        PATCH.check(self.project, apply=True)
        for mode in ("commonjs", "esm"):
            self.assertEqual(
                self.probe(mode),
                {
                    "limited": 64,
                    "ordinary": 4,
                    "large": 2000,
                    "matches": expected_matches,
                },
            )

    def test_tamper_missing_and_mixed_state_rejected_before_writes(self):
        paths = [self.package / name for name in PATCH.FILES]
        clean = [path.read_bytes() for path in paths]
        paths[1].write_bytes(clean[1] + b"tamper")
        with self.assertRaisesRegex(ValueError, "Unknown minimatch source"):
            PATCH.check(self.project, apply=True)
        self.assertEqual(paths[0].read_bytes(), clean[0])
        paths[1].unlink()
        with self.assertRaisesRegex(ValueError, "Missing package file"):
            PATCH.check(self.project, apply=True)
        paths[1].write_bytes(clean[1])
        PATCH.check(self.project, apply=True)
        paths[1].write_bytes(clean[1])
        with self.assertRaisesRegex(ValueError, "Partial patch"):
            PATCH.check(self.project, apply=True)

    def test_refuses_symlinked_package_and_tampered_metadata(self):
        metadata = self.package / "package.json"
        metadata.write_bytes(metadata.read_bytes() + b" ")
        with self.assertRaisesRegex(ValueError, "metadata"):
            PATCH.check(self.project, apply=True)
        shutil.rmtree(self.project / "node_modules")
        (self.project / "node_modules").symlink_to(Path(os.environ["SARK_MINIMATCH_SOURCE_DIR"]))
        with self.assertRaisesRegex(ValueError, "Symlink"):
            PATCH.check(self.project, apply=True)

    def test_interrupted_application_is_detected_and_requires_reinstall(self):
        actual_replace = os.replace
        calls = 0

        def fail_second(source, destination):
            nonlocal calls
            calls += 1
            if calls == 2:
                raise OSError("simulated interruption")
            actual_replace(source, destination)

        with patch.object(PATCH.os, "replace", side_effect=fail_second):
            with self.assertRaisesRegex(OSError, "simulated interruption"):
                PATCH.check(self.project, apply=True)
        for apply in (False, True):
            with self.assertRaisesRegex(ValueError, "Partial patch"):
                PATCH.check(self.project, apply=apply)

    def test_cli_requires_apply_before_verification(self):
        command = [
            sys.executable,
            str(ROOT / "scripts/patch_minimatch_10.py"),
            "--project",
            str(self.project),
        ]
        self.assertNotEqual(subprocess.run(command, capture_output=True).returncode, 0)
        self.assertEqual(subprocess.run([*command, "--apply"], capture_output=True).returncode, 0)
        self.assertEqual(subprocess.run(command, capture_output=True).returncode, 0)


if __name__ == "__main__":
    unittest.main()
