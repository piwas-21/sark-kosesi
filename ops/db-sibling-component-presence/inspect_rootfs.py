#!/usr/bin/env python3
"""Inspect a bounded, unmounted Docker export without extracting or executing it."""

import hashlib
import json
import posixpath
import re
import sys
import tarfile
from pathlib import Path

MAX_SIZE = 2 * 1024 * 1024 * 1024
MAX_MEMBER_SIZE = 1024 * 1024 * 1024
MAX_LOGICAL_SIZE = 2 * 1024 * 1024 * 1024
MAX_METADATA_SIZE = 1024 * 1024
MAX_ENTRIES = 200000
MAX_MATCHES = 4096
IMAGE = re.compile(r"sha256:[0-9a-f]{64}")
REVISION = re.compile(r"[0-9a-f]{40}")
PATTERNS = {
    "systemd_home": re.compile(
        r"(?:^|/)(?:systemd-homed[^/]*|systemd-homework|systemd-home-fallback-shell|homectl|pam_systemd_home[.]so)(?:/|$)",
        re.I,
    ),
    "archive_tar": re.compile(r"(?:^|/)archive/(?:tar(?:[.]pm|/|$))", re.I),
}


class BoundedTarInfo(tarfile.TarInfo):
    """Reject sparse processing and oversized metadata before tarfile reads it."""

    def _proc_sparse(self, archive):
        raise ValueError("sparse archive metadata is unsupported")

    def _proc_gnusparse_00(self, member, headers):
        raise ValueError("sparse archive metadata is unsupported")

    def _proc_gnusparse_01(self, member, headers):
        raise ValueError("sparse archive metadata is unsupported")

    def _proc_gnusparse_10(self, member, headers, archive):
        raise ValueError("sparse archive metadata is unsupported")

    def _proc_pax(self, archive):
        if not 0 <= self.size <= MAX_METADATA_SIZE:
            raise ValueError("oversized archive metadata")
        return super()._proc_pax(archive)

    def _proc_gnulong(self, archive):
        if not 0 <= self.size <= MAX_METADATA_SIZE:
            raise ValueError("oversized archive metadata")
        return super()._proc_gnulong(archive)


def validate_container(metadata, image):
    if not IMAGE.fullmatch(image) or not isinstance(metadata, list) or len(metadata) != 1:
        raise ValueError("invalid container identity")
    container = metadata[0]
    if (
        container.get("Image") != image
        or container.get("Mounts") != []
        or container.get("Config", {}).get("Volumes") not in (None, {})
        or container.get("HostConfig", {}).get("NetworkMode") != "none"
        or container.get("HostConfig", {}).get("PortBindings") not in (None, {})
        or container.get("State", {}).get("Status") != "created"
        or container.get("State", {}).get("Running") is not False
        or container.get("State", {}).get("Pid") != 0
    ):
        raise ValueError("container identity, mounts or state mismatch")


def check_name(name):
    if not isinstance(name, str) or not name or name.startswith("/") or "\x00" in name:
        raise ValueError("invalid archive path")
    parts = name.split("/")
    if ".." in parts:
        raise ValueError("unsafe archive path")
    return posixpath.normpath(name)


def validate_member(member, total, archive_size):
    allowed_types = {
        tarfile.REGTYPE,
        tarfile.AREGTYPE,
        tarfile.DIRTYPE,
        tarfile.SYMTYPE,
        tarfile.LNKTYPE,
        tarfile.FIFOTYPE,
        tarfile.CHRTYPE,
        tarfile.BLKTYPE,
    }
    if (
        member.type not in allowed_types
        or member.sparse is not None
        or any(
            "sparse" in key.casefold()
            or key == "SCHILY.realsize"
            or (key == "SCHILY.filetype" and value.casefold() == "sparse")
            for key, value in member.pax_headers.items()
        )
    ):
        raise ValueError("sparse or unsupported archive member")
    size = member.size
    if (
        not isinstance(size, int)
        or isinstance(size, bool)
        or size < 0
        or size > MAX_MEMBER_SIZE
        or (member.type not in (tarfile.REGTYPE, tarfile.AREGTYPE) and size != 0)
        or total + size > MAX_LOGICAL_SIZE
    ):
        raise ValueError("archive logical size exceeds bounds")
    if (
        not isinstance(member.offset_data, int)
        or member.offset_data < 0
        or member.offset_data + ((size + 511) // 512) * 512 + 1024 > archive_size
    ):
        raise ValueError("archive member extends past end marker")
    return total + size


def inspect(path, image, revision, before, after):
    if not REVISION.fullmatch(revision):
        raise ValueError("invalid revision")
    validate_container(before, image)
    validate_container(after, image)
    if before[0].get("Id") != after[0].get("Id") or not re.fullmatch(
        r"[0-9a-f]{64}", before[0].get("Id", "")
    ):
        raise ValueError("container changed")
    size = path.stat().st_size
    if not 1024 <= size <= MAX_SIZE or size % 512:
        raise ValueError("invalid export size")
    matches = {name: [] for name in PATTERNS}
    seen = set()
    count = 0
    logical_size = 0
    with path.open("rb") as stream:
        with tarfile.open(
            fileobj=stream, mode="r:", errorlevel=2, tarinfo=BoundedTarInfo
        ) as archive:
            for member in archive:
                count += 1
                if count > MAX_ENTRIES:
                    raise ValueError("too many archive entries")
                logical_size = validate_member(member, logical_size, size)
                name = check_name(member.name)
                if name in seen:
                    raise ValueError("duplicate archive path")
                seen.add(name)
                if member.isfile():
                    fileobj = archive.extractfile(member)
                    if fileobj is None:
                        raise ValueError("unreadable file")
                    remaining = member.size
                    while remaining:
                        block = fileobj.read(min(1048576, remaining))
                        if not block:
                            raise ValueError("truncated archive file")
                        remaining -= len(block)
                elif not (
                    member.isdir()
                    or member.issym()
                    or member.islnk()
                    or member.isfifo()
                    or member.isdev()
                ):
                    raise ValueError("unsupported archive member")
                candidates = [("path", name)]
                if member.issym() or member.islnk():
                    link = member.linkname
                    if not link or "\x00" in link:
                        raise ValueError("invalid link target")
                    resolved = posixpath.normpath(
                        link
                        if link.startswith("/")
                        else posixpath.join(posixpath.dirname(name), link)
                    )
                    candidates.extend((("link_target", link), ("resolved_target", resolved)))
                for kind, expression in PATTERNS.items():
                    for source, value in candidates:
                        if expression.search(value):
                            matches[kind].append({"path": name, "match": source, "value": value})
                            if len(matches[kind]) > MAX_MATCHES:
                                raise ValueError("too many matches")
            end = archive.offset
        if count == 0 or end + 1024 > size:
            raise ValueError("incomplete archive")
        stream.seek(end)
        while block := stream.read(1048576):
            if any(block):
                raise ValueError("archive has invalid end marker or trailing data")
    hasher = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(1048576):
            hasher.update(block)
    return {
        "revision": revision,
        "image_id": image,
        "container_id": before[0]["Id"],
        "export_sha256": hasher.hexdigest(),
        "export_bytes": size,
        "entry_count": count,
        "logical_bytes": logical_size,
        "matches": matches,
        "scope": "Names and link targets in one daemon-exported, unmounted filesystem. No execution, byte-signature or embedded-copy analysis; not proof of component absence or vulnerability disposition.",
    }


def main():
    archive, image, revision, before, after, output = sys.argv[1:]
    report = inspect(
        Path(archive),
        image,
        revision,
        json.loads(Path(before).read_text()),
        json.loads(Path(after).read_text()),
    )
    Path(output).write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")


if __name__ == "__main__":
    main()
