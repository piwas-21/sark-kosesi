#!/usr/bin/env python3
"""Offline controls for signed metadata and selected payload comparisons."""

import hashlib
import importlib.util
import io
import lzma
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import MagicMock, patch

spec = importlib.util.spec_from_file_location("lineage", Path(__file__).with_name("lineage.py"))
lineage = importlib.util.module_from_spec(spec)
spec.loader.exec_module(lineage)


def package_stanza(name, values):
    return (
        f"Package: {name}\nVersion: {values['version']}\nArchitecture: amd64\n"
        f"Source: {values['source']}\nSHA256: {values['sha256']}\nSize: {values['size']}\n"
        f"Filename: pool/main/x/example/{name}_synthetic_amd64.deb\n\n"
    )


class LineageControls(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.index = self.root / "Packages.xz"
        self.index.write_bytes(
            lzma.compress("".join(package_stanza(k, v) for k, v in lineage.LOCK.items()).encode())
        )

    def test_complete_locked_records_and_signed_index_binding(self):
        self.assertEqual(set(lineage.package_records(self.index)), set(lineage.LOCK))
        index_hash = lineage.digest(self.index)
        release = self.root / "Release"
        release.write_text(
            f"Codename: trixie\nSHA256:\n {index_hash} {self.index.stat().st_size} main/binary-amd64/Packages.xz\n"
        )
        with patch.object(lineage, "INDEX_SHA256", index_hash):
            lineage.signed_index(release, self.index)
            release.write_text(release.read_text().replace(index_hash, "0" * 64))
            with self.assertRaises(ValueError):
                lineage.signed_index(release, self.index)

    def test_signature_requires_success_and_approved_signer(self):
        signer = next(iter(lineage.SIGNERS))
        with patch.object(
            lineage.subprocess,
            "run",
            return_value=MagicMock(stdout=f"[GNUPG:] VALIDSIG {signer}\n"),
        ) as run:
            self.assertEqual(lineage.verify_signature(self.root, self.root, self.root), {signer})
            self.assertTrue(run.call_args.kwargs["check"])
        for status in ("", "[GNUPG:] VALIDSIG " + "A" * 40 + "\n"):
            with patch.object(lineage.subprocess, "run", return_value=MagicMock(stdout=status)):
                with self.assertRaises(ValueError):
                    lineage.verify_signature(self.root, self.root, self.root)
        with patch.object(
            lineage.subprocess, "run", side_effect=subprocess.CalledProcessError(1, "gpgv")
        ):
            with self.assertRaises(subprocess.CalledProcessError):
                lineage.verify_signature(self.root, self.root, self.root)

    def test_missing_duplicate_substituted_and_source_mismatch_fail(self):
        full = "".join(package_stanza(k, v) for k, v in lineage.LOCK.items())
        first = next(iter(lineage.LOCK))
        for text in (
            full.replace(package_stanza(first, lineage.LOCK[first]), ""),
            full + package_stanza(first, lineage.LOCK[first]),
            full.replace(lineage.LOCK[first]["sha256"], "0" * 64),
            full.replace("Source: systemd", "Source: wrong", 1),
            full.replace("Architecture: amd64", "Architecture: arm64", 1),
        ):
            self.index.write_bytes(lzma.compress(text.encode()))
            with self.subTest(text=text[:60]), self.assertRaises(ValueError):
                lineage.package_records(self.index)

    def test_selected_file_and_symlink_metadata_and_tamper(self):
        archive = self.root / "payload.tar"
        with tarfile.open(archive, "w") as tar:
            content = b"signed synthetic bytes"
            item = tarfile.TarInfo("usr/lib/libexample.so.1")
            item.mode = 0o755
            item.size = len(content)
            tar.addfile(item, io.BytesIO(content))
            item = tarfile.TarInfo("usr/lib/libexample.so")
            item.type = tarfile.SYMTYPE
            item.mode = 0o777
            item.linkname = "libexample.so.1"
            tar.addfile(item)
            item = tarfile.TarInfo("usr/lib/libexample-hardlink")
            item.type = tarfile.LNKTYPE
            item.mode = 0o755
            item.linkname = "usr/lib/libexample.so.1"
            tar.addfile(item)
        paths = ["usr/lib/libexample.so", "usr/lib/libexample.so.1", "usr/lib/libexample-hardlink"]
        result = lineage.selected(archive, paths, 64 * 1024 * 1024)
        self.assertEqual(result[paths[0]]["target"], "libexample.so.1")
        self.assertEqual(
            result[paths[1]]["sha256"], hashlib.sha256(b"signed synthetic bytes").hexdigest()
        )
        self.assertEqual(result[paths[2]]["sha256"], result[paths[1]]["sha256"])
        self.assertEqual(result[paths[2]]["hardlink_group"], sorted(paths[1:]))
        lineage.compare_files(result, result.copy())
        changed = {k: dict(v) for k, v in result.items()}
        changed[paths[1]]["sha256"] = "0" * 64
        with self.assertRaises(ValueError):
            lineage.compare_files(result, changed)
        changed = {k: dict(v) for k, v in result.items()}
        del changed[paths[2]]["hardlink_group"]
        with self.assertRaises(ValueError):
            lineage.compare_files(result, changed)
        changed = {k: dict(v) for k, v in result.items()}
        changed[paths[1]]["mode"] = 0o644
        with self.assertRaises(ValueError):
            lineage.compare_files(result, changed)
        changed = {k: dict(v) for k, v in result.items()}
        changed[paths[0]]["target"] = "other.so"
        with self.assertRaises(ValueError):
            lineage.compare_files(result, changed)
        with self.assertRaises(ValueError):
            lineage.selected(archive, paths + ["absent"], 64 * 1024 * 1024)
        with tarfile.open(archive, "r:") as parsed:
            list(parsed)
            end = parsed.offset
        archive.write_bytes(archive.read_bytes()[: end + 512])
        with self.assertRaises(ValueError):
            lineage.selected(archive, paths, 64 * 1024 * 1024)

    def test_export_digest_and_image_binding(self):
        image = self.root / "export"
        image.write_bytes(b"synthetic export")
        report = {
            "export_sha256": lineage.digest(image),
            "image_id": "sha256:" + "a" * 64,
            "revision": "b" * 40,
        }
        lineage.verify_export(image, report)
        for key, value in (
            ("export_sha256", "0" * 64),
            ("image_id", "latest"),
            ("revision", "old"),
        ):
            with self.subTest(key=key):
                altered = dict(report)
                altered[key] = value
                with self.assertRaises(ValueError):
                    lineage.verify_export(image, altered)

    def test_installed_inventory_requires_exact_complete_packages(self):
        lines = [
            f"{name}\t{entry['version']}\tinstall ok installed"
            for name, entry in lineage.LOCK.items()
        ]
        lineage.verify_installed("\n".join(lines) + "\n")
        for text in (
            "\n".join(lines[:-1]) + "\n",
            "\n".join(lines).replace("install ok installed", "deinstall ok config-files", 1),
            "\n".join(lines).replace("install ok installed", "install ok half-installed", 1),
            "\n".join(lines).replace(next(iter(lineage.LOCK.values()))["version"], "0", 1),
        ):
            with self.subTest(text=text[:40]), self.assertRaises(ValueError):
                lineage.verify_installed(text)


if __name__ == "__main__":
    unittest.main()
