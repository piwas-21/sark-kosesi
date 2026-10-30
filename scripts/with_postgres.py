#!/usr/bin/env python3
"""Run a command against a disposable, owned PostgreSQL cluster only."""

import argparse
import os
import secrets
import socket
import subprocess
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
IMAGE = "postgres:18@sha256:edb0c08dc33befd0262ab3b5cf1c4f3f8cfd8b09e88c05b45adee29d6761ddb6"


def run(*args, **kwargs):
    try:
        return subprocess.run(
            args, check=True, text=True, capture_output=True, timeout=60, **kwargs
        )
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
        raise RuntimeError(f"{Path(args[0]).name} failed; command details withheld") from None


def native_cluster(directory, binaries):
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
    run(
        str(binaries / "pg_ctl"),
        "-D",
        str(data),
        "-l",
        str(directory / "postgres.log"),
        "-o",
        f"-c listen_addresses=127.0.0.1 -c unix_socket_directories='' -p {port}",
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


def docker_cluster(name):
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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("A real command is required")
    (ROOT / ".local").mkdir(exist_ok=True)
    name = "sark-p1-" + secrets.token_hex(8)
    binaries = Path(os.environ["P1_POSTGRES_BIN"]) if os.environ.get("P1_POSTGRES_BIN") else None
    with tempfile.TemporaryDirectory(prefix="sark-p1-pg-") as temporary:
        directory = Path(temporary)
        try:
            database = native_cluster(directory, binaries) if binaries else docker_cluster(name)
            environment = {
                **{key: value for key, value in os.environ.items() if not key.startswith("PG")},
                "DATABASE_URL": database,
                "APP_ENV": "test",
            }
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
