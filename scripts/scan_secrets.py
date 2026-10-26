#!/usr/bin/env python3
"""Run the pinned upstream Gitleaks CLI, verifying the release archive hash."""

import hashlib
import io
import json
import os
import platform
import secrets
import shutil
import subprocess
import tarfile
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
VERSION = "8.30.1"
ARCHIVES = {
    ("Linux", "x86_64"): (
        "linux_x64",
        "551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb",
    ),
    ("Linux", "aarch64"): (
        "linux_arm64",
        "e4a487ee7ccd7d3a7f7ec08657610aa3606637dab924210b3aee62570fb4b080",
    ),
    ("Darwin", "arm64"): (
        "darwin_arm64",
        "b40ab0ae55c505963e365f271a8d3846efbc170aa17f2607f13df610a9aeb6a5",
    ),
    ("Darwin", "x86_64"): (
        "darwin_x64",
        "dfe101a4db2255fc85120ac7f3d25e4342c3c20cf749f2c20a18081af1952709",
    ),
}


def export_sources(root, destination):
    # Hook-local Git variables can override cwd and redirect an index. This
    # operation always owns the explicit root, never its caller's Git context.
    environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}

    selected = (
        subprocess.check_output(
            ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"],
            cwd=root,
            env=environment,
        )
        .decode()
        .split("\0")
    )
    files = sorted(set(value for value in selected if value))
    if not files:
        raise ValueError("Source secret scan cannot be empty")
    for value in files:
        relative = Path(value)
        source = root / relative
        if (
            relative.is_absolute()
            or ".." in relative.parts
            or source.is_symlink()
            or not source.is_file()
        ):
            raise ValueError("Source secret scan has a missing/unsafe file")
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
    return len(files)


def scan_current_source(binary):
    # Export Git's source inventory: ignored local runtime/build secrets are not
    # repository source. Force-tracked ignored files are still scanned here and
    # in history; no new global allowlist excludes compiled application bundles.
    with tempfile.TemporaryDirectory(prefix="sark-secrets-", dir="/tmp") as temporary:
        directory = Path(temporary)
        source = directory / "source"
        source.mkdir()
        count = export_sources(ROOT, source)
        control = directory / "control"
        control.mkdir()
        token = "ghp_" + "".join(
            secrets.choice("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789")
            for _ in range(36)
        )
        (control / "credential.txt").write_text("token=" + token)
        report = directory / "control-report.json"
        probe = subprocess.run(
            [
                str(binary),
                "dir",
                "--redact",
                "--no-banner",
                "--config",
                str(ROOT / ".gitleaks.toml"),
                "--report-format",
                "json",
                "--report-path",
                str(report),
                str(control),
            ],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )
        findings = json.loads(report.read_text()) if report.is_file() else []
        if probe.returncode == 0 or not any(
            item.get("RuleID") == "github-pat" for item in findings
        ):
            raise ValueError("Secret scanner did not detect seeded credential control")
        subprocess.run(
            [
                str(binary),
                "dir",
                "--redact",
                "--no-banner",
                "--config",
                str(ROOT / ".gitleaks.toml"),
                str(source),
            ],
            cwd=ROOT,
            check=True,
        )
        print(f"Secret scan verified {count} source files and seeded detector control")


if __name__ == "__main__":
    target, expected = ARCHIVES[(platform.system(), platform.machine())]
    url = f"https://github.com/gitleaks/gitleaks/releases/download/v{VERSION}/gitleaks_{VERSION}_{target}.tar.gz"
    archive = ROOT / ".local/tools" / f"gitleaks-{VERSION}-{target}.tar.gz"
    archive.parent.mkdir(parents=True, exist_ok=True)
    if not archive.exists():
        subprocess.run(
            [
                "curl",
                "--fail",
                "--silent",
                "--show-error",
                "--location",
                "--max-time",
                "120",
                url,
                "-o",
                str(archive),
            ],
            check=True,
        )
    data = archive.read_bytes()
    if hashlib.sha256(data).hexdigest() != expected:
        raise SystemExit("Gitleaks archive checksum mismatch; refusing execution")
    binary = archive.parent / f"gitleaks-{VERSION}"
    with tarfile.open(fileobj=io.BytesIO(data)) as tar:
        member = tar.extractfile("gitleaks")
        if member is None:
            raise SystemExit("Release archive missing binary")
        binary.write_bytes(member.read())
    binary.chmod(0o700)
    subprocess.run(
        [str(binary), "git", "--redact", "--no-banner", "--log-opts=--all", "."],
        cwd=ROOT,
        check=True,
    )
    scan_current_source(binary)
