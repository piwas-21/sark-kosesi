#!/usr/bin/env python3
"""Bind independently recorded source and binary identities to this run."""

import json
import re
import sys
from pathlib import Path

ARCHIVE = "441dfae4280cae3e0425e5f876f80559fb0731c140f1aae22b3e5a54d0411bfd"
PATHS = (
    "/opt/pg-smoke/bin/postgres",
    "/opt/pg-smoke/bin/psql",
    "/opt/pg-smoke/bin/pg_config",
    "/opt/pg-smoke/lib/libpq.so.5.18",
)


def identity(text):
    result = {}
    for line in text.splitlines():
        key, sep, value = line.partition("=")
        if not sep or not key or not value or key in result:
            raise ValueError("invalid or duplicate identity")
        result[key] = value
    return result


def manifest(
    revision,
    source_identity,
    composition_identity,
    composed_id,
    files,
    postgres_version,
    pg_config_version,
):
    if not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError("invalid source revision")
    source = identity(source_identity)
    composition = identity(composition_identity)
    if source.get("revision") != revision or source.get("source_sha256") != ARCHIVE:
        raise ValueError("source feature evidence does not match")
    source_id = source.get("candidate_image_id", "")
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", source_id) or not re.fullmatch(
        r"sha256:[0-9a-f]{64}", composed_id
    ):
        raise ValueError("invalid image identity")
    if composition.get("source") != source_id or composition.get("composed") != composed_id:
        raise ValueError("composition image identities do not match")
    if (
        postgres_version != "postgres (PostgreSQL) 18.7\n"
        or pg_config_version != "PostgreSQL 18.7\n"
    ):
        raise ValueError("compiled PostgreSQL version mismatch")
    binaries = {}
    for line in files.splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  (/opt/pg-smoke/[^\n]+)", line)
        if match is None:
            raise ValueError("invalid installation file inventory")
        if match.group(2) in PATHS:
            if match.group(2) in binaries:
                raise ValueError("duplicate installation file")
            binaries[match.group(2)] = match.group(1)
    if set(binaries) != set(PATHS):
        raise ValueError("required compiled files missing")
    return {
        "source_revision": revision,
        "postgresql_version": "18.7",
        "source_build_image_id": source_id,
        "composed_image_id": composed_id,
        "source_archive_sha256": ARCHIVE,
        "source_archive": "https://ftp.postgresql.org/pub/source/v18.7/postgresql-18.7.tar.bz2",
        "file_sha256": dict(sorted(binaries.items())),
        "coverage": "Source-built PostgreSQL and libpq are not Debian packages. Their code advisories require a separate source-aware assessment; scanner absence is not a fix or acceptance.",
        "evidence_limit": "Same-run measurements, not an independent provenance attestation.",
    }


def main():
    source, composition, output, revision, composed_id = sys.argv[1:]
    source, composition, output = Path(source), Path(composition), Path(output)
    result = manifest(
        revision,
        (source / "identity.txt").read_text(),
        (composition / "image-ids.txt").read_text(),
        composed_id,
        (composition / "composed-files.txt").read_text(),
        (output / "postgres-version.txt").read_text(),
        (output / "pg-config-version.txt").read_text(),
    )
    (output / "source-component.json").write_text(
        json.dumps(result, indent=2, sort_keys=True) + "\n"
    )


if __name__ == "__main__":
    main()
