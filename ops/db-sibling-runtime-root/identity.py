#!/usr/bin/env python3
"""Verify and retain the upstream OCI identities for the sibling experiment."""

import json
import sys
from pathlib import Path

from capture_db_image_scan import capture_registry_identity

ROOT_INDEX = "sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2"
ROOT_MANIFEST = "sha256:7378260c0a098d27ac206225db5e7d92b8c7c84ea7afccc86eeb513756bbd392"
ROOT_CONFIG = "sha256:596c6d80c879dc94eae22cb9c59a5e18f66f4f9722e170892324c282b4eb19bc"
STOCK_INDEX = "sha256:9f4fd4aa918f52ae3d6f3b687f1de90d8f867cd4d520021545a9a50c120eae3b"
STOCK_MANIFEST = "sha256:c7fedce47eff6a78cb35dad6b627f0d684107f75ab15c0870e40c093766ba452"
STOCK_CONFIG = "sha256:3cccc725163a95947178f4da5e9bf16add75cb286170ac5a2cd2f6513534955d"
SHARED_LAYER = "sha256:37fb86c2a767b88d500c6872195f88078f64765fe0d2f9cc375cc2efbd4c0a41"


def validate_manifests(root, stock):
    if (
        root.get("config", {}).get("digest") != ROOT_CONFIG
        or stock.get("config", {}).get("digest") != STOCK_CONFIG
    ):
        raise ValueError("unexpected image configuration identity")
    if len(root.get("layers", [])) != 1 or not stock.get("layers"):
        raise ValueError("unexpected root layer structure")
    if (
        root["layers"][0].get("digest") != SHARED_LAYER
        or stock["layers"][0].get("digest") != SHARED_LAYER
    ):
        raise ValueError("the source images do not share the pinned first layer")


def main(directory):
    output = Path(directory)
    capture_registry_identity(output, "library/debian", ROOT_INDEX, ROOT_MANIFEST, "root-")
    capture_registry_identity(output, "library/postgres", STOCK_INDEX, STOCK_MANIFEST, "stock-")
    root = json.loads((output / "root-manifest.json").read_text())
    stock = json.loads((output / "stock-manifest.json").read_text())
    validate_manifests(root, stock)
    print("registry digests, platforms and shared first layer verified")


if __name__ == "__main__":
    main(sys.argv[1])
