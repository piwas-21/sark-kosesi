#!/usr/bin/env python3
"""Run one quote HTTP smoke only against a newly owned native PostgreSQL cluster."""

import hashlib
import json
import os
import platform
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from quote_load_observations import evaluate
from with_postgres import ROOT, native_cluster, run

SMOKE_CASE = "owned_quote_http_smoke"
PREPARE_CASE = "quote_load_preparation::owned_quote_load_preparation"
LOAD_CASE = "quote_load_preparation::owned_quote_load"


def require_completed_case(output, selected_case):
    end = output.seek(0, os.SEEK_END)
    output.seek(max(0, end - 4096))
    lines = output.read(4096).splitlines()
    if lines.count(f"test {selected_case} ... ok".encode()) != 1 or not any(
        line.startswith(b"test result: ok. 1 passed; 0 failed;") for line in lines
    ):
        raise RuntimeError("Expected exactly one successful quote fixture case")


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


def source_state():
    return (
        run("git", "-C", str(ROOT), "rev-parse", "HEAD").stdout.strip(),
        not run(
            "git", "-C", str(ROOT), "status", "--porcelain", "--untracked-files=normal"
        ).stdout.strip(),
    )


def load_evidence(directory, binaries, k6_version, source_at_start):
    report = directory / "quote-observations.json"
    if report.is_symlink() or not report.is_file() or report.stat().st_size > 2_000_000:
        raise RuntimeError("Missing or unbounded quote observations")
    with report.open(encoding="utf-8") as source:
        evidence = json.load(source)
    observations = evidence["observations"]
    resources = evidence["resources"]
    if set(resources) != {
        "api_peak_sampled_rss_kib",
        "database_bytes_before",
        "database_bytes_after",
        "rss_sample_seconds",
    } or any(type(value) is not int or value <= 0 for value in resources.values()):
        raise RuntimeError("Invalid quote resource observations")
    metrics = evaluate(observations)
    seed_paths = (
        "crates/infrastructure/tests/support/checkout_quote_fixture.rs",
        "crates/api/tests/support/quote_load_preparation.rs",
    )
    revision, tree_clean = source_state()
    if revision != source_at_start[0]:
        raise RuntimeError("Quote workload source revision changed during execution")
    return {
        "scope": "local isolated PostgreSQL and Rust release API; synthetic NL quote",
        "revision": revision,
        "source_tree_clean": tree_clean and source_at_start[1],
        "seed_sha256": {
            name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in seed_paths
        },
        "hardware": {
            "platform": platform.system(),
            "release": platform.release(),
            "architecture": platform.machine(),
            "logical_cpus": os.cpu_count(),
            "memory_bytes": os.sysconf("SC_PHYS_PAGES") * os.sysconf("SC_PAGE_SIZE"),
        },
        "cache_state": "fresh DB and API warmed by synthetic guest and basket setup",
        "postgres": run(str(binaries / "postgres"), "--version").stdout.strip(),
        "k6": k6_version,
        "database_and_api_resources": resources,
        "metrics": metrics,
        "duration_ms": observations["burst_end_ms"] - observations["steady_start_ms"],
        "committed_quotes": observations["quote_count_after"],
        "unchanged_stock_and_commerce_ledgers": True,
    }


def run_smoke(stop):
    if sys.argv[1:] == []:
        selected_case = SMOKE_CASE
    elif sys.argv[1:] == ["--prepare-load"]:
        selected_case = PREPARE_CASE
    elif sys.argv[1:] == ["--run-load"]:
        selected_case = LOAD_CASE
    else:
        raise SystemExit("Unsupported quote fixture command")
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
    k6_version = None
    source_at_start = None
    if selected_case == LOAD_CASE:
        tool = ROOT / ".local/bin/k6"
        if tool.is_symlink() or not tool.is_file():
            raise SystemExit("Install the checksum-pinned k6 in .local/bin before quote load")
        pin = json.loads((ROOT / "config/tools.json").read_text())["k6"]["version"]
        k6_version = run(str(tool), "version").stdout.strip()
        if not k6_version.startswith(f"k6 v{pin} "):
            raise SystemExit("The installed k6 version does not match config/tools.json")
        source_at_start = source_state()
    stop.check()
    directory = Path(tempfile.mkdtemp(prefix="sark-quote-smoke-"))
    drained = True
    startup_confirmed = False
    evidence = None
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
        with tempfile.TemporaryFile(dir=directory) as output:
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
                    "--format",
                    "pretty",
                    "--color",
                    "never",
                    "--ignored",
                    "--exact",
                    selected_case,
                ],
                cwd=ROOT,
                env=environment,
                stdin=subprocess.DEVNULL,
                stdout=output,
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
                        result = child.wait(timeout=0.2)
                        break
                    except subprocess.TimeoutExpired:
                        continue
            finally:
                drained = drain_group(child)
            if result == 0 and drained:
                require_completed_case(output, selected_case)
                if selected_case == LOAD_CASE:
                    evidence = load_evidence(directory, binaries, k6_version, source_at_start)
    finally:
        if not drained:
            raise RuntimeError(f"Owned process group not drained; retained {directory}")
        pidfile = directory / "data/postmaster.pid"
        if pidfile.exists():
            # A stop failure must not erase a potentially live cluster.
            try:
                run(str(binaries / "pg_ctl"), "-D", str(directory / "data"), "stop", "-m", "fast")
            except RuntimeError:
                raise RuntimeError(f"Owned PostgreSQL stop failed; retained {directory}") from None
        if pidfile.exists():
            raise RuntimeError(f"Owned PostgreSQL stop unconfirmed; retained {directory}")
        if not startup_confirmed:
            raise RuntimeError(f"Owned PostgreSQL startup unconfirmed; retained {directory}")
        shutil.rmtree(directory)
    # A signal received while draining or stopping PostgreSQL is still failure.
    stop.check()
    if evidence is not None:
        print(json.dumps(evidence, sort_keys=True))
        if not all(group["within_budget"] for group in evidence["metrics"].values()):
            return 1
    if result == 0:
        print(f"One owned quote fixture case passed: {selected_case}")
    return result


if __name__ == "__main__":
    raise SystemExit(main())
