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
        self.stock = {
            "os": "linux",
            "architecture": "amd64",
            "config": {"Env": ["PATH=/usr/local/bin"], "Entrypoint": ["entry"]},
            "rootfs": {"type": "layers", "diff_ids": ["sha256:" + "a" * 64]},
        }
        self.entries = [("usr/local/bin/gosu", self.binary, 0o755, 0, tarfile.REGTYPE)]

    def evaluate(
        self,
        entries=None,
        changed_config=False,
        wrong_diff=False,
        wrong_candidate=False,
        compressed=False,
    ):
        layer = archive(self.entries if entries is None else entries)
        candidate = json.loads(json.dumps(self.stock))
        candidate["rootfs"]["diff_ids"].append("sha256:" + "b" * 64 if wrong_diff else sha(layer))
        if changed_config:
            candidate["config"]["Entrypoint"] = ["other"]
        candidate_raw = json.dumps(candidate).encode()
        stock_raw = json.dumps(self.stock).encode()
        candidate_id = "sha256:" + "c" * 64 if wrong_candidate else sha(candidate_raw)
        manifest = json.dumps(
            [{"Config": "config.json", "Layers": ["base.tar", "new.tar"]}]
        ).encode()
        data = archive(
            [
                ("manifest.json", manifest, 0o644, 0, tarfile.REGTYPE),
                ("config.json", candidate_raw, 0o644, 0, tarfile.REGTYPE),
                (
                    "new.tar",
                    gzip.compress(layer) if compressed else layer,
                    0o644,
                    0,
                    tarfile.REGTYPE,
                ),
            ]
        )
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "saved.tar"
            path.write_bytes(data)
            with (
                patch.object(check, "STOCK_CONFIG", sha(stock_raw)),
                patch.object(check, "GOSU_SHA256", hashlib.sha256(self.binary).hexdigest()),
            ):
                return check.inspect(path, stock_raw, candidate_id)

    def test_exact_single_gosu_layer_is_accepted(self):
        raw, result = self.evaluate()
        self.assertEqual(json.loads(raw)["config"]["Entrypoint"], ["entry"])
        self.assertIn("only Gosu", result["meaning"])
        _, compressed_result = self.evaluate(compressed=True)
        self.assertEqual(result["last_diff_id"], compressed_result["last_diff_id"])

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
