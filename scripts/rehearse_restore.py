#!/usr/bin/env python3
"""Rehearse a synthetic restore in two disposable native PostgreSQL clusters."""

import hashlib
import json
import os
import re
import secrets
import shutil
import subprocess
import sys
import tempfile
import time
from contextlib import contextmanager
from pathlib import Path
from urllib.parse import urlsplit

from with_postgres import ROOT, native_cluster, run

MAX_TABLES = 300
MAX_ROWS = 20000
MAX_TABLE_BYTES = 64 * 1024 * 1024


@contextmanager
def owned_directory():
    root = Path(tempfile.mkdtemp(prefix="sark-restore-"))
    try:
        yield root
    finally:
        if any((root / name / "data" / "postmaster.pid").exists() for name in ("source", "target")):
            print(f"Owned temporary clusters retained at {root}", file=sys.stderr)
        else:
            shutil.rmtree(root)


def pg_env(url):
    parsed = urlsplit(url)
    if parsed.hostname != "127.0.0.1" or parsed.username != "p1_test":
        raise ValueError("Unexpected owned database connection")
    return {
        **{k: v for k, v in os.environ.items() if not k.startswith("PG")},
        "PGHOST": parsed.hostname,
        "PGPORT": str(parsed.port),
        "PGUSER": parsed.username,
        "PGPASSWORD": parsed.password,
        "PGDATABASE": parsed.path[1:],
        "PGCONNECT_TIMEOUT": "5",
        "PGOPTIONS": "-c timezone=UTC -c statement_timeout=60000",
    }


def psql(binaries, env, sql):
    return run(str(binaries / "psql"), "-XAtq", "-v", "ON_ERROR_STOP=1", "-c", sql, env=env).stdout


def digest_file(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


class Snapshot:
    def __init__(self, binaries, env):
        self.binaries = binaries
        self.env = env
        self.process = None

    def __enter__(self):
        self.process = subprocess.Popen(
            [str(self.binaries / "psql"), "-XAtq", "-v", "ON_ERROR_STOP=1"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            env=self.env,
            text=True,
        )
        self.process.stdin.write(
            "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY; SELECT pg_export_snapshot();\n"
        )
        self.process.stdin.flush()
        self.value = self.process.stdout.readline().strip()
        if not re.fullmatch(r"[0-9A-Fa-f-]+", self.value):
            self.__exit__(None, None, None)
            raise RuntimeError("Could not export owned snapshot")
        return self

    def __exit__(self, *_):
        if self.process:
            self.process.stdin.close()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=10)
            self.process.stdout.close()

    def query(self, sql):
        return psql(self.binaries, self.env, self.prefix() + sql + "; COMMIT")

    def prefix(self):
        return f"BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY; SET TRANSACTION SNAPSHOT '{self.value}'; "


def table_digest(snapshot, table):
    sql = snapshot.prefix() + (
        f"COPY (SELECT row_to_json(t)::text AS value FROM public.{table} t "
        'ORDER BY row_to_json(t)::text COLLATE "C") TO STDOUT; COMMIT'
    )
    process = subprocess.Popen(
        [str(snapshot.binaries / "psql"), "-XAtq", "-v", "ON_ERROR_STOP=1", "-c", sql],
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        env=snapshot.env,
    )
    digest = hashlib.sha256()
    count = size = 0
    try:
        for line in process.stdout:
            count += 1
            size += len(line)
            if count > MAX_ROWS or size > MAX_TABLE_BYTES:
                raise ValueError("Synthetic table exceeds rehearsal bounds")
            digest.update(line)
        if process.wait(timeout=60):
            raise RuntimeError("Synthetic table comparison failed")
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=10)
        process.stdout.close()
    return count, digest.hexdigest()


def inventory(snapshot):
    names = snapshot.query(
        "SELECT c.relname FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace "
        "WHERE n.nspname='public' AND c.relkind IN ('r','p') ORDER BY c.relname COLLATE \"C\""
    ).splitlines()
    if (
        not names
        or len(names) > MAX_TABLES
        or any(not re.fullmatch(r"[a-z_][a-z0-9_]*", n) for n in names)
    ):
        raise ValueError("Unexpected synthetic table inventory")
    return {name: table_digest(snapshot, name) for name in names}


def main():
    binaries = Path(os.environ.get("P1_POSTGRES_BIN", ""))
    if not binaries.is_absolute() or not all(
        (binaries / name).is_file()
        for name in ("initdb", "pg_ctl", "psql", "pg_dump", "pg_restore")
    ):
        raise ValueError("P1_POSTGRES_BIN must identify the native PostgreSQL 18 binaries")
    version = run(str(binaries / "pg_dump"), "--version").stdout.strip()
    if not re.match(r"pg_dump \(PostgreSQL\) 18\.", version):
        raise ValueError("Native PostgreSQL 18 is required")
    git = ["git", "--no-replace-objects", "-C", str(ROOT)]
    revision = run(*git, "rev-parse", "HEAD").stdout.strip()
    clean_before = not run(*git, "status", "--porcelain", "--untracked-files=all").stdout
    database = "sark_restore_" + secrets.token_hex(16)
    started = time.monotonic()
    with owned_directory() as root:
        roots = [root / "source", root / "target"]
        urls = []
        try:
            for directory in roots:
                directory.mkdir(mode=0o700)
                url = native_cluster(directory, binaries)
                psql(binaries, pg_env(url), f'CREATE DATABASE "{database}"')
                urls.append(url.rsplit("/", 1)[0] + "/" + database)
            env = {
                **os.environ,
                "RESTORE_FIXTURE_DATABASE_URL": urls[0],
                "RESTORE_FIXTURE_DATABASE_NAME": database,
            }
            result = subprocess.run(
                [
                    "cargo",
                    "test",
                    "--locked",
                    "-p",
                    "commerce-infrastructure",
                    "--lib",
                    "mollie::tests::restore_rehearsal::seed_synthetic",
                    "--",
                    "--ignored",
                    "--exact",
                ],
                cwd=ROOT,
                env=env,
                capture_output=True,
                timeout=1200,
                check=False,
            )
            if result.returncode or b"1 passed" not in result.stdout:
                raise RuntimeError("Synthetic migration and fixture command failed")
            archive = root / "synthetic.dump"
            with Snapshot(binaries, pg_env(urls[0])) as source:
                before = inventory(source)
                for table in (
                    "_sqlx_migrations",
                    "checkout_orders",
                    "stock_movements",
                    "email_verification_delivery_envelopes",
                ):
                    if table not in before or before[table][0] == 0:
                        raise RuntimeError("Required synthetic fixture evidence is absent")
                migration_count = len(list((ROOT / "migrations").glob("*.sql")))
                if (
                    before["_sqlx_migrations"][0] != migration_count
                    or source.query(
                        "SELECT count(*) FROM _sqlx_migrations WHERE NOT success"
                    ).strip()
                    != "0"
                ):
                    raise RuntimeError("Synthetic migration history is incomplete")
                run(
                    str(binaries / "pg_dump"),
                    "--format=custom",
                    "--no-owner",
                    "--no-acl",
                    "--snapshot",
                    source.value,
                    "--file",
                    str(archive),
                    env=source.env,
                )
                archive_hash = digest_file(archive)
                restore_started = time.monotonic()
                run(
                    str(binaries / "pg_restore"),
                    "--exit-on-error",
                    "--no-owner",
                    "--no-acl",
                    "--dbname",
                    database,
                    str(archive),
                    env=pg_env(urls[1]),
                )
                restore_seconds = round(time.monotonic() - restore_started, 3)
                with Snapshot(binaries, pg_env(urls[1])) as target:
                    after = inventory(target)
                if before != after:
                    raise RuntimeError("Restored synthetic table content differs")
                # Deliberately add a harmless fixture row only to the disposable
                # restored copy and prove the comparison detects changed content.
                psql(
                    binaries,
                    pg_env(urls[1]),
                    "INSERT INTO catalogue_items (sku,slug,minor_units,currency,fill,fixture) VALUES ('DEMO-RESTORE-CONTROL','restore-control',42,'EUR','covers-only',true)",
                )
                with Snapshot(binaries, pg_env(urls[1])) as changed:
                    changed_table = table_digest(changed, "catalogue_items")
                if (
                    changed_table[0] != before["catalogue_items"][0] + 1
                    or changed_table[1] == before["catalogue_items"][1]
                ):
                    raise RuntimeError("Changed-content negative control failed")
                corrupted = root / "corrupt.dump"
                with archive.open("rb") as original, corrupted.open("wb") as changed:
                    changed.write(b"X")
                    original.read(1)
                    for chunk in iter(lambda: original.read(1024 * 1024), b""):
                        changed.write(chunk)
                if digest_file(corrupted) == archive_hash:
                    raise RuntimeError("Corrupt archive hash negative control failed")
                rejected = subprocess.run(
                    [str(binaries / "pg_restore"), "--list", str(corrupted)],
                    capture_output=True,
                    timeout=60,
                    check=False,
                )
                if rejected.returncode == 0:
                    raise RuntimeError("Corrupt archive parser negative control failed")
            unchanged = run(*git, "rev-parse", "HEAD").stdout.strip() == revision
            clean_after = not run(*git, "status", "--porcelain", "--untracked-files=all").stdout
            report = {
                "source_revision": revision,
                "source_clean_at_checks": clean_before and clean_after and unchanged,
                "postgres": version,
                "tables_compared": len(before),
                "rows_compared": sum(value[0] for value in before.values()),
                "migration_rows": before["_sqlx_migrations"][0],
                "migration_history_sha256": before["_sqlx_migrations"][1],
                "table_inventory_sha256": hashlib.sha256(
                    json.dumps(before, sort_keys=True, separators=(",", ":")).encode()
                ).hexdigest(),
                "archive_sha256": archive_hash,
                "restore_seconds": restore_seconds,
                "total_seconds": round(time.monotonic() - started, 3),
                "corrupt_archive_rejected": True,
                "changed_content_detected": True,
                "scope": "isolated synthetic local data only",
            }
        finally:
            failed = False
            for directory in roots:
                if (directory / "data" / "postmaster.pid").exists():
                    try:
                        run(
                            str(binaries / "pg_ctl"),
                            "-D",
                            str(directory / "data"),
                            "stop",
                            "-m",
                            "fast",
                        )
                    except (RuntimeError, OSError):
                        failed = True
                    if (directory / "data" / "postmaster.pid").exists():
                        failed = True
            if failed:
                raise RuntimeError("Could not stop owned temporary clusters")
    print(json.dumps(report, sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, ValueError, OSError, subprocess.SubprocessError):
        raise SystemExit("Synthetic restore rehearsal failed; sensitive details withheld") from None
