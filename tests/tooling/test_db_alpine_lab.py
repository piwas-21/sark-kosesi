"""Positive and negative controls for isolated Alpine scan comparisons."""

import copy
import os
import subprocess
import tempfile
import unittest
from datetime import UTC, datetime, timedelta
from pathlib import Path

import check_db_alpine_lab as lab

ROOT = Path(__file__).resolve().parents[2]


class AlpineLabTests(unittest.TestCase):
    def setUp(self):
        self.now = datetime.now(UTC)
        self.database = {
            "Version": 2,
            "UpdatedAt": (self.now - timedelta(hours=1)).isoformat(),
            "NextUpdate": (self.now + timedelta(hours=1)).isoformat(),
        }
        self.base_config = {"rootfs": {"diff_ids": ["sha256:" + "a" * 64]}}
        self.alpine_config = {"rootfs": {"diff_ids": ["sha256:" + "b" * 64]}}
        self.base = self.report(
            lab.DEBIAN_CONFIG,
            self.base_config,
            "debian",
            "libxml2@2.12.7+dfsg+really2.9.14-2.1+deb13u3",
            "2.12.7+dfsg+really2.9.14",
        )
        self.alpine = self.report(
            lab.ALPINE_CONFIG, self.alpine_config, "alpine", "libxml2@2.13.9-r2", "2.13.9-r2"
        )
        self.base["Results"][0]["Vulnerabilities"] = [self.finding(lab.TARGET_CVE, "CRITICAL")]
        self.alpine["Results"][0]["Vulnerabilities"] = [self.finding("CVE-2026-OTHER", "HIGH")]

    def finding(self, cve, severity):
        return {
            "VulnerabilityID": cve,
            "Severity": severity,
            "PkgName": "libxml2",
            "InstalledVersion": "example",
        }

    def report(self, config_id, config, family, package_id, package_version):
        return {
            "SchemaVersion": 2,
            "ArtifactType": "container_image",
            "CreatedAt": (self.now - timedelta(seconds=1)).isoformat(),
            "Metadata": {
                "ImageID": config_id,
                "DiffIDs": config["rootfs"]["diff_ids"],
                "ImageConfig": {"os": "linux", "architecture": "amd64"},
                "OS": {"Family": family, "Name": "test"},
            },
            "Results": [
                {
                    "Class": "os-pkgs",
                    "Type": family,
                    "Packages": [{"Name": "libxml2", "Version": package_version, "ID": package_id}],
                    "Vulnerabilities": [],
                }
            ],
        }

    def compare(self):
        return lab.compare(
            self.base, self.alpine, self.base_config, self.alpine_config, self.database, self.now
        )

    def test_positive_baseline_and_reported_candidate_absence(self):
        result = self.compare()
        self.assertEqual(len(result["baseline"]["target_cve_findings"]), 1)
        self.assertEqual(result["alpine"]["target_cve_findings"], [])
        self.assertEqual(result["alpine"]["severity_counts"]["HIGH"], 1)
        self.assertIn("neither image is accepted", result["interpretation"])

    def test_candidate_finding_is_retained_not_declared_clean(self):
        self.alpine["Results"][0]["Vulnerabilities"].append(self.finding(lab.TARGET_CVE, "MEDIUM"))
        self.assertEqual(self.compare()["alpine"]["target_cve_findings"][0]["severity"], "MEDIUM")

    def test_missing_baseline_positive_fails(self):
        self.base["Results"][0]["Vulnerabilities"] = []
        with self.assertRaisesRegex(ValueError, "positive CVE"):
            self.compare()

    def test_empty_inventory_wrong_image_and_modified_findings_fail(self):
        for kind in ("inventory", "image", "layers", "modified", "package", "finding"):
            with self.subTest(kind=kind):
                original = copy.deepcopy(self.alpine)
                if kind == "inventory":
                    self.alpine["Results"][0]["Packages"] = []
                elif kind == "image":
                    self.alpine["Metadata"]["ImageID"] = lab.DEBIAN_CONFIG
                elif kind == "layers":
                    self.alpine["Metadata"]["DiffIDs"] = []
                elif kind == "modified":
                    self.alpine["Results"][0]["ExperimentalModifiedFindings"] = [{"ignored": True}]
                elif kind == "package":
                    self.alpine["Results"][0]["Packages"][0]["ID"] = "libxml2@wrong"
                else:
                    self.alpine["Results"][0]["Vulnerabilities"] = [{"Severity": "HIGH"}]
                with self.assertRaises(ValueError):
                    self.compare()
                self.alpine = original

    def test_stale_database_fails(self):
        self.database["NextUpdate"] = (self.now - timedelta(seconds=1)).isoformat()
        with self.assertRaisesRegex(ValueError, "stale"):
            self.compare()

    def test_non_hosted_execution_never_calls_docker(self):
        script = ROOT / "scripts/test_db_alpine_lab.sh"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            marker = root / "docker-called"
            docker = root / "docker"
            docker.write_text(f"#!/bin/sh\ntouch '{marker}'\nexit 99\n")
            docker.chmod(0o755)
            env = {
                "PATH": f"{root}:{os.defpath}",
                "RUNNER_TEMP": directory,
                "RUNNER_ENVIRONMENT": "self-hosted",
                "GITHUB_ACTIONS": "true",
                "ALPINE_SOURCE_REVISION": "a" * 40,
                "ALPINE_EVIDENCE_DIR": str(root / "db-alpine-evidence"),
                "ALPINE_SCRATCH_DIR": str(root / "db-alpine-scratch"),
            }
            result = subprocess.run(
                ["bash", str(script)], env=env, capture_output=True, check=False
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(marker.exists())
            self.assertFalse((root / "db-alpine-scratch").exists())
            self.assertFalse((root / "db-alpine-evidence").exists())
