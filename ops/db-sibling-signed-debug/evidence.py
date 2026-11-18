#!/usr/bin/env python3
"""Authenticate matching Debian debug packages and inventory their DWARF units."""

import importlib.util
import json
import lzma
import os
import re
import resource
import struct
import subprocess
import sys
import tarfile
import urllib.parse
import urllib.request
import zlib
from pathlib import Path

BASE = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location(
    "lineage", BASE / "db-sibling-binary-lineage/lineage.py"
)
lineage = importlib.util.module_from_spec(spec)
spec.loader.exec_module(lineage)
spec = importlib.util.spec_from_file_location(
    "custody", BASE / "db-sibling-component-presence/custody.py"
)
custody = importlib.util.module_from_spec(spec)
spec.loader.exec_module(custody)

ORIGIN = "https://snapshot.debian.org/archive/debian-debug/20261118T000000Z/"
VERSION = "257.13-1~deb13u1"
LOCK = {
    "libsystemd0": {
        "build_id": "50d75e545b53f25845679f2eccf50cf0ac3c475e",
        "path": "usr/lib/x86_64-linux-gnu/libsystemd.so.0.40.0",
        "sha256": "600d7b27ff53962ec80ac4b60bc10b122cff971d069c7c7ce3df3921af22e884",
        "size": 1666820,
    },
    "libudev1": {
        "build_id": "c487530c059a7301c3a1b033d5a1c122a7a85180",
        "path": "usr/lib/x86_64-linux-gnu/libudev.so.1.7.10",
        "sha256": "b932bbf2f902f7ff651a4d65d35095fb23a9c5a287efc22cd9e4cc447d0effa9",
        "size": 546516,
    },
}


def fetch(relative, output, limit):
    req = urllib.request.Request(ORIGIN + relative, headers={"Accept-Encoding": "identity"})
    with urllib.request.urlopen(req, timeout=30) as response, output.open("xb") as dst:
        u = urllib.parse.urlparse(response.geturl())
        if (
            response.status != 200
            or u.scheme != "https"
            or u.hostname not in {"snapshot.debian.org", "snapshot-cloudflare.debian.org"}
        ):
            raise ValueError("unexpected archive response")
        total = 0
        while block := response.read(65536):
            total += len(block)
            if total > limit:
                raise ValueError("archive response too large")
            dst.write(block)
        if not total:
            raise ValueError("empty archive response")


def check_index(release, index):
    text = release.read_text(encoding="utf-8")
    if len(re.findall(r"(?m)^Codename: trixie-debug$", text)) != 1:
        raise ValueError("debug suite mismatch")
    # The requested checksum must occur in the SHA256 section, not another field.
    sections = re.findall(r"(?ms)^SHA256:\n((?: [^\n]*\n)+)", text)
    if len(sections) != 1:
        raise ValueError("invalid Release SHA256 section")
    rows = re.findall(
        r"(?m)^ ([0-9a-f]{64})\s+(\d+)\s+main/binary-amd64/Packages[.]xz$", sections[0]
    )
    if (
        len(rows) != 1
        or int(rows[0][1]) != index.stat().st_size
        or lineage.digest(index) != rows[0][0]
    ):
        raise ValueError("debug index does not match signed Release")


def index_lines(index):
    decoder = lzma.LZMADecompressor(format=lzma.FORMAT_XZ, memlimit=64 * 1024 * 1024)
    pending = b""
    total = 0
    with index.open("rb") as stream:
        while not decoder.eof:
            compressed = stream.read(65536) if decoder.needs_input else b""
            if decoder.needs_input and not compressed:
                raise ValueError("truncated debug index")
            decoded = decoder.decompress(compressed, max_length=65536)
            total += len(decoded)
            if total > 128 * 1024 * 1024:
                raise ValueError("debug index too large")
            pending += decoded
            while b"\n" in pending:
                end = pending.index(b"\n") + 1
                if end > 1024 * 1024:
                    raise ValueError("debug index line too large")
                yield pending[:end].decode("utf-8", errors="strict")
                pending = pending[end:]
            if len(pending) > 1024 * 1024:
                raise ValueError("debug index line too large")
        if decoder.unused_data or stream.read(1):
            raise ValueError("trailing or concatenated debug index")
    if pending:
        raise ValueError("unterminated debug index line")


def records(index):
    found, row = {}, {}
    names = {name + "-dbgsym": item for name, item in LOCK.items()}
    for line in index_lines(index):
        if line == "\n":
            name = row.get("Package")
            if name in names:
                if name in found:
                    raise ValueError("duplicate debug package")
                found[name] = row
            row = {}
        elif line[0] not in " \t":
            key, sep, value = line.rstrip("\n").partition(": ")
            if not sep or key in row:
                raise ValueError("malformed debug index")
            row[key] = value
    if row:
        raise ValueError("unterminated debug index")
    if set(found) != set(names):
        raise ValueError("debug package missing")
    for name, entry in found.items():
        item = names[name]
        if (
            entry.get("Version") != VERSION
            or entry.get("Architecture") != "amd64"
            or entry.get("SHA256") != item["sha256"]
            or entry.get("Size") != str(item["size"])
            or entry.get("Source") not in ("systemd", f"systemd ({VERSION})")
            or not re.fullmatch(
                r"pool/main/s/systemd/[A-Za-z0-9.+~_-]+[.]deb", entry.get("Filename", "")
            )
        ):
            raise ValueError("debug package record mismatch")
    return found


def extract_one(archive_path, member_name, output):
    # Inspect all archive members using the same bounds as the lineage comparison.
    lineage.selected(archive_path, [member_name], 64 * 1024 * 1024)
    with tarfile.open(archive_path, "r:", tarinfo=lineage.rootfs.BoundedTarInfo) as archive:
        member = next(
            (m for m in archive if lineage.rootfs.check_name(m.name) == member_name), None
        )
        if member is None or not member.isfile() or member.size > 64 * 1024 * 1024:
            raise ValueError("invalid selected debug payload")
        data = archive.extractfile(member)
        if data is None:
            raise ValueError("cannot extract debug payload")
        with output.open("xb") as dst:
            while block := data.read(1024 * 1024):
                dst.write(block)


def run_to_file(command, output, maximum=64 * 1024 * 1024):
    def limits():
        resource.setrlimit(resource.RLIMIT_FSIZE, (maximum, maximum))
        resource.setrlimit(resource.RLIMIT_CPU, (60, 60))

    with output.open("xb") as dst:
        result = subprocess.run(
            command,
            stdout=dst,
            stderr=subprocess.PIPE,
            check=True,
            timeout=90,
            preexec_fn=limits,
            env={**os.environ, "LC_ALL": "C"},
        )
    if result.stderr or not output.stat().st_size:
        raise ValueError("tool produced errors or empty output")


def build_id(text, expected):
    ids = re.findall(r"(?m)^\s*Build ID: ([0-9a-f]+)\s*$", text)
    if ids != [expected]:
        raise ValueError("ELF build ID mismatch")


def debuglink(section, debug_bytes):
    nul = section.find(b"\0")
    if nul < 1:
        raise ValueError("invalid debuglink")
    offset = (nul + 4) & ~3
    if len(section) != offset + 4 or any(section[nul:offset]):
        raise ValueError("invalid debuglink padding")
    name = section[:nul].decode("ascii")
    if not re.fullmatch(r"[A-Za-z0-9_.+-]+", name):
        raise ValueError("unexpected debuglink name")
    if struct.unpack("<I", section[offset:])[0] != zlib.crc32(debug_bytes):
        raise ValueError("debuglink CRC mismatch")
    return name


def has_debuglink(sections):
    matches = re.findall(r"(?m)^[ \t]*\[[^\]\n]+\][ \t]+[.]gnu_debuglink[ \t]", sections)
    if len(matches) > 1:
        raise ValueError("duplicate debuglink section")
    return bool(matches)


def units(text):
    blocks = re.split(r"(?m)^[ \t]*Compilation Unit @ offset (?:0x)?[0-9a-f]+:[ \t]*$", text)
    if len(blocks) < 2 or len(blocks) > 100001:
        raise ValueError("missing or excessive DWARF units")
    result = []
    for block in blocks[1:]:
        tags = re.findall(
            r"(?m)^[ \t]*<0><[0-9a-f]+>: Abbrev Number: \d+ \(DW_TAG_([a-z_]+)\)", block
        )
        if tags != ["compile_unit"]:
            raise ValueError("unsupported DWARF unit")
        root = re.split(r"(?m)^[ \t]*<1><[0-9a-f]+>:", block, maxsplit=1)[0]
        names = re.findall(r"(?m)^[ \t]*<[^>\n]+>[ \t]+DW_AT_name[ \t]*:[ \t]*(.*)$", root)
        if len(names) != 1 or not names[0].strip() or len(names[0]) > 4096:
            raise ValueError("invalid DWARF unit name")
        directories = re.findall(
            r"(?m)^[ \t]*<[^>\n]+>[ \t]+DW_AT_comp_dir[ \t]*:[ \t]*(.*)$", root
        )
        if len(directories) > 1 or any(
            not value.strip() or len(value) > 4096 for value in directories
        ):
            raise ValueError("invalid DWARF compilation directory")
        result.append(
            {
                "name_rendered": names[0].strip(),
                "compilation_directory_rendered": directories[0].strip() if directories else None,
            }
        )
    return result


def main(root, output, scratch, revision, image):
    custody.verify(
        root,
        {
            "identity.txt",
            "lineage.json",
            "status.txt",
            "debian-archive-keyring.gpg",
            "libsystemd0.deb",
            "libudev1.deb",
        },
    )
    report = json.loads((root / "lineage.json").read_text())
    if report["revision"] != revision or report["image_id"] != image:
        raise ValueError("lineage identity mismatch")
    keyring = root / "debian-archive-keyring.gpg"
    if lineage.digest(keyring) != lineage.KEYRING_SHA256:
        raise ValueError("keyring mismatch")
    release, signature, index = (output / x for x in ("Release", "Release.gpg", "Packages.xz"))
    fetch("dists/trixie-debug/Release", release, 2 * 1024 * 1024)
    fetch("dists/trixie-debug/Release.gpg", signature, 128 * 1024)
    signers = lineage.verify_signature(keyring, signature, release)
    fetch("dists/trixie-debug/main/binary-amd64/Packages.xz", index, 64 * 1024 * 1024)
    check_index(release, index)
    entries = records(index)
    evidence = {}
    for name, item in LOCK.items():
        package = output / f"{name}-dbgsym.deb"
        fetch(entries[name + "-dbgsym"]["Filename"], package, item["size"])
        if package.stat().st_size != item["size"] or lineage.digest(package) != item["sha256"]:
            raise ValueError("debug package digest mismatch")
        if lineage.digest(root / f"{name}.deb") != lineage.LOCK[name]["sha256"]:
            raise ValueError("binary package mismatch")
        if report["packages"][name]["files"][item["path"]]["kind"] != "file":
            raise ValueError("lineage payload not a file")
        original_tar = scratch / f"{name}.tar"
        debug_tar = scratch / f"{name}-debug.tar"
        run_to_file(["dpkg-deb", "--fsys-tarfile", str(root / f"{name}.deb")], original_tar)
        run_to_file(["dpkg-deb", "--fsys-tarfile", str(package)], debug_tar)
        elf, debug = scratch / f"{name}.elf", scratch / f"{name}.debug"
        extract_one(original_tar, item["path"], elf)
        path = (
            "usr/lib/debug/.build-id/"
            + item["build_id"][:2]
            + "/"
            + item["build_id"][2:]
            + ".debug"
        )
        extract_one(debug_tar, path, debug)
        if lineage.digest(elf) != report["packages"][name]["files"][item["path"]]["sha256"]:
            raise ValueError("ELF does not match lineage")
        for source, suffix in ((elf, "elf"), (debug, "debug")):
            notes = scratch / f"{name}-{suffix}-notes.txt"
            run_to_file(["readelf", "-n", str(source)], notes)
            build_id(notes.read_text(), item["build_id"])
            (output / notes.name).write_bytes(notes.read_bytes())
        sections = scratch / f"{name}-sections.txt"
        run_to_file(["readelf", "-SW", str(elf)], sections)
        link = "absent"
        if has_debuglink(sections.read_text()):
            section = scratch / f"{name}-debuglink.bin"
            subprocess.run(
                ["objcopy", "--dump-section", f".gnu_debuglink={section}", str(elf), "/dev/null"],
                check=True,
                timeout=30,
            )
            link = debuglink(section.read_bytes(), debug.read_bytes())
        dwarf = scratch / f"{name}-dwarf.txt"
        run_to_file(["readelf", "--debug-dump=info", "--wide", str(debug)], dwarf)
        unit_names = units(dwarf.read_text(encoding="utf-8", errors="strict"))
        (output / f"{name}-units.json").write_text(json.dumps(unit_names, indent=2) + "\n")
        evidence[name] = {
            "build_id": item["build_id"],
            "elf_sha256": lineage.digest(elf),
            "debug_sha256": lineage.digest(debug),
            "debug_package_sha256": item["sha256"],
            "debuglink": link,
            "compilation_units": len(unit_names),
        }
    result = {
        "revision": revision,
        "image_id": image,
        "lineage_manifest_sha256": lineage.digest(root / "SHA256SUMS"),
        "debug_release_sha256": lineage.digest(release),
        "debug_index_sha256": lineage.digest(index),
        "valid_signers": sorted(signers),
        "libraries": evidence,
        "scope": "Trimmed readelf-rendered DWARF unit attributes; not normalized paths or proof of code absence, vulnerability applicability, suppression or acceptance.",
    }
    (output / "debug-evidence.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")


if __name__ == "__main__":
    main(Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3]), sys.argv[4], sys.argv[5])
