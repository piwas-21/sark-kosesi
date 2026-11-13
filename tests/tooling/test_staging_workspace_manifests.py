import json
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def missing_manifests(source, workspaces):
    stage = source.split(" AS web-build\n", 1)[1].split("\nFROM ", 1)[0]
    before_install, install, _ = stage.partition("RUN npm ci --ignore-scripts\n")
    if not install:
        return list(workspaces)
    lines = before_install.splitlines()
    return [
        path
        for path in workspaces
        if f"COPY {path}/package.json ./{path}/package.json" not in lines
    ]


class StagingWorkspaceManifests(unittest.TestCase):
    def test_locked_workspaces_are_copied_before_install(self):
        manifest = json.loads((ROOT / "package.json").read_text())
        lock = json.loads((ROOT / "package-lock.json").read_text())
        workspaces = manifest["workspaces"]
        self.assertEqual(workspaces, lock["packages"][""]["workspaces"])
        for path in workspaces:
            self.assertIn(path, lock["packages"])
            self.assertTrue((ROOT / path / "package.json").is_file())

        source = (ROOT / "deploy/staging/Dockerfile").read_text()
        self.assertEqual(missing_manifests(source, workspaces), [])
        self.assertEqual(
            missing_manifests(source.replace("RUN npm ci --ignore-scripts\n", ""), workspaces),
            workspaces,
        )
        for path in workspaces:
            copy = f"COPY {path}/package.json ./{path}/package.json\n"
            with self.subTest(workspace=path):
                omitted = source.replace(copy, "")
                self.assertEqual(missing_manifests(omitted, workspaces), [path])
                too_late = omitted.replace(
                    "RUN npm ci --ignore-scripts\n", f"RUN npm ci --ignore-scripts\n{copy}"
                )
                self.assertEqual(missing_manifests(too_late, workspaces), [path])
