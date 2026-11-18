#!/usr/bin/env python3
"""Verify every saved layer and inspect the candidate layer without extracting to disk."""

import gzip
import hashlib
import io
import json
import posixpath
import re
import sys
import tarfile
from pathlib import Path

from compare_db_zlib_scan import BASE_CONFIG as DEBIAN_CONFIG

STOCK_CONFIG = "sha256:0f6c51e48b3458e87ab43692e49c9dafc72ffe0a0dbde3fd8cdb6ab92d5c025e"
GOSU_SHA256 = "ad2e645f69d82e23e0b54a208828589ff4a021a343ad5090cd36f2a2ae7aa556"
LIMIT = 8 * 1024 * 1024
MAX_ARCHIVED_LAYER = 1024 * 1024 * 1024
MAX_UNCOMPRESSED_LAYER = 2 * 1024 * 1024 * 1024
CHUNK = 1024 * 1024


def digest(raw):
    return "sha256:" + hashlib.sha256(raw).hexdigest()


def find_member(tar, name, limit):
    matches = [item for item in tar.getmembers() if item.name == name]
    if len(matches) != 1:
        raise ValueError("ambiguous or missing archive member")
    item = matches[0]
    if not item.isfile() or not 0 < item.size <= limit:
        raise ValueError("invalid archive member")
    return item


def read_member(tar, name, limit=LIMIT):
    item = find_member(tar, name, limit)
    stream = tar.extractfile(item)
    if stream is None:
        raise ValueError("archive member unavailable")
    data = stream.read(limit + 1)
    if len(data) != item.size:
        raise ValueError("incomplete archive member")
    return data


def verify_layer(tar, name, expected, capture=False):
    if not isinstance(expected, str) or not re.fullmatch(r"sha256:[0-9a-f]{64}", expected):
        raise ValueError("invalid layer digest")
    item = find_member(tar, name, LIMIT if capture else MAX_ARCHIVED_LAYER)
    stream = tar.extractfile(item)
    if stream is None:
        raise ValueError("layer unavailable")
    magic = stream.read(2)
    stream.seek(0)
    reader = gzip.GzipFile(fileobj=stream) if magic == b"\x1f\x8b" else stream
    max_size = LIMIT if capture else MAX_UNCOMPRESSED_LAYER
    hasher = hashlib.sha256()
    total = 0
    captured = io.BytesIO() if capture else None
    while True:
        chunk = reader.read(CHUNK)
        if not chunk:
            break
        total += len(chunk)
        if total > max_size:
            raise ValueError("uncompressed layer too large")
        hasher.update(chunk)
        if captured is not None:
            captured.write(chunk)
    if total == 0 or "sha256:" + hasher.hexdigest() != expected:
        raise ValueError("saved layer digest mismatch")
    return captured.getvalue() if captured is not None else None


def inspect(archive_path, stock_raw, candidate_id, *, debian=False):
    stock_id = DEBIAN_CONFIG if debian else STOCK_CONFIG
    additions = 2 if debian else 1
    if digest(stock_raw) != stock_id or not re.fullmatch(r"sha256:[0-9a-f]{64}", candidate_id):
        raise ValueError("unexpected image identity")
    stock = json.loads(stock_raw)
    with tarfile.open(archive_path, mode="r:") as tar:
        manifest = json.loads(read_member(tar, "manifest.json", 1024 * 1024))
        if not isinstance(manifest, list) or len(manifest) != 1:
            raise ValueError("unexpected image archive manifest")
        record = manifest[0]
        config_raw = read_member(tar, record["Config"], 2 * 1024 * 1024)
        if digest(config_raw) != candidate_id:
            raise ValueError("candidate config digest mismatch")
        candidate = json.loads(config_raw)
        if any(
            config.get("os") != "linux" or config.get("architecture") != "amd64"
            for config in (stock, candidate)
        ):
            raise ValueError("unexpected image platform")
        if any(config.get("rootfs", {}).get("type") != "layers" for config in (stock, candidate)):
            raise ValueError("unexpected rootfs type")
        base_diff = stock["rootfs"]["diff_ids"]
        diff = candidate["rootfs"]["diff_ids"]
        if (
            not isinstance(base_diff, list)
            or diff[:-additions] != base_diff
            or len(diff) != len(base_diff) + additions
        ):
            raise ValueError("unexpected candidate layer lineage")
        if candidate.get("config") != stock.get("config") or not isinstance(
            stock.get("config"), dict
        ):
            raise ValueError("candidate runtime configuration changed")
        layers = record["Layers"]
        if not isinstance(layers, list) or len(layers) != len(diff):
            raise ValueError("image layer inventory mismatch")
        for name, expected in zip(layers[:-1], diff[:-1], strict=True):
            verify_layer(tar, name, expected)
        layer = verify_layer(tar, layers[-1], diff[-1], capture=True)
    if layer is None:
        raise ValueError("candidate layer unavailable")
    allowed_dirs = {".", "usr", "usr/local", "usr/local/bin"}
    seen = set()
    with tarfile.open(fileobj=io.BytesIO(layer), mode="r:") as tar:
        for item in tar:
            name = posixpath.normpath(item.name)
            if item.name.startswith("/") or name == ".." or name.startswith("../") or name in seen:
                raise ValueError("unsafe or duplicate layer entry")
            seen.add(name)
            if item.isdir() and name in allowed_dirs:
                continue
            if (
                name != "usr/local/bin/gosu"
                or not item.isfile()
                or item.uid != 0
                or item.gid != 0
                or item.mode & 0o7777 != 0o755
            ):
                raise ValueError("unexpected candidate layer entry")
            binary = tar.extractfile(item)
            if binary is None or not 0 < item.size <= LIMIT:
                raise ValueError("missing Gosu bytes")
            content = binary.read(LIMIT + 1)
            if len(content) != item.size or hashlib.sha256(content).hexdigest() != GOSU_SHA256:
                raise ValueError("candidate Gosu digest mismatch")
    if "usr/local/bin/gosu" not in seen:
        raise ValueError("candidate Gosu absent")
    return config_raw, {
        "stock_config": stock_id,
        "candidate_config": candidate_id,
        "verified_stock_layers": len(base_diff),
        "last_diff_id": diff[-1],
        "gosu_sha256": GOSU_SHA256,
        "meaning": "Final layer replaces only Gosu. Package provenance and security acceptance require separate evidence.",
    }


def main():
    args = sys.argv[1:]
    debian = args[:1] == ["--debian"]
    if debian:
        args = args[1:]
    if len(args) != 4:
        raise ValueError("expected archive, stock config, candidate ID and evidence directory")
    archive, stock_path, candidate_id, output = args
    raw, summary = inspect(archive, Path(stock_path).read_bytes(), candidate_id, debian=debian)
    directory = Path(output)
    (directory / "candidate-config.json").write_bytes(raw)
    (directory / "candidate-layer-check.json").write_text(json.dumps(summary, indent=2) + "\n")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError, tarfile.TarError, EOFError) as error:
        print(f"candidate image verification failed: {type(error).__name__}", file=sys.stderr)
        sys.exit(1)
