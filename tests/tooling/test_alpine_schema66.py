"""Boundary and sequencing controls for the synthetic schema-66 rehearsal."""

import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import rehearse_alpine_schema66 as lab


class FakeSnapshot:
    counter = 0

    def __init__(self, wrapper, env):
        self.env = env
        self.kind = FakeSnapshot.counter
        FakeSnapshot.counter += 1
        self.value = "A1-B2"

    def __enter__(self):
        return self

    def __exit__(self, *_):
        pass

    def query(self, sql):
        if "max(version)" in sql:
            return "81\n"
        raise AssertionError("unexpected query")


class Schema66Tests(unittest.TestCase):
    def test_fixture_runs_only_exact_allowlisted_test(self):
        with patch.object(lab.subprocess, "run") as run:
            run.return_value = SimpleNamespace(
                returncode=0, stdout=b"test result: ok. 1 passed; 0 failed;"
            )
            lab.run_fixture({}, "restore_schema66_seed")
            self.assertIn("guest_order_link_delivery", run.call_args.args[0])
            self.assertIn("--exact", run.call_args.args[0])
            for stdout, status in (
                (b"test result: ok. 0 passed; 0 failed;", 0),
                (b"test result: ok. 1 passed; 0 failed;", 1),
            ):
                run.return_value = SimpleNamespace(returncode=status, stdout=stdout)
                with self.assertRaises(RuntimeError):
                    lab.run_fixture({}, "restore_schema66_seed")
            with self.assertRaises(ValueError):
                lab.run_fixture({}, "unrelated")

    def exercise(self, corrupt_target=False, profile=None):
        FakeSnapshot.counter = 0
        calls = []
        before = {
            "_sqlx_migrations": (66, "a" * 64),
            "checkout_orders": (1, "b" * 64),
            "stock_movements": (2, "c" * 64),
            "guest_order_link_proofs": (1, "d" * 64),
        }
        urls = [
            "postgresql://p1_test:synthetic@127.0.0.1:1234/sark_restore_old",
            "postgresql://p1_test:synthetic@127.0.0.1:1235/sark_restore_old",
        ]
        envs = [lab.pg_env(url) for url in urls]
        containers = ["d" * 64, "e" * 64]
        client_env = {
            "RESTORE_OWNED_PORTS": json.dumps({"1234": containers[0], "1235": containers[1]})
        }
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)

            def fixture(env, name):
                calls.append(
                    (name, env["RESTORE_FIXTURE_DATABASE_URL"], env["RESTORE_FIXTURE_PROFILE"])
                )
                if name == "restore_schema66_seed":
                    (root / "schema66-synthetic-proof").write_bytes(b"x" * 32)

            def inventory(snapshot):
                if snapshot.kind == 2:
                    return {"_sqlx_migrations": (81, "e" * 64)}
                if snapshot.kind == 1 and corrupt_target:
                    return {**before, "checkout_orders": (0, "z" * 64)}
                return before

            def archive(args, env, *, data=None):
                calls.append(("archive", args, env["PGDATABASE"]))
                return b"synthetic archive" if "pg_dump" in args else b""

            progress = SimpleNamespace(enter=lambda name: calls.append(("stage", name)))
            with (
                patch.object(lab, "Snapshot", FakeSnapshot),
                patch.object(lab, "psql"),
                patch.object(lab, "run_fixture", side_effect=fixture),
                patch.object(lab, "inventory", side_effect=inventory),
                patch.object(lab, "sequence_inventory", return_value={"sequence": [10, True]}),
                patch.object(lab, "exercise_quiescence_control"),
                patch.object(lab, "require_quiescent_source"),
                patch.object(lab, "source_state", return_value=("a" * 40, True)),
                patch.object(lab, "archive_command", side_effect=archive),
            ):
                arguments = (
                    root,
                    envs,
                    urls,
                    client_env,
                    containers,
                    "b" * 64,
                    root,
                    root,
                    "sha256:" + "c" * 64,
                    "a" * 40,
                    progress,
                )
                if corrupt_target:
                    with self.assertRaisesRegex(ValueError, "restored content mismatch"):
                        lab.run_rehearsal(*arguments)
                    self.assertNotIn("restore_schema66_upgrade", [item[0] for item in calls])
                    self.assertFalse((root / "schema66-restore-upgrade.json").exists())
                    return
                if profile is None:
                    lab.run_rehearsal(*arguments)
                else:
                    lab.run_rehearsal(*arguments, profile=profile)
            report = json.loads((root / "schema66-restore-upgrade.json").read_text())
            self.assertEqual(report["source_migration_count"], 66)
            self.assertEqual(report["upgraded_migration_max"], 81)
            self.assertTrue(report["legacy_proof_redemption_checked"])
            self.assertEqual(report["candidate_image"], "sha256:" + "c" * 64)
            self.assertFalse((root / "schema66-synthetic-proof").exists())
            fixture_calls = [item for item in calls if item[0].startswith("restore_schema66_")]
            self.assertIn("127.0.0.1:1234/", fixture_calls[0][1])
            self.assertIn("127.0.0.1:1235/", fixture_calls[1][1])
            self.assertEqual([item[2] for item in fixture_calls], [profile or "alpine"] * 2)
            dump = next(item for item in calls if item[0] == "archive" and "pg_dump" in item[1])
            self.assertIn("--snapshot", dump[1])
            restore = next(
                item for item in calls if item[0] == "archive" and "pg_restore" in item[1]
            )
            self.assertIn(containers[1], restore[1])

    def test_restore_then_upgrade_uses_distinct_owned_databases(self):
        self.exercise()

    def test_debian_profile_is_passed_to_both_fixture_invocations(self):
        self.exercise(profile="debian")

    def test_unknown_profile_is_rejected_before_database_creation(self):
        with patch.object(lab, "psql") as psql:
            for profile in ("", "Debian", "../debian", "db-debian-candidate-scratch", None):
                with self.subTest(profile=profile), self.assertRaises(ValueError):
                    lab.run_rehearsal(
                        None,
                        [],
                        [],
                        {},
                        [],
                        "",
                        Path("/tmp"),
                        Path("/tmp"),
                        "",
                        "",
                        None,
                        profile=profile,
                    )
            psql.assert_not_called()

    def test_corrupted_restore_never_runs_migrations(self):
        self.exercise(corrupt_target=True)

    def test_foreign_or_mismatched_endpoint_is_rejected_before_database_creation(self):
        progress = SimpleNamespace(enter=lambda _: None)
        urls = [
            "postgresql://p1_test:x@example.test:1/old",
            "postgresql://p1_test:x@127.0.0.1:2/old",
        ]
        envs = [lab.pg_env("postgresql://p1_test:x@127.0.0.1:1/old"), lab.pg_env(urls[1])]
        containers = ["d" * 64, "e" * 64]
        client_env = {"RESTORE_OWNED_PORTS": json.dumps({"1": containers[0], "2": containers[1]})}
        with patch.object(lab, "psql") as psql:
            with self.assertRaises(ValueError):
                lab.run_rehearsal(
                    Path("/tmp"),
                    envs,
                    urls,
                    client_env,
                    containers,
                    "a" * 64,
                    Path("/tmp"),
                    Path("/tmp"),
                    "",
                    "",
                    progress,
                )
            psql.assert_not_called()


if __name__ == "__main__":
    unittest.main()
