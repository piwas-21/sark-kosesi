"""The dedicated coverage control must use one current binary and real profile."""

import tempfile
import unittest
from pathlib import Path

from coverage_fsync import executable_from_log, one_profile, profile_prefix


class ExclusiveCoverageTests(unittest.TestCase):
    def test_current_artifact_is_required_and_unambiguous(self):
        with tempfile.TemporaryDirectory() as temp:
            target = Path(temp).resolve()
            binary = target / "commerce_infrastructure-123abc"
            binary.write_bytes(b"binary")
            line = f"     Running unittests src/lib.rs ({binary})"
            self.assertEqual(executable_from_log([line], target), binary)
            for lines in ([], [line, line], ["Running unittests src/lib.rs (/wrong/path)"]):
                with self.assertRaises(ValueError):
                    executable_from_log(lines, target)
            with tempfile.TemporaryDirectory() as elsewhere:
                with self.assertRaises(ValueError):
                    executable_from_log([line], Path(elsewhere).resolve())

    def test_profile_pattern_must_come_from_current_target(self):
        with tempfile.TemporaryDirectory() as temp:
            target = Path(temp).resolve()
            line = f"export LLVM_PROFILE_FILE='{target}/workspace-%p-%10m.profraw'"
            self.assertEqual(profile_prefix(line, target), "workspace-")
            for value in (
                "",
                line + "\n" + line,
                "export LLVM_PROFILE_FILE='/other/x-%p.profraw'",
                f"export LLVM_PROFILE_FILE='{target}/plain.profraw'",
            ):
                with self.assertRaises(ValueError):
                    profile_prefix(value, target)

    def test_profile_must_be_single_nonempty_regular_file(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            with self.assertRaises(ValueError):
                one_profile(directory)
            profile = directory / "one.profraw"
            profile.touch()
            with self.assertRaises(ValueError):
                one_profile(directory)
            profile.write_bytes(b"profile")
            self.assertEqual(one_profile(directory), profile)
            (directory / "two.profraw").write_bytes(b"profile")
            with self.assertRaises(ValueError):
                one_profile(directory)

    def test_producer_requires_profile_before_export(self):
        from types import SimpleNamespace
        from unittest.mock import patch

        import coverage_fsync

        for mode in ("stable", "nightly"):
            for include_profile in (False, True):
                with self.subTest(mode=mode, include_profile=include_profile):
                    with tempfile.TemporaryDirectory() as temp:
                        root = Path(temp).resolve()
                        target = root / ".local" / f"coverage-target-{mode}"
                        target.mkdir(parents=True)
                        profile_target = target / "llvm-cov-target"
                        profile_target.mkdir()
                        (profile_target / "existing.profraw").write_bytes(b"normal suite")
                        binary = target / "commerce_infrastructure-123abc"
                        binary.write_bytes(b"binary")
                        calls = []

                        def run(
                            args,
                            *,
                            calls=calls,
                            target=target,
                            include_profile=include_profile,
                            **kwargs,
                        ):
                            calls.append(args)
                            if "show-env" in args:
                                return SimpleNamespace(
                                    stdout=f"export LLVM_PROFILE_FILE='{target}/test-%p-%m.profraw'"
                                )
                            if "scripts/test_continuation_fsync.py" in args and include_profile:
                                pattern = kwargs["env"]["LLVM_PROFILE_FILE"]
                                Path(pattern.replace("%p", "1").replace("%m", "2")).write_bytes(
                                    b"profile"
                                )
                            return SimpleNamespace(stdout="")

                        with (
                            patch.object(coverage_fsync, "ROOT", root),
                            patch.object(coverage_fsync.sys, "argv", ["coverage_fsync.py", mode]),
                            patch.object(coverage_fsync, "run_tests", return_value=binary),
                            patch.object(coverage_fsync.subprocess, "run", side_effect=run),
                        ):
                            if include_profile:
                                coverage_fsync.main()
                                self.assertEqual(
                                    len(list(profile_target.glob("test-exclusive-*.profraw"))), 1
                                )
                                exports = [v for v in calls if "report" in v]
                                self.assertEqual(len(exports), 2 if mode == "nightly" else 1)
                            else:
                                with self.assertRaises(ValueError):
                                    coverage_fsync.main()
                                self.assertFalse(any("report" in v for v in calls))
