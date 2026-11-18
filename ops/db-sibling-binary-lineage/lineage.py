#!/usr/bin/env python3
"""Authenticate Debian snapshot packages and compare selected image bytes."""

import hashlib
import importlib.util
import json
import lzma
import re
import resource
import subprocess
import sys
import tarfile
import urllib.parse
import urllib.request
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "rootfs",
    Path(__file__).resolve().parents[1] / "db-sibling-component-presence/inspect_rootfs.py",
)
rootfs = importlib.util.module_from_spec(spec)
spec.loader.exec_module(rootfs)
spec = importlib.util.spec_from_file_location(
    "sibling_packages", Path(__file__).resolve().parents[1] / "db-sibling-runtime-root/validate.py"
)
sibling_packages = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sibling_packages)

ORIGIN = "https://snapshot.debian.org/archive/debian/20261118T000000Z/"
INDEX_SHA256 = "7778d3e3f303b7ddb8ce0fe7c8d57473a076c6bf2e8f241f75421d2396352498"
KEYRING_SHA256 = "506b815cbb32d9b6066b4a2aa524071e071761e7e7f68c3ac74f3061ba852017"
SIGNERS = {"04B54C3CDCA79751B16BC6B5225629DF75B188BD", "41587F7DB8C774BCCF131416762F67A0B2C39DE4"}
LOCK = {
    "libsystemd0": {
        "version": "257.13-1~deb13u1",
        "source": "systemd",
        "sha256": "ab0d4127b5e46e6f8c015a1db15a62ba9ae274cdefa150083adf90ada0600ea1",
        "size": 454648,
        "paths": [
            "usr/lib/x86_64-linux-gnu/libsystemd.so.0",
            "usr/lib/x86_64-linux-gnu/libsystemd.so.0.40.0",
        ],
    },
    "libudev1": {
        "version": "257.13-1~deb13u1",
        "source": "systemd",
        "sha256": "5d41c284f5a93b05bc7d648b61a02dd2bb9ff05b2261ad1a8b7d96044a0cfa88",
        "size": 152300,
        "paths": [
            "usr/lib/x86_64-linux-gnu/libudev.so.1",
            "usr/lib/x86_64-linux-gnu/libudev.so.1.7.10",
        ],
    },
    "perl-base": {
        "version": "5.40.1-6+deb13u1",
        "source": "perl",
        "sha256": "b795464137a0f4d443fc9284f4b93e883fb83883cb533adf300ac660807a352a",
        "size": 1666320,
        "paths": ["usr/bin/perl", "usr/bin/perl5.40.1"],
    },
}


def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as f:
        while block := f.read(1024 * 1024):
            h.update(block)
    return h.hexdigest()


def fetch(relative, destination, limit):
    request = urllib.request.Request(ORIGIN + relative, headers={"Accept-Encoding": "identity"})
    with urllib.request.urlopen(request, timeout=30) as response, destination.open("xb") as output:
        final = urllib.parse.urlparse(response.geturl())
        if (
            final.scheme != "https"
            or final.hostname not in {"snapshot.debian.org", "snapshot-cloudflare.debian.org"}
            or response.status != 200
        ):
            raise ValueError("unexpected Debian archive response")
        total = 0
        while block := response.read(65536):
            total += len(block)
            if total > limit:
                raise ValueError("archive response exceeds bound")
            output.write(block)
        if not total:
            raise ValueError("empty archive response")


def verify_signature(keyring, signature, release):
    verified = subprocess.run(
        ["gpgv", "--status-fd", "1", "--keyring", str(keyring), str(signature), str(release)],
        capture_output=True,
        text=True,
        timeout=60,
        check=True,
    )
    fingerprints = set()
    for line in verified.stdout.splitlines():
        if line.startswith("[GNUPG:] VALIDSIG "):
            fields = line.split()
            if len(fields) >= 3:
                fingerprints.add(fields[2])
            if len(fields) >= 12:
                fingerprints.add(fields[11])
    signers = fingerprints.intersection(SIGNERS)
    if not signers:
        raise ValueError("unapproved archive signer")
    return signers


def signed_index(release, index):
    text = release.read_text(encoding="utf-8")
    if not re.search(r"(?m)^Codename: trixie$", text):
        raise ValueError("unexpected release codename")
    matches = re.findall(r"(?m)^ ([0-9a-f]{64})\s+(\d+)\s+main/binary-amd64/Packages[.]xz$", text)
    if (
        len(matches) != 1
        or matches[0][0] != INDEX_SHA256
        or int(matches[0][1]) != index.stat().st_size
        or digest(index) != INDEX_SHA256
    ):
        raise ValueError("signed index mismatch")


def package_records(index):
    found = {}
    total = 0
    stanza = {}
    with lzma.open(index, "rt", encoding="utf-8", errors="strict") as source:
        for line in source:
            total += len(line.encode("utf-8"))
            if total > 128 * 1024 * 1024 or len(line) > 1024 * 1024:
                raise ValueError("package index exceeds bound")
            if line == "\n":
                name = stanza.get("Package")
                if name in LOCK:
                    if name in found:
                        raise ValueError("duplicate package")
                    found[name] = stanza
                stanza = {}
            elif line[0] not in " \t":
                key, sep, value = line.rstrip("\n").partition(": ")
                if not sep or key in stanza:
                    raise ValueError("invalid package index")
                stanza[key] = value
        if stanza:
            raise ValueError("unterminated package index")
    if set(found) != set(LOCK):
        raise ValueError("locked package missing from signed index")
    for name, entry in found.items():
        lock = LOCK[name]
        source = entry.get("Source", "")
        match = re.fullmatch(r"([a-z0-9.+-]+)(?: \(([^()]+)\))?", source)
        if (
            entry.get("Version") != lock["version"]
            or entry.get("Architecture") != "amd64"
            or entry.get("SHA256") != lock["sha256"]
            or entry.get("Size") != str(lock["size"])
            or not match
            or match.group(1) != lock["source"]
            or (match.group(2) or entry["Version"]) != lock["version"]
            or not re.fullmatch(
                r"pool/main/[a-z0-9+.-]+/[a-z0-9+.-]+/[A-Za-z0-9.+~_-]+[.]deb",
                entry.get("Filename", ""),
            )
        ):
            raise ValueError("signed package record mismatch")
    return found


def selected(tar_path, paths, archive_limit):
    size = tar_path.stat().st_size
    if not 1024 <= size <= archive_limit or size % 512:
        raise ValueError("invalid tar size")
    result = {}
    hardlinks = {}
    total = 0
    seen = set()
    with tar_path.open("rb") as stream:
        with tarfile.open(
            fileobj=stream, mode="r:", errorlevel=2, tarinfo=rootfs.BoundedTarInfo
        ) as archive:
            for n, member in enumerate(archive, 1):
                if n > rootfs.MAX_ENTRIES:
                    raise ValueError("too many tar members")
                total = rootfs.validate_member(member, total, size)
                name = rootfs.check_name(member.name)
                if name in seen:
                    raise ValueError("duplicate tar member")
                seen.add(name)
                if name not in paths:
                    continue
                if member.issym():
                    result[name] = {
                        "kind": "symlink",
                        "mode": member.mode,
                        "target": member.linkname,
                    }
                elif member.isfile() or member.islnk():
                    if member.islnk():
                        target = rootfs.check_name(member.linkname)
                        if target not in paths:
                            raise ValueError("selected hardlink target outside comparison")
                        hardlinks[name] = target
                    data = archive.extractfile(member)
                    if data is None:
                        raise ValueError("selected file cannot be read")
                    h = hashlib.sha256()
                    count = 0
                    while block := data.read(1024 * 1024):
                        count += len(block)
                        if count > 64 * 1024 * 1024:
                            raise ValueError("selected file too large")
                        h.update(block)
                    result[name] = {
                        "kind": "file",
                        "mode": member.mode,
                        "sha256": h.hexdigest(),
                        "size": count,
                    }
                else:
                    raise ValueError("unsupported selected file")
            end = archive.offset
        if end + 1024 > size:
            raise ValueError("truncated tar")
        stream.seek(end)
        while block := stream.read(1024 * 1024):
            if any(block):
                raise ValueError("invalid tar trailer")
    if set(result) != set(paths):
        raise ValueError("selected file missing")
    groups = {name: {name} for name in paths}
    for name, target in hardlinks.items():
        if result[target]["kind"] != "file":
            raise ValueError("hardlink target is not a selected file")
        group = groups[name] | groups[target]
        for peer in group:
            groups[peer] = group
    for name, group in groups.items():
        if len(group) > 1:
            result[name]["hardlink_group"] = sorted(group)
    return result


def compare_files(expected, observed):
    if expected != observed:
        raise ValueError("image package payload mismatch")


def verify_installed(text):
    installed = sibling_packages.parse_live_packages(text)
    if any(installed.get(name) != item["version"] for name, item in LOCK.items()):
        raise ValueError("installed package version or status mismatch")


def verify_export(image_tar, report):
    if (
        digest(image_tar) != report["export_sha256"]
        or not rootfs.IMAGE.fullmatch(report["image_id"])
        or not rootfs.REVISION.fullmatch(report["revision"])
    ):
        raise ValueError("export identity mismatch")


def limit_output():
    resource.setrlimit(resource.RLIMIT_FSIZE, (64 * 1024 * 1024, 64 * 1024 * 1024))


def main():
    image_tar, component_report, source_evidence, work, output = map(Path, sys.argv[1:])
    report = json.loads(component_report.read_text())
    verify_export(image_tar, report)
    inventory = source_evidence.parent / "db-sibling-postgres-evidence/composed-packages.txt"
    verify_installed(inventory.read_text())
    keyring_member = "usr/share/keyrings/debian-archive-keyring.gpg"
    with tarfile.open(image_tar, "r:", tarinfo=rootfs.BoundedTarInfo) as archive:
        f = archive.extractfile(keyring_member)
        if f is None:
            raise ValueError("keyring missing")
        key = f.read(1024 * 1024 + 1)
    if len(key) > 1024 * 1024 or hashlib.sha256(key).hexdigest() != KEYRING_SHA256:
        raise ValueError("image keyring mismatch")
    if (
        f"{KEYRING_SHA256}  /usr/share/keyrings/debian-archive-keyring.gpg"
        not in (source_evidence / "trust-inputs.txt").read_text().splitlines()
    ):
        raise ValueError("root trust evidence mismatch")
    keyring = output / "debian-archive-keyring.gpg"
    keyring.write_bytes(key)
    release, signature, index = (
        output / name for name in ("Release", "Release.gpg", "Packages.xz")
    )
    fetch("dists/trixie/Release", release, 2 * 1024 * 1024)
    fetch("dists/trixie/Release.gpg", signature, 128 * 1024)
    signers = verify_signature(keyring, signature, release)
    fetch("dists/trixie/main/binary-amd64/Packages.xz", index, 64 * 1024 * 1024)
    signed_index(release, index)
    entries = package_records(index)
    image_paths = [path for item in LOCK.values() for path in item["paths"]]
    image_files = selected(image_tar, image_paths, rootfs.MAX_SIZE)
    result = {}
    for name, entry in entries.items():
        package = output / f"{name}.deb"
        fetch(entry["Filename"], package, LOCK[name]["size"])
        if package.stat().st_size != LOCK[name]["size"] or digest(package) != LOCK[name]["sha256"]:
            raise ValueError("package digest mismatch")
        package_tar = work / f"{name}.tar"
        with package_tar.open("xb") as stream:
            subprocess.run(
                ["dpkg-deb", "--fsys-tarfile", str(package)],
                stdout=stream,
                stderr=subprocess.DEVNULL,
                timeout=60,
                check=True,
                preexec_fn=limit_output,
            )
        package_files = selected(package_tar, LOCK[name]["paths"], 64 * 1024 * 1024)
        compare_files(package_files, {path: image_files[path] for path in LOCK[name]["paths"]})
        result[name] = {
            "binary_version": entry["Version"],
            "source": LOCK[name]["source"],
            "source_version": LOCK[name]["version"],
            "package_sha256": digest(package),
            "files": package_files,
        }
    evidence = {
        "image_id": report["image_id"],
        "revision": report["revision"],
        "export_sha256": report["export_sha256"],
        "installed_inventory_sha256": digest(inventory),
        "keyring_sha256": KEYRING_SHA256,
        "release_sha256": digest(release),
        "index_sha256": digest(index),
        "valid_signers": sorted(signers),
        "packages": result,
        "scope": "Exact selected path bytes, modes and symlink targets only; no source-code applicability, vulnerability fix or risk acceptance.",
    }
    (output / "lineage.json").write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n")


if __name__ == "__main__":
    main()
