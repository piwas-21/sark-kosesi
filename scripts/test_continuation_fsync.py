#!/usr/bin/env python3
"""Run one fsync control on an exclusively owned disposable native cluster."""

import os
import platform
import secrets
import shutil
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from with_postgres import IMAGE, ROOT, exact_test_passed, native_cluster, run

TEST = (
    "mollie::tests::test_continuation_custody::receipt_cases::"
    "continuation_exclusive_fsync_off_denies_starts_and_receipt"
)


def validate_list(lines):
    if lines.count(f"{TEST}: test") != 1:
        raise ValueError("The exact ignored control is absent")


def validate_result(returncode, output):
    if returncode != 0 or not exact_test_passed(output, TEST):
        raise ValueError("Exclusive fsync control did not pass exactly one named test")


def remove_docker_data(directory):
    # The official image owns its data as a container UID. Remove only the
    # wrapper-created data subdirectory, with networking disabled, after stop.
    run(
        "docker",
        "run",
        "--rm",
        "--network",
        "none",
        "--mount",
        f"type=bind,src={directory},dst={directory}",
        "--user",
        "0:0",
        "--entrypoint",
        "/bin/rm",
        IMAGE,
        "-rf",
        "--",
        str(directory / "data"),
    )


def docker_cluster(directory, marker):
    """Linux host networking and identical bind path preserve the native guard."""
    if platform.system() != "Linux":
        raise ValueError("Exclusive Docker control requires Linux host networking")
    directory.chmod(0o755)
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    name = "sark-fsync-" + secrets.token_hex(16)
    password = secrets.token_urlsafe(32)
    data = directory / "data"
    try:
        run(
            "docker",
            "run",
            "--detach",
            "--rm",
            "--name",
            name,
            "--label",
            f"piwas.sark.exclusive={marker}",
            "--network",
            "host",
            "--mount",
            f"type=bind,src={directory},dst={directory}",
            "--env",
            "POSTGRES_USER=p1_test",
            "--env",
            f"POSTGRES_PASSWORD={password}",
            "--env",
            "POSTGRES_DB=postgres",
            "--env",
            f"PGDATA={data}",
            IMAGE,
            "-c",
            "listen_addresses=127.0.0.1",
            "-c",
            f"port={port}",
            "-c",
            f"sark.disposable_marker={marker}",
        )
        for _ in range(60):
            ready = subprocess.run(
                [
                    "docker",
                    "exec",
                    name,
                    "pg_isready",
                    "-h",
                    "127.0.0.1",
                    "-p",
                    str(port),
                    "-U",
                    "p1_test",
                ],
                capture_output=True,
                timeout=5,
            )
            if ready.returncode == 0:
                return name, f"postgresql://p1_test:{password}@127.0.0.1:{port}/postgres"
            time.sleep(0.5)
        raise ValueError("Exclusive PostgreSQL did not become ready")
    except BaseException:
        # The launcher can fail after creating the container. Only act on an
        # exact per-run label; never remove a pre-existing colliding name.
        try:
            inspect = subprocess.run(
                [
                    "docker",
                    "inspect",
                    "--format",
                    '{{index .Config.Labels "piwas.sark.exclusive"}}',
                    name,
                ],
                capture_output=True,
                text=True,
                timeout=10,
            )
            if inspect.returncode == 0 and inspect.stdout.strip() == marker:
                run("docker", "rm", "--force", name)
                remove_docker_data(directory)
            else:
                # An ambiguous launcher may still be creating the container,
                # even when this immediate inspection reports no such object.
                print(
                    "Exclusive Docker cleanup could not verify the owned container; data retained",
                    file=sys.stderr,
                )
        except Exception:
            print("Exclusive Docker cleanup failed after launch error", file=sys.stderr)
        raise


def cluster_directory():
    return Path(tempfile.mkdtemp(prefix="sark-continuation-fsync-")).resolve()


def native_binaries():
    configured = os.environ.get("P1_POSTGRES_BIN")
    if not configured:
        if platform.system() != "Linux":
            raise ValueError("Exclusive Docker control requires Linux host networking")
        return None
    binaries = Path(configured)
    if not binaries.is_absolute() or not (binaries / "postgres").is_file():
        raise ValueError("Native PostgreSQL 18.1 binaries are required")
    version = subprocess.run(
        [str(binaries / "postgres"), "--version"], check=True, capture_output=True, text=True
    ).stdout.strip()
    if version != "postgres (PostgreSQL) 18.1":
        raise ValueError("Native PostgreSQL 18.1 is required")
    return binaries


def stop_cluster(directory, binaries, container, native_ready):
    # Delete the data only after the owned process has been stopped.
    if container:
        run("docker", "rm", "--force", container)
        remove_docker_data(directory)
    elif binaries and (directory / "data/postmaster.pid").is_file():
        # Startup may have failed after launching the owned server.
        run(str(binaries / "pg_ctl"), "-D", str(directory / "data"), "stop", "-m", "fast")
    elif binaries and native_ready:
        raise ValueError("Native cluster stop could not be confirmed; data retained")
    else:
        # A failed launcher can still have a server starting without a visible PID or container.
        print("Exclusive cluster directory retained after launch failure", file=sys.stderr)
        return
    shutil.rmtree(directory)


def main():
    if len(sys.argv) != 2:
        raise ValueError("One compiled test executable is required")
    binary = Path(sys.argv[1])
    if not binary.is_absolute() or binary.is_symlink() or not binary.is_file():
        raise ValueError("The test executable must be an absolute regular file")
    binaries = native_binaries()
    # Instrumented libtest also emits a profile for --list. Isolate and discard
    # it so only the executed control contributes to the producer's profile.
    with tempfile.TemporaryDirectory(prefix="sark-fsync-list-") as listing_dir:
        listing_env = {**os.environ, "LLVM_PROFILE_FILE": str(Path(listing_dir) / "%p-%m.profraw")}
        listing = subprocess.run(
            [str(binary), "--list", "--ignored"],
            check=True,
            capture_output=True,
            text=True,
            env=listing_env,
        ).stdout.splitlines()
        validate_list(listing)
    marker = secrets.token_hex(32)
    directory = cluster_directory()
    container = None
    native_ready = False
    try:
        if binaries:
            database = native_cluster(directory, binaries, marker)
            native_ready = True
        else:
            container, database = docker_cluster(directory, marker)
        environment = {
            **{key: value for key, value in os.environ.items() if not key.startswith("PG")},
            "DATABASE_URL": database,
            "P1_POSTGRES_DISPOSABLE_MARKER": marker,
            "SARK_EXCLUSIVE_FSYNC_DATA": str(directory / "data"),
            "APP_ENV": "test",
        }
        result = subprocess.run(
            [str(binary), "--exact", TEST, "--ignored", "--test-threads=1"],
            cwd=ROOT,
            env=environment,
            capture_output=True,
            text=True,
            timeout=120,
            check=False,
        )
        validate_result(result.returncode, result.stdout)
    finally:
        active_error = sys.exc_info()[0] is not None
        try:
            stop_cluster(directory, binaries, container, native_ready)
        except Exception:
            if not active_error:
                raise
            print("Exclusive cluster cleanup failed; original error preserved", file=sys.stderr)
    print("Exclusive disposable PostgreSQL fsync control: 1 passed")


if __name__ == "__main__":
    main()
