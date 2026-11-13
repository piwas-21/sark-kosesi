"""CodeQL steps must use the same immutable release."""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def matching_pins(workflow):
    actions = re.findall(
        r"^\s*-\s*uses:\s*github/codeql-action/([a-z][a-z0-9_-]*)@([^\s]+)\s*$",
        workflow,
        flags=re.MULTILINE,
    )
    return (
        sum(name == "analyze" for name, _ in actions) == 1
        and sum(name == "init" for name, _ in actions) == 1
        and len({pin for _, pin in actions}) == 1
        and all(re.fullmatch(r"[0-9a-f]{40}", pin) for _, pin in actions)
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


if __name__ == "__main__":
    unittest.main()
