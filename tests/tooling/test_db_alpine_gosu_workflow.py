"""Boundary controls for the disposable Alpine Gosu workflow."""

import os
import re
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/test_db_alpine_gosu.sh"
WORKFLOW = ROOT / ".github/workflows/db-alpine-gosu-lab.yml"


def validate(source):
    required = (
        '[[ "$(cat "$gosu_evidence/status.txt")" == \'stage=complete outcome=success\' ]]',
        '(cd "$gosu_evidence" && sha256sum -c SHA256SUMS >/dev/null)',
        "source_revision=$ALPINE_GOSU_SOURCE_REVISION",
        "expected_gosu=ad2e645f69d82e23e0b54a208828589ff4a021a343ad5090cd36f2a2ae7aa556",
        "grep -Fxq 'profile=1.20'",
        "candidate_id=$(sed -n",
        'image="$candidate_id"',
        "ops/db-zlib-candidate/pull_verified_base.sh",
        'scripts/rehearse_alpine_gosu_restore.py "$candidate_id"',
        'cmp "$gosu_scratch/one/gosu" "$gosu_scratch/two/gosu"',
        "stock_config='sha256:0f6c51e48b3458e87ab43692e49c9dafc72ffe0a0dbde3fd8cdb6ab92d5c025e'",
        '"$uid" == 70 && "$gid" == 70 && "$group_set" == "$stock_group_set"',
        '",$group_set," != *,0,*',
        '--user 70:70 --entrypoint /usr/local/bin/gosu "$image" root id',
        "docker run --detach --network none",
        "python3 scripts/check_alpine_gosu_image.py",
        "scan stock --image-src remote",
        "scan candidate --skip-db-update --input /scratch/candidate.tar",
        'cmp "$evidence/db-metadata.json" "$scratch/cache/db/metadata.json"',
        'scripts/check_db_alpine_gosu.py "$evidence" "$candidate_id"',
    )
    return all(part in source for part in required)


class AlpineGosuWorkflowTests(unittest.TestCase):
    def test_safety_and_custody_controls_have_negative_cases(self):
        source = SCRIPT.read_text()
        self.assertTrue(validate(source))
        for old, new in (
            ("sha256sum -c SHA256SUMS", "true"),
            ("grep -Fxq 'profile=1.20'", "true"),
            ('image="$candidate_id"', 'image="other"'),
            ('cmp "$gosu_scratch/one/gosu" "$gosu_scratch/two/gosu"', "true"),
            ('"$group_set" == "$stock_group_set"', "true"),
            ('",$group_set," != *,0,*', "true"),
            ("docker run --detach --network none", "docker run --detach"),
            ("--skip-db-update --input /scratch/candidate.tar", "--input /scratch/candidate.tar"),
            ("python3 scripts/check_alpine_gosu_image.py", "true"),
        ):
            with self.subTest(old=old):
                self.assertIn(old, source)
                self.assertFalse(validate(source.replace(old, new)))
        workflow = WORKFLOW.read_text()
        self.assertIn('GOSU_PROFILE: "1.20"', workflow)
        self.assertLess(
            workflow.index("bash scripts/test_gosu_candidate.sh"),
            workflow.index("bash scripts/test_db_alpine_gosu.sh"),
        )
        self.assertLess(
            source.index('scripts/check_db_alpine_gosu.py "$evidence"'),
            source.index('scripts/rehearse_alpine_gosu_restore.py "$candidate_id"'),
        )
        self.assertIn('outcome=success\\n\' > "$evidence/scan-status.txt"', source)
        self.assertIn('outcome=success\\n\' > "$evidence/restore-status.txt"', source)

    def test_scanner_artifact_keeps_only_fixed_outcomes_and_exit_codes(self):
        source = SCRIPT.read_text()
        block = re.search(r"(?ms)^scan\(\) \{\n.*?^\}", source)
        self.assertIsNotNone(block)
        mock = r"""
set -euo pipefail
docker() {
    [ "$1" = run ] && [ "$2" = --pull=never ] || return 99
    printf '%s\n' "$MOCK_DIAGNOSTIC" >&2
    return "$MOCK_EXIT"
}
scanner=mock-scanner
scanner_args=(--format json)
"""
        for label, code, diagnostic, category in (
            ("stock", 0, "scanner complete", "success"),
            ("stock", 125, "mirror.gcr.io: toomanyrequests:", "mirror_rate_limit"),
            (
                "stock",
                43,
                "https://mirror.gcr.io/v2/ returned 429 Too Many Requests",
                "mirror_rate_limit",
            ),
            ("candidate", 43, "https://mirror.gcr.io/v2/ returned 403 Forbidden", "generic_exit"),
            (
                "stock",
                43,
                "mirror.gcr.io configured\nhttps://other.test/ status: 429",
                "generic_exit",
            ),
            ("stock", 43, "https://mirror.gcr.io.other.test/ status: 429", "generic_exit"),
        ):
            with self.subTest(diagnostic=diagnostic), tempfile.TemporaryDirectory() as directory:
                evidence = Path(directory) / "evidence"
                scratch = Path(directory) / "scratch"
                evidence.mkdir()
                scratch.mkdir()
                raw = (
                    diagnostic
                    + "\nAuthorization: Bearer private-auth-fixture ?sig=private-url-fixture"
                )
                result = subprocess.run(
                    ["bash", "-c", mock + block.group() + f"\nscan {label}"],
                    env={
                        "PATH": os.defpath,
                        "evidence": str(evidence),
                        "scratch": str(scratch),
                        "MOCK_EXIT": str(code),
                        "MOCK_DIAGNOSTIC": raw,
                    },
                    capture_output=True,
                    text=True,
                    check=False,
                )
                self.assertEqual(result.returncode, code, result.stderr)
                self.assertEqual((result.stdout, result.stderr), ("", ""))
                self.assertEqual(
                    sorted(path.name for path in evidence.iterdir()),
                    [f"{label}-command.txt", f"{label}-scan-status.txt"],
                )
                self.assertEqual(
                    (evidence / f"{label}-scan-status.txt").read_text(),
                    f"category={category} exit={code}\n",
                )
                self.assertNotIn("private-", (evidence / f"{label}-command.txt").read_text())
                log = scratch / f"{label}-scan.log"
                self.assertEqual(log.read_text(), raw + "\n")
                self.assertEqual(log.stat().st_mode & 0o077, 0)

    def test_non_hosted_request_never_invokes_docker_or_creates_outputs(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            marker = root / "docker-called"
            docker = root / "docker"
            docker.write_text(f"#!/bin/sh\ntouch '{marker}'\nexit 99\n")
            docker.chmod(0o755)
            env = {
                "PATH": f"{root}:{os.defpath}",
                "RUNNER_TEMP": directory,
                "RUNNER_ENVIRONMENT": "self-hosted",
                "GITHUB_ACTIONS": "true",
                "ALPINE_GOSU_SOURCE_REVISION": "a" * 40,
                "ALPINE_GOSU_EVIDENCE_DIR": str(root / "db-alpine-gosu-evidence"),
                "ALPINE_GOSU_SCRATCH_DIR": str(root / "db-alpine-gosu-scratch"),
            }
            result = subprocess.run(
                ["bash", str(SCRIPT)], env=env, capture_output=True, check=False
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(marker.exists())
            self.assertFalse((root / "db-alpine-gosu-evidence").exists())
            self.assertFalse((root / "db-alpine-gosu-scratch").exists())
