#!/usr/bin/env python3
"""Verify an owned Docker save without unpacking its layers on the runner."""

import json
import re
import sys
import tarfile
from pathlib import Path

from check_alpine_gosu_image import digest, read_member, verify_layer
from check_db_alpine_gosu import unique_object


def verify_archive(archive, image_id):
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", image_id):
        raise ValueError("invalid image identity")
    with tarfile.open(archive, "r:") as tar:
        manifest = json.loads(
            read_member(tar, "manifest.json", 1024 * 1024), object_pairs_hook=unique_object
        )
        if (
            not isinstance(manifest, list)
            or len(manifest) != 1
            or not isinstance(manifest[0], dict)
        ):
            raise ValueError("invalid image archive manifest")
        record = manifest[0]
        config_raw = read_member(tar, record["Config"], 2 * 1024 * 1024)
        if digest(config_raw) != image_id:
            raise ValueError("image config digest mismatch")
        config = json.loads(config_raw, object_pairs_hook=unique_object)
        if config.get("os") != "linux" or config.get("architecture") != "amd64":
            raise ValueError("image platform mismatch")
        rootfs = config.get("rootfs", {})
        layers, diffs = record.get("Layers"), rootfs.get("diff_ids")
        if (
            rootfs.get("type") != "layers"
            or not isinstance(layers, list)
            or not isinstance(diffs, list)
            or not 0 < len(layers) == len(diffs) <= 128
            or len(set(layers)) != len(layers)
        ):
            raise ValueError("invalid archive layer inventory")
        for name, expected in zip(layers, diffs, strict=True):
            verify_layer(tar, name, expected)
    return config_raw


def main():
    archive, image_id, destination = sys.argv[1:]
    Path(destination).write_bytes(verify_archive(archive, image_id))


if __name__ == "__main__":
    main()
