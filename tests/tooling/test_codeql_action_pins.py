"""CodeQL steps must use the same immutable release.

The workflow convention permits only single-line, literal ``uses`` values.
Unsupported YAML forms fail closed rather than escaping this check.
"""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
USE_KEY = re.compile(r"(?:\buses|['\"]uses['\"])\s*:")
USE_LINE = re.compile(
    r"^\s*(?:-\s+)?(?:uses|'uses'|\"uses\")\s*:\s*"
    r"(?:'([^']*)'|\"([^\"\\]*)\"|([^\s#'\"\\]+))\s*(?:#.*)?$"
)
CODEQL = re.compile(r"github/codeql-action/([a-z0-9_-]+(?:/[a-z0-9_-]+)*)@([0-9a-f]{40})", re.I)


def matching_pins(workflow):
    actions = []
    for line in workflow.splitlines():
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        match = USE_LINE.fullmatch(line)
        if not match:
            if USE_KEY.search(line) or "github/codeql-action" in line.lower():
                return False
            continue
        value = next(value for value in match.groups() if value is not None)
        # Aliases, expressions and block scalars cannot be verified as literals.
        if value.startswith(("*", "&", "|", ">", "${")):
            return False
        if value.lower().startswith("github/codeql-action"):
            action = CODEQL.fullmatch(value)
            if not action:
                return False
            actions.append((action.group(1).lower(), action.group(2).lower()))
    return (
        sum(name == "analyze" for name, _ in actions) == 1
        and sum(name == "init" for name, _ in actions) == 1
        and len({pin for _, pin in actions}) == 1
    )


class CodeQLPins(unittest.TestCase):
    def test_workflow_pins_both_actions_to_one_commit(self):
        self.assertTrue(matching_pins((ROOT / ".github/workflows/ci.yml").read_text()))

    def test_mismatched_missing_and_mutable_pins_fail(self):
        pin = "a" * 40
        healthy = (
            f"- uses: github/codeql-action/init@{pin}\n- uses: github/codeql-action/analyze@{pin}\n"
        )
        self.assertTrue(matching_pins(healthy))
        self.assertFalse(matching_pins(healthy.replace("analyze@" + pin, "analyze@" + "b" * 40)))
        self.assertFalse(matching_pins(f"- uses: github/codeql-action/init@{pin}\n"))
        self.assertFalse(matching_pins(healthy.replace(pin, "v4")))
        self.assertFalse(
            matching_pins(healthy + f"- uses: github/codeql-action/upload-sarif@{'b' * 40}\n")
        )
        self.assertTrue(
            matching_pins(healthy + f"- uses: github/codeql-action/upload-sarif@{pin}\n")
        )

    def test_comments_quotes_and_extra_subactions_are_checked(self):
        pin = "a" * 40
        healthy = f"""- uses: "github/codeql-action/init@{pin}" # pinned
  uses: 'github/codeql-action/analyze@{pin}' # pinned
- uses: github/codeql-action/upload-sarif@{pin} # pinned
"""
        self.assertTrue(matching_pins(healthy))
        self.assertFalse(
            matching_pins(healthy.replace("upload-sarif@" + pin, "upload-sarif@" + "b" * 40))
        )
        self.assertFalse(matching_pins(healthy.replace("analyze@" + pin, "analyze@" + "b" * 40)))
        self.assertFalse(matching_pins(healthy.replace("init@" + pin, "init@" + "b" * 40)))

    def test_unverifiable_yaml_forms_fail_closed(self):
        pin = "a" * 40
        healthy = (
            f"- uses: github/codeql-action/init@{pin}\n- uses: github/codeql-action/analyze@{pin}\n"
        )
        for line in (
            "- uses: *alias",
            "- uses: >",
            "- uses: ${{ inputs.action }}",
            f'- uses: "github/\\u0063odeql-action/upload-sarif@{pin}"',
            f"- {{uses: github/codeql-action/upload-sarif@{pin}}}",
            f"- uses: github/codeql-action/upload-sarif@{'b' * 40} # wrong",
        ):
            with self.subTest(line=line):
                self.assertFalse(matching_pins(healthy + line + "\n"))


if __name__ == "__main__":
    unittest.main()
