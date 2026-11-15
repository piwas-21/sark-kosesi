"""Checks for scan evidence identity and report binding."""

import hashlib
import importlib.util
import json
import tempfile
import unittest
import urllib.error
from pathlib import Path
from unittest import mock

SPEC = importlib.util.spec_from_file_location(
    "capture_db_image_scan",
    Path(__file__).resolve().parents[2] / "scripts/capture_db_image_scan.py",
)
capture = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(capture)


class CaptureTests(unittest.TestCase):
    def test_registry_http_failure_retains_sanitized_stage_and_endpoint(self):
        revision = "a" * 40
        http_error = urllib.error.HTTPError(
            "https://example.invalid/?secret=do-not-log", 400, "", {}, None
        )
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "capture"
            with (
                mock.patch.dict(
                    capture.os.environ,
                    {"CAPTURE_REVISION": revision, "CAPTURE_OUTPUT": str(output)},
                ),
                mock.patch.object(capture.subprocess, "check_output", return_value=revision),
                mock.patch.object(capture, "verify_staging_image"),
                mock.patch.object(capture.os, "geteuid", return_value=1001),
                mock.patch.object(capture.os, "getegid", return_value=1001),
                mock.patch.object(capture.urllib.request, "urlopen", side_effect=http_error),
                self.assertRaises(capture.RegistryFailure) as failure,
            ):
                capture.main()
            status_raw = (output / "capture-status.json").read_text()
            status = json.loads(status_raw)
            self.assertEqual(status["stage"], "registry_identity")
            self.assertEqual(status["outcome"], "failed")
            self.assertEqual(status["http_status"], 400)
            self.assertEqual(
                status["endpoint"], f"mirror.gcr.io/v2/library/postgres/manifests/{capture.INDEX}"
            )
            self.assertNotIn("do-not-log", status_raw + str(failure.exception))
            self.assertFalse((output / "receipt.json").exists())

    def test_scanner_failure_cannot_produce_a_receipt(self):
        revision = "a" * 40
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "capture"
            output.mkdir()
            (output / "capture-status.json").write_text('{"stage":"checkout"}')
            with (
                mock.patch.dict(
                    capture.os.environ,
                    {"CAPTURE_REVISION": revision, "CAPTURE_OUTPUT": str(output)},
                ),
                mock.patch.object(capture.subprocess, "check_output", return_value=revision),
                mock.patch.object(capture, "verify_staging_image"),
                mock.patch.object(capture, "capture_identity"),
                mock.patch.object(capture.os, "geteuid", return_value=1234),
                mock.patch.object(capture.os, "getegid", return_value=2345),
                mock.patch.object(
                    capture.subprocess, "run", return_value=mock.Mock(returncode=2)
                ) as run,
                self.assertRaisesRegex(RuntimeError, "scanner failed"),
            ):
                capture.main()
            self.assertFalse((output / "receipt.json").exists())
            status = json.loads((output / "capture-status.json").read_text())
            self.assertEqual((status["stage"], status["outcome"]), ("scanner", "failed"))
            command = run.call_args.args[0]
            self.assertEqual(command[command.index("--image-src") + 1], "remote")
            self.assertEqual(command[command.index("--user") + 1], "1234:2345")
            self.assertEqual((output / "scratch").stat().st_mode & 0o777, 0o700)
            self.assertNotIn("/var/run/docker.sock", " ".join(command))
            self.assertIn("--show-suppressed", command)

    def test_capture_rejects_privileged_identity_before_running_commands(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "capture"
            for uid, gid in [(0, 1001), (1001, 0)]:
                with (
                    mock.patch.dict(capture.os.environ, {"CAPTURE_OUTPUT": str(output)}),
                    mock.patch.object(capture.os, "geteuid", return_value=uid),
                    mock.patch.object(capture.os, "getegid", return_value=gid),
                    mock.patch.object(capture.subprocess, "check_output") as commands,
                    self.assertRaisesRegex(ValueError, "unprivileged runner identity"),
                ):
                    capture.main()
                commands.assert_not_called()

    def test_staging_image_must_belong_to_db_service(self):
        image = f"postgres:18.7-trixie@{capture.INDEX}"
        output = mock.Mock(stdout=json.dumps({"services": {"db": {"image": image}}}))
        with mock.patch.object(capture.subprocess, "run", return_value=output) as run:
            capture.verify_staging_image(Path("compose.yml"))
        self.assertIn("--no-interpolate", run.call_args.args[0])
        self.assertTrue(run.call_args.kwargs["check"])
        output.stdout = json.dumps(
            {"services": {"db": {"image": "postgres:wrong"}, "other": {"image": image}}}
        )
        with (
            mock.patch.object(capture.subprocess, "run", return_value=output),
            self.assertRaisesRegex(ValueError, "db image pin changed"),
        ):
            capture.verify_staging_image(Path("compose.yml"))

    def test_receipt_binds_actual_report_and_unfiltered_scope(self):
        report = b'{"Results":[{"Packages":[{"Name":"libc"}]}]}'
        metadata = {
            "Version": 2,
            "UpdatedAt": "2026-11-15T10:00:00Z",
            "NextUpdate": "2026-11-16T10:00:00Z",
        }
        receipt = capture.make_receipt("a" * 40, metadata, report, "2026-11-15T11:00:00Z")
        self.assertEqual(receipt["report_digest"], "sha256:" + hashlib.sha256(report).hexdigest())
        self.assertEqual(receipt["scanner_digest"], capture.SCANNER)
        self.assertTrue(receipt["scope"]["show_suppressed"])
        self.assertTrue(receipt["scope"]["include_unfixed"])
        self.assertFalse(receipt["scope"]["ignore_rules"])
        self.assertEqual(
            receipt["database"],
            {
                "version": 2,
                "updated_at": metadata["UpdatedAt"],
                "next_update": metadata["NextUpdate"],
            },
        )

    def test_identity_rejects_wrong_index_before_manifest(self):
        response = mock.MagicMock()
        response.__enter__.return_value = response
        response.read.return_value = b'{"token":"unused"}'
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            with (
                mock.patch.object(capture.urllib.request, "urlopen", return_value=response),
                mock.patch.object(capture, "get_registry", return_value=b"{}") as registry,
                self.assertRaisesRegex(ValueError, "index digest mismatch"),
            ):
                capture.capture_registry_identity(
                    output, "library/postgres", capture.INDEX, capture.PLATFORM, ""
                )
            self.assertEqual(registry.call_count, 1)
            self.assertFalse(list(output.iterdir()))

    def test_identity_accepts_only_pinned_platform_and_matching_config(self):
        config = json.dumps({"os": "linux", "architecture": "amd64"}).encode()
        manifest = json.dumps({"config": {"digest": capture.sha(config)}}).encode()
        platform = capture.sha(manifest)
        index = json.dumps(
            {
                "manifests": [
                    {"platform": {"architecture": "amd64", "os": "linux"}, "digest": platform}
                ]
            }
        ).encode()
        response = mock.MagicMock()
        response.__enter__.return_value = response
        response.read.return_value = b'{"token":"unused"}'
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            with (
                mock.patch.object(capture.urllib.request, "urlopen", return_value=response),
                mock.patch.object(capture, "get_registry", side_effect=[index, manifest, config]),
            ):
                capture.capture_registry_identity(
                    output, "library/postgres", capture.sha(index), platform, ""
                )
            self.assertEqual((output / "manifest.json").read_bytes(), manifest)
            self.assertEqual((output / "config.json").read_bytes(), config)
