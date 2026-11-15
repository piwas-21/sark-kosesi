"""An initialization-only Unix socket must not release host database consumers."""

import subprocess
import unittest
from unittest.mock import patch

from with_postgres import docker_cluster


class PostgresStartup(unittest.TestCase):
    def test_initialization_server_is_not_host_database_readiness(self):
        tcp_ready = False
        probes = 0

        def probe(command, **_kwargs):
            nonlocal tcp_ready, probes
            probes += 1
            # PostgreSQL's init server answers the default Unix-socket probe.
            if "--host" not in command:
                return subprocess.CompletedProcess(command, 0)
            return subprocess.CompletedProcess(command, 0 if tcp_ready else 1)

        def finish_initialization(_delay):
            nonlocal tcp_ready
            tcp_ready = True

        with (
            patch("with_postgres.run") as docker,
            patch("with_postgres.subprocess.run", side_effect=probe),
            patch("with_postgres.time.sleep", side_effect=finish_initialization),
        ):
            docker.return_value.stdout = "127.0.0.1:54321\n"
            url = docker_cluster("owned-test-only")

        self.assertTrue(tcp_ready, "Consumers were released during Unix-only initialization")
        self.assertEqual(probes, 2)
        self.assertTrue(url.endswith("@127.0.0.1:54321/postgres"))

    def test_docker_startup_passes_the_validated_cluster_marker(self):
        marker = "a" * 64
        with (
            patch("with_postgres.run") as docker,
            patch("with_postgres.subprocess.run", return_value=subprocess.CompletedProcess([], 0)),
        ):
            docker.return_value.stdout = "127.0.0.1:54321\n"
            docker_cluster("owned-test-only", marker)
        launch = docker.call_args_list[0].args
        self.assertIn("sark.disposable_marker=" + marker, launch)
        self.assertEqual(launch[launch.index("sark.disposable_marker=" + marker) - 1], "-c")

    def test_native_startup_passes_the_marker_before_consumers_start(self):
        from pathlib import Path

        from with_postgres import native_cluster

        marker = "b" * 64
        rejected = subprocess.CompletedProcess([], 1, stderr="password authentication failed")
        with (
            patch("with_postgres.run") as command,
            patch("with_postgres.subprocess.run", return_value=rejected),
        ):
            native_cluster(Path("/synthetic-owned"), Path("/synthetic-bin"), marker)
        start = command.call_args_list[1].args
        self.assertIn("sark.disposable_marker=" + marker, start[start.index("-o") + 1])
        self.assertNotIn("fsync=off", start[start.index("-o") + 1])

    def test_native_fsync_fault_is_an_explicit_startup_option(self):
        from pathlib import Path

        from with_postgres import native_cluster

        rejected = subprocess.CompletedProcess([], 1, stderr="password authentication failed")
        with (
            patch("with_postgres.run") as command,
            patch("with_postgres.subprocess.run", return_value=rejected),
        ):
            native_cluster(Path("/synthetic-owned"), Path("/synthetic-bin"), disable_fsync=True)
        start = command.call_args_list[1].args
        self.assertTrue(start[start.index("-o") + 1].endswith(" -c fsync=off"))

    def test_marker_validation_rejects_values_that_could_change_startup_options(self):
        from with_postgres import disposable_marker

        for marker in ("", "a" * 63, "A" * 64, "a" * 64 + " -c other=value"):
            with self.subTest(marker=marker), self.assertRaises(ValueError):
                disposable_marker(marker)
        self.assertEqual(disposable_marker("a" * 64), "a" * 64)

    def test_child_receives_fresh_marker_and_no_inherited_pg_options(self):
        import os
        import sys

        from with_postgres import main

        marker = "c" * 64
        inherited = {
            "P1_POSTGRES_BIN": "/synthetic-bin",
            "P1_POSTGRES_DISPOSABLE_MARKER": "untrusted-old-marker",
            "P1_POSTGRES_EXPECTED_FSYNC": "off",
            "PGOPTIONS": "-c sark.disposable_marker=forged",
        }
        with (
            patch.dict(os.environ, inherited),
            patch.object(sys, "argv", ["with_postgres.py", "--", "synthetic-child"]),
            patch("with_postgres.disposable_marker", return_value=marker),
            patch("with_postgres.native_cluster", return_value="postgresql://fixture") as native,
            patch(
                "with_postgres.subprocess.run", return_value=subprocess.CompletedProcess([], 0)
            ) as child,
        ):
            self.assertEqual(main(), 0)
        self.assertEqual(native.call_args.args[2], marker)
        environment = child.call_args.kwargs["env"]
        self.assertEqual(environment["P1_POSTGRES_DISPOSABLE_MARKER"], marker)
        self.assertEqual(environment["P1_POSTGRES_EXPECTED_FSYNC"], "on")
        self.assertNotIn("PGOPTIONS", environment)
