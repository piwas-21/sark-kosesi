#!/usr/bin/env python3
"""Read-only package and exported-root checks for the AMD64 Distroless lab."""

import hashlib
import lzma
import os
import re
import stat
import subprocess
import tarfile
from datetime import UTC, datetime
from email.utils import parsedate_to_datetime
from pathlib import Path, PurePosixPath

HERE = Path(__file__).resolve().parent
BUDGET = 2 * 1024**3
MAX_FIELD = 64 * 1024
STATUS = "var/lib/dpkg/status.d/"
SERVER_FILES = ("opt/pg-smoke/bin/postgres", "opt/pg-smoke/lib/libpq.so.5.18")
TOOL_ONLY = {
    "sh",
    "bash",
    "dash",
    "busybox",
    "apt",
    "apt-get",
    "dpkg",
    "initdb",
    "pg_ctl",
    "psql",
    "pg_dump",
    "pg_restore",
}


def require(ok, message):
    if not ok:
        raise ValueError(message)


def digest(file):
    with file.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def regular(path, limit):
    info = path.lstat()
    require(
        path.resolve() == path
        and stat.S_ISREG(info.st_mode)
        and info.st_nlink == 1
        and 0 < info.st_size <= limit,
        "Unexpected evidence path, type or size",
    )
    return path


def owned_directory(path, *, private=True):
    info = path.lstat()
    mode = stat.S_IMODE(info.st_mode)
    require(
        path.is_absolute()
        and path.resolve(strict=True) == path
        and stat.S_ISDIR(info.st_mode)
        and info.st_uid == os.geteuid()
        and (mode == 0o700 if private else mode & 0o022 == 0),
        "Unexpected owner or path for lab directory",
    )
    return path


def paragraphs(lines):
    current, total = {}, 0
    for line in lines:
        total += len(line.encode("utf-8"))
        require(total <= 128 * 1024**2 and len(line) <= 1024**2, "Oversized package metadata")
        if line in {"", "\n"}:
            if current:
                yield current
                current = {}
        elif not line.startswith((" ", "\t")):
            key, sep, value = line.rstrip("\n").partition(": ")
            require(sep and key not in current, "Malformed or duplicate Debian field")
            current[key] = value
    if current:
        yield current


def only_record(raw):
    (result,) = tuple(paragraphs(raw.decode().splitlines(keepends=True)))
    return result


def release_window(text, locked, now):
    dates = re.findall(r"(?m)^Date: (.+)$", text)
    expiration = re.findall(r"(?m)^Valid-Until: (.+)$", text)
    expected_expiration = locked["valid_until"]
    require(dates == [locked["release_date"]], "Debian Release date differs")
    require(
        expiration == ([expected_expiration] if expected_expiration else []),
        "Debian Release Valid-Until differs",
    )
    published = parsedate_to_datetime(dates[0])
    snapshot = datetime.fromisoformat(locked["snapshot_time"])
    require(published <= snapshot <= now, "Unexpected frozen Debian Release window")
    if expiration:
        expires = parsedate_to_datetime(expiration[0])
        require(snapshot < expires and now < expires, "Debian Release Valid-Until has passed")
    return int(published.timestamp()), int(snapshot.timestamp())


def verify_signature(keyring, detached, release, approval, window):
    # Same gpgv status/fingerprint contract as the tracked sibling lineage lab.
    result = subprocess.run(
        ["gpgv", "--status-fd", "1", "--keyring", str(keyring), str(detached), str(release)],
        capture_output=True,
        text=True,
        timeout=60,
        check=True,
    )
    present = set()
    dates = set()
    for line in result.stdout.splitlines():
        if line.startswith("[GNUPG:] VALIDSIG "):
            values = line.split()
            require(len(values) >= 11 and values[4].isdigit(), "Malformed archive signature status")
            signers = {values[2], *values[11:12]} & set(approval)
            if signers:
                signed_at = int(values[4])
                require(
                    window[0] <= signed_at <= window[1],
                    "Archive signature time differs from frozen window",
                )
                present.update(signers)
                dates.add(signed_at)
    require(present, "No reviewed Debian archive signer")
    return sorted(present), sorted(dates)


def package_inputs(folder, lock):
    debian = lock["debian"]
    expected = debian["packages"]
    trust = regular(folder / "debian-archive-keyring.gpg", 1024**2)
    release = regular(folder / "Release", 2 * 1024**2)
    signature = regular(folder / "Release.gpg", 128 * 1024)
    index = regular(folder / "Packages.xz", 64 * 1024**2)
    debs = {
        name: regular(folder / (name + ".deb"), recipe["bytes"])
        for name, recipe in expected.items()
    }
    inputs = [trust, release, signature, index, *debs.values()]
    require(
        sum(file.stat().st_size for file in inputs) <= BUDGET, "Package staging budget exceeded"
    )
    require(
        digest(trust) == debian["keyring_sha256"]
        and digest(release) == debian["release_sha256"]
        and digest(signature) == debian["signature_sha256"],
        "Debian keyring, Release or signature pin differs",
    )
    text = release.read_text()
    window = release_window(text, debian, datetime.now(UTC))
    signers, signature_times = verify_signature(
        trust, signature, release, debian["signers"], window
    )
    require(
        re.findall(r"(?m)^Codename: (.+)$", text) == [debian["suite"]],
        "Debian Release codename differs",
    )
    section = re.search(r"(?m)^SHA256:\n((?: [^\n]*\n)+)", text)
    require(section is not None, "Missing signed Debian SHA256 section")
    needle = re.escape(debian["index_path"])
    records = re.findall(rf"(?m)^ ([a-f0-9]{{64}})\s+([0-9]+)\s+{needle}$", section[1])
    require(
        records == [(debian["index_sha256"], str(debian["index_bytes"]))]
        and index.stat().st_size == debian["index_bytes"]
        and digest(index) == debian["index_sha256"],
        "Signed AMD64 index identity differs",
    )

    found = set()
    with lzma.open(index, "rt", encoding="utf-8") as rows:
        for row in paragraphs(rows):
            name = row.get("Package")
            if name not in expected:
                continue
            recipe = expected[name]
            require(
                name not in found
                and all(
                    row[key] == value
                    for key, value in (
                        ("Version", recipe["version"]),
                        ("Architecture", recipe["arch"]),
                        ("Size", str(recipe["bytes"])),
                        ("Filename", recipe["filename"]),
                        ("SHA256", recipe["sha256"]),
                    )
                ),
                "Signed package row differs from lock",
            )
            found.add(name)
    require(found == set(expected), "Missing signed package row")
    for name, recipe in expected.items():
        path = debs[name]
        require(
            path.stat().st_size == recipe["bytes"] and digest(path) == recipe["sha256"],
            "Package bytes differ from signed index",
        )
        result = subprocess.run(
            ["dpkg-deb", "--field", str(path), "Package", "Version", "Architecture"],
            capture_output=True,
            timeout=30,
            check=True,
        )
        control = only_record(result.stdout)
        require(
            control
            == {"Package": name, "Version": recipe["version"], "Architecture": recipe["arch"]},
            "Package control record differs",
        )
    return {
        "approved_signers": signers,
        "signature_times": signature_times,
        "index_sha256": debian["index_sha256"],
        "package_count": len(found),
        "verified_bytes": sum(file.stat().st_size for file in inputs),
    }


def image_inventory(archive, lock):
    regular(archive, BUDGET)
    base = lock["base"]["packages"]
    additions = lock["debian"]["packages"]
    require(not base.keys() & additions.keys(), "Existing base package changed in closure")
    expected = {
        name: (entry["version"], entry["arch"]) for name, entry in (base | additions).items()
    }
    expected_members = {name: entry["member"] for name, entry in base.items()}
    expected_members.update({name: STATUS + name for name in additions})
    hashes = {name: lock["postgres"]["file_sha256"]["/" + name] for name in SERVER_FILES}
    installed, exact_files, seen = {}, set(), set()
    with tarfile.open(archive, mode="r:") as exported:
        for number, entry in enumerate(exported):
            require(number < 100_000, "Too many root filesystem entries")
            root = PurePosixPath(entry.name)
            require(
                not root.is_absolute() and ".." not in root.parts, "Unexpected root filesystem path"
            )
            name = str(root)
            if name == ".":
                continue
            require(name not in seen, "Duplicate root filesystem entry")
            seen.add(name)
            if str(root.parent) in {"bin", "usr/bin", "opt/pg-smoke/bin"}:
                require(
                    root.name not in TOOL_ONLY, "Server image includes bootstrap or client tools"
                )
            if name.startswith(STATUS) and not entry.isdir():
                require(
                    entry.isfile() and 0 < entry.size <= MAX_FIELD, "Unexpected dpkg status.d entry"
                )
                row = only_record(exported.extractfile(entry).read())
                package = row["Package"]
                require(
                    package not in installed
                    and row.get("Status", "install ok installed") == "install ok installed",
                    "Duplicate or noninstalled package record",
                )
                require(
                    name == expected_members.get(package), "Unexpected dpkg status.d package path"
                )
                installed[package] = (row["Version"], row["Architecture"])
            if name in hashes:
                require(
                    entry.isfile()
                    and 0 < entry.size <= 128 * 1024**2
                    and hashlib.file_digest(exported.extractfile(entry), "sha256").hexdigest()
                    == hashes[name],
                    "Measured PostgreSQL file differs",
                )
                exact_files.add(name)
    require(
        installed == expected and exact_files == set(hashes),
        "Server package or PostgreSQL inventory differs",
    )
    # Native Distroless status.d has no Status field. After exact inventory validation,
    # use the existing paired scanner comparator's three columns.
    return [
        f"{name}\t{version}\tinstall ok installed"
        for name, (version, _) in sorted(installed.items())
    ]
