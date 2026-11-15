"""Safety controls for the hosted synthetic application-schema restore adapter."""

import json
import os
import socket
import socketserver
import subprocess
import tempfile
import threading
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import rehearse_alpine_gosu_restore as lab
import restore_loopback as loopback


class RestoreAdapterTests(unittest.TestCase):
    def test_corrupt_archive_control_requires_pg_restore_diagnostic(self):
        expected = (
            b"pg_restore: error: input file does not appear to be a valid archive (too short?)\n"
        )
        with patch.object(lab.subprocess, "run") as run:
            run.return_value = SimpleNamespace(returncode=1, stderr=expected)
            lab.require_invalid_archive_rejection(["docker", "exec", "pg_restore"], {})
            self.assertEqual(run.call_args.kwargs["input"], b"invalid archive")
            for status, stderr in (
                (0, b""),
                (0, expected),
                (1, b"docker: Cannot connect to the Docker daemon"),
                (1, b"pg_restore: error: connection refused"),
            ):
                with self.subTest(status=status, stderr=stderr):
                    run.return_value = SimpleNamespace(returncode=status, stderr=stderr)
                    with self.assertRaises(ValueError):
                        lab.require_invalid_archive_rejection(["docker", "exec", "pg_restore"], {})

    def test_cleanup_attempts_all_owned_resources_after_timeout(self):
        owned = json.dumps([{"Config": {"Labels": {"sark.restore.owner": "marker"}}}])
        network = json.dumps([{"Labels": {"sark.restore.owner": "marker"}}])
        responses = [
            subprocess.TimeoutExpired("docker", 60),
            SimpleNamespace(returncode=0, stdout=owned, stderr=""),
            SimpleNamespace(returncode=0),
            SimpleNamespace(returncode=0, stdout=network, stderr=""),
            SimpleNamespace(returncode=0),
        ]
        with patch.object(lab.subprocess, "run", side_effect=responses) as run:
            self.assertFalse(lab.cleanup_resources(["source", "target"], "network", "marker"))
        self.assertEqual(run.call_count, 5)
        self.assertEqual(run.call_args_list[-1].args[0], ["docker", "network", "rm", "network"])

    def test_cleanup_handles_ambiguous_creates_and_refuses_foreign_labels(self):
        absent = SimpleNamespace(returncode=1, stderr="Error: No such container: source", stdout="")
        foreign = SimpleNamespace(
            returncode=0,
            stderr="",
            stdout=json.dumps([{"Labels": {"sark.restore.owner": "other"}}]),
        )
        with patch.object(lab.subprocess, "run", side_effect=[absent, foreign]) as run:
            self.assertFalse(lab.cleanup_resources(["source"], "network", "marker"))
        self.assertEqual(run.call_count, 2)
        self.assertEqual(
            run.call_args_list[-1].args[0], ["docker", "network", "inspect", "network"]
        )

    def test_password_control_requires_explicit_postgres_authentication_failure(self):
        with patch.object(lab.subprocess, "run") as run:
            run.return_value = SimpleNamespace(
                returncode=2,
                stderr='psql: error: connection failed: FATAL:  password authentication failed for user "p1_test"\n',
            )
            lab.require_password_rejection(Path("/tmp/owned"), {"PGUSER": "p1_test"}, "secret")
            self.assertEqual(run.call_args.kwargs["env"]["PGPASSWORD"], "secret-invalid")
            for status, stderr in (
                (0, ""),
                (1, "docker: Cannot connect to the Docker daemon"),
                (2, "psql: error: connection refused"),
                (2, 'FATAL:  password authentication failed for user "other"'),
                (0, 'FATAL:  password authentication failed for user "p1_test"'),
            ):
                with self.subTest(status=status, stderr=stderr):
                    run.return_value = SimpleNamespace(returncode=status, stderr=stderr)
                    with self.assertRaises(ValueError):
                        lab.require_password_rejection(Path("/tmp/owned"), {}, "secret")

    def test_context_rejects_non_hosted_wrong_paths_image_and_revision(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            scratch = root / "db-alpine-gosu-scratch"
            evidence = root / "db-alpine-gosu-evidence"
            revision = "a" * 40
            candidate = "sha256:" + "b" * 64
            env = {
                "RUNNER_TEMP": str(root),
                "GITHUB_ACTIONS": "true",
                "RUNNER_ENVIRONMENT": "github-hosted",
            }
            with (
                patch.dict(os.environ, env, clear=True),
                patch.object(lab, "source_state", return_value=(revision, True)),
            ):
                lab.validate_context(candidate, revision, scratch, evidence)
                for image, rev, tmp, out in (
                    (lab.BASE_ID, revision, scratch, evidence),
                    ("latest", revision, scratch, evidence),
                    (candidate, "c" * 40, scratch, evidence),
                    (candidate, revision, root, evidence),
                    (candidate, revision, scratch, root),
                ):
                    with self.subTest(image=image, revision=rev, scratch=tmp, evidence=out):
                        with self.assertRaises(ValueError):
                            lab.validate_context(image, rev, tmp, out)
                with patch.dict(os.environ, {"RUNNER_ENVIRONMENT": "self-hosted"}):
                    with patch.object(lab, "run") as docker:
                        with self.assertRaises(ValueError):
                            lab.main(candidate, revision, scratch, evidence)
                        docker.assert_not_called()
                with patch.object(lab, "source_state", return_value=(revision, False)):
                    with self.assertRaises(ValueError):
                        lab.validate_context(candidate, revision, scratch, evidence)

    def test_container_has_no_published_port_and_password_not_in_arguments(self):
        with patch.object(lab, "run", return_value=SimpleNamespace(stdout="a" * 64)) as run:
            self.assertEqual(
                lab.start_container("image", "isolated", "owned", "marker", "secret"), "a" * 64
            )
        args = run.call_args.args
        self.assertNotIn("--publish", args)
        self.assertNotIn("-p", args)
        self.assertIn("isolated", args)
        self.assertIn("POSTGRES_PASSWORD", args)
        self.assertNotIn("secret", " ".join(args))
        self.assertEqual(run.call_args.kwargs["env"]["POSTGRES_PASSWORD"], "secret")

    def test_inspection_requires_owned_internal_network_and_exact_members(self):
        import copy

        ids = ["a" * 64, "b" * 64]
        images = ["sha256:" + "c" * 64, "sha256:" + "d" * 64]
        net_id = "e" * 64
        network = {
            "Name": "owned",
            "Id": net_id,
            "Driver": "bridge",
            "Internal": True,
            "Labels": {"sark.restore.owner": "marker"},
            "IPAM": {"Config": [{"Subnet": "172.31.0.0/16"}]},
            "Containers": {
                ids[0]: {"IPv4Address": "172.31.0.2/16"},
                ids[1]: {"IPv4Address": "172.31.0.3/16"},
            },
        }
        containers = [
            {
                "Id": ident,
                "Image": image,
                "State": {"Running": True},
                "Config": {"Labels": {"sark.restore.owner": "marker"}},
                "HostConfig": {"PortBindings": {}},
                "NetworkSettings": {
                    "Ports": {"5432/tcp": None},
                    "Networks": {"owned": {"NetworkID": net_id, "IPAddress": address}},
                },
            }
            for ident, image, address in zip(ids, images, ("172.31.0.2", "172.31.0.3"), strict=True)
        ]

        def inspect(net, records):
            output = [SimpleNamespace(stdout=json.dumps([obj])) for obj in [net, *records]]
            with patch.object(loopback, "run", side_effect=output):
                return loopback.verified_addresses("owned", "marker", ids, images)

        self.assertEqual(inspect(network, containers), ["172.31.0.2", "172.31.0.3"])
        for change in (
            "external",
            "label",
            "member",
            "image",
            "binding",
            "port",
            "address",
            "subnet",
            "network_id",
            "extra_network",
            "stopped",
        ):
            with self.subTest(change=change):
                net, records = copy.deepcopy(network), copy.deepcopy(containers)
                if change == "external":
                    net["Internal"] = False
                elif change == "label":
                    net["Labels"]["sark.restore.owner"] = "foreign"
                elif change == "member":
                    net["Containers"].pop(ids[1])
                elif change == "image":
                    records[0]["Image"] = images[1]
                elif change == "binding":
                    records[0]["HostConfig"]["PortBindings"] = {
                        "5432/tcp": [{"HostIp": "127.0.0.1"}]
                    }
                elif change == "port":
                    records[0]["NetworkSettings"]["Ports"]["5432/tcp"] = [{"HostIp": "0.0.0.0"}]
                elif change == "address":
                    records[0]["NetworkSettings"]["Networks"]["owned"]["IPAddress"] = "172.31.0.8"
                elif change == "subnet":
                    net["IPAM"]["Config"][0]["Subnet"] = "8.8.8.0/24"
                elif change == "network_id":
                    records[0]["NetworkSettings"]["Networks"]["owned"]["NetworkID"] = "f" * 64
                elif change == "stopped":
                    records[0]["State"]["Running"] = False
                else:
                    records[0]["NetworkSettings"]["Networks"]["extra"] = {}
                with self.assertRaises(ValueError):
                    inspect(net, records)

    def test_loopback_relay_forwards_bidirectionally_and_closes(self):
        class Echo(socketserver.BaseRequestHandler):
            def handle(self):
                while data := self.request.recv(100):
                    self.request.sendall(data[::-1])

        with socketserver.TCPServer(("127.0.0.1", 0), Echo) as upstream:
            thread = threading.Thread(target=upstream.serve_forever, daemon=True)
            thread.start()
            relay = loopback.RelayServer("127.0.0.1", upstream.server_address[1])
            address = relay.server_address
            client = None
            try:
                self.assertEqual(address[0], "127.0.0.1")
                client = socket.create_connection(address, timeout=3)
                for value in (b"fixture", b"second"):
                    client.sendall(value)
                    self.assertEqual(client.recv(100), value[::-1])
            finally:
                relay.close()
                if client is not None:
                    client.close()
                upstream.shutdown()
                thread.join(timeout=3)
            with self.assertRaises(OSError):
                socket.create_connection(address, timeout=1)

    def test_relay_forwards_response_after_client_half_close(self):
        class RespondAfterEof(socketserver.BaseRequestHandler):
            def handle(self):
                received = bytearray()
                while data := self.request.recv(1024):
                    received.extend(data)
                self.request.sendall(bytes(received).upper())

        with socketserver.TCPServer(("127.0.0.1", 0), RespondAfterEof) as upstream:
            thread = threading.Thread(target=upstream.serve_forever, daemon=True)
            thread.start()
            relay = loopback.RelayServer("127.0.0.1", upstream.server_address[1])
            try:
                with socket.create_connection(relay.server_address, timeout=3) as client:
                    client.sendall(b"half-close")
                    client.shutdown(socket.SHUT_WR)
                    self.assertEqual(client.recv(1024), b"HALF-CLOSE")
                    self.assertEqual(client.recv(1024), b"")
            finally:
                relay.close()
                upstream.shutdown()
                thread.join(timeout=3)

    def test_client_rejects_unowned_ports_and_non_loopback(self):
        owned = "a" * 64
        env = {
            "RESTORE_OWNED_PORTS": json.dumps({"1234": owned}),
            "PGHOST": "127.0.0.1",
            "PGPORT": "1234",
            "PGUSER": "p1_test",
        }
        with (
            patch.dict(os.environ, env, clear=True),
            patch.object(lab.os, "execvp") as execute,
            patch.object(lab.sys, "argv", ["psql", "-XAtq", "-v", "ON_ERROR_STOP=1"]),
        ):
            lab.client()
            self.assertIn(owned, execute.call_args.args[1])
            self.assertEqual(execute.call_args.args[1][-3:], ["-XAtq", "-v", "ON_ERROR_STOP=1"])
            with patch.dict(os.environ, {"PGPORT": "9999"}):
                with self.assertRaises(ValueError):
                    lab.client()
            with patch.dict(os.environ, {"PGHOST": "example.test"}):
                with self.assertRaises(ValueError):
                    lab.client()


if __name__ == "__main__":
    unittest.main()
