#!/usr/bin/env python3
"""Validate the measured composition, without asserting runtime parity."""

import importlib.util
import json
import re
import sys
from pathlib import Path


def load_root_parser():
    path = Path("ops/db-sibling-runtime-root/validate.py")
    spec = importlib.util.spec_from_file_location("sibling_root_validate", path)
    if spec is None or spec.loader is None:
        raise ValueError("sibling root validator unavailable")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.parse_live_packages


parse_live_packages = load_root_parser()

READLINE = {"libreadline8t64": "8.2-6", "readline-common": "8.2-6"}
FORBIDDEN = re.compile(r"lib(?:xml2|xslt|gssapi|krb5|ldap|lber|curl)[^\s/]*[.]", re.I)


def check_image(sibling, candidate):
    if any(
        image.get("Os") != "linux" or image.get("Architecture") != "amd64"
        for image in (sibling, candidate)
    ):
        raise ValueError("invalid image platform")
    base_layers = sibling.get("RootFS", {}).get("Layers", [])
    result_layers = candidate.get("RootFS", {}).get("Layers", [])
    if (
        not base_layers
        or result_layers[: len(base_layers)] != base_layers
        or len(result_layers) <= len(base_layers)
    ):
        raise ValueError("composed image does not extend the verified sibling root")
    if "LANG=en_US.utf8" not in candidate.get("Config", {}).get("Env", []):
        raise ValueError("candidate locale configuration changed")


def check_packages(sibling, candidate):
    if any(name in sibling for name in READLINE):
        raise ValueError("readline addition unexpectedly present")
    if candidate != {**sibling, **READLINE}:
        raise ValueError("unexpected installed package change")
    if sibling.get("libtinfo6") != "6.5+20250216-2":
        raise ValueError("unexpected terminal library version")


def check_linkage(text):
    if (
        not text.startswith("FILE /opt/pg-smoke/")
        or "not found" in text.lower()
        or FORBIDDEN.search(text)
    ):
        raise ValueError("missing or prohibited linkage")
    blocks = re.split(r"(?m)^FILE ", text)
    entries = {}
    for block in blocks[1:]:
        path, sep, body = block.partition("\n")
        if not sep or not path.startswith("/opt/pg-smoke/") or path in entries or not body.strip():
            raise ValueError("invalid executable linkage inventory")
        entries[path] = body
    required = {
        "/opt/pg-smoke/bin/postgres",
        "/opt/pg-smoke/bin/psql",
        "/opt/pg-smoke/lib/postgresql/plpgsql.so",
    }
    if not required <= entries.keys() or not any(
        re.fullmatch(r"/opt/pg-smoke/lib/libpq[.]so[.][0-9.]+", path) for path in entries
    ):
        raise ValueError("required binary or module missing")
    if (
        "libreadline.so.8" not in entries["/opt/pg-smoke/bin/psql"]
        or "libtinfo.so.6" not in entries["/opt/pg-smoke/bin/psql"]
    ):
        raise ValueError("psql readline closure is missing")
    for library in ("libicuuc.so.76", "libssl.so.3", "libcrypto.so.3", "libz.so.1"):
        if library not in entries["/opt/pg-smoke/bin/postgres"]:
            raise ValueError("postgres runtime dependency is missing")


def check_loaded(text):
    if FORBIDDEN.search(text):
        raise ValueError("prohibited library loaded in the sampled processes")
    server, marker, client = text.partition("PROCESS psql\n")
    if not marker or not server.startswith("PROCESS postgres\n"):
        raise ValueError("incomplete process library evidence")
    for library in ("libicuuc.so.76", "libssl.so.3", "libcrypto.so.3", "libz.so.1"):
        if library not in server:
            raise ValueError("postgres library is absent from process maps")
    for library in ("libreadline.so.8", "libtinfo.so.6"):
        if library not in client:
            raise ValueError("psql library is absent from process maps")


def check_tree(source_files, candidate_files, source_metadata, candidate_metadata):
    if not source_files or source_files != candidate_files:
        raise ValueError("installed file hashes differ")
    if not source_metadata or source_metadata != candidate_metadata:
        raise ValueError("installed ownership, modes or symlinks differ")
    paths = set()
    for line in source_files.splitlines():
        match = re.fullmatch(r"[0-9a-f]{64}  (/opt/pg-smoke/[^\n]+)", line)
        if match is None or match.group(1) in paths:
            raise ValueError("invalid file hash inventory")
        paths.add(match.group(1))
    if "/opt/pg-smoke/bin/postgres" not in paths or "/opt/pg-smoke/bin/psql" not in paths:
        raise ValueError("required installed binaries absent")
    for line in source_metadata.splitlines():
        if not re.match(r"^[0-7]{3,4} 0 0 /opt/pg-smoke(?:/|[ ]|$)", line):
            raise ValueError("invalid installed file metadata")


def main(directory):
    root = Path(directory)

    def read(name):
        return (root / name).read_text(encoding="utf-8")

    images = []
    for name in ("sibling-image.json", "composed-image.json"):
        value = json.loads(read(name))
        if not isinstance(value, list) or len(value) != 1:
            raise ValueError("invalid image inspection")
        images.append(value[0])
    check_image(*images)
    check_packages(
        parse_live_packages(read("sibling-packages.txt")),
        parse_live_packages(read("composed-packages.txt")),
    )
    check_tree(
        read("source-files.txt"),
        read("composed-files.txt"),
        read("source-metadata.txt"),
        read("composed-metadata.txt"),
    )
    check_linkage(read("composed-linkage.txt"))
    check_loaded(read("loaded-libraries.txt"))
    print("package, install tree and measured ELF linkage controls passed")


if __name__ == "__main__":
    main(sys.argv[1])
