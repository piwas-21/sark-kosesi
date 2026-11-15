#!/usr/bin/env python3
"""Inspect the saved candidate's config and last layer without extracting to disk."""

import gzip
import hashlib
import io
import json
import posixpath
import re
import sys
import tarfile
from pathlib import Path

STOCK_CONFIG = "sha256:0f6c51e48b3458e87ab43692e49c9dafc72ffe0a0dbde3fd8cdb6ab92d5c025e"
GOSU_SHA256 = "8177d9590273ed8699065bf0c9776b634fde24bfc75375c83e927134ce868b7a"
LIMIT = 8 * 1024 * 1024


def digest(raw):
    return "sha256:" + hashlib.sha256(raw).hexdigest()


def read_member(tar, name, limit=LIMIT):
    matches = [item for item in tar.getmembers() if item.name == name]
    if len(matches) != 1:
        raise ValueError("ambiguous or missing archive member")
    item = matches[0]
    if not item.isfile() or not 0 < item.size <= limit:
        raise ValueError("invalid archive member")
    stream = tar.extractfile(item)
    if stream is None:
        raise ValueError("archive member unavailable")
    data = stream.read(limit + 1)
    if len(data) != item.size:
        raise ValueError("incomplete archive member")
    return data


def inspect(archive_path, stock_raw, candidate_id):
    if digest(stock_raw) != STOCK_CONFIG or not re.fullmatch(r"sha256:[0-9a-f]{64}", candidate_id):
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
            or diff[:-1] != base_diff
            or len(diff) != len(base_diff) + 1
        ):
            raise ValueError("candidate is not one layer above stock")
        if candidate.get("config") != stock.get("config") or not isinstance(
            stock.get("config"), dict
        ):
            raise ValueError("candidate runtime configuration changed")
        layers = record["Layers"]
        if not isinstance(layers, list) or len(layers) != len(diff):
            raise ValueError("image layer inventory mismatch")
        layer = read_member(tar, layers[-1])
    if layer.startswith(b"\x1f\x8b"):
        with gzip.GzipFile(fileobj=io.BytesIO(layer)) as zipped:
            layer = zipped.read(LIMIT + 1)
    if len(layer) > LIMIT or digest(layer) != diff[-1]:
        raise ValueError("candidate layer digest mismatch")
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
        "stock_config": STOCK_CONFIG,
        "candidate_config": candidate_id,
        "last_diff_id": diff[-1],
        "gosu_sha256": GOSU_SHA256,
        "meaning": "one verified layer replaces only Gosu; image provenance and security acceptance remain separate",
    }


def main():
    if len(sys.argv) != 5:
        raise ValueError("expected archive, stock config, candidate ID and evidence directory")
    archive, stock_path, candidate_id, output = sys.argv[1:]
    raw, summary = inspect(archive, Path(stock_path).read_bytes(), candidate_id)
    directory = Path(output)
    (directory / "candidate-config.json").write_bytes(raw)
    (directory / "candidate-layer-check.json").write_text(json.dumps(summary, indent=2) + "\n")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError, tarfile.TarError, EOFError) as error:
        print(f"candidate image verification failed: {type(error).__name__}", file=sys.stderr)
        sys.exit(1)
