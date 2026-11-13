#!/usr/bin/env python3
"""Run one quote HTTP smoke only against a newly owned native PostgreSQL cluster."""

import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from with_postgres import ROOT, native_cluster, run


def group_present(pgid):
    try:
        os.killpg(pgid, 0)
    except ProcessLookupError:
        return False
    except OSError:
        return None
    return True


def drain_group(child, grace=2.0):
    """Drain the owned session, including descendants after the leader exits."""
    for requested in (signal.SIGTERM, signal.SIGKILL):
        child.poll()  # Reap the leader before testing whether its group remains.
        present = group_present(child.pid)
        if present is False:
            return True
        if present is None:
            return False
        try:
            os.killpg(child.pid, requested)
        except ProcessLookupError:
            return True
        except OSError:
            return False
        deadline = time.monotonic() + grace
        while time.monotonic() < deadline:
            child.poll()
            present = group_present(child.pid)
            if present is False:
                return True
            if present is None:
                return False
            time.sleep(0.02)
    child.poll()
    return group_present(child.pid) is False


class StopRequest:
    def __init__(self):
        self.received = False

    def handle(self, _signum, _frame):
        # Defer handling during native_cluster's bounded startup so a postgres
        # process cannot be abandoned just before its pidfile is created.
        self.received = True

    def check(self):
        if self.received:
            raise InterruptedError("Quote smoke interrupted")


def main():
    stop = StopRequest()
    previous = {sig: signal.signal(sig, stop.handle) for sig in (signal.SIGINT, signal.SIGTERM)}
    try:
        return run_smoke(stop)
    finally:
        for sig, handler in previous.items():
            signal.signal(sig, handler)


def run_smoke(stop):
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
    stop.check()
    directory = Path(tempfile.mkdtemp(prefix="sark-quote-smoke-"))
    drained = True
    startup_confirmed = False
    try:
        database = native_cluster(directory, binaries)
        startup_confirmed = True
        stop.check()
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
        stop.check()
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
        drained = False
        try:
            deadline = time.monotonic() + 1800
            while True:
                stop.check()
                if time.monotonic() >= deadline:
                    raise RuntimeError("Quote smoke exceeded its deadline")
                try:
                    return child.wait(timeout=0.2)
                except subprocess.TimeoutExpired:
                    continue
        finally:
            drained = drain_group(child)
    finally:
        if not drained:
            raise RuntimeError(f"Owned process group not drained; retained {directory}")
        if not startup_confirmed:
            raise RuntimeError(f"Owned PostgreSQL startup unconfirmed; retained {directory}")
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
