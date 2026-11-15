"""Positive and negative controls for the zlib scan comparison."""

import copy
import importlib.util
import subprocess
import tempfile
import unittest
from datetime import UTC, datetime, timedelta
from pathlib import Path

SPEC = importlib.util.spec_from_file_location(
    "compare_db_zlib_scan", Path(__file__).resolve().parents[2] / "scripts/compare_db_zlib_scan.py"
)
compare = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(compare)


class CompareZlibScanTests(unittest.TestCase):
    def test_non_hosted_execution_stops_before_creating_scratch(self):
        script = Path(__file__).resolve().parents[2] / "scripts/test_db_zlib_candidate.sh"
        with tempfile.TemporaryDirectory() as directory:
            env = {
                "PATH": "/usr/bin:/bin",
                "RUNNER_TEMP": directory,
                "RUNNER_ENVIRONMENT": "self-hosted",
                "GITHUB_ACTIONS": "true",
                "ZLIB_SOURCE_REVISION": "a" * 40,
                "ZLIB_EVIDENCE_DIR": f"{directory}/db-zlib-evidence",
                "ZLIB_SCRATCH_DIR": f"{directory}/db-zlib-scratch",
                "DOCKER_HOST": "tcp://example.invalid:2375",
            }
            result = subprocess.run(
                ["bash", str(script)], env=env, capture_output=True, check=False
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(list(Path(directory).iterdir()))

    def setUp(self):
        now = datetime.now(UTC)
        self.database = {
            "Version": 2,
            "UpdatedAt": (now - timedelta(hours=1)).isoformat(),
            "NextUpdate": (now + timedelta(hours=1)).isoformat(),
        }
        self.candidate_id = "sha256:" + "a" * 64
        self.base = self.report(compare.BASE_CONFIG, compare.BASE_ZLIB)
        self.candidate = self.report(self.candidate_id, compare.CANDIDATE_ZLIB)
        self.base["Results"][0]["Vulnerabilities"] = [
            {"PkgName": "zlib1g", "Severity": "HIGH", "VulnerabilityID": cve}
            for cve in sorted(compare.FINDINGS)
        ]

    @staticmethod
    def report(image_id, package_id):
        return {
            "SchemaVersion": 2,
            "ArtifactType": "container_image",
            "CreatedAt": (datetime.now(UTC) - timedelta(seconds=1)).isoformat(),
            "Metadata": {"ImageID": image_id, "OS": {"Family": "debian", "Name": "13.7"}},
            "Results": [
                {
                    "Class": "os-pkgs",
                    "Type": "debian",
                    "Packages": [{"Name": "zlib1g", "ID": package_id}],
                    "Vulnerabilities": [],
                }
            ],
        }

    def compare(self):
        return compare.compare(self.base, self.candidate, self.candidate_id, self.database)

    def test_populated_baseline_and_fixed_candidate(self):
        result = self.compare()
        self.assertEqual(result["targeted_cves_reported_before"], sorted(compare.FINDINGS))
        self.assertEqual(result["targeted_cves_reported_after"], [])

    def test_baseline_missing_positive_controls_fails(self):
        self.base["Results"][0]["Vulnerabilities"].pop()
        with self.assertRaisesRegex(ValueError, "positive findings"):
            self.compare()

    def test_candidate_still_reporting_target_fails(self):
        self.candidate["Results"][0]["Vulnerabilities"] = copy.deepcopy(
            self.base["Results"][0]["Vulnerabilities"]
        )
        with self.assertRaisesRegex(ValueError, "still reports"):
            self.compare()

    def test_downgraded_target_still_fails(self):
        self.candidate["Results"][0]["Vulnerabilities"] = [
            {"PkgName": "zlib1g", "Severity": "MEDIUM", "VulnerabilityID": "CVE-2026-85091"}
        ]
        with self.assertRaisesRegex(ValueError, "still reports"):
            self.compare()

    def test_wrong_package_missing_inventory_or_image_fails(self):
        for edit in ("version", "inventory", "image"):
            with self.subTest(edit=edit):
                original = copy.deepcopy(self.candidate)
                if edit == "version":
                    self.candidate["Results"][0]["Packages"][0]["ID"] = compare.BASE_ZLIB
                elif edit == "inventory":
                    self.candidate["Results"][0]["Packages"] = []
                else:
                    self.candidate["Metadata"]["ImageID"] = compare.BASE_CONFIG
                with self.assertRaises(ValueError):
                    self.compare()
                self.candidate = original

    def test_expired_database_fails(self):
        self.database["NextUpdate"] = (datetime.now(UTC) - timedelta(seconds=1)).isoformat()
        with self.assertRaisesRegex(ValueError, "expired"):
            self.compare()
