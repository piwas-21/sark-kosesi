import unittest
from datetime import UTC, datetime
from unittest.mock import patch

import scan


def row(cve, package):
    return {
        "target": "debian",
        "class": "os-pkgs",
        "type": "debian",
        "finding": {
            "VulnerabilityID": cve,
            "PkgName": package,
            "InstalledVersion": "1",
            "Status": "affected",
            "Severity": "HIGH",
        },
    }


class PairPolicyTest(unittest.TestCase):
    def test_paired_scan_preserves_open_findings_and_rejects_wrong_base_or_stale_db(self):
        stock = {"CreatedAt": "2026-11-24T00:30:00Z"}
        server = {"CreatedAt": "2026-11-24T00:40:00Z"}
        db = {
            "Version": 2,
            "UpdatedAt": "2026-11-24T00:00:00Z",
            "NextUpdate": "2026-11-25T00:00:00Z",
        }
        now = datetime(2026, 11, 24, 1, tzinfo=UTC)
        records = {"stock": {}, "server": {}}
        stock_finding = row(*scan.POSITIVE)
        server_finding = row("CVE-2026-124709", "libssl3t64")
        with (
            patch.object(scan, "inspect") as parse,
            patch.object(scan, "check_dpkg"),
            patch.object(scan, "check_sbom", return_value=1),
        ):

            def compare_at(version, evaluated=now):
                parse.side_effect = [
                    ([stock_finding], [], [], "13.7"),
                    ([server_finding], [], [], version),
                ]
                return scan.compared(
                    stock, server, {}, {}, "sha256:" + "a" * 64, records, records, db, evaluated
                )

            comparison = compare_at("13.8")
            self.assertEqual(comparison["findings_status"], "review_required")
            self.assertEqual(comparison["server"]["severity_counts"]["HIGH"], 1)
            self.assertEqual(comparison["server_only_exact_rows"], [server_finding])
            with self.assertRaisesRegex(ValueError, "Debian version differs"):
                compare_at("13.7")
            with self.assertRaisesRegex(ValueError, "Stale Trivy evidence"):
                compare_at("13.8", datetime(2026, 11, 25, 1, tzinfo=UTC))


if __name__ == "__main__":
    unittest.main()
