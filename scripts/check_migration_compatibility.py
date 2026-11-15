#!/usr/bin/env python3
"""Compare a captured SQLx manifest and migration history without a database connection."""

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path

MAX_ARTIFACT_BYTES = 1024 * 1024
KINDS = ("simple", "reversible_up", "reversible_down")


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("Artifact contains a duplicate key")
        result[key] = value
    return result


def read_artifact(path):
    with path.open("rb") as stream:
        data = stream.read(MAX_ARTIFACT_BYTES + 1)
    if len(data) > MAX_ARTIFACT_BYTES:
        raise ValueError("Artifact exceeds the size limit")
    try:
        document = json.loads(data.decode("utf-8"), object_pairs_hook=unique_object)
    except (UnicodeError, json.JSONDecodeError, RecursionError) as error:
        raise ValueError("Artifact is not valid JSON") from error
    return document, hashlib.sha256(data).hexdigest()


def rows(document, extra_keys=()):
    if (
        not isinstance(document, dict)
        or document.keys() != {"schema_version", "migrations", *extra_keys}
        or type(document["schema_version"]) is not int
        or document["schema_version"] != 1
        or not isinstance(document["migrations"], list)
    ):
        raise ValueError("Invalid migration artifact schema")
    return document["migrations"]


def identity(row, value_key):
    if (
        not isinstance(row, dict)
        or row.keys() != {"version", "checksum_sha384", value_key}
        or not isinstance(row["version"], str)
        or not re.fullmatch(r"0|-?[1-9][0-9]{0,18}", row["version"])
        or not isinstance(row["checksum_sha384"], str)
        or not re.fullmatch(r"[0-9a-f]{96}", row["checksum_sha384"])
    ):
        raise ValueError("Invalid migration version or checksum")
    version = int(row["version"])
    if not -(2**63) <= version < 2**63:
        raise ValueError("Migration version is outside the signed 64-bit range")
    return version, row["checksum_sha384"]


def manifest_migrations(document):
    entries = rows(document, ("ignore_missing",))
    if type(document["ignore_missing"]) is not bool or not entries:
        raise ValueError("Invalid embedded migration manifest")
    by_version = {}
    previous = None
    for entry in entries:
        version, checksum = identity(entry, "kind")
        kind = entry["kind"]
        if kind not in KINDS:
            raise ValueError("Unsupported embedded migration kind")
        position = (version, KINDS.index(kind))
        if previous is not None and position <= previous:
            raise ValueError("Embedded migration order or identity is invalid")
        previous = position
        by_version.setdefault(version, {})[kind] = checksum
    forward = {}
    for version, versions in by_version.items():
        if versions.keys() == {"simple"}:
            forward[version] = versions["simple"]
        elif versions.keys() == {"reversible_up", "reversible_down"}:
            forward[version] = versions["reversible_up"]
        else:
            raise ValueError(f"Invalid embedded migration files for version {version}")
    return forward


def applied_migrations(document):
    applied = {}
    for entry in rows(document):
        version, checksum = identity(entry, "success")
        if type(entry["success"]) is not bool or version in applied:
            raise ValueError("Invalid applied migration identity or status")
        if not entry["success"]:
            raise ValueError(f"Migration history is dirty at version {version}")
        applied[version] = checksum
    return applied


def compare(manifest, history, *, require_applied=False):
    forward = manifest_migrations(manifest)
    applied = applied_migrations(history)
    unknown = sorted(applied.keys() - forward.keys())
    if unknown and not manifest["ignore_missing"]:
        raise ValueError(f"Embedded manifest is missing applied version {unknown[0]}")
    for version in sorted(applied.keys() & forward.keys()):
        if applied[version] != forward[version]:
            raise ValueError(f"Checksum differs for applied version {version}")
    pending = sorted(forward.keys() - applied.keys())
    if require_applied and pending:
        raise ValueError(f"Embedded migration {pending[0]} is pending")
    return {
        "schema_version": 1,
        "applied_count": len(applied),
        "ignore_missing": manifest["ignore_missing"],
        "ignored_applied_versions": [str(version) for version in unknown],
        "pending_versions": [str(version) for version in pending],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("history", type=Path)
    parser.add_argument(
        "--require-applied", action="store_true", help="reject any pending migration"
    )
    args = parser.parse_args()
    try:
        manifest, manifest_digest = read_artifact(args.manifest)
        history, history_digest = read_artifact(args.history)
        result = compare(manifest, history, require_applied=args.require_applied)
    except OSError:
        sys.exit("Migration history check failed: artifact could not be read")
    except ValueError as error:
        sys.exit(f"Migration history check failed: {error}")
    print(
        json.dumps({**result, "manifest_sha256": manifest_digest, "history_sha256": history_digest})
    )


if __name__ == "__main__":
    main()
