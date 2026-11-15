#!/usr/bin/env python3
"""Include the exclusive continuation fsync test in canonical Rust coverage."""

import hashlib
import os
import re
import shlex
import subprocess
import sys
import tempfile
from pathlib import Path

from with_postgres import ROOT

ARTIFACT = re.compile(
    r"^\s*Running unittests src/lib\.rs \((.+/commerce_infrastructure-[0-9a-f]+)\)\s*$"
)


def executable_from_log(lines, target):
    paths = [
        Path(match.group(1)).resolve() for line in lines if (match := ARTIFACT.fullmatch(line))
    ]
    if len(paths) != 1 or not paths[0].is_relative_to(target) or not paths[0].is_file():
        raise ValueError("Missing or ambiguous current infrastructure test executable")
    return paths[0]


def hash_file(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").digest()


def profile_prefix(output, target):
    assignments = []
    for line in output.splitlines():
        parts = shlex.split(line)
        if len(parts) == 2 and parts[0] == "export" and parts[1].startswith("LLVM_PROFILE_FILE="):
            assignments.append(parts[1].split("=", 1)[1])
    if len(assignments) != 1:
        raise ValueError("Missing or ambiguous cargo coverage profile pattern")
    pattern = Path(assignments[0])
    if pattern.parent.resolve() != target or "%" not in pattern.name:
        raise ValueError("Unexpected cargo coverage profile location")
    prefix = pattern.name.split("%", 1)[0]
    if not prefix or "/" in prefix:
        raise ValueError("Invalid cargo coverage profile prefix")
    return prefix


def one_profile(directory):
    files = list(directory.glob("*.profraw"))
    if (
        len(files) != 1
        or files[0].is_symlink()
        or not files[0].is_file()
        or files[0].stat().st_size == 0
    ):
        raise ValueError("Missing or ambiguous exclusive coverage profile")
    return files[0]


def run_tests(command, env, target):
    lines = []
    with subprocess.Popen(
        command,
        cwd=ROOT,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    ) as process:
        assert process.stdout is not None
        for line in process.stdout:
            print(line, end="", flush=True)
            if ARTIFACT.fullmatch(line.rstrip("\n")):
                lines.append(line.rstrip("\n"))
        if process.wait() != 0:
            raise ValueError("Rust coverage tests failed")
    return executable_from_log(lines, target)


def main():
    if sys.argv[1:] not in (["stable"], ["nightly"]):
        raise ValueError("Expected stable or nightly")
    mode = sys.argv[1]
    target = (ROOT / ".local" / f"coverage-target-{mode}").resolve()
    env = {**os.environ, "CARGO_TARGET_DIR": str(target), "CARGO_TERM_COLOR": "never"}
    cargo = ["cargo"] + (["+nightly-2026-10-25"] if mode == "nightly" else [])
    cov = cargo + ["llvm-cov"]
    (ROOT / "coverage").mkdir(exist_ok=True)
    outputs = ["rust-stable.json"] if mode == "stable" else ["rust.json", "rust.lcov"]
    for output in outputs:
        (ROOT / "coverage" / output).unlink(missing_ok=True)
    subprocess.run(cov + ["clean", "--workspace"], cwd=ROOT, env=env, check=True)
    binary = run_tests(
        [
            sys.executable,
            "scripts/with_postgres.py",
            "--",
            *cov,
            "--workspace",
            "--all-features",
            "--locked",
            "--no-report",
        ]
        + (["--branch"] if mode == "nightly" else []),
        env,
        target,
    )
    before = hash_file(binary)
    shown = subprocess.run(
        cov + ["show-env", "--sh"], cwd=ROOT, env=env, check=True, capture_output=True, text=True
    ).stdout
    prefix = profile_prefix(shown, target)
    profile_target = target / "llvm-cov-target"
    if not profile_target.is_dir() or not any(profile_target.glob("*.profraw")):
        raise ValueError("Missing ordinary cargo coverage profiles")
    with tempfile.TemporaryDirectory(prefix="exclusive-fsync-profile-", dir=target) as temp:
        directory = Path(temp)
        profile_env = {**env, "LLVM_PROFILE_FILE": str(directory / "%p-%m.profraw")}
        subprocess.run(
            [sys.executable, "scripts/test_continuation_fsync.py", str(binary)],
            cwd=ROOT,
            env=profile_env,
            check=True,
        )
        profile = one_profile(directory)
        if hash_file(binary) != before:
            raise ValueError("Coverage executable changed during exclusive control")
        destination = profile_target / f"{prefix}exclusive-{profile.name}"
        if destination.exists():
            raise ValueError("Exclusive profile destination already exists")
        profile.rename(destination)
    report = cov + ["report"]
    if mode == "nightly":
        subprocess.run(
            report + ["--json", "--output-path", "coverage/rust.json"],
            cwd=ROOT,
            env=env,
            check=True,
        )
        subprocess.run(
            report + ["--lcov", "--output-path", "coverage/rust.lcov"],
            cwd=ROOT,
            env=env,
            check=True,
        )
    else:
        subprocess.run(
            report + ["--json", "--output-path", "coverage/rust-stable.json"],
            cwd=ROOT,
            env=env,
            check=True,
        )


if __name__ == "__main__":
    main()
