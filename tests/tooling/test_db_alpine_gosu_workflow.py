"""Boundary controls for the disposable Alpine Gosu workflow."""

import os
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
        "expected_gosu=8177d9590273ed8699065bf0c9776b634fde24bfc75375c83e927134ce868b7a",
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
        self.assertLess(
            workflow.index("bash scripts/test_gosu_candidate.sh"),
            workflow.index("bash scripts/test_db_alpine_gosu.sh"),
        )

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
