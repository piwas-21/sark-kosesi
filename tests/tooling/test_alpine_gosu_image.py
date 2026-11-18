"""Synthetic controls for one-layer candidate custody; no image execution."""

import gzip
import hashlib
import io
import json
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import check_alpine_gosu_image as check


def sha(data):
    return "sha256:" + hashlib.sha256(data).hexdigest()


def archive(entries):
    result = io.BytesIO()
    with tarfile.open(fileobj=result, mode="w") as tar:
        for name, data, mode, uid, kind in entries:
            item = tarfile.TarInfo(name)
            item.type = kind
            item.mode = mode
            item.uid = uid
            item.gid = 0
            item.size = len(data) if kind == tarfile.REGTYPE else 0
            tar.addfile(item, io.BytesIO(data) if item.isfile() else None)
    return result.getvalue()


class CandidateImageTests(unittest.TestCase):
    def setUp(self):
        self.binary = b"synthetic-gosu-bytes"
        self.base_layers = [
            archive([("usr/base-file", b"synthetic base", 0o644, 0, tarfile.REGTYPE)]),
            archive([("etc/base-config", b"second layer", 0o644, 0, tarfile.REGTYPE)]),
        ]
        self.stock = {
            "os": "linux",
            "architecture": "amd64",
            "config": {"Env": ["PATH=/usr/local/bin"], "Entrypoint": ["entry"]},
            "rootfs": {"type": "layers", "diff_ids": [sha(layer) for layer in self.base_layers]},
        }
        self.entries = [("usr/local/bin/gosu", self.binary, 0o755, 0, tarfile.REGTYPE)]

    def evaluate(
        self,
        entries=None,
        changed_config=False,
        wrong_diff=False,
        wrong_candidate=False,
        compressed=False,
        base_compressed=False,
        omit_base=False,
        corrupt_base=False,
        malformed_gzip=False,
        nonregular_base=False,
        duplicate_base=False,
        debian=False,
        zlib_layer=False,
        corrupt_zlib=False,
    ):
        layer = archive(self.entries if entries is None else entries)
        candidate = json.loads(json.dumps(self.stock))
        apt_layer = archive([("usr/lib/zlib", b"synthetic package", 0o644, 0, tarfile.REGTYPE)])
        layers = ["base-0.tar", "base-1.tar"]
        if zlib_layer:
            layers.append("zlib.tar")
            candidate["rootfs"]["diff_ids"].append(sha(apt_layer))
        layers.append("new.tar")
        candidate["rootfs"]["diff_ids"].append("sha256:" + "b" * 64 if wrong_diff else sha(layer))
        if changed_config:
            candidate["config"]["Entrypoint"] = ["other"]
        candidate_raw = json.dumps(candidate).encode()
        stock_raw = json.dumps(self.stock).encode()
        candidate_id = "sha256:" + "c" * 64 if wrong_candidate else sha(candidate_raw)
        manifest = json.dumps([{"Config": "config.json", "Layers": layers}]).encode()
        outer_entries = [
            ("manifest.json", manifest, 0o644, 0, tarfile.REGTYPE),
            ("config.json", candidate_raw, 0o644, 0, tarfile.REGTYPE),
        ]
        for index, raw_base in enumerate(self.base_layers):
            if omit_base and index == 1:
                continue
            base = gzip.compress(raw_base) if base_compressed else raw_base
            if corrupt_base and index == 1:
                base = b"changed base layer"
            if malformed_gzip and index == 1:
                base = b"\x1f\x8bnot-a-valid-gzip-stream"
            kind = tarfile.SYMTYPE if nonregular_base and index == 1 else tarfile.REGTYPE
            outer_entries.append((f"base-{index}.tar", base, 0o644, 0, kind))
            if duplicate_base and index == 1:
                outer_entries.append((f"base-{index}.tar", base, 0o644, 0, kind))
        outer_entries.append(("base.tar", base, 0o644, 0, tarfile.REGTYPE))
        if zlib_layer:
            apt = b"changed package layer" if corrupt_zlib else apt_layer
            outer_entries.append(("zlib.tar", apt, 0o644, 0, tarfile.REGTYPE))
        outer_entries.append(
            ("new.tar", gzip.compress(layer) if compressed else layer, 0o644, 0, tarfile.REGTYPE)
        )
        data = archive(outer_entries)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "saved.tar"
            path.write_bytes(data)
            with (
                patch.object(check, "STOCK_CONFIG", sha(stock_raw)),
                patch.object(check, "DEBIAN_CONFIG", sha(stock_raw)),
                patch.object(check, "GOSU_SHA256", hashlib.sha256(self.binary).hexdigest()),
            ):
                return check.inspect(path, stock_raw, candidate_id, debian=debian)

    def test_debian_requires_an_exact_extra_package_layer(self):
        _, result = self.evaluate(debian=True, zlib_layer=True)
        self.assertEqual(result["verified_stock_layers"], 2)
        for args in (
            {"debian": True},
            {"debian": True, "zlib_layer": True, "corrupt_zlib": True},
            {"zlib_layer": True},
        ):
            with self.subTest(args=args), self.assertRaises(ValueError):
                self.evaluate(**args)

    def test_exact_single_gosu_layer_is_accepted(self):
        raw, result = self.evaluate()
        self.assertEqual(json.loads(raw)["config"]["Entrypoint"], ["entry"])
        self.assertIn("only Gosu", result["meaning"])
        _, compressed_result = self.evaluate(compressed=True, base_compressed=True)
        self.assertEqual(result["last_diff_id"], compressed_result["last_diff_id"])
        self.assertEqual(result["verified_stock_layers"], 2)

    def test_missing_corrupt_or_malformed_stock_layer_is_rejected(self):
        for args in (
            {"omit_base": True},
            {"corrupt_base": True},
            {"malformed_gzip": True},
            {"nonregular_base": True},
            {"duplicate_base": True},
        ):
            with self.subTest(args=args), self.assertRaises((ValueError, OSError, EOFError)):
                self.evaluate(**args)

    def test_layer_size_bounds_reject_raw_and_gzip_expansion(self):
        with patch.object(check, "MAX_ARCHIVED_LAYER", 1024), self.assertRaises(ValueError):
            self.evaluate()
        with patch.object(check, "MAX_UNCOMPRESSED_LAYER", 1024), self.assertRaises(ValueError):
            self.evaluate(base_compressed=True)

    def test_changed_config_or_unbound_layer_is_rejected(self):
        for args in ({"changed_config": True}, {"wrong_diff": True}, {"wrong_candidate": True}):
            with self.subTest(args=args), self.assertRaises(ValueError):
                self.evaluate(**args)

    def test_unexpected_files_links_ownership_and_mode_are_rejected(self):
        for entries in (
            self.entries + [("etc/passwd", b"x", 0o644, 0, tarfile.REGTYPE)],
            [("usr/local/bin/gosu", b"wrong", 0o755, 0, tarfile.REGTYPE)],
            [("usr/local/bin/gosu", self.binary, 0o755, 99, tarfile.REGTYPE)],
            [("usr/local/bin/gosu", self.binary, 0o4755, 0, tarfile.REGTYPE)],
            [("usr/local/bin/gosu", b"", 0o755, 0, tarfile.SYMTYPE)],
            self.entries + self.entries,
        ):
            with self.subTest(entries=entries), self.assertRaises(ValueError):
                self.evaluate(entries=entries)
