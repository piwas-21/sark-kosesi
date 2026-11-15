"""Safety controls for the hosted synthetic application-schema restore adapter."""

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import rehearse_alpine_gosu_restore as lab


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

    def test_container_uses_private_binding_and_does_not_put_password_in_arguments(self):
        with patch.object(lab, "run", return_value=SimpleNamespace(stdout="a" * 64)) as run:
            self.assertEqual(
                lab.start_container("image", "isolated", "owned", "marker", "secret"), "a" * 64
            )
        args = run.call_args.args
        self.assertIn("127.0.0.1::5432", args)
        self.assertIn("POSTGRES_PASSWORD", args)
        self.assertNotIn("secret", " ".join(args))
        self.assertEqual(run.call_args.kwargs["env"]["POSTGRES_PASSWORD"], "secret")

    def test_inspection_rejects_wrong_image_or_non_loopback(self):
        image = "sha256:" + "b" * 64
        data = [
            {
                "Image": image,
                "NetworkSettings": {
                    "Ports": {"5432/tcp": [{"HostIp": "127.0.0.1", "HostPort": "12345"}]}
                },
            }
        ]
        with patch.object(lab, "run", return_value=SimpleNamespace(stdout=json.dumps(data))):
            self.assertEqual(lab.inspect_port("a" * 64, image), "12345")
            with self.assertRaises(ValueError):
                lab.inspect_port("a" * 64, lab.BASE_ID)
        data[0]["NetworkSettings"]["Ports"]["5432/tcp"][0]["HostIp"] = "0.0.0.0"
        with patch.object(lab, "run", return_value=SimpleNamespace(stdout=json.dumps(data))):
            with self.assertRaises(ValueError):
                lab.inspect_port("a" * 64, image)

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
