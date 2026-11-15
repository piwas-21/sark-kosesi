"""Offline source controls for the optional Gosu image experiment.

These checks detect changes to selected safeguards; they do not build an image or
attest the provenance, behavior, or security of any resulting binary.
"""

import os
import re
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SOURCE_SHA = "33d7537d588ea49458b9509bcf4554bdf5ceacc66da71e5caa1058ea3b689c3b"
GO_SHA = "7226736c2339719cda28fbf3a54d71aa63bcefe376e98cef00034268dab24840"
POSTGRES_DIGEST = "sha256:c7fedce47eff6a78cb35dad6b627f0d684107f75ab15c0870e40c093766ba452"


def active_lines(source):
    """Exclude blank and comment-only lines, so comments cannot satisfy checks."""
    return "\n".join(
        line for line in source.splitlines() if line.strip() and not line.lstrip().startswith("#")
    )


def check_dockerfile(source):
    lines = active_lines(source).splitlines()
    instructions = [line.strip() for line in lines]
    if not instructions or not re.fullmatch(
        r"FROM\s+\S+@" + POSTGRES_DIGEST, instructions[0], re.IGNORECASE
    ):
        raise ValueError("PostgreSQL base must be pinned")
    if not any(
        re.fullmatch(r"COPY\s+--chmod=0755\s+gosu\s+/usr/local/bin/gosu", line, re.IGNORECASE)
        for line in instructions
    ):
        raise ValueError("candidate binary must be copied to the Gosu path")


def require(source, pattern, message):
    if not re.search(pattern, source, re.MULTILINE):
        raise ValueError(message)


def check_script(source):
    source = active_lines(source)
    # A digest in a comment or detached from the checksum invocation is not enough.
    require(source, rf"^source_sha={SOURCE_SHA}$", "source digest")
    require(source, rf"^go_sha={GO_SHA}$", "Go digest")
    require(source, r"^base=\'[^\']+@" + POSTGRES_DIGEST + r"'$", "base digest")
    for digest, filename in (("source_sha", "source"), ("go_sha", "go")):
        require(
            source,
            rf'^printf .*"\${digest}" "\$scratch/{filename}\.tar\.gz" \| sha256sum -c -$',
            f"{filename} archive verification",
        )
    require(source, r"^set -euo pipefail$", "fail closed")
    require(source, r"^for output in one two; do$", "two independent build outputs")
    # Join shell continuation lines to inspect the actual build command.
    joined = source.replace("\\\n", " ")
    require(
        joined,
        r"docker run [^\n]*--network none [^\n]*src=\$scratch/\$output,dst=/out[^\n]*"
        r"GOCACHE=/tmp/cache[^\n]*build [^\n]*-mod=vendor[^\n]*-o /out/gosu",
        "offline vendor build with an output-specific mount and temporary cache",
    )
    require(
        source,
        r'^cmp "\$scratch/one/gosu" "\$scratch/two/gosu" \|\| fail ',
        "independent byte comparison",
    )


def check_candidate_execution(source):
    source = active_lines(source)
    require(
        source,
        r'installed_sha=\$\(docker run [^\n]*--entrypoint sha256sum "\$image" /usr/local/bin/gosu',
        "installed binary hash",
    )
    require(
        source,
        r'"\$installed_sha" == "\$\(cat "\$evidence/gosu-sha256.txt"\)"',
        "installed binary matches build",
    )
    require(
        source,
        r"selected_path=\$\(docker run [^\n]*--entrypoint sh \"\$image\" -c 'command -v gosu'\)",
        "entrypoint path selection",
    )
    require(source, r'"\$selected_path" == /usr/local/bin/gosu', "selected binary path")
    for flag in ("-u", "-g"):
        require(
            source,
            rf'--entrypoint /usr/local/bin/gosu "\$image" postgres id {flag}',
            "candidate identity check",
        )
    require(source, r'"\$uid" == 999 && "\$gid" == 999', "identity checked")
    require(
        source,
        r"if docker run [^\n]*--user 999:999 --entrypoint /usr/local/bin/gosu "
        r'"\$image" root id [^\n]*; then\n\s*fail ',
        "non-root to root denied",
    )


def check_workflow(source):
    source = active_lines(source)
    require(source, r"^  workflow_dispatch:$", "manual dispatch")
    require(source, r"^  pull_request:$", "pull request event")
    require(source, r"^    paths:$", "scoped pull request paths")
    for path in (
        ".github/workflows/db-gosu-candidate.yml",
        "ops/db-gosu-candidate/**",
        "scripts/test_gosu_candidate.sh",
        "tests/tooling/test_gosu_candidate.py",
    ):
        require(source, rf"^      - {re.escape(path)}$", "owned candidate path")
    require(source, r"^permissions: \{\}$", "default no permissions")
    require(source, r"^      contents: read$", "read-only contents")
    require(source, r"^        run: bash scripts/test_gosu_candidate\.sh$", "candidate script")
    if re.search(
        r"^\s*(?:pull_request_target|workflow_run|write-all|id-token|deployments):",
        source,
        re.MULTILINE,
    ):
        raise ValueError("unexpected privileged trigger or permission")
    if re.search(
        r"^\s*(?:-\s*)?(?:run|uses):.*(?:deploy|release|docker push)",
        source,
        re.MULTILINE | re.IGNORECASE,
    ):
        raise ValueError("unexpected release or deployment command")
    for action in re.findall(r"^\s*-?\s*uses:\s*(\S+)", source, re.MULTILINE):
        if not re.fullmatch(r"[\w.-]+/[\w.-]+@[0-9a-f]{40}", action):
            raise ValueError("action is not pinned")


class GosuCandidatePolicy(unittest.TestCase):
    def test_offline_build_and_digest_controls(self):
        source = (ROOT / "scripts/test_gosu_candidate.sh").read_text()
        check_script(source)
        for old, new in (
            (SOURCE_SHA, "0" * 64),
            (GO_SHA, "0" * 64),
            (POSTGRES_DIGEST, "sha256:" + "0" * 64),
            (
                '"$source_sha" "$scratch/source.tar.gz" | sha256sum -c -',
                '"$source_sha" "$scratch/source.tar.gz"',
            ),
            ('"$go_sha" "$scratch/go.tar.gz" | sha256sum -c -', '"$go_sha" "$scratch/go.tar.gz"'),
            ("for output in one two; do", "for output in one; do"),
            ("docker run --rm --network none --read-only", "docker run --rm --read-only"),
            ("-mod=vendor", "-mod=mod"),
            (
                'cmp "$scratch/one/gosu" "$scratch/two/gosu" || fail ',
                '# cmp "$scratch/one/gosu" "$scratch/two/gosu" || fail ',
            ),
        ):
            with self.subTest(old=old):
                self.assertIn(old, source)
                with self.assertRaises(ValueError):
                    check_script(source.replace(old, new))

    def test_non_hosted_runner_stops_before_scratch_or_docker(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            marker = root / "docker-was-called"
            docker = root / "docker"
            docker.write_text(f"#!/bin/sh\ntouch '{marker}'\nexit 99\n")
            docker.chmod(0o755)
            environment = {
                "PATH": f"{root}:{os.defpath}",
                "GITHUB_ACTIONS": "true",
                "RUNNER_ENVIRONMENT": "self-hosted",
                "RUNNER_TEMP": str(root),
                "GOSU_EVIDENCE_DIR": str(root / "db-gosu-evidence"),
                "GOSU_SCRATCH_DIR": str(root / "db-gosu-scratch"),
                "GOSU_SOURCE_REVISION": "a" * 40,
            }
            result = subprocess.run(
                ["bash", str(ROOT / "scripts/test_gosu_candidate.sh")],
                env=environment,
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("GitHub-hosted runner required", result.stderr)
            self.assertFalse(marker.exists())
            self.assertFalse((root / "db-gosu-scratch").exists())
            self.assertFalse((root / "db-gosu-evidence").exists())

    def test_candidate_identity_and_denial_are_exercised(self):
        source = (ROOT / "scripts/test_gosu_candidate.sh").read_text()
        check_candidate_execution(source)
        for old, new in (
            (
                '"$installed_sha" == "$(cat "$evidence/gosu-sha256.txt")"',
                '"$installed_sha" == ignored',
            ),
            ('"$selected_path" == /usr/local/bin/gosu', '"$selected_path" == ignored'),
            ('"$uid" == 999 && "$gid" == 999', '"$uid" == 0 && "$gid" == 0'),
            (
                '--user 999:999 --entrypoint /usr/local/bin/gosu "$image" root id',
                '--user 999:999 --entrypoint /usr/local/bin/gosu "$image" postgres id',
            ),
        ):
            with self.subTest(old=old):
                self.assertIn(old, source)
                with self.assertRaises(ValueError):
                    check_candidate_execution(source.replace(old, new))

    def test_workflow_is_path_scoped_and_has_no_release_action(self):
        source = (ROOT / ".github/workflows/db-gosu-candidate.yml").read_text()
        check_workflow(source)
        for old, new in (
            ("workflow_dispatch:", "# workflow_dispatch:"),
            ("    paths:", "    paths-ignore:"),
            ("permissions: {}", "permissions: write-all"),
            ("contents: read", "contents: write"),
            ("run: bash scripts/test_gosu_candidate.sh", "run: docker push example"),
            ("actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1", "actions/checkout@main"),
        ):
            with self.subTest(old=old):
                self.assertIn(old, source)
                with self.assertRaises(ValueError):
                    check_workflow(source.replace(old, new))
        for extra in (
            "\n      - uses: example/deploy@" + "a" * 40,
            "\n      - run: docker push example",
        ):
            with self.subTest(extra=extra), self.assertRaises(ValueError):
                check_workflow(source + extra)

    def test_dockerfile_pin_and_binary_copy(self):
        source = (ROOT / "ops/db-gosu-candidate/Dockerfile").read_text()
        check_dockerfile(source)
        for old, new in (
            (POSTGRES_DIGEST, "sha256:" + "0" * 64),
            ("COPY --chmod=0755 gosu /usr/local/bin/gosu", "# removed candidate copy"),
        ):
            with self.subTest(old=old):
                self.assertIn(old, source)
                with self.assertRaises(ValueError):
                    check_dockerfile(source.replace(old, new))


if __name__ == "__main__":
    unittest.main()
