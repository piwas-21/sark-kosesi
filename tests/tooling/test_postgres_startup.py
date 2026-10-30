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
