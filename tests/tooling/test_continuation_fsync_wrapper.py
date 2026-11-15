"""The exclusive runner cannot pass on missing, skipped or failed tests."""

import unittest

from test_continuation_fsync import TEST, validate_list, validate_result


class ExclusiveFsyncRunnerTests(unittest.TestCase):
    def test_requires_exact_listed_test(self):
        validate_list([f"{TEST}: test"])
        for lines in ([], ["other: test"], [f"{TEST}: test", f"{TEST}: test"]):
            with self.assertRaises(ValueError):
                validate_list(lines)

    def test_rejects_zero_ignored_failed_or_unsuccessful_runs(self):
        valid = f"running 1 test\ntest {TEST} ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured\n"
        validate_result(0, valid)
        for status, output in (
            (0, "running 1 test\ntest result: ok. 1 passed; 0 failed; 0 ignored;"),
            (0, "running 0 tests\ntest result: ok. 0 passed; 0 failed; 0 ignored;"),
            (0, "running 1 test\ntest result: ok. 0 passed; 0 failed; 1 ignored;"),
            (1, "running 1 test\ntest result: ok. 1 passed; 0 failed; 0 ignored;"),
            (0, "running 1 test\ntest result: FAILED. 0 passed; 1 failed; 0 ignored;"),
        ):
            with self.assertRaises(ValueError):
                validate_result(status, output)


class ExclusiveDockerTests(unittest.TestCase):
    def test_only_linux_allows_exclusive_host_network(self):
        from pathlib import Path
        from unittest.mock import patch

        from test_continuation_fsync import docker_cluster

        with patch("test_continuation_fsync.platform.system", return_value="Darwin"):
            with self.assertRaises(ValueError):
                docker_cluster(Path("/tmp/unused"), "f" * 64)

    def test_non_linux_preflight_creates_no_cluster_directory(self):
        import os
        import tempfile
        from pathlib import Path
        from unittest.mock import patch

        import test_continuation_fsync as runner

        with tempfile.TemporaryDirectory() as temp:
            binary = Path(temp) / "binary"
            binary.write_bytes(b"binary")
            with (
                patch.object(runner.sys, "argv", ["runner", str(binary)]),
                patch.dict(os.environ, {}, clear=True),
                patch.object(runner.platform, "system", return_value="Darwin"),
                patch.object(runner, "cluster_directory") as directory,
                patch.object(runner, "docker_cluster") as launch,
            ):
                with self.assertRaisesRegex(ValueError, "Linux host networking"):
                    runner.main()
            directory.assert_not_called()
            launch.assert_not_called()

    def test_cleanup_is_restricted_to_owned_data_and_no_network(self):
        from pathlib import Path
        from unittest.mock import patch

        from test_continuation_fsync import remove_docker_data

        with patch("test_continuation_fsync.run") as call:
            remove_docker_data(Path("/tmp/sark-continuation-fsync-example"))
        args = call.call_args.args
        self.assertIn("none", args)
        self.assertEqual(args[-1], "/tmp/sark-continuation-fsync-example/data")

    def test_docker_uses_pinned_image_same_path_and_loopback(self):
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import patch

        from test_continuation_fsync import docker_cluster
        from with_postgres import IMAGE

        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp).resolve()
            with (
                patch("test_continuation_fsync.platform.system", return_value="Linux"),
                patch("test_continuation_fsync.run") as run,
                patch(
                    "test_continuation_fsync.subprocess.run",
                    return_value=SimpleNamespace(returncode=0),
                ),
            ):
                name, database = docker_cluster(root, "f" * 64)
            args = run.call_args.args
            self.assertTrue(name.startswith("sark-fsync-"))
            self.assertIn("host", args)
            self.assertIn(f"type=bind,src={root},dst={root}", args)
            self.assertIn(f"PGDATA={root / 'data'}", args)
            self.assertIn("listen_addresses=127.0.0.1", args)
            self.assertIn(IMAGE, args)
            self.assertIn("127.0.0.1", database)

    def test_listing_profile_is_private_and_does_not_reach_test_profile(self):
        import os
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import patch

        import test_continuation_fsync as runner

        with tempfile.TemporaryDirectory() as temp:
            binary = Path(temp) / "test-binary"
            binary.write_bytes(b"executable")
            profile = str(Path(temp) / "actual-%p-%m.profraw")
            envs = []

            def command(args, **kwargs):
                envs.append(kwargs["env"]["LLVM_PROFILE_FILE"])
                if "--list" in args:
                    Path(envs[-1].replace("%p", "1").replace("%m", "2")).write_bytes(b"list")
                    return SimpleNamespace(stdout=f"{runner.TEST}: test\n", returncode=0)
                return SimpleNamespace(
                    stdout=f"running 1 test\ntest {runner.TEST} ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored;\n",
                    returncode=0,
                )

            with (
                patch.object(runner.sys, "argv", ["runner", str(binary)]),
                patch.dict(os.environ, {"LLVM_PROFILE_FILE": profile}, clear=True),
                patch.object(runner.subprocess, "run", side_effect=command),
                patch.object(runner, "docker_cluster", return_value=("owned", "synthetic")),
                patch.object(runner.platform, "system", return_value="Linux"),
                patch.object(runner, "remove_docker_data"),
                patch.object(runner, "run"),
            ):
                runner.main()
            self.assertNotEqual(envs[0], profile)
            self.assertEqual(envs[1], profile)
            self.assertFalse(Path(envs[0].replace("%p", "1").replace("%m", "2")).exists())

    def test_failed_launcher_cleans_only_matching_label_and_preserves_error(self):
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import patch

        from test_continuation_fsync import docker_cluster

        marker = "f" * 64
        for observed in (marker, "foreign"):
            with self.subTest(observed=observed), tempfile.TemporaryDirectory() as temp:
                with (
                    patch("test_continuation_fsync.platform.system", return_value="Linux"),
                    patch(
                        "test_continuation_fsync.run", side_effect=[RuntimeError("launch"), None]
                    ) as run,
                    patch(
                        "test_continuation_fsync.subprocess.run",
                        return_value=SimpleNamespace(returncode=0, stdout=observed, stderr=""),
                    ),
                    patch("test_continuation_fsync.remove_docker_data") as cleanup,
                ):
                    with self.assertRaisesRegex(RuntimeError, "launch"):
                        docker_cluster(Path(temp), marker)
                self.assertEqual(run.call_count, 2 if observed == marker else 1)
                self.assertEqual(cleanup.call_count, 1 if observed == marker else 0)

    def test_timeout_and_no_such_container_retain_owned_data(self):
        import subprocess
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import patch

        from test_continuation_fsync import docker_cluster

        marker = "f" * 64
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            data = directory / "data"
            data.mkdir()
            witness = data / "still-possibly-in-use"
            witness.write_text("unchanged")
            timeout = subprocess.TimeoutExpired("docker run", 60)
            with (
                patch("test_continuation_fsync.platform.system", return_value="Linux"),
                patch("test_continuation_fsync.run", side_effect=timeout) as run,
                patch(
                    "test_continuation_fsync.subprocess.run",
                    return_value=SimpleNamespace(
                        returncode=1, stdout="", stderr="Error: No such object: synthetic"
                    ),
                ),
                patch("test_continuation_fsync.remove_docker_data") as remove_data,
            ):
                with self.assertRaises(subprocess.TimeoutExpired) as error:
                    docker_cluster(directory, marker)
            self.assertIs(error.exception, timeout)
            self.assertEqual(run.call_count, 1)
            remove_data.assert_not_called()
            self.assertEqual(witness.read_text(), "unchanged")

    def test_launcher_cleanup_failure_preserves_original_error(self):
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import patch

        from test_continuation_fsync import docker_cluster

        marker = "f" * 64
        with tempfile.TemporaryDirectory() as temp:
            with (
                patch("test_continuation_fsync.platform.system", return_value="Linux"),
                patch(
                    "test_continuation_fsync.run",
                    side_effect=[RuntimeError("original launch"), RuntimeError("cleanup")],
                ),
                patch("test_continuation_fsync.remove_docker_data") as remove_data,
                patch(
                    "test_continuation_fsync.subprocess.run",
                    return_value=SimpleNamespace(returncode=0, stdout=marker, stderr=""),
                ),
            ):
                with self.assertRaisesRegex(RuntimeError, "original launch"):
                    docker_cluster(Path(temp), marker)
                remove_data.assert_not_called()

    def test_failed_removal_never_deletes_live_data_or_masks_test_failure(self):
        import os
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import patch

        import test_continuation_fsync as runner

        for test_passed in (False, True):
            with self.subTest(test_passed=test_passed), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                binary = root / "test-binary"
                binary.write_bytes(b"binary")
                directory = root / "owned"
                directory.mkdir()
                data = directory / "data"
                data.mkdir()
                marker = data / "still-live"
                marker.write_text("unchanged")

                def command(args, *, test_passed=test_passed, **kwargs):
                    if args[:2] == ["docker", "exec"]:
                        return SimpleNamespace(returncode=0)
                    if "--list" in args:
                        return SimpleNamespace(stdout=f"{runner.TEST}: test\n", returncode=0)
                    if test_passed:
                        return SimpleNamespace(
                            stdout=f"running 1 test\ntest {runner.TEST} ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored;\n",
                            returncode=0,
                        )
                    return SimpleNamespace(stdout="test failure", returncode=1)

                calls = []

                def docker_run(*args, calls=calls):
                    calls.append(args)
                    if args[:2] == ("docker", "rm"):
                        raise RuntimeError("docker remove failed")
                    return SimpleNamespace(stdout="owned")

                real_rmtree = runner.shutil.rmtree
                with (
                    patch.object(runner.sys, "argv", ["runner", str(binary)]),
                    patch.dict(os.environ, {}, clear=True),
                    patch.object(runner, "cluster_directory", return_value=directory),
                    patch.object(runner.subprocess, "run", side_effect=command),
                    patch.object(runner.platform, "system", return_value="Linux"),
                    patch.object(runner, "remove_docker_data") as remove_data,
                    patch.object(
                        runner.shutil, "rmtree", side_effect=real_rmtree
                    ) as remove_directory,
                    patch.object(runner, "run", side_effect=docker_run),
                ):
                    error = RuntimeError if test_passed else ValueError
                    with self.assertRaises(error):
                        runner.main()
                self.assertTrue(any(arg.startswith("piwas.sark.exclusive=") for arg in calls[0]))
                remove_data.assert_not_called()
                self.assertTrue(
                    all(Path(call.args[0]) != directory for call in remove_directory.call_args_list)
                )
                self.assertEqual(marker.read_text(), "unchanged")


class ExclusiveNativeTests(unittest.TestCase):
    def test_ambiguous_start_without_pid_retains_partial_data_and_error(self):
        import os
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import patch

        import test_continuation_fsync as runner

        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            binary = root / "test-binary"
            binary.write_bytes(b"binary")
            native = root / "bin"
            native.mkdir()
            (native / "postgres").write_bytes(b"binary")
            directory = root / "cluster"
            (directory / "data").mkdir(parents=True)
            witness = directory / "data" / "partially-created"
            witness.write_text("unchanged")
            original = RuntimeError("native launch timeout")

            def command(args, **kwargs):
                if "--version" in args:
                    return SimpleNamespace(stdout="postgres (PostgreSQL) 18.1\n")
                return SimpleNamespace(stdout=f"{runner.TEST}: test\n", returncode=0)

            real_rmtree = runner.shutil.rmtree
            with (
                patch.object(runner.sys, "argv", ["runner", str(binary)]),
                patch.dict(os.environ, {"P1_POSTGRES_BIN": str(native)}, clear=True),
                patch.object(runner, "cluster_directory", return_value=directory),
                patch.object(runner.subprocess, "run", side_effect=command),
                patch.object(runner, "native_cluster", side_effect=original),
                patch.object(runner, "run") as stop,
                patch.object(runner.shutil, "rmtree", side_effect=real_rmtree) as remove,
            ):
                with self.assertRaises(RuntimeError) as failure:
                    runner.main()
            self.assertIs(failure.exception, original)
            stop.assert_not_called()
            self.assertTrue(all(Path(call.args[0]) != directory for call in remove.call_args_list))
            self.assertEqual(witness.read_text(), "unchanged")
            self.assertFalse((directory / "data" / "postmaster.pid").exists())

    def test_successful_native_cluster_stops_before_removing_data(self):
        import os
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import patch

        import test_continuation_fsync as runner

        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            binary = root / "test-binary"
            binary.write_bytes(b"binary")
            native = root / "bin"
            native.mkdir()
            (native / "postgres").write_bytes(b"binary")
            directory = root / "cluster"
            (directory / "data").mkdir(parents=True)
            (directory / "data" / "postmaster.pid").write_text("synthetic")

            def command(args, **kwargs):
                if "--version" in args:
                    return SimpleNamespace(stdout="postgres (PostgreSQL) 18.1\n")
                if "--list" in args:
                    return SimpleNamespace(stdout=f"{runner.TEST}: test\n", returncode=0)
                return SimpleNamespace(
                    stdout=f"running 1 test\ntest {runner.TEST} ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored;\n",
                    returncode=0,
                )

            def confirmed_stop(*args):
                self.assertEqual(
                    args,
                    (str(native / "pg_ctl"), "-D", str(directory / "data"), "stop", "-m", "fast"),
                )
                self.assertTrue(directory.exists())

            with (
                patch.object(runner.sys, "argv", ["runner", str(binary)]),
                patch.dict(os.environ, {"P1_POSTGRES_BIN": str(native)}, clear=True),
                patch.object(runner, "cluster_directory", return_value=directory),
                patch.object(runner.subprocess, "run", side_effect=command),
                patch.object(runner, "native_cluster", return_value="synthetic"),
                patch.object(runner, "run", side_effect=confirmed_stop) as stop,
            ):
                runner.main()
            stop.assert_called_once()
            self.assertFalse(directory.exists())

    def test_ready_native_without_pid_fails_and_keeps_data(self):
        import os
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import patch

        import test_continuation_fsync as runner

        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            binary = root / "test-binary"
            binary.write_bytes(b"binary")
            native = root / "bin"
            native.mkdir()
            (native / "postgres").write_bytes(b"binary")
            directory = root / "cluster"
            (directory / "data").mkdir(parents=True)
            witness = directory / "data" / "retain"
            witness.write_text("unchanged")

            def command(args, **kwargs):
                if "--version" in args:
                    return SimpleNamespace(stdout="postgres (PostgreSQL) 18.1\n")
                if "--list" in args:
                    return SimpleNamespace(stdout=f"{runner.TEST}: test\n", returncode=0)
                return SimpleNamespace(
                    stdout=f"running 1 test\ntest {runner.TEST} ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored;\n",
                    returncode=0,
                )

            with (
                patch.object(runner.sys, "argv", ["runner", str(binary)]),
                patch.dict(os.environ, {"P1_POSTGRES_BIN": str(native)}, clear=True),
                patch.object(runner, "cluster_directory", return_value=directory),
                patch.object(runner.subprocess, "run", side_effect=command),
                patch.object(runner, "native_cluster", return_value="synthetic"),
                patch.object(runner, "run") as stop,
            ):
                with self.assertRaisesRegex(ValueError, "stop could not be confirmed"):
                    runner.main()
            stop.assert_not_called()
            self.assertEqual(witness.read_text(), "unchanged")

    def test_post_start_auth_failure_attempts_owned_stop_and_retains_on_failure(self):
        import os
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import patch

        import test_continuation_fsync as runner

        for stop_succeeds in (True, False):
            with self.subTest(stop_succeeds=stop_succeeds), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                binary = root / "test-binary"
                binary.write_bytes(b"binary")
                native = root / "bin"
                native.mkdir()
                (native / "postgres").write_bytes(b"binary")
                directory = root / "cluster"
                (directory / "data").mkdir(parents=True)
                witness = directory / "data" / "owned-data"
                witness.write_text("unchanged")
                (directory / "data" / "postmaster.pid").write_text("synthetic")
                original = RuntimeError("native authentication failure")

                def command(args, **kwargs):
                    if "--version" in args:
                        return SimpleNamespace(stdout="postgres (PostgreSQL) 18.1\n")
                    return SimpleNamespace(stdout=f"{runner.TEST}: test\n", returncode=0)

                def stop_server(
                    *args,
                    stop_succeeds=stop_succeeds,
                    native=native,
                    directory=directory,
                    witness=witness,
                ):
                    self.assertEqual(
                        args,
                        (
                            str(native / "pg_ctl"),
                            "-D",
                            str(directory / "data"),
                            "stop",
                            "-m",
                            "fast",
                        ),
                    )
                    self.assertEqual(witness.read_text(), "unchanged")
                    if not stop_succeeds:
                        raise RuntimeError("uncertain stop")

                with (
                    patch.object(runner.sys, "argv", ["runner", str(binary)]),
                    patch.dict(os.environ, {"P1_POSTGRES_BIN": str(native)}, clear=True),
                    patch.object(runner, "cluster_directory", return_value=directory),
                    patch.object(runner.subprocess, "run", side_effect=command),
                    patch.object(runner, "native_cluster", side_effect=original),
                    patch.object(runner, "run", side_effect=stop_server) as stop,
                ):
                    with self.assertRaises(RuntimeError) as failure:
                        runner.main()
                self.assertIs(failure.exception, original)
                stop.assert_called_once()
                if stop_succeeds:
                    self.assertFalse(directory.exists())
                else:
                    self.assertEqual(witness.read_text(), "unchanged")


if __name__ == "__main__":
    unittest.main()
