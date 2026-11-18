#!/usr/bin/env python3
"""Independent negative controls for scan scope, identity and source custody."""

import copy
import hashlib
import io
import json
import tarfile
import tempfile
import unittest
from datetime import UTC, datetime, timedelta
from pathlib import Path

from archive import verify_archive
from compare import STOCK_ID, compare
from source_manifest import ARCHIVE, manifest


def report(image_id, diff, findings):
    packages = [
        {"Name": "libc6", "Version": "2.41", "ID": "libc6@2.41-12"},
        {"Name": "zlib1g", "Version": "1.3", "ID": "zlib1g@1:1.3-1"},
        {"Name": "libxml2", "Version": "2.9", "ID": "libxml2@2.9-1"},
    ]
    return {
        "SchemaVersion": 2,
        "ArtifactType": "container_image",
        "CreatedAt": datetime.now(UTC).isoformat(),
        "Metadata": {
            "ImageID": image_id,
            "DiffIDs": [diff],
            "ImageConfig": {"os": "linux", "architecture": "amd64"},
            "OS": {"Family": "debian", "Name": "13.7"},
        },
        "Results": [
            {
                "Class": "os-pkgs",
                "Type": "debian",
                "Target": "debian",
                "Packages": packages,
                "Vulnerabilities": findings,
            }
        ],
    }


def finding(cve, package="libxml2", status="affected", severity="CRITICAL"):
    return {
        "VulnerabilityID": cve,
        "PkgName": package,
        "InstalledVersion": "2.9-1",
        "Status": status,
        "Severity": severity,
    }


class ScanControls(unittest.TestCase):
    def setUp(self):
        self.now = datetime.now(UTC)
        self.base_diff = "sha256:" + "a" * 64
        self.next_diff = "sha256:" + "b" * 64
        self.candidate_id = "sha256:" + "c" * 64
        self.stock_config = {
            "os": "linux",
            "architecture": "amd64",
            "rootfs": {"type": "layers", "diff_ids": [self.base_diff]},
        }
        self.candidate_config = {
            "os": "linux",
            "architecture": "amd64",
            "rootfs": {"type": "layers", "diff_ids": [self.next_diff]},
        }
        self.stock = report(STOCK_ID, self.base_diff, [finding("CVE-2026-6653")])
        self.candidate = report(
            self.candidate_id,
            self.next_diff,
            [finding("CVE-TEST", status="unknown", severity="UNKNOWN")],
        )
        self.db = {
            "Version": 2,
            "UpdatedAt": (self.now - timedelta(hours=1)).isoformat(),
            "NextUpdate": (self.now + timedelta(hours=1)).isoformat(),
        }
        self.sbom = {
            "bomFormat": "CycloneDX",
            "specVersion": "1.5",
            "components": [
                {"name": "libc6", "version": "2.41-12"},
                {"name": "zlib1g", "version": "1:1.3-1"},
                {"name": "libxml2", "version": "2.9-1"},
            ],
        }
        self.dpkg = {"libc6": "2.41-12", "zlib1g": "1:1.3-1", "libxml2": "2.9-1"}

    def result(self):
        return compare(
            self.stock,
            self.candidate,
            self.stock_config,
            self.candidate_config,
            self.candidate_id,
            self.db,
            self.sbom,
            self.sbom,
            self.dpkg,
            self.dpkg,
            self.now + timedelta(seconds=5),
        )

    def test_success_preserves_unknown_and_exact_rows(self):
        result = self.result()
        self.assertEqual(result["status_counts"]["composed"], {"unknown": 1})
        self.assertEqual(result["severity_counts"]["composed"]["UNKNOWN"], 1)
        self.assertEqual(
            result["stock_only_exact_rows"][0]["finding"]["VulnerabilityID"], "CVE-2026-6653"
        )
        self.assertIn("not resolved", result["interpretation"])

    def test_empty_invalid_and_suppressed_fail(self):
        mutations = [
            lambda r: r.update({"Results": []}),
            lambda r: r["Results"][0].update({"Packages": []}),
            lambda r: r["Results"][0].update({"ExperimentalModifiedFindings": None}),
            lambda r: r["Results"][0]["Vulnerabilities"][0].update({"Suppressed": True}),
            lambda r: r["Results"][0]["Vulnerabilities"][0].update({"Suppressed": None}),
            lambda r: r["Results"][0].update(
                {"ExperimentalModifiedFindings": [{"reason": "ignored"}]}
            ),
            lambda r: r["Results"][0]["Vulnerabilities"][0].update({"Severity": "BOGUS"}),
            lambda r: r["Results"][0]["Vulnerabilities"][0].update({"Status": "unrecognized"}),
            lambda r: r["Metadata"].update({"ImageID": "sha256:" + "d" * 64}),
            lambda r: r["Results"][0].update({"Vulnerabilities": []}),
        ]
        for mutation in mutations:
            original = self.stock
            self.stock = copy.deepcopy(original)
            mutation(self.stock)
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                self.result()
            self.stock = original

    def test_database_sbom_and_inventory_fail_closed(self):
        for attribute, bad in (
            ("db", {}),
            ("sbom", {"bomFormat": "CycloneDX", "specVersion": "1.5", "components": []}),
            ("dpkg", {"libc6": "wrong"}),
        ):
            original = getattr(self, attribute)
            setattr(self, attribute, bad)
            with self.subTest(attribute=attribute), self.assertRaises(ValueError):
                self.result()
            setattr(self, attribute, original)

    def test_go_package_must_be_in_sbom(self):
        self.stock["Results"].append(
            {
                "Class": "lang-pkgs",
                "Type": "gobinary",
                "Target": "usr/local/bin/gosu",
                "Packages": [{"Name": "stdlib", "Version": "v1.24.6", "ID": "stdlib@v1.24.6"}],
            }
        )
        with self.assertRaises(ValueError):
            self.result()
        self.sbom["components"].append({"name": "stdlib", "version": "v1.24.6"})
        self.result()

    def test_partial_sbom_and_expired_database_fail(self):
        self.sbom["components"] = self.sbom["components"][:-1]
        with self.assertRaises(ValueError):
            self.result()
        self.sbom["components"].append({"name": "libxml2", "version": "2.9-1"})
        self.db["NextUpdate"] = (self.now - timedelta(seconds=1)).isoformat()
        with self.assertRaises(ValueError):
            self.result()

    def test_source_component_is_not_scanner_acceptance(self):
        revision = "1" * 40
        source_id = "sha256:" + "2" * 64
        composed_id = "sha256:" + "3" * 64
        identity = f"revision={revision}\nsource_sha256={ARCHIVE}\ncandidate_image_id={source_id}\n"
        composition = f"source={source_id}\ncomposed={composed_id}\n"
        paths = (
            "/opt/pg-smoke/bin/postgres",
            "/opt/pg-smoke/bin/psql",
            "/opt/pg-smoke/bin/pg_config",
            "/opt/pg-smoke/lib/libpq.so.5.18",
        )
        files = "".join("a" * 64 + "  " + path + "\n" for path in paths)
        result = manifest(
            revision,
            identity,
            composition,
            composed_id,
            files,
            "postgres (PostgreSQL) 18.7\n",
            "PostgreSQL 18.7\n",
        )
        self.assertEqual(len(result["file_sha256"]), 4)
        self.assertIn("not a fix", result["coverage"])
        for args in (
            (
                revision,
                identity,
                composition,
                composed_id,
                "",
                "postgres (PostgreSQL) 18.7\n",
                "PostgreSQL 18.7\n",
            ),
            (revision, identity, composition, composed_id, files, "wrong\n", "PostgreSQL 18.7\n"),
            (
                revision,
                identity,
                composition,
                "sha256:" + "4" * 64,
                files,
                "postgres (PostgreSQL) 18.7\n",
                "PostgreSQL 18.7\n",
            ),
            (
                revision,
                identity + f"source_sha256={ARCHIVE}\n",
                composition,
                composed_id,
                files,
                "postgres (PostgreSQL) 18.7\n",
                "PostgreSQL 18.7\n",
            ),
        ):
            with self.assertRaises(ValueError):
                manifest(*args)

    def test_archive_digest_and_layer_tampering(self):
        layer = b"synthetic layer bytes"
        config = {
            "os": "linux",
            "architecture": "amd64",
            "rootfs": {
                "type": "layers",
                "diff_ids": ["sha256:" + hashlib.sha256(layer).hexdigest()],
            },
        }
        raw = json.dumps(config).encode()
        image_id = "sha256:" + hashlib.sha256(raw).hexdigest()

        def make(path, payload):
            with tarfile.open(path, "w") as tar:
                for name, content in (
                    ("config.json", raw),
                    ("layer.tar", payload),
                    (
                        "manifest.json",
                        json.dumps([{"Config": "config.json", "Layers": ["layer.tar"]}]).encode(),
                    ),
                ):
                    info = tarfile.TarInfo(name)
                    info.size = len(content)
                    tar.addfile(info, io.BytesIO(content))

        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "candidate.tar"
            make(path, layer)
            self.assertEqual(verify_archive(path, image_id), raw)
            with self.assertRaises(ValueError):
                verify_archive(path, "sha256:" + "f" * 64)
            make(path, b"different bytes")
            with self.assertRaises(ValueError):
                verify_archive(path, image_id)


if __name__ == "__main__":
    unittest.main()
