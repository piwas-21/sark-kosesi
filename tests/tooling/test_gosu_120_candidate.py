"""Source controls for the separate opt-in Gosu 1.20 Alpine experiment."""

import re
import subprocess
import unittest
from pathlib import Path

from tests.tooling.test_gosu_candidate import active_lines, require

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/test_gosu_candidate.sh"
ALPINE = "sha256:fa1b61f5a4844f4d4b73c9b15716cfc75fe70c931f2d32723630f3b900056366"


def check_120(source):
    source = active_lines(source)
    for pattern in (
        r"source_commit=29b939aa5bd4dc588af765b8248f6e39150d2565",
        r"source_sha=2b70da8a5480fafedc16bb1fc91b572f064d03df736cbd91a616af2f1b66ab51",
        r"base='[^']+@" + ALPINE + "'",
        r"base_config='sha256:0f6c51e48b3458e87ab43692e49c9dafc72ffe0a0dbde3fd8cdb6ab92d5c025e'",
        r"user_version=v0\.4\.1; expected_version=1\.20; dockerfile=ops/db-gosu-candidate/Dockerfile\.alpine",
        r"unexpected compiled user dependency",
        r"unexpected compiled system dependency",
        r'"\$uid" == 70 && "\$gid" == 70 && "\$group_set" == "\$base_group_set"',
        r'",\$group_set," != \*,0,\* && ",\$group_set," == \*,70,\*',
        r'--user 70:70 --entrypoint /usr/local/bin/gosu "\$image" root id',
        r"\[\[ \"\$stock_uid\" == 70 \]\]",
        r"grep -Fq 'bufio.Scanner: token too long' \"\$scratch/oversized.log\"",
        r"candidate accepted oversized group line",
    ):
        require(source, pattern, "missing 1.20 control")


class Gosu120Policy(unittest.TestCase):
    def test_profile_selects_exact_inputs_and_rejects_unknown(self):
        source = SCRIPT.read_text()
        block = re.search(r"(?ms)^profile=\$\{GOSU_PROFILE:-1\.19\}\n.*?^esac$", source)
        self.assertIsNotNone(block)
        code = (
            "fail() { exit 42; }\nsource_commit=stock\nsource_sha=stock\n"
            "base=stock\nbase_config=stock\n"
            + block.group()
            + '\nprintf "%s|%s|%s|%s|%s" "$profile" "$source_commit" "$source_sha" "$user_version" "$dockerfile"'
        )
        for profile, expected in (
            ("1.19", "1.19|stock|stock|v0.1.0|ops/db-gosu-candidate/Dockerfile"),
            (
                "1.20",
                "1.20|29b939aa5bd4dc588af765b8248f6e39150d2565|"
                "2b70da8a5480fafedc16bb1fc91b572f064d03df736cbd91a616af2f1b66ab51|"
                "v0.4.1|ops/db-gosu-candidate/Dockerfile.alpine",
            ),
        ):
            result = subprocess.run(
                ["bash", "-c", code],
                env={"GOSU_PROFILE": profile},
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(result.returncode, 0)
            self.assertEqual(result.stdout, expected)
        result = subprocess.run(
            ["bash", "-c", code], env={"GOSU_PROFILE": "custom"}, capture_output=True, check=False
        )
        self.assertEqual(result.returncode, 42)

    def test_security_controls_reject_mutations(self):
        source = SCRIPT.read_text()
        check_120(source)
        for old, new in (
            ("29b939aa5bd4dc588af765b8248f6e39150d2565", "0" * 40),
            (ALPINE, "sha256:" + "0" * 64),
            ("user_version=v0.4.1", "user_version=v0.1.0"),
            ('"$uid" == 70 && "$gid" == 70', '"$uid" == 0 && "$gid" == 0'),
            (
                '--user 70:70 --entrypoint /usr/local/bin/gosu "$image" root id',
                '--user 70:70 --entrypoint /usr/local/bin/gosu "$image" postgres id',
            ),
            (
                "grep -Fq 'bufio.Scanner: token too long'",
                "# grep -Fq 'bufio.Scanner: token too long'",
            ),
        ):
            with self.subTest(old=old):
                self.assertIn(old, source)
                with self.assertRaises(ValueError):
                    check_120(source.replace(old, new))

    def test_workflow_and_synthetic_dockerfile_are_restricted(self):
        dockerfile = active_lines(
            (ROOT / "ops/db-gosu-candidate/Dockerfile.alpine").read_text()
        ).splitlines()
        self.assertEqual(
            dockerfile,
            [
                "FROM mirror.gcr.io/library/postgres@" + ALPINE,
                "COPY --chmod=0755 gosu /usr/local/bin/gosu",
            ],
        )
        workflow = active_lines((ROOT / ".github/workflows/db-gosu-120-candidate.yml").read_text())
        for item in (
            "workflow_dispatch:",
            "pull_request:",
            "permissions: {}",
            "contents: read",
            'GOSU_PROFILE: "1.20"',
            "run: bash scripts/test_gosu_candidate.sh",
            "persist-credentials: false",
            "if-no-files-found: error",
        ):
            self.assertIn(item, workflow)
        self.assertNotRegex(workflow, r"pull_request_target|write-all|docker push|id-token")
        for action in re.findall(r"^\s*-?\s*uses:\s*(\S+)", workflow, re.MULTILINE):
            self.assertRegex(action, r"^[\w.-]+/[\w.-]+@[0-9a-f]{40}$")
