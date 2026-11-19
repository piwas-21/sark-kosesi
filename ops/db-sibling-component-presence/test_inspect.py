#!/usr/bin/env python3
"""Positive and failure controls for static filesystem inspection."""

import importlib.util
import io
import tarfile
import tempfile
import unittest
from copy import deepcopy
from pathlib import Path
from unittest.mock import MagicMock, patch

spec = importlib.util.spec_from_file_location(
    "inspection", Path(__file__).with_name("inspect_rootfs.py")
)
inspection = importlib.util.module_from_spec(spec)
spec.loader.exec_module(inspection)
IMAGE = "sha256:" + "a" * 64
REVISION = "b" * 40


def metadata():
    return [
        {
            "Id": "c" * 64,
            "Image": IMAGE,
            "Mounts": [],
            "Config": {"Volumes": None},
            "HostConfig": {"NetworkMode": "none", "PortBindings": None},
            "State": {"Status": "created", "Running": False, "Pid": 0},
        }
    ]


def make_archive(path):
    with tarfile.open(path, "w") as tar:
        for name in (
            "usr/lib/systemd/systemd-homed",
            "usr/share/perl5/Archive/Tar.pm",
            "etc/harmless",
        ):
            data = b"synthetic"
            info = tarfile.TarInfo(name)
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
        info = tarfile.TarInfo("usr/bin/alias")
        info.type = tarfile.SYMTYPE
        info.linkname = "../lib/systemd/systemd-homework"
        tar.addfile(info)
        info = tarfile.TarInfo("usr/bin/perl-alias")
        info.type = tarfile.SYMTYPE
        info.linkname = "../share/perl5/Archive/Tar.pm"
        tar.addfile(info)


class InspectionControls(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name) / "rootfs.tar"
        make_archive(self.path)
        self.before = metadata()
        self.after = deepcopy(self.before)

    def inspect(self):
        return inspection.inspect(self.path, IMAGE, REVISION, self.before, self.after)

    def test_positive_files_and_link_targets(self):
        report = self.inspect()
        self.assertEqual(report["image_id"], IMAGE)
        self.assertTrue(
            any(x["path"].endswith("systemd-homed") for x in report["matches"]["systemd_home"])
        )
        self.assertTrue(
            any(
                x["match"] == "resolved_target" and x["path"] == "usr/bin/alias"
                for x in report["matches"]["systemd_home"]
            )
        )
        self.assertTrue(
            any(x["path"].endswith("Archive/Tar.pm") for x in report["matches"]["archive_tar"])
        )
        self.assertTrue(
            any(
                x["match"] == "resolved_target" and x["path"] == "usr/bin/perl-alias"
                for x in report["matches"]["archive_tar"]
            )
        )

    def test_clean_synthetic_file_has_no_name_matches(self):
        with tarfile.open(self.path, "w") as tar:
            info = tarfile.TarInfo("usr/share/unrelated.txt")
            data = b"synthetic"
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
        result = self.inspect()
        self.assertEqual(result["matches"], {"systemd_home": [], "archive_tar": []})

    def test_regular_size_accounting_and_sparse_metadata_fail(self):
        member = tarfile.TarInfo("regular")
        member.size = 7
        member.offset_data = 512
        self.assertEqual(inspection.validate_member(member, 4, 4096), 11)
        for kind in (
            "gnu_type",
            "unsupported_type",
            "sparse_map",
            "pax_sparse",
            "pax_realsize",
            "pax_filetype",
        ):
            with self.subTest(kind=kind):
                fake = tarfile.TarInfo("synthetic")
                fake.size = 7
                fake.offset_data = 512
                if kind == "gnu_type":
                    fake.type = tarfile.GNUTYPE_SPARSE
                elif kind == "unsupported_type":
                    fake.type = b"Z"
                elif kind == "sparse_map":
                    fake.sparse = [(0, 1)]
                elif kind == "pax_sparse":
                    fake.pax_headers = {"GNU.sparse.size": "7"}
                elif kind == "pax_realsize":
                    fake.pax_headers = {"SCHILY.realsize": "7"}
                else:
                    fake.pax_headers = {"SCHILY.filetype": "sparse"}
                with self.assertRaises(ValueError):
                    inspection.validate_member(fake, 0, 4096)

    def test_member_and_cumulative_bounds_before_content_read(self):
        member = tarfile.TarInfo("synthetic")
        member.size = inspection.MAX_MEMBER_SIZE + 1
        member.offset_data = 512
        fake_archive = MagicMock()
        fake_archive.__enter__.return_value = fake_archive
        fake_archive.__iter__.return_value = iter([member])
        with patch.object(inspection.tarfile, "open", return_value=fake_archive):
            with self.assertRaises(ValueError):
                self.inspect()
        fake_archive.extractfile.assert_not_called()
        member.size = 7
        with self.assertRaises(ValueError):
            inspection.validate_member(member, inspection.MAX_LOGICAL_SIZE - 6, 4096)
        with self.assertRaises(ValueError):
            inspection.validate_member(member, 0, 1024)

    def test_sparse_and_large_metadata_rejected_before_parser_reads(self):
        member = inspection.BoundedTarInfo("synthetic")
        archive = MagicMock()
        for method, args in (
            (member._proc_sparse, (archive,)),
            (member._proc_gnusparse_00, (None, {})),
            (member._proc_gnusparse_01, (None, {})),
            (member._proc_gnusparse_10, (None, {}, archive)),
        ):
            with self.subTest(method=method.__name__), self.assertRaises(ValueError):
                method(*args)
        member.size = inspection.MAX_METADATA_SIZE + 1
        for method in (member._proc_pax, member._proc_gnulong):
            with self.subTest(method=method.__name__), self.assertRaises(ValueError):
                method(archive)
        archive.fileobj.read.assert_not_called()

    def test_truncated_and_malformed_fail(self):
        raw = self.path.read_bytes()
        with tarfile.open(self.path, "r:") as archive:
            list(archive)
            end = archive.offset
        for value in (
            raw[:512],
            raw[:1024],
            raw[:2048],
            b"x" * 10240,
            raw[:-1],
            raw + b"x" * 512,
            raw[:end] + b"\0" * 512,
            raw[:end] + b"\0" * 512 + b"x" + b"\0" * 511,
        ):
            self.path.write_bytes(value)
            with self.subTest(size=len(value)), self.assertRaises((ValueError, tarfile.TarError)):
                self.inspect()

    def test_unbound_mounted_and_running_fail(self):
        for field, bad in (
            ("Image", "sha256:" + "d" * 64),
            ("Mounts", [{"Type": "volume"}]),
            ("Id", "d" * 64),
        ):
            with self.subTest(field=field):
                self.after = deepcopy(self.before)
                self.after[0][field] = bad
                with self.assertRaises(ValueError):
                    self.inspect()
        self.after = deepcopy(self.before)
        self.after[0]["State"]["Running"] = True
        with self.assertRaises(ValueError):
            self.inspect()
        with self.assertRaises(ValueError):
            inspection.inspect(self.path, IMAGE, "wrong", self.before, self.before)


if __name__ == "__main__":
    unittest.main()
