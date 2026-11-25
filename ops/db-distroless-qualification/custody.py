"""Recheck whole-daemon and exact resource identity before any top-level cleanup."""

import argparse
import re

from run_lab import daemon, docker, inspect
from runtime import instance_id, require

SOURCE_MARKER = "org.piwas.qualification.source"
EXPORT_MARKER = "org.piwas.qualification.export"


def resolve(kind, name, *, expected="", nonce="", container_id=""):
    record = inspect(kind, name)
    if record is None:
        return ""
    identity = record["Id"]
    if kind == "image":
        require(
            re.fullmatch(r"sha256:[0-9a-f]{64}", identity)
            and record["Os"] == "linux"
            and record["Architecture"] == "amd64"
            and name in record["RepoTags"],
            "Unverified tagged lab image",
        )
        if nonce:
            require(
                (record["Config"]["Labels"] or {}).get(SOURCE_MARKER) == nonce,
                "Unverified source build label",
            )
    else:
        require(
            instance_id(identity)
            and (not container_id or identity == container_id)
            and record["Name"] == "/" + name
            and record["Image"] == expected
            and (record["Config"]["Labels"] or {}).get(EXPORT_MARKER) == nonce
            and record["HostConfig"]["NetworkMode"] == "none"
            and record["HostConfig"]["ReadonlyRootfs"] is True
            and record["State"]["Running"] is False
            and record["Mounts"] == [],
            "Unverified stopped export fixture",
        )
    if kind == "image":
        require(
            (expected and identity == expected) or (not expected and nonce),
            "Unverified candidate image ID",
        )
    return identity


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("absent", "owned", "remove"))
    parser.add_argument("kind", choices=("image", "container"))
    parser.add_argument("name")
    parser.add_argument("daemon_id")
    parser.add_argument("--expected", default="")
    parser.add_argument("--nonce", default="")
    parser.add_argument("--container-id", default="")
    args = parser.parse_args()
    require(daemon() == args.daemon_id, "Different lab Docker daemon")
    if args.operation == "absent":
        require(inspect(args.kind, args.name) is None, "Scoped resource already exists")
        return
    found = resolve(
        args.kind,
        args.name,
        expected=args.expected,
        nonce=args.nonce,
        container_id=args.container_id,
    )
    if args.operation == "owned":
        require(found, "Scoped resource is missing")
        print(found)
    elif found:
        if args.kind == "image":
            # The tag can be removed only after matching the sealed image or build marker.
            docker("image", "rm", args.name)
        else:
            docker("container", "rm", found)
            require(inspect("container", found) is None, "Export fixture still exists by ID")
        require(inspect(args.kind, args.name) is None, "Scoped resource still exists by name")
    elif args.kind == "container" and args.container_id:
        require(
            inspect("container", args.container_id) is None, "Export fixture still exists by ID"
        )


if __name__ == "__main__":
    main()
