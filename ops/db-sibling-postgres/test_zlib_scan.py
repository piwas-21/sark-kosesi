#!/usr/bin/env python3
"""Controls for a single target in otherwise unfiltered scan evidence."""

import copy
import unittest

from check_zlib_scan import check

STOCK_ID = "sha256:3cccc725163a95947178f4da5e9bf16add75cb286170ac5a2cd2f6513534955d"
CANDIDATE_ID = "sha256:" + "a" * 64


def report(image_id, package_id, findings):
    return {
        "SchemaVersion": 2,
        "ArtifactType": "container_image",
        "Metadata": {"ImageID": image_id, "OS": {"Family": "debian", "Name": "13.7"}},
        "Results": [
            {
                "Class": "os-pkgs",
                "Type": "debian",
                "Packages": [{"Name": "zlib1g", "ID": package_id}],
                "Vulnerabilities": findings,
            }
        ],
    }


def finding(cve, severity="HIGH"):
    return {
        "VulnerabilityID": cve,
        "PkgName": "zlib1g",
        "InstalledVersion": "1",
        "Severity": severity,
    }


class ZlibScanControls(unittest.TestCase):
    def setUp(self):
        self.stock = report(
            STOCK_ID, "zlib1g@1:1.3.dfsg+really1.3.1-1+b1", [finding("CVE-2026-85091")]
        )
        self.candidate = report(
            CANDIDATE_ID,
            "zlib1g@1:1.3.dfsg+really1.3.1-1+deb13u1",
            [finding("CVE-OTHER", "CRITICAL")],
        )
        self.comparison = {"stock_image_id": STOCK_ID, "composed_image_id": CANDIDATE_ID}
        self.source = {"composed_image_id": CANDIDATE_ID}

    def test_positive_and_other_findings_preserved(self):
        result = check(self.stock, self.candidate, self.comparison, self.source, CANDIDATE_ID)
        self.assertEqual(result["candidate_other_zlib_findings"], ["CVE-OTHER"])
        self.assertIn("remain undisposed", result["interpretation"])

    def test_missing_positive_wrong_package_and_target_at_any_severity_fail(self):
        for target, mutate in (
            ("stock", lambda r: r["Results"][0].update({"Vulnerabilities": []})),
            ("stock", lambda r: r["Results"][0]["Vulnerabilities"][0].update({"Severity": "LOW"})),
            ("candidate", lambda r: r["Results"][0]["Packages"][0].update({"ID": "zlib1g@wrong"})),
            (
                "candidate",
                lambda r: r["Results"][0]["Vulnerabilities"].append(
                    finding("CVE-2026-85091", "UNKNOWN")
                ),
            ),
        ):
            old = getattr(self, target)
            setattr(self, target, copy.deepcopy(old))
            mutate(getattr(self, target))
            with self.subTest(target=target), self.assertRaises(ValueError):
                check(self.stock, self.candidate, self.comparison, self.source, CANDIDATE_ID)
            setattr(self, target, old)

    def test_mismatched_pair_or_source_fails(self):
        with self.assertRaises(ValueError):
            check(
                self.stock,
                self.candidate,
                {**self.comparison, "composed_image_id": STOCK_ID},
                self.source,
                CANDIDATE_ID,
            )
        with self.assertRaises(ValueError):
            check(
                self.stock,
                self.candidate,
                self.comparison,
                {"composed_image_id": STOCK_ID},
                CANDIDATE_ID,
            )
        with self.assertRaises(ValueError):
            check(self.stock, self.candidate, self.comparison, self.source, STOCK_ID)


if __name__ == "__main__":
    unittest.main()
