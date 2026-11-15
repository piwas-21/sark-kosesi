"""A separately owned 66-to-current synthetic logical restore and upgrade."""

import hashlib
import json
import re
import secrets
import subprocess

from rehearse_restore import (
    Snapshot,
    digest_file,
    exercise_quiescence_control,
    inventory,
    pg_env,
    psql,
    require_quiescent_source,
    sequence_inventory,
    source_state,
)
from with_postgres import ROOT


def run_fixture(env, name):
    if name not in ("restore_schema66_seed", "restore_schema66_upgrade"):
        raise ValueError("Unexpected schema-66 fixture")
    result = subprocess.run(
        [
            "cargo",
            "test",
            "--locked",
            "-p",
            "commerce-infrastructure",
            "--test",
            "guest_order_link_delivery",
            name,
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
        raise RuntimeError("Synthetic schema-66 fixture failed")


def archive_command(args, env, *, data=None):
    result = subprocess.run(
        args, env=env, input=data, capture_output=True, timeout=120, check=False
    )
    if result.returncode:
        raise RuntimeError("Synthetic schema-66 archive command failed")
    return result.stdout


def run_rehearsal(
    wrapper,
    base_envs,
    urls,
    client_env,
    containers,
    marker,
    scratch,
    evidence,
    candidate_id,
    revision,
    progress,
):
    if (
        len(base_envs) != 2
        or len(urls) != 2
        or len(containers) != 2
        or len(set(containers)) != 2
        or any(re.fullmatch(r"[0-9a-f]{64}", item) is None for item in containers)
        or re.fullmatch(r"[0-9a-f]{64}", marker) is None
    ):
        raise ValueError("Unexpected owned restore resources")
    ports = []
    for url, env in zip(urls, base_envs, strict=True):
        owned = pg_env(url)
        if any(
            owned[name] != env[name]
            for name in ("PGHOST", "PGPORT", "PGUSER", "PGDATABASE", "PGPASSWORD")
        ):
            raise ValueError("Owned database endpoint mismatch")
        ports.append(owned["PGPORT"])
    if len(set(ports)) != 2 or json.loads(client_env["RESTORE_OWNED_PORTS"]) != dict(
        zip(ports, containers, strict=True)
    ):
        raise ValueError("Owned relay identity mismatch")
    database = "sark_restore_" + secrets.token_hex(16)
    schema_urls = [url.rsplit("/", 1)[0] + "/" + database for url in urls]
    envs = [{**env, "PGDATABASE": database} for env in base_envs]
    common = {
        **client_env,
        "RESTORE_FIXTURE_DATABASE_NAME": database,
        "RESTORE_FIXTURE_MARKER": marker,
    }
    proof_path = scratch / "schema66-synthetic-proof"
    progress.enter("schema66_database_create")
    for env in envs:
        psql(wrapper, {**env, "PGDATABASE": "postgres"}, f'CREATE DATABASE "{database}"')
    progress.enter("schema66_seed")
    run_fixture({**common, "RESTORE_FIXTURE_DATABASE_URL": schema_urls[0]}, "restore_schema66_seed")
    if not proof_path.is_file() or proof_path.stat().st_size != 32:
        raise ValueError("Synthetic proof custody missing")
    archive = scratch / "schema66.dump"
    with Snapshot(wrapper, envs[0]) as source:
        progress.enter("schema66_source_inventory")
        exercise_quiescence_control(source)
        before = inventory(source)
        sequences = sequence_inventory(source)
        if before.get("_sqlx_migrations", (0,))[0] != 66:
            raise ValueError("Schema 66 migration count mismatch")
        for table in ("checkout_orders", "stock_movements", "guest_order_link_proofs"):
            if table not in before or before[table][0] < 1:
                raise ValueError("Schema 66 fixture is incomplete")
        progress.enter("schema66_source_dump")
        base = [
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
        ]
        dump = archive_command(
            base
            + [
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
            raise ValueError("Schema 66 archive size invalid")
        archive.write_bytes(dump)
        require_quiescent_source(source)
        if sequence_inventory(source) != sequences:
            raise ValueError("Schema 66 source sequence changed")
        progress.enter("schema66_candidate_restore")
        archive_command(
            base
            + [
                containers[1],
                "pg_restore",
                "--exit-on-error",
                "--no-owner",
                "--no-acl",
                "--dbname",
                database,
            ],
            envs[1],
            data=dump,
        )
        progress.enter("schema66_restored_inventory")
        with Snapshot(wrapper, envs[1]) as target:
            if inventory(target) != before or sequence_inventory(target) != sequences:
                raise ValueError("Schema 66 restored content mismatch")
        progress.enter("schema66_candidate_upgrade")
        run_fixture(
            {**common, "RESTORE_FIXTURE_DATABASE_URL": schema_urls[1]}, "restore_schema66_upgrade"
        )
        progress.enter("schema66_upgraded_inventory")
        with Snapshot(wrapper, envs[1]) as upgraded:
            final_history = inventory(upgraded)["_sqlx_migrations"]
            latest = int(upgraded.query("SELECT max(version) FROM public._sqlx_migrations").strip())
            expected = list((ROOT / "migrations").glob("*.sql"))
            if final_history[0] != len(expected) or latest != max(
                int(path.name.split("_", 1)[0]) for path in expected
            ):
                raise ValueError("Unexpected upgraded migration history")
        progress.enter("schema66_source_unchanged")
        require_quiescent_source(source)
        with Snapshot(wrapper, envs[0]) as fresh:
            if inventory(fresh) != before or sequence_inventory(fresh) != sequences:
                raise ValueError("Schema 66 source changed")
        require_quiescent_source(source)
        if source_state(ROOT) != (revision, True):
            raise ValueError("Reviewed source changed during schema 66 rehearsal")
        report = {
            "source_revision": revision,
            "candidate_image": candidate_id,
            "source_migration_count": 66,
            "source_migration_history_sha256": before["_sqlx_migrations"][1],
            "upgraded_migration_history_sha256": final_history[1],
            "upgraded_migration_count": final_history[0],
            "upgraded_migration_max": latest,
            "source_table_inventory_sha256": hashlib.sha256(
                json.dumps(before, sort_keys=True).encode()
            ).hexdigest(),
            "source_tables": len(before),
            "source_rows": sum(row[0] for row in before.values()),
            "source_sequences": len(sequences),
            "source_sequence_inventory_sha256": hashlib.sha256(
                json.dumps(sequences, sort_keys=True).encode()
            ).hexdigest(),
            "archive_sha256": digest_file(archive),
            "restore_before_upgrade_compared": True,
            "latest_history_and_idempotence_checked": True,
            "legacy_proof_redemption_checked": True,
            "source_unchanged": True,
        }
    proof_path.unlink()
    (evidence / "schema66-restore-upgrade.json").write_text(json.dumps(report, indent=2) + "\n")
