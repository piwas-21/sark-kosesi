#!/usr/bin/env python3
"""Check the disposable sibling root's package and loader evidence."""

import json
import re
import sys
from pathlib import Path

ADDITIONS = {
    "libicu76": "76.1-4",
    "libc-l10n": "2.41-12+deb13u4",
    "locales": "2.41-12+deb13u4",
}
PRESERVED = {
    "libc6": "2.41-12+deb13u4",
    "libssl3t64": "3.5.7-1~deb13u3",
    "zlib1g": "1:1.3.dfsg+really1.3.1-1+b1",
    "util-linux": "2.41.5-0+deb13u1",
    "libmount1": "2.41.5-0+deb13u1",
    "bsdutils": "1:2.41.5-0+deb13u1",
}
STOCK_ONLY = ("libxml2", "libgssapi-krb5-2", "libkrb5-3", "libldap2")


def parse_packages(text):
    result = {}
    for line in text.splitlines():
        fields = line.split("\t")
        if len(fields) != 2 or not all(fields):
            raise ValueError("invalid package inventory")
        name = fields[0].split(":", 1)[0]
        if name in result:
            raise ValueError("duplicate package")
        result[name] = fields[1]
    if not result:
        raise ValueError("empty package inventory")
    return result


def parse_libraries(text):
    if "not found" in text:
        raise ValueError("unresolved library")
    libraries = set(re.findall(r"^\s*(\S+)\s+\([^\n]+\)\s+=>\s+(/\S+)$", text, re.M))
    if not libraries:
        raise ValueError("empty loader cache")
    return {name for name, _ in libraries}


def validate_packages(reference, base, stock, candidate):
    if base != reference:
        raise ValueError("root package inventory differs from upstream")
    if any(name in base for name in ADDITIONS):
        raise ValueError("runtime addition unexpectedly present in base")
    if candidate != {**base, **ADDITIONS}:
        raise ValueError("candidate package closure changed unexpectedly")
    for name, version in {**PRESERVED, **ADDITIONS}.items():
        if candidate.get(name) != version or stock.get(name) != version:
            raise ValueError("stock or candidate package version mismatch")
    for name in STOCK_ONLY:
        if name not in stock or name in base or name in candidate:
            raise ValueError("stock-only package control failed")


def validate_libraries(base, stock, candidate):
    essential = {"libc.so.6", "libssl.so.3", "libcrypto.so.3", "libz.so.1", "libzstd.so.1"}
    if not essential <= base or not essential <= stock or not essential <= candidate:
        raise ValueError("required loader entries absent")
    icu = {"libicuuc.so.76", "libicui18n.so.76", "libicudata.so.76"}
    if base & icu or not icu <= candidate or not icu <= stock:
        raise ValueError("ICU loader control failed")
    for library in ("libxml2.so.2", "libgssapi_krb5.so.2", "libldap.so.2"):
        if library not in stock or library in base or library in candidate:
            raise ValueError("stock-only loader control failed")


def validate_image_metadata(base, stock, candidate):
    for image in (base, stock, candidate):
        if image.get("Os") != "linux" or image.get("Architecture") != "amd64":
            raise ValueError("image platform mismatch")
    layers = base.get("RootFS", {}).get("Layers", [])
    if len(layers) != 1:
        raise ValueError("unexpected base filesystem")
    if stock.get("RootFS", {}).get("Layers", [])[:1] != layers:
        raise ValueError("stock filesystem root does not match")
    candidate_layers = candidate.get("RootFS", {}).get("Layers", [])
    if candidate_layers[:1] != layers or len(candidate_layers) <= 1:
        raise ValueError("candidate filesystem root does not match")
    if "LANG=en_US.utf8" not in candidate.get("Config", {}).get("Env", []):
        raise ValueError("candidate locale configuration mismatch")


def main(directory):
    folder = Path(directory)

    def read(name):
        return (folder / name).read_text(encoding="utf-8")

    images = []
    for profile in ("base", "stock", "candidate"):
        value = json.loads(read(f"{profile}-image.json"))
        if not isinstance(value, list) or len(value) != 1:
            raise ValueError("invalid docker image metadata")
        images.append(value[0])
    validate_image_metadata(*images)
    validate_packages(
        *(
            parse_packages(read(name))
            for name in (
                "upstream-packages.txt",
                "base-packages.txt",
                "stock-packages.txt",
                "candidate-packages.txt",
            )
        )
    )
    validate_libraries(
        *(
            parse_libraries(read(name))
            for name in ("base-loader.txt", "stock-loader.txt", "candidate-loader.txt")
        )
    )
    if "en_US.utf8\n" not in read("candidate-locale.txt") or read("stock-locale.txt") != read(
        "candidate-locale.txt"
    ):
        raise ValueError("stock and candidate locale behavior differs")
    if not re.fullmatch(r"[0-9a-f]{64}\n", read("candidate-libc.txt")) or read(
        "stock-libc.txt"
    ) != read("candidate-libc.txt"):
        raise ValueError("stock and candidate libc bytes differ")
    print("package, loader, locale and libc controls passed")


if __name__ == "__main__":
    main(sys.argv[1])
