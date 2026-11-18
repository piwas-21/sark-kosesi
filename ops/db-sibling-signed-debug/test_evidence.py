#!/usr/bin/env python3
"""Small offline controls; no archive access or candidate execution."""

import importlib.util
import lzma
import struct
import tempfile
import unittest
import zlib
from pathlib import Path

spec = importlib.util.spec_from_file_location("evidence", Path(__file__).with_name("evidence.py"))
evidence = importlib.util.module_from_spec(spec)
spec.loader.exec_module(evidence)


def stanza(name, item):
    return (
        f"Package: {name}-dbgsym\nVersion: {evidence.VERSION}\nArchitecture: amd64\n"
        f"Source: systemd\nSHA256: {item['sha256']}\nSize: {item['size']}\n"
        f"Filename: pool/main/s/systemd/{name}-dbgsym_fixture_amd64.deb\n\n"
    )


class Controls(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.index = self.root / "Packages.xz"
        self.full = "".join(stanza(k, v) for k, v in evidence.LOCK.items())
        self.index.write_bytes(lzma.compress(self.full.encode()))
        self.release = self.root / "Release"
        self.release.write_text(
            f"Codename: trixie-debug\nSHA256:\n {evidence.lineage.digest(self.index)} {self.index.stat().st_size} main/binary-amd64/Packages.xz\n"
        )

    def test_signed_index_binding(self):
        evidence.check_index(self.release, self.index)
        for text in (
            self.release.read_text().replace("trixie-debug", "trixie"),
            self.release.read_text().replace(evidence.lineage.digest(self.index), "0" * 64),
            self.release.read_text().replace("SHA256:", "Other:"),
        ):
            self.release.write_text(text)
            with self.assertRaises(ValueError):
                evidence.check_index(self.release, self.index)

    def test_records_require_two_exact_debian_packages(self):
        self.assertEqual(len(evidence.records(self.index)), 2)
        first = next(iter(evidence.LOCK))
        for text in (
            self.full.replace(stanza(first, evidence.LOCK[first]), ""),
            self.full + stanza(first, evidence.LOCK[first]),
            self.full.replace(evidence.LOCK[first]["sha256"], "0" * 64),
            self.full.replace("Source: systemd", "Source: unknown", 1),
            self.full.replace("Architecture: amd64", "Architecture: arm64", 1),
        ):
            self.index.write_bytes(lzma.compress(text.encode()))
            with self.assertRaises(ValueError):
                evidence.records(self.index)

    def test_build_id_and_debuglink_tamper(self):
        ident = next(iter(evidence.LOCK.values()))["build_id"]
        evidence.build_id("  Build ID: " + ident + "\n", ident)
        for value in (
            "",
            "  Build ID: " + "f" * 40 + "\n",
            "  Build ID: " + ident + "\n" * 2 + "  Build ID: " + ident,
        ):
            with self.assertRaises(ValueError):
                evidence.build_id(value, ident)
        payload = b"synthetic debug bytes"
        name = b"test.debug\0"
        section = name + b"\0" * ((-len(name)) % 4) + struct.pack("<I", zlib.crc32(payload))
        self.assertEqual(evidence.debuglink(section, payload), "test.debug")
        self.assertEqual(evidence.debuglink(b"", payload), "absent")
        for link, data in (
            (section, b"other"),
            (section[:-1], payload),
            (b"../evil\0\0" + b"\0" * 4, payload),
        ):
            with self.assertRaises(ValueError):
                evidence.debuglink(link, data)

    def test_dwarf_units_need_root_and_name(self):
        text = (
            "  Compilation Unit @ offset 0:\n <0><c>: Abbrev Number: 1 (DW_TAG_compile_unit)\n"
            "    <d> DW_AT_name : src/fixture.c\n"
            "    <e> DW_AT_comp_dir : /build/fixture\n <1><12>: Abbrev Number: 0\n"
        )
        self.assertEqual(
            evidence.units(text),
            [{"name": "src/fixture.c", "compilation_directory": "/build/fixture"}],
        )
        for malformed in (
            "",
            text.replace("DW_AT_name", "DW_AT_other"),
            text.replace("DW_TAG_compile_unit", "DW_TAG_skeleton_unit"),
            text + text.replace("DW_AT_name", "DW_AT_other"),
        ):
            with self.assertRaises(ValueError):
                evidence.units(malformed)


if __name__ == "__main__":
    unittest.main()
