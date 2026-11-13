#!/usr/bin/env python3
"""Rehearse a synthetic restore in two disposable native PostgreSQL clusters."""

import hashlib
import json
import os
import re
import secrets
import selectors
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
MAX_SEQUENCES = 300


class OtherSourceClients(RuntimeError):
    """The owned source is not quiescent."""


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


def source_state(root):
    # Hooks can inherit a foreign worktree, index, or Git object directory.
    environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    git = ["git", "--no-replace-objects", "-C", str(root)]

    def read(*args):
        return run(*git, *args, env=environment).stdout

    revision = read("rev-parse", "HEAD").strip()
    tracked = read("ls-files", "-v", "-z").split("\0")
    clean = not read(
        "status", "--porcelain=v1", "--untracked-files=all", "--ignore-submodules=none"
    )
    clean = clean and all(not entry or entry.startswith("H ") for entry in tracked)
    clean = clean and read("rev-parse", "HEAD").strip() == revision
    return revision, clean


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
            "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY; "
            "SELECT pg_export_snapshot(); SELECT pg_backend_pid();\n"
        )
        self.process.stdin.flush()
        self.value = self.process.stdout.readline().strip()
        pid = self.process.stdout.readline().strip()
        if not re.fullmatch(r"[0-9A-Fa-f-]+", self.value) or not re.fullmatch(r"[1-9][0-9]*", pid):
            self.__exit__(None, None, None)
            raise RuntimeError("Could not export owned snapshot")
        self.pid = int(pid)
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


def quoted_identifier(value):
    return '"' + value.replace('"', '""') + '"'


def sequence_inventory(snapshot):
    # Names come from the held schema snapshot. Sequence values are explicitly
    # non-MVCC and are read separately only while the owned source is quiescent.
    rows = snapshot.query(
        "SELECT pg_catalog.json_build_array(n.nspname,c.relname)::text "
        "FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace "
        "WHERE n.nspname='public' AND c.relkind='S' ORDER BY c.relname COLLATE \"C\""
    ).splitlines()
    if not rows or len(rows) > MAX_SEQUENCES:
        raise ValueError("Unexpected synthetic sequence inventory")
    result = {}
    for row in rows:
        pair = json.loads(row)
        if (
            not isinstance(pair, list)
            or len(pair) != 2
            or pair[0] != "public"
            or not isinstance(pair[1], str)
        ):
            raise ValueError("Invalid synthetic sequence identity")
        qualified = ".".join(quoted_identifier(value) for value in pair)
        state = json.loads(
            snapshot.query(
                f"SELECT pg_catalog.json_build_array(last_value,is_called)::text FROM {qualified}"
            ).strip()
        )
        if (
            not isinstance(state, list)
            or len(state) != 2
            or type(state[0]) is not int
            or type(state[1]) is not bool
        ):
            raise ValueError("Invalid synthetic sequence state")
        if qualified in result:
            raise ValueError("Duplicate synthetic sequence identity")
        result[qualified] = state
    return result


def source_other_clients(snapshot):
    other_clients = snapshot.query(
        "SELECT count(*) FROM pg_catalog.pg_stat_activity "
        "WHERE datname=pg_catalog.current_database() AND backend_type='client backend' "
        f"AND pid NOT IN (pg_catalog.pg_backend_pid(),{snapshot.pid})"
    ).strip()
    if not re.fullmatch(r"0|[1-9][0-9]*", other_clients):
        raise RuntimeError("Synthetic source client count is invalid")
    return int(other_clients)


def require_quiescent_source(snapshot):
    if source_other_clients(snapshot):
        raise OtherSourceClients("Synthetic source has another database client")


def run_fixture(env, name):
    if name not in ("seed_synthetic", "verify_restored_keys"):
        raise ValueError("Unexpected synthetic fixture")
    result = subprocess.run(
        [
            "cargo",
            "test",
            "--locked",
            "-p",
            "commerce-infrastructure",
            "--lib",
            f"mollie::tests::restore_rehearsal::{name}",
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
    if result.returncode or not re.search(rb"test result: ok\. 1 passed; 0 failed;", result.stdout):
        raise RuntimeError("Synthetic restore fixture command failed")


def exercise_quiescence_control(snapshot):
    # The named client is held open, and its real backend identity must be visible
    # before interpreting the specific non-quiescent rejection as a success.
    require_quiescent_source(snapshot)
    client = subprocess.Popen(
        [str(snapshot.binaries / "psql"), "-XAtq", "-v", "ON_ERROR_STOP=1"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        env={**snapshot.env, "PGAPPNAME": "sark-restore-extra-client"},
        text=True,
    )
    try:
        client.stdin.write("SELECT pg_catalog.pg_backend_pid();\n")
        client.stdin.flush()
        with selectors.DefaultSelector() as selector:
            selector.register(client.stdout, selectors.EVENT_READ)
            if not selector.select(timeout=10):
                raise RuntimeError("Synthetic control client did not start")
        pid = client.stdout.readline().strip()
        if not re.fullmatch(r"[1-9][0-9]*", pid):
            raise RuntimeError("Synthetic control client identity is invalid")
        observed = snapshot.query(
            "SELECT count(*) FROM pg_catalog.pg_stat_activity "
            "WHERE datname=pg_catalog.current_database() AND backend_type='client backend' "
            f"AND pid={pid} AND application_name='sark-restore-extra-client'"
        ).strip()
        if observed != "1" or source_other_clients(snapshot) != 1:
            raise RuntimeError("Synthetic control client was not uniquely observed")
        try:
            require_quiescent_source(snapshot)
        except OtherSourceClients:
            pass
        else:
            raise RuntimeError("Synthetic extra-client negative control failed")
    finally:
        client.stdin.close()
        try:
            status = client.wait(timeout=10)
        except subprocess.TimeoutExpired:
            client.kill()
            client.wait(timeout=10)
            raise RuntimeError("Synthetic control client did not stop") from None
        finally:
            client.stdout.close()
        if status:
            raise RuntimeError("Synthetic control client failed")
    require_quiescent_source(snapshot)


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
    revision, clean_before = source_state(ROOT)
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
                **{key: value for key, value in os.environ.items() if not key.startswith("GIT_")},
                "RESTORE_FIXTURE_DATABASE_URL": urls[0],
                "RESTORE_FIXTURE_DATABASE_NAME": database,
            }
            run_fixture(env, "seed_synthetic")
            archive = root / "synthetic.dump"
            with Snapshot(binaries, pg_env(urls[0])) as source:
                exercise_quiescence_control(source)
                sequences_before = sequence_inventory(source)
                control_sequence = '"public"."stock_movements_sequence_seq"'
                control_state = sequences_before.get(control_sequence)
                if (
                    control_state is None
                    or not control_state[1]
                    or not (0 < control_state[0] < 9_000_000_000_000_000_000)
                ):
                    raise RuntimeError("Expected synthetic stock sequence was not exercised")
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
                require_quiescent_source(source)
                if sequence_inventory(source) != sequences_before:
                    raise RuntimeError("Synthetic source sequence changed during dump")
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
                    sequences_after = sequence_inventory(target)
                if before != after:
                    raise RuntimeError("Restored synthetic table content differs")
                if sequences_before != sequences_after:
                    raise RuntimeError("Restored synthetic sequence state differs")
                run_fixture(
                    {**env, "RESTORE_FIXTURE_DATABASE_URL": urls[1]}, "verify_restored_keys"
                )
                require_quiescent_source(source)
                if sequence_inventory(source) != sequences_before:
                    raise RuntimeError("Synthetic source sequence changed during comparison")
                # Alter a sequence only in the disposable restored database.
                psql(
                    binaries,
                    pg_env(urls[1]),
                    f"SELECT pg_catalog.setval('{control_sequence}'::pg_catalog.regclass,{control_state[0] + 1},true)",
                )
                with Snapshot(binaries, pg_env(urls[1])) as changed:
                    damaged_sequences = sequence_inventory(changed)
                expected_damage = dict(sequences_before)
                expected_damage[control_sequence] = [control_state[0] + 1, True]
                if damaged_sequences != expected_damage or damaged_sequences == sequences_before:
                    raise RuntimeError("Changed-sequence negative control failed")
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
            final_revision, clean_after = source_state(ROOT)
            unchanged = final_revision == revision
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
                "sequences_compared": len(sequences_before),
                "sequence_inventory_sha256": hashlib.sha256(
                    json.dumps(sequences_before, sort_keys=True, separators=(",", ":")).encode()
                ).hexdigest(),
                "source_sequence_quiescence": "owned source; no other clients at checks; before/after state unchanged",
                "changed_sequence_detected": True,
                "synthetic_restored_key_opening_checked": True,
                "extra_client_rejected": True,
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
