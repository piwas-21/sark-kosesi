"""Safety controls for the hosted synthetic application-schema restore adapter."""

import hashlib
import ipaddress
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
    def test_collation_probe_checks_controls_and_records_mismatch(self):
        import copy

        # Independent bytewise oracle, not computed using the implementation's sorter.
        c_order = ["A", "A-1", "A1", "B", "a", "É", "é", "ا", "ب"]
        healthy = {
            "default_order": c_order.copy(),
            "c_order": c_order.copy(),
            "default_case": ["i", "I", "É", "é"],
            "c_case": ["i", "I", "É", "é"],
            "default_equality": [False, False, False, True],
            "c_equality": [False, False, False, True],
        }
        result = lab.compare_collations(healthy, copy.deepcopy(healthy))
        self.assertTrue(result["default_order_matches"])
        self.assertTrue(result["explicit_c_controls_match"])
        self.assertTrue(result["source_default_order_matches_c"])
        changed = copy.deepcopy(healthy)
        changed["default_order"][0], changed["default_order"][1] = (
            changed["default_order"][1],
            changed["default_order"][0],
        )
        changed["default_case"][2:] = ["é", "É"]
        changed["default_equality"][0] = True
        result = lab.compare_collations(healthy, changed)
        for key in (
            "default_order_matches",
            "default_case_matches",
            "default_equality_matches",
            "target_default_order_matches_c",
            "target_default_case_matches_c",
            "target_default_equality_matches_c",
        ):
            self.assertFalse(result[key], key)
        self.assertTrue(result["explicit_c_controls_match"])
        self.assertEqual(result["target"], changed)

        for key, value in (
            ("default_order", []),
            ("default_order", ["A"] * len(c_order)),
            ("c_order", list(reversed(c_order))),
            ("default_case", ["i", "I", "é"]),
            ("default_case", ["i", "I", "x" * 9, "é"]),
            ("default_case", ["i", "I", "x", "é"]),
            ("c_case", ["i", "I", "é", "É"]),
            ("default_equality", [False, False, False, 1]),
            ("default_equality", [False, False, False, False]),
            ("c_equality", [False, False, False, False]),
        ):
            with self.subTest(key=key, value=value):
                bad = copy.deepcopy(healthy)
                bad[key] = value
                with self.assertRaises(ValueError):
                    lab.compare_collations(healthy, bad)
        missing = copy.deepcopy(healthy)
        del missing["default_order"]
        with self.assertRaises(ValueError):
            lab.compare_collations(missing, healthy)
        with self.assertRaises(ValueError):
            lab.compare_collations(None, healthy)

    def test_collation_capture_queries_each_database_and_rejects_missing_output(self):
        expected_order = ["A", "A-1", "A1", "B", "a", "É", "é", "ا", "ب"]
        source = {
            "default_order": expected_order,
            "c_order": expected_order,
            "default_case": ["i", "I", "É", "é"],
            "c_case": ["i", "I", "É", "é"],
            "default_equality": [False, False, False, True],
            "c_equality": [False, False, False, True],
        }
        target = {**source, "default_order": ["a", "A", "A-1", "A1", "B", "É", "é", "ا", "ب"]}
        wrapper = Path("/owned/client")
        envs = [{"PGDATABASE": "source"}, {"PGDATABASE": "target"}]
        with patch.object(
            lab, "psql", side_effect=[json.dumps(source), json.dumps(target)]
        ) as query:
            report = lab.capture_collations(wrapper, envs)
        self.assertEqual(query.call_count, 2)
        self.assertEqual(query.call_args_list[0].args, (wrapper, envs[0], lab.COLLATION_SQL))
        self.assertEqual(query.call_args_list[1].args, (wrapper, envs[1], lab.COLLATION_SQL))
        self.assertFalse(report["default_order_matches"])
        self.assertTrue(report["explicit_c_controls_match"])
        with patch.object(lab, "psql", side_effect=[json.dumps(source), ""]):
            with self.assertRaises(ValueError):
                lab.capture_collations(wrapper, envs)
        with patch.object(lab, "psql") as query:
            with self.assertRaises(ValueError):
                lab.capture_collations(wrapper, [envs[0]])
            query.assert_not_called()

    def test_reviewed_private_network_policy_is_exact_and_tamper_evident(self):
        self.assertEqual(
            {str(network) for network in loopback.private_network_policy()},
            {"10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16"},
        )
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "policy.json"
            for value in (
                b"{}",
                b'{"schema_version":1,"networks":["0.0.0.0/0","172.16.0.0/12","192.168.0.0/16"]}',
                b'{"schema_version":1,"networks":["10.0.0.0/8","10.0.0.0/8","192.168.0.0/16"]}',
                b'{"schema_version":1,"networks":["10.0.0.1/8","172.16.0.0/12","192.168.0.0/16"]}',
            ):
                with self.subTest(value=value):
                    path.write_bytes(value)
                    with patch.object(loopback, "PRIVATE_NETWORK_POLICY", path):
                        with self.assertRaises(loopback.IsolationFailure):
                            loopback.private_network_policy()
                        # Also exercise structural checks independently of the digest pin.
                        with patch.object(
                            loopback,
                            "PRIVATE_NETWORK_POLICY_SHA256",
                            hashlib.sha256(value).hexdigest(),
                        ):
                            with self.assertRaises(loopback.IsolationFailure):
                                loopback.private_network_policy()

    def test_private_ipv4_policy_rejects_public_and_special_ranges(self):
        for value in ("10.23.0.0/16", "172.31.0.0/16", "192.168.1.0/24"):
            with self.subTest(value=value):
                network = ipaddress.ip_network(value)
                for part in (network, network.network_address, network.broadcast_address):
                    self.assertTrue(loopback.suitable_private_ipv4(part))
        for value in (
            "0.0.0.0/0",
            "0.0.0.0/8",
            "127.0.0.0/8",
            "169.254.0.0/16",
            "224.0.0.0/4",
            "240.0.0.0/4",
            "100.64.0.0/10",
            "8.8.8.0/24",
            "::1/128",
        ):
            with self.subTest(value=value):
                network = ipaddress.ip_network(value)
                self.assertFalse(
                    all(
                        loopback.suitable_private_ipv4(part)
                        for part in (network, network.network_address, network.broadcast_address)
                    )
                )
        for value in ("0.0.0.0", "127.0.0.1", "169.254.1.2", "224.0.0.1", "240.0.0.1", "8.8.8.8"):
            with self.subTest(value=value):
                self.assertFalse(loopback.suitable_private_ipv4(ipaddress.ip_address(value)))

    def test_diagnostics_are_fixed_codes_without_exception_details(self):
        with tempfile.TemporaryDirectory() as directory:
            progress = lab.Progress()
            progress.directory = Path(directory)
            progress.enter("isolation_inspection")
            progress.cleanup = "success"
            progress.write(loopback.IsolationFailure("container_ports_shape"))
            report = json.loads((Path(directory) / "app-restore-status.json").read_text())
            self.assertEqual(report["stage"], "isolation_inspection")
            self.assertEqual(report["code"], "container_ports_shape")
            self.assertEqual(report["outcome"], "failed")
            self.assertEqual(report["cleanup"], "success")
            secret = "synthetic-private-password"
            progress.write(RuntimeError(secret))
            text = (Path(directory) / "app-restore-status.json").read_text()
            self.assertNotIn(secret, text)
            self.assertEqual(json.loads(text)["code"], "operation_failed")
            progress.write(loopback.IsolationFailure(secret))
            self.assertNotIn(secret, (Path(directory) / "app-restore-status.json").read_text())

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
        self.assertIn("POSTGRES_INITDB_ARGS=--auth-host=scram-sha-256", args)
        self.assertIn("POSTGRES_HOST_AUTH_METHOD=scram-sha-256", args)
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
        for cidr, first, second in (
            ("10.8.0.0/16", "10.8.0.2", "10.8.0.3"),
            ("192.168.42.0/24", "192.168.42.2", "192.168.42.3"),
        ):
            with self.subTest(cidr=cidr):
                net, records = copy.deepcopy(network), copy.deepcopy(containers)
                net["IPAM"]["Config"][0]["Subnet"] = cidr
                prefix = ipaddress.ip_network(cidr).prefixlen
                for ident, record, address in zip(ids, records, (first, second), strict=True):
                    net["Containers"][ident]["IPv4Address"] = f"{address}/{prefix}"
                    record["NetworkSettings"]["Networks"]["owned"]["IPAddress"] = address
                self.assertEqual(inspect(net, records), [first, second])
        for change in (
            "external",
            "label",
            "member",
            "image",
            "binding",
            "port",
            "ports_shape",
            "address",
            "global_member",
            "subnet",
            "broad_subnet",
            "documentation_subnet",
            "benchmark_subnet",
            "ietf_protocol_subnet",
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
                elif change == "ports_shape":
                    records[0]["NetworkSettings"]["Ports"] = None
                elif change == "address":
                    records[0]["NetworkSettings"]["Networks"]["owned"]["IPAddress"] = "172.31.0.8"
                elif change == "global_member":
                    records[0]["NetworkSettings"]["Networks"]["owned"]["IPAddress"] = "8.8.8.8"
                    net["Containers"][ids[0]]["IPv4Address"] = "8.8.8.8/16"
                elif change == "subnet":
                    net["IPAM"]["Config"][0]["Subnet"] = "8.8.8.0/24"
                elif change == "broad_subnet":
                    net["IPAM"]["Config"][0]["Subnet"] = "0.0.0.0/0"
                elif change == "documentation_subnet":
                    net["IPAM"]["Config"][0]["Subnet"] = "192.0.2.0/24"
                elif change == "benchmark_subnet":
                    net["IPAM"]["Config"][0]["Subnet"] = "198.18.0.0/15"
                elif change == "ietf_protocol_subnet":
                    net["IPAM"]["Config"][0]["Subnet"] = "192.0.0.0/24"
                elif change == "network_id":
                    records[0]["NetworkSettings"]["Networks"]["owned"]["NetworkID"] = "f" * 64
                elif change == "stopped":
                    records[0]["State"]["Running"] = False
                else:
                    records[0]["NetworkSettings"]["Networks"]["extra"] = {}
                with self.assertRaises(ValueError) as failure:
                    inspect(net, records)
                if change == "ports_shape":
                    self.assertEqual(str(failure.exception), "container_ports_shape")
                if change in (
                    "broad_subnet",
                    "documentation_subnet",
                    "benchmark_subnet",
                    "ietf_protocol_subnet",
                ):
                    self.assertEqual(str(failure.exception), "network_subnet_policy")

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
