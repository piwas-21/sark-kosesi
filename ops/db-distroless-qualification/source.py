"""Seal the lab's same-job public PostgreSQL build against measured file baselines."""

import argparse
import json
import re
from pathlib import Path

from assembly import read_json

from packages import BUDGET, HERE, digest, owned_directory, regular, require

EVIDENCE_LIMIT = 2 * 1024**2
SOURCE_FIELDS = ("files_sha256", "metadata_sha256")
RECEIPT_FIELDS = {
    "artifact_id",
    "upstream_sha256",
    "source_image",
    "source_archive_sha256",
    *SOURCE_FIELDS,
}


def source_receipt(folder, image_id, lock):
    from archive import verify_archive

    owned_directory(folder)
    measured = {}
    for label, key in zip(("files", "metadata"), SOURCE_FIELDS, strict=True):
        path = regular(folder / f"source-{label}.txt", EVIDENCE_LIMIT)
        measured[key] = digest(path)
        require(measured[key] == lock[key], f"Public PostgreSQL {label} baseline differs")
    archive = regular(folder / "source-image.tar", BUDGET)
    sha_before = digest(archive)
    verify_archive(archive, image_id)
    require(digest(archive) == sha_before, "Source archive changed during verification")
    return {
        "artifact_id": lock["artifact_id"],
        "upstream_sha256": lock["upstream_sha256"],
        "source_image": image_id,
        "source_archive_sha256": sha_before,
        **measured,
    }


def bound_source(folder, assembly):
    owned_directory(folder)
    lock_path = regular(HERE / "source-lock.json", 65536)
    require(digest(lock_path) == assembly["source_lock_sha256"], "Reviewed source lock differs")
    lock = read_json(lock_path)
    receipt = read_json(folder / "source-receipt.json", 65536)
    require(receipt.keys() == RECEIPT_FIELDS, "Unexpected fresh source receipt fields")
    require(
        all(
            receipt[key] == lock[key] for key in ("artifact_id", "upstream_sha256", *SOURCE_FIELDS)
        ),
        "Fresh source receipt answers a different public baseline",
    )
    require(
        re.fullmatch(r"[0-9a-f]{64}", receipt["source_archive_sha256"])
        and re.fullmatch(r"sha256:[0-9a-f]{64}", receipt["source_image"]),
        "Malformed fresh source asset identity",
    )
    return {
        **lock,
        "source_archive_sha256": receipt["source_archive_sha256"],
        "source_image": receipt["source_image"],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("folder", type=Path)
    parser.add_argument("image_id")
    args = parser.parse_args()
    lock = read_json(HERE / "source-lock.json", 65536)
    try:
        receipt = source_receipt(args.folder, args.image_id, lock)
        with (args.folder / "source-receipt.json").open("x") as output:
            output.write(json.dumps(receipt, sort_keys=True) + "\n")
    except (ValueError, KeyError, OSError):
        parser.exit(1, "Public source archive or measured file baseline differs.\n")


if __name__ == "__main__":
    main()
