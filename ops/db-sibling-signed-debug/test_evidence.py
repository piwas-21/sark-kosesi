#!/usr/bin/env python3
"""Small offline controls; no archive access or candidate execution."""

import importlib.util
import lzma
import struct
import subprocess
import tempfile
import traceback
import unittest
import zlib
from pathlib import Path
from unittest.mock import patch

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

    def test_index_stream_bounds_and_completion(self):
        large = self.full + "Package: unrelated\nDescription: " + "a" * 200000 + "\n\n"
        self.index.write_bytes(lzma.compress(large.encode()))
        self.assertEqual(len(evidence.records(self.index)), 2)
        valid = lzma.compress(self.full.encode())
        for data in (
            lzma.compress(b"x" * (1024 * 1024 + 1)),
            valid[:-1],
            valid + b"unexpected",
            valid + lzma.compress(b"\n"),
            lzma.compress(self.full.encode() + b"\xff\n"),
        ):
            self.index.write_bytes(data)
            with self.assertRaises((ValueError, UnicodeDecodeError, lzma.LZMAError)):
                evidence.records(self.index)

    def test_build_id_and_debuglink_tamper(self):
        ident = "50d75e545b53f25845679f2eccf50cf0ac3c475e"
        note = (
            "\nHex dump of section '.note.gnu.build-id':\n"
            "  0x000002a8 04000000 14000000 03000000 474e5500 ............GNU.\n"
            "  0x000002b8 50d75e54 5b53f258 45679f2e ccf50cf0 P.^T[S.XEg......\n"
            "  0x000002c8 ac3c475e                            .<G^\n\n"
        )
        evidence.build_id(note, ident)
        for value in (
            "",
            note.replace("50d75e54", "50d75e55"),
            note.replace("03000000", "01000000"),
            note.replace("474e5500", "46444f00"),
            note.replace("0x000002b8", "0x000002b9"),
            note.replace("ac3c475e", "ac3c475e ffffffff"),
            note + note,
        ):
            with self.assertRaises(ValueError):
                evidence.build_id(value, ident)
        payload = b"synthetic debug bytes"
        name = b"test.debug\0"
        section = name + b"\0" * ((-len(name)) % 4) + struct.pack("<I", zlib.crc32(payload))
        self.assertEqual(evidence.debuglink(section, payload), "test.debug")
        self.assertFalse(evidence.has_debuglink("  [ 1] .text PROGBITS\n"))
        self.assertTrue(evidence.has_debuglink("  [ 2] .gnu_debuglink PROGBITS\n"))
        with self.assertRaises(ValueError):
            evidence.has_debuglink(" [ 2] .gnu_debuglink PROGBITS\n [ 3] .gnu_debuglink PROGBITS\n")
        for link, data in (
            (b"", payload),
            (section, b"other"),
            (section[:-1], payload),
            (b"../evil\0\0" + b"\0" * 4, payload),
        ):
            with self.assertRaises(ValueError):
                evidence.debuglink(link, data)

    def test_dwarf_units_need_root_and_name(self):
        text = (
            "  Compilation Unit @ offset 0:\n   Version: 5\n   Unit Type: DW_UT_compile (1)\n"
            " <0><c>: Abbrev Number: 1 (DW_TAG_compile_unit)\n"
            "    <d> DW_AT_name : src/fixture.c\n"
            "    <e> DW_AT_comp_dir : /build/fixture\n <1><12>: Abbrev Number: 0\n"
        )
        self.assertEqual(
            evidence.units(text),
            [
                {
                    "kind": "compile_unit",
                    "name_rendered": "src/fixture.c",
                    "compilation_directory_rendered": "/build/fixture",
                }
            ],
        )
        rendered = text.replace(
            "src/fixture.c", "(indirect line string, offset: 0x20): src/fixture.c"
        ).replace("/build/fixture", "(line_strp) (offset: 0x40): /build/fixture")
        self.assertEqual(
            evidence.units(rendered),
            [
                {
                    "kind": "compile_unit",
                    "name_rendered": "(indirect line string, offset: 0x20): src/fixture.c",
                    "compilation_directory_rendered": "(line_strp) (offset: 0x40): /build/fixture",
                }
            ],
        )
        for malformed in (
            "",
            text.replace("DW_AT_name", "DW_AT_other"),
            text.replace("DW_TAG_compile_unit", "DW_TAG_skeleton_unit"),
            text.replace("Version: 5", "Version: unknown"),
            text.replace("DW_UT_compile", "DW_UT_unknown"),
            text + text.replace("DW_AT_name", "DW_AT_other"),
            text.replace("src/fixture.c", ""),
            text.replace("/build/fixture", ""),
            text.replace("src/fixture.c", "a" * 4097),
        ):
            with self.assertRaises(ValueError):
                evidence.units(malformed)

    def test_partial_units_are_distinct_and_require_named_compilation_units(self):
        # Shape from GNU readelf 2.47 on the authenticated #217 debug package.
        partial = (
            "  Compilation Unit @ offset 0:\n   Version: 5\n   Unit Type:     DW_UT_partial (3)\n"
            " <0><c>: Abbrev Number: 38 (DW_TAG_partial_unit)\n"
            "    <11>   DW_AT_comp_dir    : (line_strp) (offset: 0): ./obj-x86_64-linux-gnu\n"
            " <1><15>: Abbrev Number: 0\n"
        )
        compile_unit = (
            "  Compilation Unit @ offset 0x29:\n   Version: 5\n   Unit Type:     DW_UT_compile (1)\n"
            " <0><35>: Abbrev Number: 1 (DW_TAG_compile_unit)\n"
            "    <36> DW_AT_name : (line_strp) (offset: 0x20): src/fixture.c\n"
            " <1><3e>: Abbrev Number: 0\n"
        )
        entries = evidence.units(partial + compile_unit)
        self.assertEqual([entry["kind"] for entry in entries], ["partial_unit", "compile_unit"])
        self.assertIsNone(entries[0]["name_rendered"])
        self.assertEqual(entries[1]["name_rendered"], "(line_strp) (offset: 0x20): src/fixture.c")
        for invalid in (
            partial,
            partial.replace("DW_UT_partial", "DW_UT_compile") + compile_unit,
            partial.replace("DW_TAG_partial_unit", "DW_TAG_skeleton_unit") + compile_unit,
            partial + compile_unit.replace("DW_AT_name", "DW_AT_other"),
        ):
            with self.assertRaises(ValueError):
                evidence.units(invalid)

    def test_public_readelf_diagnostic_is_filtered_bounded_and_still_fails(self):
        error = b"readelf: Error: /home/runner/private/fixture.elf: malformed note\nother secret\n"

        def fake_run(command, **kwargs):
            kwargs["stdout"].write(b"some output\n")
            kwargs["stderr"].write(error)
            return subprocess.CompletedProcess(command, 1)

        with patch.object(evidence.subprocess, "run", side_effect=fake_run):
            with self.assertRaises(ValueError) as caught:
                evidence.run_to_file(["readelf", "-n", "/private/fixture.elf"], self.root / "note")
        message = str(caught.exception)
        self.assertIn("readelf: Error: <path>: malformed note", message)
        self.assertNotIn("/home/runner", message)
        self.assertNotIn("other secret", message)
        self.assertNotIn("secret", evidence.tool_diagnostic("dpkg-deb", 1, error))
        self.assertNotIn("malformed", evidence.tool_diagnostic("readelf", 1, error * 100))
        self.assertIn("exit 1", evidence.tool_diagnostic("readelf", 1, b""))

        def warning_run(command, **kwargs):
            kwargs["stdout"].write(b"some output\n")
            kwargs["stderr"].write(b"readelf: Warning: /private/file: bad note\n")
            return subprocess.CompletedProcess(command, 0)

        with patch.object(evidence.subprocess, "run", side_effect=warning_run):
            with self.assertRaisesRegex(ValueError, r"Warning: <path>: bad note"):
                evidence.run_to_file(["readelf", "-n", "/private/file"], self.root / "warning")

    def test_timeout_and_launch_errors_do_not_expose_exception_details(self):
        for suffix, command, error, expected in (
            (
                "timeout",
                ["readelf", "/private/SECRET_ARG"],
                subprocess.TimeoutExpired(
                    ["readelf", "/private/SECRET_ARG"],
                    90,
                    output=b"SECRET_STDOUT",
                    stderr=b"SECRET_STDERR",
                ),
                "ELF inspection timed out",
            ),
            (
                "launch",
                ["dpkg-deb", "/private/SECRET_ARG"],
                OSError(2, "SECRET_ERROR", "/private/SECRET_PATH"),
                "package extraction could not start",
            ),
            (
                "unknown",
                ["/private/SECRET_TOOL", "SECRET_ARG"],
                OSError(2, "SECRET_ERROR", "/private/SECRET_PATH"),
                "tool execution could not start",
            ),
        ):
            with (
                self.subTest(suffix=suffix),
                patch.object(evidence.subprocess, "run", side_effect=error),
            ):
                with self.assertRaises(ValueError) as caught:
                    evidence.run_to_file(command, self.root / suffix)
                self.assertEqual(str(caught.exception), expected)
                self.assertIsNone(caught.exception.__context__)
                rendered = "".join(traceback.format_exception(caught.exception))
                self.assertNotIn("SECRET", rendered)
                self.assertNotIn("/private/", rendered)


if __name__ == "__main__":
    unittest.main()
