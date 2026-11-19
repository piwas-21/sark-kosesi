#!/usr/bin/env python3
"""Independent positive and negative controls for inspection validation."""

import copy
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

from validate import validate


class InspectionTests(unittest.TestCase):
    def setUp(self):
        self.container = {
            "Image": "sha256:image",
            "Config": {
                "User": "999:999",
                "Entrypoint": ["/opt/pg-smoke/bin/postgres"],
                "Cmd": [
                    "-D",
                    "/var/lib/postgresql/18/docker",
                    "-c",
                    "listen_addresses=127.0.0.1",
                    "-c",
                    "unix_socket_directories=/tmp",
                    "-c",
                    "shared_buffers=16MB",
                ],
                "StopSignal": "SIGINT",
                "Env": ["PGDATA=/var/lib/postgresql/18/docker"],
            },
            "HostConfig": {
                "NetworkMode": "none",
                "ReadonlyRootfs": True,
                "CapDrop": ["ALL"],
                "SecurityOpt": ["no-new-privileges"],
                "Tmpfs": {"/tmp": "rw"},
            },
            "Mounts": [
                {
                    "Type": "volume",
                    "Name": "owned",
                    "Destination": "/var/lib/postgresql",
                    "RW": True,
                }
            ],
            "State": {"Running": True, "OOMKilled": False, "ExitCode": 0},
        }

    def test_expected_running_and_clean_exit(self):
        validate(self.container, "sha256:image", "owned", True)
        self.container["State"]["Running"] = False
        validate(self.container, "sha256:image", "owned", False)

    def test_rejects_unsafe_or_incorrect_inspection(self):
        for section, key, value in [
            ("Config", "User", "0:0"),
            ("Config", "StopSignal", "SIGTERM"),
            ("HostConfig", "NetworkMode", "bridge"),
            ("HostConfig", "ReadonlyRootfs", False),
            ("HostConfig", "CapDrop", []),
            ("State", "OOMKilled", True),
        ]:
            candidate = copy.deepcopy(self.container)
            candidate[section][key] = value
            with self.subTest(section=section, key=key), self.assertRaises(ValueError):
                validate(candidate, "sha256:image", "owned", True)
        with self.assertRaises(ValueError):
            validate(self.container, "sha256:image", "other-volume", True)
        self.container["State"].update(Running=False, ExitCode=137)
        with self.assertRaises(ValueError):
            validate(self.container, "sha256:image", "owned", False)


class RunnerSafetyTests(unittest.TestCase):
    def test_non_hosted_fails_before_creating_evidence(self):
        with tempfile.TemporaryDirectory() as scratch:
            env = {
                **os.environ,
                "GITHUB_ACTIONS": "false",
                "RUNNER_ENVIRONMENT": "self-hosted",
                "RUNNER_TEMP": scratch,
                "LIFECYCLE_REVISION": "0" * 40,
            }
            result = subprocess.run(
                ["bash", str(Path(__file__).with_name("run.sh"))],
                env=env,
                capture_output=True,
                check=False,
                timeout=5,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(b"hosted runner required", result.stderr)
            self.assertEqual(list(Path(scratch).iterdir()), [])


if __name__ == "__main__":
    unittest.main()
