#!/usr/bin/env python3
"""Run a command against a disposable, owned PostgreSQL cluster only."""

import argparse
import os
import secrets
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
    data, sockets = directory / "data", directory / "socket"
    sockets.mkdir(mode=0o700)
    run(
        str(binaries / "initdb"),
        "-D",
        str(data),
        "-U",
        "p1_test",
        "-A",
        "trust",
        "--no-locale",
        "--encoding=UTF8",
    )
    run(
        str(binaries / "pg_ctl"),
        "-D",
        str(data),
        "-l",
        str(directory / "postgres.log"),
        "-o",
        f"-c listen_addresses='' -k {sockets} -p 56541",
        "start",
    )
    return f"postgresql://p1_test@localhost/postgres?host={sockets}&port=56541"


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
        result = subprocess.run(
            ["docker", "exec", name, "pg_isready", "-U", "p1_test"], capture_output=True, timeout=5
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
    with tempfile.TemporaryDirectory(prefix="sark-p1-pg-", dir="/tmp") as temporary:
        directory = Path(temporary)
        try:
            database = native_cluster(directory, binaries) if binaries else docker_cluster(name)
            environment = {**os.environ, "DATABASE_URL": database, "APP_ENV": "test"}
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
