#!/usr/bin/env python3
"""Run a command against a disposable, owned PostgreSQL cluster only."""

import argparse
import os
import re
import secrets
import socket
import subprocess
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
IMAGE = (
    "postgres:18.7-trixie@sha256:9f4fd4aa918f52ae3d6f3b687f1de90d8f867cd4d520021545a9a50c120eae3b"
)


def run(*args, **kwargs):
    try:
        return subprocess.run(
            args, check=True, text=True, capture_output=True, timeout=60, **kwargs
        )
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
        raise RuntimeError(f"{Path(args[0]).name} failed; command details withheld") from None


def disposable_marker(marker=None):
    value = secrets.token_hex(32) if marker is None else marker
    if not isinstance(value, str) or re.fullmatch(r"[0-9a-f]{64}", value) is None:
        raise ValueError("Invalid disposable PostgreSQL marker")
    return value


def native_cluster(directory, binaries, marker=None, *, disable_fsync=False):
    marker = disposable_marker(marker)
    data = directory / "data"
    password = secrets.token_urlsafe(32)
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    run(
        str(binaries / "initdb"),
        "-D",
        str(data),
        "-U",
        "p1_test",
        "-A",
        "scram-sha-256",
        "--pwfile",
        "/dev/stdin",
        "--no-locale",
        "--encoding=UTF8",
        # initdb reads an anonymous subprocess pipe; no plaintext credential file.
        input=password + "\n",
    )
    server_options = (
        f"-c listen_addresses=127.0.0.1 -c unix_socket_directories='' -p {port} "
        f"-c sark.disposable_marker={marker}"
    )
    if disable_fsync:
        server_options += " -c fsync=off"
    run(
        str(binaries / "pg_ctl"),
        "-D",
        str(data),
        "-l",
        str(directory / "postgres.log"),
        "-o",
        server_options,
        "start",
    )
    negative = subprocess.run(
        [
            str(binaries / "psql"),
            "--host",
            "127.0.0.1",
            "--port",
            str(port),
            "--username",
            "p1_test",
            "--dbname",
            "postgres",
            "--no-password",
            "--command",
            "SELECT 1",
        ],
        env={
            **{key: value for key, value in os.environ.items() if not key.startswith("PG")},
            "PGPASSWORD": password + "-invalid",
        },
        capture_output=True,
        text=True,
        timeout=10,
    )
    if negative.returncode == 0 or "password authentication failed" not in negative.stderr:
        raise ValueError("Native PostgreSQL authentication negative control failed")
    return f"postgresql://p1_test:{password}@127.0.0.1:{port}/postgres"


def docker_cluster(name, marker=None):
    marker = disposable_marker(marker)
    password = secrets.token_urlsafe(32)
    run(
        "docker",
        "run",
        "--detach",
        "--rm",
        "--name",
        name,
        "--publish",
        "127.0.0.1::5432",
        "--env",
        "POSTGRES_USER=p1_test",
        "--env",
        f"POSTGRES_PASSWORD={password}",
        "--env",
        "POSTGRES_DB=postgres",
        IMAGE,
        "-c",
        f"sark.disposable_marker={marker}",
    )
    port = run("docker", "port", name, "5432/tcp").stdout.strip().rsplit(":", 1)[1]
    for _ in range(60):
        # The image's temporary initialization server accepts Unix sockets only.
        # Wait for the final TCP server used by the host's database connection.
        result = subprocess.run(
            ["docker", "exec", name, "pg_isready", "--host", "127.0.0.1", "-U", "p1_test"],
            capture_output=True,
            timeout=5,
        )
        if result.returncode == 0:
            return f"postgresql://p1_test:{password}@127.0.0.1:{port}/postgres"
        time.sleep(0.5)
    raise ValueError("Disposable PostgreSQL did not become ready")


def exact_test_passed(stdout, name):
    lines = stdout.splitlines()
    summary = "test result: ok. 1 passed; 0 failed; 0 ignored;"
    return (
        lines.count("running 1 test") == 1
        and lines.count(f"test {name} ... ok") == 1
        and sum(line.startswith(summary) for line in lines) == 1
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--fsync-off", action="store_true", help="Start the owned native cluster with fsync off"
    )
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("A real command is required")
    binaries = Path(os.environ["P1_POSTGRES_BIN"]) if os.environ.get("P1_POSTGRES_BIN") else None
    if args.fsync_off:
        if binaries is None:
            parser.error("--fsync-off requires P1_POSTGRES_BIN for an owned native cluster")
        if (
            command[1:2] != ["--exact"]
            or command[3:] not in ([], ["--ignored"])
            or len(command) < 3
            or re.fullmatch(r"[A-Za-z0-9_]+(?:::[A-Za-z0-9_]+)*", command[2]) is None
        ):
            parser.error(
                "--fsync-off requires an exact Rust test: <binary> --exact <test> [--ignored]"
            )
    (ROOT / ".local").mkdir(exist_ok=True)
    name = "sark-p1-" + secrets.token_hex(8)
    marker = disposable_marker()
    with tempfile.TemporaryDirectory(prefix="sark-p1-pg-") as temporary:
        directory = Path(temporary)
        try:
            database = (
                native_cluster(directory, binaries, marker, disable_fsync=args.fsync_off)
                if binaries
                else docker_cluster(name, marker)
            )
            environment = {
                **{key: value for key, value in os.environ.items() if not key.startswith("PG")},
                "DATABASE_URL": database,
                "P1_POSTGRES_DISPOSABLE_MARKER": marker,
                "P1_POSTGRES_EXPECTED_FSYNC": "off" if args.fsync_off else "on",
                "APP_ENV": "test",
            }
            if args.fsync_off:
                result = subprocess.run(
                    command,
                    cwd=ROOT,
                    env=environment,
                    check=False,
                    stdout=subprocess.PIPE,
                    text=True,
                )
                print(result.stdout, end="")
                if result.returncode != 0:
                    return result.returncode
                if not exact_test_passed(result.stdout, command[2]):
                    raise ValueError("The exact server fsync control did not pass one test")
                return 0
            result = subprocess.run(command, cwd=ROOT, env=environment, check=False)
            return result.returncode
        finally:
            if binaries and (directory / "data/postmaster.pid").exists():
                run(str(binaries / "pg_ctl"), "-D", str(directory / "data"), "stop", "-m", "fast")
            elif not binaries:
                subprocess.run(
                    ["docker", "rm", "--force", "--volumes", name], capture_output=True, timeout=30
                )


if __name__ == "__main__":
    raise SystemExit(main())
