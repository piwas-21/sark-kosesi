#!/usr/bin/env python3
"""Validate the actual disposable container's identity and lifecycle state."""

import json
import sys


def validate(container, image, volume, running):
    config = container["Config"]
    host = container["HostConfig"]
    state = container["State"]
    mounts = container["Mounts"]
    expected_command = [
        "-D",
        "/var/lib/postgresql/18/docker",
        "-c",
        "listen_addresses=127.0.0.1",
        "-c",
        "unix_socket_directories=/tmp",
        "-c",
        "shared_buffers=16MB",
    ]
    checks = [
        container["Image"] == image,
        config["User"] == "999:999",
        config["Entrypoint"] == ["/opt/pg-smoke/bin/postgres"],
        config["Cmd"] == expected_command,
        config["StopSignal"] == "SIGINT",
        host["NetworkMode"] == "none",
        host["ReadonlyRootfs"] is True,
        "ALL" in (host["CapDrop"] or []),
        "no-new-privileges" in (host["SecurityOpt"] or []),
        "PGDATA=/var/lib/postgresql/18/docker" in config["Env"],
        "/tmp" in (host["Tmpfs"] or {}),
        len(mounts) == 1,
        mounts[0]["Type"] == "volume",
        mounts[0]["Name"] == volume,
        mounts[0]["Destination"] == "/var/lib/postgresql",
        mounts[0]["RW"] is True,
        state["Running"] is running,
        state["OOMKilled"] is False,
    ]
    if not running:
        checks.append(state["ExitCode"] == 0)
    if not all(checks):
        raise ValueError("container identity, isolation, or shutdown check failed")


def main():
    path, image, volume, status = sys.argv[1:]
    if status not in ("running", "stopped"):
        raise ValueError("unexpected lifecycle status")
    with open(path, encoding="utf-8") as source:
        (container,) = json.load(source)
    validate(container, image, volume, status == "running")


if __name__ == "__main__":
    main()
