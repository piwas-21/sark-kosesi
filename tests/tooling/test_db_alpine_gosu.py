"""Controls for paired Alpine and Gosu scan evidence."""

import copy
import hashlib
import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from datetime import UTC, datetime, timedelta
from pathlib import Path
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[2] / "scripts/check_db_alpine_gosu.py"
SPEC = importlib.util.spec_from_file_location("check_db_alpine_gosu", SCRIPT)
check = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(check)


def config(diff):
    doc = {
        "os": "linux",
        "architecture": "amd64",
        "rootfs": {"type": "layers", "diff_ids": ["sha256:" + diff * 64]},
    }
    raw = json.dumps(doc).encode()
    return raw, doc, "sha256:" + hashlib.sha256(raw).hexdigest()


def finding(cve, severity="HIGH", version="v1.24.6", package="stdlib"):
    return {
        "VulnerabilityID": cve,
        "PkgName": package,
        "InstalledVersion": version,
        "Severity": severity,
    }


def report(image_id, diff, version):
    return {
        "SchemaVersion": 2,
        "ArtifactType": "container_image",
        "CreatedAt": datetime.now(UTC).isoformat(),
        "Metadata": {
            "ImageID": image_id,
            "DiffIDs": ["sha256:" + diff * 64],
            "ImageConfig": {"os": "linux", "architecture": "amd64"},
            "OS": {"Family": "alpine", "Name": "3.23.2"},
        },
        "Results": [
            {
                "Class": "os-pkgs",
                "Target": "alpine",
                "Type": "alpine",
                "Packages": [{"Name": "musl", "Version": "1"}],
                "Vulnerabilities": [],
            },
            {
                "Class": "lang-pkgs",
                "Target": "usr/local/bin/gosu",
                "Type": "gobinary",
                "Packages": [{"Name": "stdlib", "Version": version}],
                "Vulnerabilities": [],
            },
        ],
    }


class CompareTests(unittest.TestCase):
    def setUp(self):
        self.stock_raw, self.stock_config, self.stock_id = config("a")
        self.candidate_raw, self.candidate_config, self.candidate_id = config("b")
        self.stock = report(self.stock_id, "a", "v1.24.6")
        self.candidate = report(self.candidate_id, "b", "v1.27.4")
        packages = []
        for name, version in (
            ("stdlib", "v1.27.4"),
            ("github.com/moby/sys/user", "v0.4.1"),
            ("golang.org/x/sys", "v0.1.0"),
            ("github.com/tianon/gosu", None),
        ):
            identity = name + (f"@{version}" if version else "")
            package = {
                "Name": name,
                "ID": identity,
                "Identifier": {"PURL": "pkg:golang/" + identity},
            }
            if version:
                package["Version"] = version
            else:
                package["Relationship"] = "root"
            packages.append(package)
        self.candidate["Results"][1]["Packages"] = packages
        self.stock["Results"][1]["Vulnerabilities"] = [
            finding("CVE-2025-68121"),
            finding("CVE-OLD", "CRITICAL"),
        ]
        self.candidate["Results"][1]["Vulnerabilities"] = [finding("CVE-NEW", "HIGH", "v1.27.4")]
        now = datetime.now(UTC)
        self.database = {
            "Version": 2,
            "UpdatedAt": (now - timedelta(hours=1)).isoformat(),
            "NextUpdate": (now + timedelta(hours=1)).isoformat(),
        }

    def compare(self, *, debian=False):
        key = "DEBIAN_CONFIG" if debian else "BASE_CONFIG"
        with patch.object(check, key, self.stock_id):
            return check.compare(
                self.stock,
                self.candidate,
                self.stock_raw,
                self.stock_config,
                self.candidate_raw,
                self.candidate_config,
                self.candidate_id,
                self.database,
                debian=debian,
            )

    def test_debian_combination_requires_targeted_findings_and_exact_package_change(self):
        for report, identity in (
            (self.stock, check.BASE_ZLIB),
            (self.candidate, check.CANDIDATE_ZLIB),
        ):
            report["Metadata"]["OS"] = {"Family": "debian", "Name": "13.7"}
            os_target = report["Results"][0]
            os_target["Type"] = "debian"
            os_target["Packages"] = [
                {"Name": "libxml2", "Version": "2.9"},
                {"Name": "zlib1g", "Version": identity.split("@")[1], "ID": identity},
            ]
            os_target["Vulnerabilities"] = [finding("CVE-RESIDUAL", "CRITICAL", "2.9", "libxml2")]
        zlib_finding = finding("CVE-2026-85091", "HIGH", "1", "zlib1g")
        self.stock["Results"][0]["Vulnerabilities"].append(zlib_finding)
        self.assertEqual(self.compare(debian=True)["severity_counts"]["candidate"]["CRITICAL"], 1)
        for target, key, value in (
            (self.stock["Results"][0], "Vulnerabilities", []),
            (self.candidate["Results"][0], "Vulnerabilities", [zlib_finding]),
            (self.candidate["Results"][1], "Vulnerabilities", [finding("CVE-2025-68121")]),
            (self.candidate["Results"][0], "Packages", self.stock["Results"][0]["Packages"]),
            (self.candidate["Results"][0]["Packages"][0], "Version", "unexpected"),
        ):
            with self.subTest(key=key, value=value), patch.dict(target, {key: value}):
                with self.assertRaises(ValueError):
                    self.compare(debian=True)

    def test_all_findings_and_deltas_reported_even_when_high_remains(self):
        self.candidate["Results"][1]["Vulnerabilities"].append(
            finding("CVE-OLD", "MEDIUM", "v1.27.4")
        )
        result = self.compare()
        self.assertEqual(result["severity_counts"]["stock"]["CRITICAL"], 1)
        self.assertEqual(result["severity_counts"]["candidate"]["HIGH"], 1)
        self.assertEqual(
            result["finding_counts"],
            {"stock": 2, "candidate": 2, "added_rows": 2, "removed_rows": 2},
        )
        self.assertEqual(len(result["changed_groups"]), 1)
        self.assertEqual(result["changed_groups"][0]["identity"][-2], "CVE-OLD")
        self.assertEqual(result["candidate_findings"][0]["finding"]["VulnerabilityID"], "CVE-NEW")

    def test_os_targets_are_canonical_for_comparison_but_preserved_in_output(self):
        stock_os, candidate_os = self.stock["Results"][0], self.candidate["Results"][0]
        stock_os["Target"] = "mirror.example/postgres@sha256:stock (alpine 3.23.2)"
        candidate_os["Target"] = "archive candidate (alpine 3.23.2)"
        same = finding("CVE-SAME", "HIGH", "1", "musl")
        changed = finding("CVE-CHANGED", "HIGH", "1", "musl")
        stock_os["Vulnerabilities"] = [same, changed]
        candidate_os["Vulnerabilities"] = [
            copy.deepcopy(same),
            finding("CVE-CHANGED", "CRITICAL", "1", "musl"),
            finding("CVE-NEW-OS", "HIGH", "1", "musl"),
        ]
        result = self.compare()
        added = result["added_rows"]
        removed = result["removed_rows"]
        self.assertNotIn("CVE-SAME", [row["finding"]["VulnerabilityID"] for row in added + removed])
        self.assertIn("CVE-NEW-OS", [row["finding"]["VulnerabilityID"] for row in added])
        group = next(
            group for group in result["changed_groups"] if group["identity"][-2] == "CVE-CHANGED"
        )
        self.assertEqual(group["stock"][0]["target"], stock_os["Target"])
        self.assertEqual(group["candidate"][0]["target"], candidate_os["Target"])
        self.assertEqual(result["stock_findings"][0]["target"], stock_os["Target"])
        self.assertEqual(result["candidate_findings"][0]["target"], candidate_os["Target"])

    def test_duplicate_rows_are_counted(self):
        self.candidate["Results"][1]["Vulnerabilities"] *= 2
        result = self.compare()
        self.assertEqual(result["finding_counts"]["added_rows"], 2)
        self.assertEqual(len(result["added_rows"]), 2)

    def test_baseline_positive_and_versions_required(self):
        for change in ("missing", "wrong_severity", "wrong_version", "wrong_candidate_version"):
            with self.subTest(change=change):
                stock, candidate = copy.deepcopy(self.stock), copy.deepcopy(self.candidate)
                if change == "missing":
                    self.stock["Results"][1]["Vulnerabilities"].pop(0)
                elif change == "wrong_severity":
                    self.stock["Results"][1]["Vulnerabilities"][0]["Severity"] = "LOW"
                elif change == "wrong_version":
                    self.stock["Results"][1]["Packages"][0]["Version"] = "v1.24.5"
                else:
                    self.candidate["Results"][1]["Packages"][0]["Version"] = "v1.24.6"
                with self.assertRaises(ValueError):
                    self.compare()
                self.stock, self.candidate = stock, candidate

    def test_only_candidate_gosu_root_may_lack_version(self):
        root = self.candidate["Results"][1]["Packages"][-1]
        self.assertEqual(self.compare()["finding_counts"]["candidate"], 1)

        for change in (
            "stock",
            "name",
            "id",
            "relationship",
            "null",
            "empty",
            "target",
            "type",
            "stdlib",
            "dependency",
        ):
            with self.subTest(change=change):
                stock, candidate = copy.deepcopy(self.stock), copy.deepcopy(self.candidate)
                package = self.candidate["Results"][1]["Packages"][-1]
                if change == "stock":
                    self.stock["Results"][1]["Packages"].append(copy.deepcopy(root))
                elif change == "name":
                    package["Name"] = "github.com/other/module"
                elif change == "id":
                    package["ID"] += "@unknown"
                elif change == "relationship":
                    package["Relationship"] = "direct"
                elif change == "null":
                    package["Version"] = None
                elif change == "empty":
                    package["Version"] = ""
                elif change == "target":
                    self.candidate["Results"][1]["Target"] = "usr/local/bin/other"
                elif change == "type":
                    self.candidate["Results"][1]["Type"] = "other"
                elif change == "stdlib":
                    self.candidate["Results"][1]["Packages"][0].pop("Version")
                else:
                    self.candidate["Results"][1]["Packages"].append({"Name": "golang.org/x/sys"})
                with self.assertRaises(ValueError):
                    self.compare()
                self.stock, self.candidate = stock, candidate

    def test_candidate_module_versions_and_purls_are_exact(self):
        for change in ("missing", "duplicate", "version", "id", "purl", "extra"):
            with self.subTest(change=change):
                original = copy.deepcopy(self.candidate)
                packages = self.candidate["Results"][1]["Packages"]
                if change == "missing":
                    packages.pop(2)
                elif change == "duplicate":
                    packages[2] = copy.deepcopy(packages[1])
                elif change == "version":
                    packages[1]["Version"] = "v0.1.0"
                elif change == "id":
                    packages[2]["ID"] = "golang.org/x/sys@v0.2.0"
                elif change == "purl":
                    packages[2]["Identifier"]["PURL"] = "pkg:golang/other@v0.1.0"
                else:
                    packages.append({"Name": "extra", "Version": "v1"})
                with self.assertRaises(ValueError):
                    self.compare()
                self.candidate = original

    def test_unknown_xsys_finding_is_retained(self):
        row = finding("CVE-2026-39824", "UNKNOWN", "v0.1.0", "golang.org/x/sys")
        self.candidate["Results"][1]["Vulnerabilities"].append(row)
        result = self.compare()
        self.assertEqual(result["severity_counts"]["candidate"]["UNKNOWN"], 1)
        self.assertIn(row, [item["finding"] for item in result["candidate_findings"]])

    def test_identity_and_inventory_fail_closed(self):
        for change in (
            "raw",
            "image",
            "diff",
            "platform",
            "os",
            "inventory",
            "finding",
            "modified",
            "duplicate_gosu",
        ):
            with self.subTest(change=change):
                stock, candidate, raw = (
                    copy.deepcopy(self.stock),
                    copy.deepcopy(self.candidate),
                    self.candidate_raw,
                )
                if change == "raw":
                    self.candidate_raw += b" "
                elif change == "image":
                    self.candidate["Metadata"]["ImageID"] = self.stock_id
                elif change == "diff":
                    self.candidate["Metadata"]["DiffIDs"] = self.stock["Metadata"]["DiffIDs"]
                elif change == "platform":
                    self.candidate["Metadata"]["ImageConfig"]["architecture"] = "arm64"
                elif change == "os":
                    self.candidate["Metadata"]["OS"]["Name"] = "3.22"
                elif change == "inventory":
                    self.candidate["Results"][0]["Packages"] = []
                elif change == "finding":
                    self.candidate["Results"][1]["Vulnerabilities"][0].pop("InstalledVersion")
                elif change == "modified":
                    self.candidate["Results"][0]["ExperimentalModifiedFindings"] = [{}]
                else:
                    self.candidate["Results"].append(copy.deepcopy(self.candidate["Results"][1]))
                with self.assertRaises(ValueError):
                    self.compare()
                self.stock, self.candidate, self.candidate_raw = stock, candidate, raw

    def test_stale_or_future_database_and_report_rejected(self):
        for field, value in (
            ("NextUpdate", datetime.now(UTC) - timedelta(seconds=1)),
            ("UpdatedAt", datetime.now(UTC) + timedelta(seconds=1)),
        ):
            with self.subTest(field=field):
                original = self.database[field]
                self.database[field] = value.isoformat()
                with self.assertRaises(ValueError):
                    self.compare()
                self.database[field] = original
        self.candidate["CreatedAt"] = (datetime.now(UTC) + timedelta(hours=1)).isoformat()
        with self.assertRaises(ValueError):
            self.compare()

    def test_cli_rejects_forged_stock_config_and_duplicate_keys(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for name, data in (
                ("stock-report.json", self.stock),
                ("candidate-report.json", self.candidate),
                ("stock-config.json", self.stock_config),
                ("candidate-config.json", self.candidate_config),
                ("db-metadata.json", self.database),
            ):
                (root / name).write_text(json.dumps(data))
            result = subprocess.run(
                [sys.executable, str(SCRIPT), str(root), self.candidate_id],
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("config digest mismatch", result.stderr)
            (root / "stock-report.json").write_text('{"SchemaVersion":2,"SchemaVersion":2}')
            result = subprocess.run(
                [sys.executable, str(SCRIPT), str(root), self.candidate_id],
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("duplicate JSON member", result.stderr)


if __name__ == "__main__":
    unittest.main()
