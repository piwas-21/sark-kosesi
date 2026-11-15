#!/usr/bin/env python3
"""Restore project synthetic data into the exact hosted Alpine Gosu candidate."""

import hashlib
import json
import os
import re
import secrets
import subprocess
import sys
import time
from pathlib import Path

from rehearse_alpine_schema66 import run_rehearsal as run_schema66
from rehearse_restore import (
    Snapshot,
    digest_file,
    exercise_quiescence_control,
    inventory,
    pg_env,
    psql,
    require_quiescent_source,
    run_fixture,
    sequence_inventory,
    source_state,
)
from restore_loopback import IsolationFailure, RelayServer, verified_addresses
from with_postgres import ROOT, run

BASE_ID = "sha256:3cccc725163a95947178f4da5e9bf16add75cb286170ac5a2cd2f6513534955d"
ISOLATION_CODES = frozenset(
    {
        "network_inspect_shape",
        "network_identity_or_membership",
        "network_subnet_shape",
        "network_subnet_policy",
        "container_inspect_shape",
        "container_identity_or_state",
        "container_host_bindings",
        "container_ports_shape",
        "container_published_ports",
        "container_network_identity",
        "container_ipv4_identity",
        "duplicate_container_ipv4",
    }
)


class Progress:
    """Only fixed stage and failure codes may leave the private lab."""

    def __init__(self):
        self.started = time.monotonic()
        self.stage_started = self.started
        self.stage = "arguments"
        self.directory = None
        self.cleanup = "not_started"
        self.stage_finished = None

    def enter(self, stage):
        self.stage = stage
        self.stage_started = time.monotonic()
        self.stage_finished = None

    def finish_stage(self):
        self.stage_finished = time.monotonic()

    def result(self, error=None):
        if error is None:
            code = "ok"
        elif isinstance(error, IsolationFailure) and str(error) in ISOLATION_CODES:
            code = str(error)
        elif isinstance(error, subprocess.TimeoutExpired):
            code = "timeout"
        elif isinstance(error, KeyError):
            code = "metadata_missing"
        elif isinstance(error, TypeError):
            code = "metadata_type"
        elif isinstance(error, OSError):
            code = "os_error"
        elif isinstance(error, ValueError):
            code = "invariant_failed"
        else:
            code = "operation_failed"
        return {
            "outcome": "success" if error is None else "failed",
            "stage": self.stage,
            "code": code,
            "stage_elapsed_seconds": round(
                (self.stage_finished or time.monotonic()) - self.stage_started, 3
            ),
            "total_elapsed_seconds": round(time.monotonic() - self.started, 3),
            "cleanup": self.cleanup,
        }

    def write(self, error=None):
        if self.directory is not None:
            (self.directory / "app-restore-status.json").write_text(
                json.dumps(self.result(error), sort_keys=True) + "\n"
            )


def client():
    # The executable wrapper only permits the two random, owned container IDs.
    ports = json.loads(os.environ["RESTORE_OWNED_PORTS"])
    container = ports.get(os.environ.get("PGPORT"))
    if not container or not re.fullmatch(r"[0-9a-f]{64}", container):
        raise ValueError("Unowned database client")
    if os.environ.get("PGHOST") != "127.0.0.1" or os.environ.get("PGUSER") != "p1_test":
        raise ValueError("Unowned database connection")
    args = ["docker", "exec", "-i", "-e", "PGHOST=127.0.0.1", "-e", "LC_ALL=C"]
    for name in ("PGUSER", "PGPASSWORD", "PGDATABASE", "PGOPTIONS", "PGAPPNAME"):
        if name in os.environ:
            args.extend(["-e", name])
    os.execvp("docker", [*args, container, "psql", *sys.argv[1:]])


def validate_context(candidate, revision, scratch, evidence):
    if (
        os.environ.get("GITHUB_ACTIONS") != "true"
        or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted"
    ):
        raise ValueError("GitHub-hosted runner required")
    temp = Path(os.environ["RUNNER_TEMP"]).resolve(strict=True)
    if scratch != temp / "db-alpine-gosu-scratch" or evidence != temp / "db-alpine-gosu-evidence":
        raise ValueError("Unexpected hosted directories")
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", candidate) or candidate == BASE_ID:
        raise ValueError("Invalid candidate image identity")
    head, clean = source_state(ROOT)
    if not clean or head != revision or not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError("Source revision is not clean")


def start_container(image, network, name, marker, password):
    env = {**os.environ, "POSTGRES_PASSWORD": password}
    result = run(
        "docker",
        "run",
        "--detach",
        "--name",
        name,
        "--label",
        f"sark.restore.owner={marker}",
        "--network",
        network,
        "--memory",
        "1g",
        "--cpus",
        "1",
        "--pids-limit",
        "256",
        "--tmpfs",
        "/var/lib/postgresql:rw,size=512m",
        "--env",
        "POSTGRES_USER=p1_test",
        "--env",
        "POSTGRES_PASSWORD",
        "--env",
        "POSTGRES_DB=postgres",
        "--env",
        "POSTGRES_INITDB_ARGS=--auth-host=scram-sha-256",
        "--env",
        "POSTGRES_HOST_AUTH_METHOD=scram-sha-256",
        image,
        "-c",
        f"sark.disposable_marker={marker}",
        env=env,
    )
    container = result.stdout.strip()
    if not re.fullmatch(r"[0-9a-f]{64}", container):
        raise ValueError("Invalid container identity")
    return container


def checked_command(args, env, *, input_bytes=None):
    result = subprocess.run(
        args, env=env, input=input_bytes, capture_output=True, timeout=120, check=False
    )
    if result.returncode:
        raise RuntimeError("Synthetic archive command failed")
    return result.stdout


def require_password_rejection(wrapper, env, password):
    result = subprocess.run(
        [str(wrapper / "psql"), "-XAtq", "-v", "ON_ERROR_STOP=1", "-c", "SELECT 1"],
        env={**env, "PGPASSWORD": password + "-invalid"},
        capture_output=True,
        text=True,
        timeout=60,
        check=False,
    )
    if result.returncode == 0 or not re.search(
        r'(?m)FATAL:\s+password authentication failed for user "p1_test"\s*$', result.stderr
    ):
        raise ValueError("Database password rejection control failed")


def require_invalid_archive_rejection(restore, env):
    result = subprocess.run(
        restore + ["--list"],
        env=env,
        input=b"invalid archive",
        capture_output=True,
        timeout=60,
        check=False,
    )
    expected = b"pg_restore: error: input file does not appear to be a valid archive (too short?)"
    if result.returncode == 0 or result.stderr.strip() != expected:
        raise ValueError("Invalid archive rejection control failed")


def cleanup_resources(container_names, network, marker):
    """Try every owned name even if an earlier Docker operation fails."""
    failed = False
    resources = [("container", name) for name in container_names] + [("network", network)]
    for kind, name in resources:
        try:
            inspected = subprocess.run(
                ["docker", kind, "inspect", name],
                capture_output=True,
                text=True,
                timeout=60,
                check=False,
            )
            if inspected.returncode:
                if (
                    f"No such {kind}: {name}" in inspected.stderr
                    or f"No such object: {name}" in inspected.stderr
                ):
                    continue
                failed = True
                continue
            data = json.loads(inspected.stdout)
            if len(data) != 1:
                failed = True
                continue
            labels = data[0]["Config"]["Labels"] if kind == "container" else data[0]["Labels"]
            if not isinstance(labels, dict) or labels.get("sark.restore.owner") != marker:
                failed = True
                continue
            command = (
                ["docker", "rm", "-f", "-v", name]
                if kind == "container"
                else ["docker", "network", "rm", name]
            )
            removed = subprocess.run(command, capture_output=True, timeout=60, check=False)
            failed |= removed.returncode != 0
        except (OSError, KeyError, TypeError, ValueError, subprocess.SubprocessError):
            failed = True
    return not failed


def main(candidate, revision, scratch, evidence, progress=None):
    progress = progress or Progress()
    progress.enter("context")
    if (
        os.environ.get("GITHUB_ACTIONS") == "true"
        and os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted"
        and "RUNNER_TEMP" in os.environ
        and evidence == Path(os.environ["RUNNER_TEMP"]).resolve() / "db-alpine-gosu-evidence"
        and evidence.is_dir()
    ):
        progress.directory = evidence
    validate_context(candidate, revision, scratch, evidence)
    progress.enter("image_inspection")
    if (
        run("docker", "image", "inspect", candidate, "--format", "{{.Id}}").stdout.strip()
        != candidate
    ):
        raise ValueError("Candidate image is unavailable")
    if run("docker", "image", "inspect", BASE_ID, "--format", "{{.Id}}").stdout.strip() != BASE_ID:
        raise ValueError("Verified source image is unavailable")
    network = "sark-restore-" + secrets.token_hex(12)
    marker = secrets.token_hex(32)
    database = "sark_restore_" + secrets.token_hex(16)
    password = secrets.token_urlsafe(32)
    containers = []
    container_names = []
    relays = []
    try:
        progress.enter("network_create")
        run(
            "docker",
            "network",
            "create",
            "--internal",
            "--label",
            f"sark.restore.owner={marker}",
            network,
        )
        for image, suffix in ((BASE_ID, "source"), (candidate, "target")):
            progress.enter(f"{suffix}_container_create")
            name = f"{network}-{suffix}"
            container_names.append(name)
            containers.append(start_container(image, network, name, marker, password))
        progress.enter("isolation_inspection")
        addresses = verified_addresses(network, marker, containers, (BASE_ID, candidate))
        progress.enter("relay_start")
        for address in addresses:
            relays.append(RelayServer(address))
        ports = [str(relay.server_address[1]) for relay in relays]
        if len(set(ports)) != 2:
            raise ValueError("Overlapping database ports")
        wrapper = scratch / "restore-client"
        wrapper.mkdir()
        (wrapper / "psql").write_text(
            f"#!{sys.executable}\nimport sys\nsys.path.insert(0, {str(ROOT / 'scripts')!r})\nfrom rehearse_alpine_gosu_restore import client\nclient()\n"
        )
        (wrapper / "psql").chmod(0o700)
        client_env = {
            **{k: v for k, v in os.environ.items() if not k.startswith("PG")},
            "RESTORE_OWNED_PORTS": json.dumps(dict(zip(ports, containers, strict=True))),
        }
        urls = [f"postgresql://p1_test:{password}@127.0.0.1:{port}/{database}" for port in ports]
        envs = [
            {
                **pg_env(url),
                **client_env,
                "PGPORT": port,
                "PGDATABASE": database,
                "PGPASSWORD": password,
            }
            for url, port in zip(urls, ports, strict=True)
        ]
        for env, role in zip(envs, ("source", "target"), strict=True):
            admin = {**env, "PGDATABASE": "postgres"}
            progress.enter(f"{role}_ready_marker")
            for attempt in range(120):
                try:
                    identity = psql(
                        wrapper, admin, "SELECT current_setting('sark.disposable_marker')"
                    )
                    break
                except RuntimeError:
                    if attempt == 119:
                        raise
                    time.sleep(0.5)
            if identity.strip() != marker:
                raise ValueError("Database ownership marker mismatch")
            progress.enter(f"{role}_password_control")
            require_password_rejection(wrapper, admin, password)
            progress.enter(f"{role}_database_create")
            psql(wrapper, admin, f'CREATE DATABASE "{database}"')
        progress.enter("synthetic_seed")
        run_fixture(
            {
                **client_env,
                "RESTORE_FIXTURE_DATABASE_URL": urls[0],
                "RESTORE_FIXTURE_DATABASE_NAME": database,
            },
            "seed_synthetic",
        )
        archive = scratch / "app-schema.dump"
        progress.enter("source_inventory")
        with Snapshot(wrapper, envs[0]) as source:
            exercise_quiescence_control(source)
            before = inventory(source)
            sequences = sequence_inventory(source)
            for table in (
                "_sqlx_migrations",
                "catalogue_items",
                "checkout_orders",
                "stock_movements",
                "email_verification_delivery_envelopes",
            ):
                if table not in before or not before[table][0]:
                    raise ValueError("Required fixture rows absent")
            if (
                before["_sqlx_migrations"][0] != len(list((ROOT / "migrations").glob("*.sql")))
                or source.query("SELECT count(*) FROM _sqlx_migrations WHERE NOT success").strip()
                != "0"
            ):
                raise ValueError("Incomplete migration history")
            progress.enter("source_dump")
            dump = checked_command(
                [
                    "docker",
                    "exec",
                    "-i",
                    "-e",
                    "PGHOST=127.0.0.1",
                    "-e",
                    "PGUSER",
                    "-e",
                    "PGPASSWORD",
                    "-e",
                    "PGDATABASE",
                    containers[0],
                    "pg_dump",
                    "--format=custom",
                    "--no-owner",
                    "--no-acl",
                    "--snapshot",
                    source.value,
                ],
                envs[0],
            )
            if not dump or len(dump) > 64 * 1024 * 1024:
                raise ValueError("Unexpected archive size")
            archive.write_bytes(dump)
            require_quiescent_source(source)
            if sequence_inventory(source) != sequences:
                raise ValueError("Source sequence changed during snapshot")
            restore = [
                "docker",
                "exec",
                "-i",
                "-e",
                "PGHOST=127.0.0.1",
                "-e",
                "PGUSER",
                "-e",
                "PGPASSWORD",
                "-e",
                "PGDATABASE",
                "-e",
                "LC_ALL=C",
                containers[1],
                "pg_restore",
                "--exit-on-error",
                "--no-owner",
                "--no-acl",
            ]
            progress.enter("candidate_restore")
            checked_command(restore + ["--dbname", database], envs[1], input_bytes=dump)
            progress.enter("candidate_inventory")
            with Snapshot(wrapper, envs[1]) as target:
                after = inventory(target)
                if after != before or sequence_inventory(target) != sequences:
                    raise ValueError("Restored data mismatch")
            progress.enter("candidate_metadata")
            locale_sql = "SELECT json_build_array(pg_encoding_to_char(encoding),datlocprovider,datlocale,datcollate,datctype,datcollversion)::text FROM pg_database WHERE datname=current_database()"
            locales = [json.loads(psql(wrapper, env, locale_sql)) for env in envs]
            if any(locale[0] != "UTF8" for locale in locales):
                raise ValueError("Unexpected database encoding")
            roles = [
                psql(
                    wrapper,
                    env,
                    "SELECT current_user || '|' || pg_get_userbyid(datdba) FROM pg_database WHERE datname=current_database()",
                ).strip()
                for env in envs
            ]
            if roles != ["p1_test|p1_test", "p1_test|p1_test"]:
                raise ValueError("Unexpected synthetic database owner")
            versions = [psql(wrapper, env, "SHOW server_version").strip() for env in envs]
            if any(not version.startswith("18.7") for version in versions):
                raise ValueError("Unexpected PostgreSQL version")
            identities = [
                [
                    run("docker", "exec", container, "id", option, "postgres").stdout.strip()
                    for option in ("-u", "-g")
                ]
                for container in containers
            ]
            if identities != [["999", "999"], ["70", "70"]]:
                raise ValueError("Unexpected postgres UID or GID")
            if (
                psql(
                    wrapper,
                    envs[1],
                    "SELECT i.indisunique AND i.indisvalid FROM pg_index i JOIN pg_class c ON c.oid=i.indexrelid WHERE c.relname='catalogue_items_pkey' AND c.relnamespace='public'::regnamespace",
                ).strip()
                != "t"
            ):
                raise ValueError("Restored primary key index missing or invalid")
            progress.enter("candidate_negative_controls")
            psql(
                wrapper,
                envs[1],
                "DO $$ BEGIN BEGIN INSERT INTO catalogue_items SELECT * FROM catalogue_items LIMIT 1; RAISE EXCEPTION 'duplicate accepted'; EXCEPTION WHEN unique_violation THEN NULL; END; END $$",
            )
            checked_command(restore + ["--list"], envs[1], input_bytes=dump)
            require_invalid_archive_rejection(restore, envs[1])
            progress.enter("restored_key_fixture")
            run_fixture(
                {
                    **client_env,
                    "RESTORE_FIXTURE_DATABASE_URL": urls[1],
                    "RESTORE_FIXTURE_DATABASE_NAME": database,
                },
                "verify_restored_keys",
            )
            progress.enter("changed_content_control")
            psql(
                wrapper,
                envs[1],
                "INSERT INTO catalogue_items (sku,slug,minor_units,currency,fill,fixture) VALUES ('DEMO-RESTORE-CONTROL','restore-control',42,'EUR','covers-only',true)",
            )
            with Snapshot(wrapper, envs[1]) as changed:
                changed_rows, changed_hash = inventory(changed)["catalogue_items"]
                if (
                    changed_rows != before["catalogue_items"][0] + 1
                    or changed_hash == before["catalogue_items"][1]
                ):
                    raise ValueError("Changed content not detected")
            progress.enter("changed_sequence_control")
            control = '"public"."stock_movements_sequence_seq"'
            if (
                control not in sequences
                or not sequences[control][1]
                or not 0 < sequences[control][0] < 9_000_000_000_000_000_000
            ):
                raise ValueError("Expected stock sequence was not exercised")
            psql(
                wrapper,
                envs[1],
                f"SELECT pg_catalog.setval('{control}'::regclass,{sequences[control][0] + 1},true)",
            )
            with Snapshot(wrapper, envs[1]) as changed:
                expected_damage = dict(sequences)
                expected_damage[control] = [sequences[control][0] + 1, True]
                if sequence_inventory(changed) != expected_damage:
                    raise ValueError("Changed sequence not detected")
            progress.enter("preserved_source_check")
            require_quiescent_source(source)
            # A fresh snapshot is essential: the original held snapshot cannot
            # observe writes committed after the dump.
            with Snapshot(wrapper, envs[0]) as fresh:
                if inventory(fresh) != before or sequence_inventory(fresh) != sequences:
                    raise ValueError("Preserved source changed")
            require_quiescent_source(source)
            if source_state(ROOT) != (revision, True):
                raise ValueError("Reviewed source changed during rehearsal")
            report = {
                "source_revision": revision,
                "source_image": BASE_ID,
                "candidate_image": candidate,
                "archive_sha256": digest_file(archive),
                "tables": len(before),
                "rows": sum(row[0] for row in before.values()),
                "table_inventory_sha256": hashlib.sha256(
                    json.dumps(before, sort_keys=True).encode()
                ).hexdigest(),
                "sequences": len(sequences),
                "sequence_inventory_sha256": hashlib.sha256(
                    json.dumps(sequences, sort_keys=True).encode()
                ).hexdigest(),
                "migration_history_sha256": before["_sqlx_migrations"][1],
                "locales": locales,
                "roles": roles,
                "postgres_versions": versions,
                "postgres_uid_gid": identities,
                "source_unchanged": True,
                "restored_keys_checked": True,
                "extra_client_rejected": True,
                "duplicate_rejected": True,
                "corrupt_archive_rejected": True,
                "changed_content_detected": True,
                "changed_sequence_detected": True,
            }
        progress.enter("write_evidence")
        (evidence / "app-restore.json").write_text(json.dumps(report, indent=2) + "\n")
        try:
            run_schema66(
                wrapper,
                envs,
                urls,
                client_env,
                containers,
                marker,
                scratch,
                evidence,
                candidate,
                revision,
                progress,
            )
        finally:
            (scratch / "schema66-synthetic-proof").unlink(missing_ok=True)
    finally:
        handling_failure = sys.exc_info()[0] is not None
        if handling_failure:
            progress.finish_stage()
        relay_failed = False
        for relay in relays:
            try:
                relay.close()
            except (OSError, RuntimeError):
                relay_failed = True
        cleaned = cleanup_resources(container_names, network, marker)
        progress.cleanup = "failed" if relay_failed or not cleaned else "success"
        if relay_failed or not cleaned:
            if not handling_failure:
                progress.enter("cleanup")
                raise RuntimeError("Could not remove owned disposable resources")
    progress.enter("complete")


if __name__ == "__main__":
    progress = Progress()
    try:
        if len(sys.argv) != 5:
            raise ValueError("Expected image, revision, scratch and evidence")
        main(
            sys.argv[1],
            sys.argv[2],
            Path(sys.argv[3]).resolve(strict=True),
            Path(sys.argv[4]).resolve(strict=True),
            progress,
        )
        progress.write()
    except (
        KeyError,
        TypeError,
        OSError,
        RuntimeError,
        ValueError,
        subprocess.SubprocessError,
    ) as error:
        result = progress.result(error)
        try:
            progress.write(error)
        except OSError:
            pass
        raise SystemExit(
            f"Synthetic Alpine Gosu restore failed: stage={result['stage']} code={result['code']}"
        ) from None
