"""Offline controls; synthetic reports are not image or scanner attestations."""

import copy
import json
import subprocess
import sys
import tempfile
import unittest
from datetime import UTC, datetime, timedelta
from pathlib import Path

from inspect_image_scan import SCOPE, InvalidEvidence, digest, inspect, read_document

NOW = datetime(2026, 11, 15, 12, tzinfo=UTC)
REVISION = "a" * 40
SCANNER = "sha256:" + "b" * 64


def encode(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def fixture():
    config = {
        "os": "linux",
        "architecture": "amd64",
        "rootfs": {"type": "layers", "diff_ids": ["sha256:" + "d" * 64]},
    }
    config_raw = encode(config)
    manifest = {
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {"digest": digest(config_raw), "size": len(config_raw)},
        "layers": [{"digest": "sha256:" + "e" * 64, "size": 123}],
    }
    manifest_raw = encode(manifest)
    report = {
        "SchemaVersion": 2,
        "ArtifactType": "container_image",
        "CreatedAt": "2026-11-15T10:59:59.123456789Z",
        "Metadata": {
            "ImageID": digest(config_raw),
            "DiffIDs": config["rootfs"]["diff_ids"].copy(),
            "ImageConfig": {"os": "linux", "architecture": "amd64"},
            "OS": {"Family": "debian", "Name": "13.7"},
        },
        "Results": [
            {
                "Target": "debian (debian 13.7)",
                "Class": "os-pkgs",
                "Type": "debian",
                "Packages": [{"Name": "libexample", "Version": "1.0"}],
                "Vulnerabilities": [
                    {
                        "VulnerabilityID": "CVE-2026-0001",
                        "PkgName": "libexample",
                        "InstalledVersion": "1.0",
                        "Severity": "LOW",
                    }
                ],
            },
            {
                "Target": "/usr/bin/example",
                "Class": "lang-pkgs",
                "Type": "gobinary",
                "Packages": [{"Name": "stdlib", "Version": "v1.26.0"}],
            },
        ],
    }
    report_raw = encode(report)
    receipt = {
        "schema_version": 1,
        "source_revision": REVISION,
        "manifest_digest": digest(manifest_raw),
        "scanner_digest": SCANNER,
        "report_digest": digest(report_raw),
        "scanned_at": "2026-11-15T11:00:00Z",
        "exit_code": 0,
        "database": {
            "version": 2,
            "updated_at": "2026-11-15T10:00:00Z",
            "next_update": "2026-11-16T10:00:00Z",
        },
        "scope": copy.deepcopy(SCOPE),
    }
    return receipt, manifest, config, report


def evaluate(data, *, revision=REVISION, scanner=SCANNER, platform="linux/amd64", now=NOW):
    receipt, manifest, config, report = data
    return inspect(
        receipt,
        encode(manifest),
        manifest,
        encode(config),
        config,
        encode(report),
        report,
        revision=revision,
        image_digest=receipt["manifest_digest"],
        scanner_digest=scanner,
        platform=platform,
        now=now,
    )


def seal(data):
    receipt, manifest, config, report = data
    manifest["config"]["digest"] = digest(encode(config))
    manifest["config"]["size"] = len(encode(config))
    receipt["manifest_digest"] = digest(encode(manifest))
    receipt["report_digest"] = digest(encode(report))
    return data


class ImageScanEvidence(unittest.TestCase):
    def test_populated_clean_control_and_high_critical_block(self):
        data = fixture()
        result = evaluate(data)
        self.assertEqual(result["status"], "no_reported_high_or_critical")
        self.assertEqual(result["package_count"], 2)
        self.assertEqual(result["severity_counts"]["LOW"], 1)
        for severity in ("HIGH", "CRITICAL"):
            changed = fixture()
            changed[3]["Results"][0]["Vulnerabilities"][0]["Severity"] = severity
            result = evaluate(seal(changed))
            self.assertEqual(result["status"], "blocked")
            self.assertEqual(result["severity_counts"][severity], 1)

    def test_empty_damaged_or_filtered_report_is_not_clean(self):
        variants = []
        for change in (
            lambda r: r.update(Results=[]),
            lambda r: r["Results"][0].update(Packages=[]),
            lambda r: r["Results"][0].pop("Packages"),
            lambda r: r["Results"].pop(0),
            lambda r: r["Results"][0].update(ExperimentalModifiedFindings=[{"reason": "ignored"}]),
            lambda r: r["Results"][0].update(ExperimentalModifiedFindings={}),
            lambda r: r["Results"][0].update(Vulnerabilities=None),
            lambda r: r["Results"][0]["Vulnerabilities"][0].update(Severity="ignored"),
            lambda r: r.update(ArtifactType="filesystem"),
        ):
            data = fixture()
            change(data[3])
            variants.append(seal(data))
        for data in variants:
            with self.subTest(report=data[3]):
                with self.assertRaises(InvalidEvidence):
                    evaluate(data)

    def test_scope_bypasses_and_lookalike_boolean_are_rejected(self):
        for key, value in (
            ("include_unfixed", False),
            ("include_all_packages", False),
            ("show_suppressed", False),
            ("show_suppressed", 1),
            ("ignore_rules", True),
            ("vex", True),
            ("include_unfixed", 1),
            ("severities", ["LOW"]),
            ("package_types", ["os"]),
        ):
            data = fixture()
            data[0]["scope"][key] = value
            with self.subTest(key=key, value=value), self.assertRaises(InvalidEvidence):
                evaluate(data)
        data = fixture()
        del data[0]["scope"]["show_suppressed"]
        with self.assertRaises(InvalidEvidence):
            evaluate(data)

    def test_pinned_trivy_debian_result_field_compatibility(self):
        # Upstream v0.70.0 debian-buster.json.golden has these raw target and
        # finding fields, but intentionally lacks Packages: TestTar runs with
        # --list-all-pkgs=false. This is a field control, not a complete scan.
        data = fixture()
        data[3]["Metadata"]["OS"]["Name"] = "10.1"
        data[3]["Results"] = [
            {
                "Target": "testdata/fixtures/images/debian-buster.tar.gz (debian 10.1)",
                "Class": "os-pkgs",
                "Type": "debian",
                "Packages": [{"Name": "bash", "Version": "5.0-4"}],
                "Vulnerabilities": [
                    {
                        "VulnerabilityID": "CVE-2019-18276",
                        "PkgName": "bash",
                        "InstalledVersion": "5.0-4",
                        "Severity": "LOW",
                    }
                ],
            }
        ]
        result = evaluate(seal(data))
        self.assertEqual(result["status"], "no_reported_high_or_critical")
        self.assertEqual(result["severity_counts"]["LOW"], 1)
        # The pinned busybox-with-lockfile.json.golden supplies this separate
        # language target and finding; its package entry is synthetic as above.
        data[3]["Results"].append(
            {
                "Target": "Cargo.lock",
                "Class": "lang-pkgs",
                "Type": "cargo",
                "Packages": [{"Name": "ammonia", "Version": "1.9.0"}],
                "Vulnerabilities": [
                    {
                        "VulnerabilityID": "CVE-2019-15542",
                        "PkgName": "ammonia",
                        "InstalledVersion": "1.9.0",
                        "Severity": "HIGH",
                    }
                ],
            }
        )
        result = evaluate(seal(data))
        self.assertEqual(result["status"], "blocked")
        self.assertEqual(result["severity_counts"]["HIGH"], 1)

    def test_time_revision_digest_platform_and_scanner_are_bound(self):
        for mutate in (
            lambda d: d[0].update(source_revision="f" * 40),
            lambda d: d[0].update(scanner_digest="sha256:" + "f" * 64),
            lambda d: d[0].update(report_digest="sha256:" + "f" * 64),
            lambda d: d[0].update(manifest_digest="sha256:" + "f" * 64),
            lambda d: d[0]["database"].update(next_update="2026-11-15T11:59:59Z"),
            lambda d: d[0]["database"].update(updated_at="2026-11-15T11:30:00Z"),
            lambda d: d[0].update(scanned_at="2026-11-15T12:00:01Z"),
            lambda d: d[0].update(exit_code=False),
            lambda d: d[0].update(exit_code=1),
        ):
            data = fixture()
            mutate(data)
            with self.subTest(receipt=data[0]), self.assertRaises(InvalidEvidence):
                evaluate(data)
        with self.assertRaises(InvalidEvidence):
            evaluate(fixture(), revision="f" * 40)
        with self.assertRaises(InvalidEvidence):
            evaluate(fixture(), scanner="sha256:" + "f" * 64)
        with self.assertRaises(InvalidEvidence):
            evaluate(fixture(), platform="linux/arm64")

    def test_resealed_deep_identity_and_timestamp_failures(self):
        for mutate in (
            lambda d: d[1].update(mediaType="application/vnd.oci.image.index.v1+json"),
            lambda d: d[3]["Metadata"].update(ImageID="sha256:" + "f" * 64),
            lambda d: d[3]["Metadata"].update(DiffIDs=["sha256:" + "f" * 64]),
            lambda d: d[3]["Metadata"]["ImageConfig"].update(architecture="arm64"),
            lambda d: d[3].update(CreatedAt="2026-11-15T09:59:59Z"),
            lambda d: d[3].update(CreatedAt="2026-11-15T11:00:01Z"),
        ):
            data = fixture()
            mutate(data)
            with self.subTest(mutate=mutate), self.assertRaises(InvalidEvidence):
                evaluate(seal(data))
        data = fixture()
        data[2]["architecture"] = "arm64"
        data[3]["Metadata"]["ImageID"] = digest(encode(data[2]))
        data[3]["Metadata"]["ImageConfig"]["architecture"] = "arm64"
        with self.assertRaises(InvalidEvidence):
            evaluate(seal(data))

    def test_cli_result_and_failure_are_redacted(self):
        data = fixture()
        now = datetime.now(UTC)

        def fmt(value):
            return value.isoformat().replace("+00:00", "Z")

        data[0]["database"]["updated_at"] = fmt(now - timedelta(hours=1))
        data[0]["database"]["next_update"] = fmt(now + timedelta(hours=1))
        data[0]["scanned_at"] = fmt(now - timedelta(seconds=1))
        data[3]["CreatedAt"] = fmt(now - timedelta(seconds=2))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            script = Path(__file__).resolve().parents[2] / "scripts" / "inspect_image_scan.py"
            paths = [root / name for name in ("receipt", "manifest", "config", "report")]

            def run():
                seal(data)
                for path, value in zip(paths, data, strict=True):
                    path.write_bytes(encode(value))
                return subprocess.run(
                    [
                        sys.executable,
                        str(script),
                        "--receipt",
                        str(paths[0]),
                        "--manifest",
                        str(paths[1]),
                        "--image-config",
                        str(paths[2]),
                        "--report",
                        str(paths[3]),
                        "--revision",
                        REVISION,
                        "--image-digest",
                        data[0]["manifest_digest"],
                        "--scanner-digest",
                        SCANNER,
                        "--platform",
                        "linux/amd64",
                    ],
                    capture_output=True,
                    text=True,
                    check=False,
                )

            result = run()
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(result.stdout)["status"], "no_reported_high_or_critical")
            data[3]["Results"][0]["Vulnerabilities"][0]["Severity"] = "HIGH"
            result = run()
            self.assertEqual(result.returncode, 1)
            self.assertEqual(json.loads(result.stdout)["status"], "blocked")
            self.assertNotIn(directory, result.stdout + result.stderr)
            data[3]["Results"][0]["Vulnerabilities"][0]["Severity"] = "secret-invalid"
            result = run()
            self.assertEqual(result.returncode, 1)
            self.assertNotIn("secret-invalid", result.stdout + result.stderr)
            self.assertNotIn(directory, result.stdout + result.stderr)

    def test_duplicate_json_and_symlink_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            file = root / "report.json"
            file.write_text('{"Results":[],"Results":[{}]}')
            with self.assertRaises(InvalidEvidence):
                read_document(file)
            file.write_text('{"ok":true}')
            self.assertEqual(read_document(file)[1], {"ok": True})
            link = root / "link.json"
            link.symlink_to(file)
            with self.assertRaises(InvalidEvidence):
                read_document(link)


if __name__ == "__main__":
    unittest.main()
