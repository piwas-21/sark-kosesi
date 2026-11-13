#!/usr/bin/env python3
"""Run one quote HTTP smoke only against a newly owned native PostgreSQL cluster."""

import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
from pathlib import Path

from with_postgres import ROOT, native_cluster, run


def stop_child(child):
    try:
        os.killpg(child.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        child.wait(timeout=10)
    except subprocess.TimeoutExpired:
        os.killpg(child.pid, signal.SIGKILL)
        child.wait()


def main():
    if len(sys.argv) != 1:
        raise SystemExit("The quote smoke accepts no arguments")
    configured = os.environ.get("P1_POSTGRES_BIN")
    if not configured or not Path(configured).is_absolute():
        raise SystemExit("An absolute P1_POSTGRES_BIN is required")
    binaries = Path(configured)
    allowed = ("PATH", "HOME", "TMPDIR", "CARGO_HOME", "RUSTUP_HOME", "CARGO_TARGET_DIR")
    environment = {key: os.environ[key] for key in allowed if key in os.environ}
    os.environ.clear()
    os.environ.update(environment)
    version = run(str(binaries / "postgres"), "--version").stdout.strip()
    if not version.startswith("postgres (PostgreSQL) 18."):
        raise SystemExit("The quote smoke requires native PostgreSQL 18")
    directory = Path(tempfile.mkdtemp(prefix="sark-quote-smoke-"))
    try:
        database = native_cluster(directory, binaries)
        proof = directory / "owned-db.json"
        descriptor = os.open(proof, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            json.dump({"database_url": database}, output)
        # Do not pass unrelated endpoints, provider configuration, proxies or
        # customer credentials to the test process or the API it starts.
        environment.update(
            {
                "APP_ENV": "test",
                "DATABASE_URL": database,
                "SARK_QUOTE_OWNED_DB_PROOF": str(proof),
                "CARGO_BUILD_JOBS": "2",
                "CARGO_INCREMENTAL": "0",
            }
        )
        child = subprocess.Popen(
            [
                "cargo",
                "test",
                "--release",
                "--offline",
                "--locked",
                "-p",
                "commerce-api",
                "--test",
                "quote_http_smoke",
                "--",
                "--ignored",
                "--exact",
                "owned_quote_http_smoke",
            ],
            cwd=ROOT,
            env=environment,
            start_new_session=True,
        )
        try:
            return child.wait(timeout=1800)
        except subprocess.TimeoutExpired:
            stop_child(child)
            raise RuntimeError("Quote smoke exceeded its deadline") from None
        except BaseException:
            stop_child(child)
            raise
    finally:
        pidfile = directory / "data/postmaster.pid"
        if pidfile.exists():
            # A stop failure must not erase a potentially live cluster.
            try:
                run(str(binaries / "pg_ctl"), "-D", str(directory / "data"), "stop", "-m", "fast")
            except RuntimeError:
                raise RuntimeError(f"Owned PostgreSQL stop failed; retained {directory}") from None
        if pidfile.exists():
            raise RuntimeError(f"Owned PostgreSQL stop unconfirmed; retained {directory}")
        shutil.rmtree(directory)


if __name__ == "__main__":
    raise SystemExit(main())
