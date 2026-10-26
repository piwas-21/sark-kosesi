import copy
import tempfile
import unittest
from pathlib import Path

from rust_coverage import validate


class RustCoverage(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        files = []
        for name in ("domain", "application", "infrastructure", "api", "worker"):
            source = self.root / f"crates/{name}/src/lib.rs"
            source.parent.mkdir(parents=True)
            source.write_text("// independent fixture\n" * 20)
            files.append(
                {
                    "filename": str(source),
                    "summary": {
                        kind: {"count": 20, "covered": 20}
                        for kind in ("lines", "regions", "branches")
                    },
                }
            )
        self.document = {"type": "llvm.coverage.json.export", "data": [{"files": files}]}

    def test_real_package_thresholds_and_zero_branch_evidence(self):
        self.assertEqual(len(validate(self.document, self.root)), 5)
        self.document["data"][0]["files"][1]["summary"]["branches"] = {"count": 0, "covered": 0}
        result = validate(self.document, self.root)
        self.assertEqual(result[1]["counts"]["branches"], (0, 0))
        self.document["data"][0]["files"][1]["summary"]["regions"]["covered"] = 18
        with self.assertRaises(ValueError):
            validate(self.document, self.root)

    def test_missing_stale_empty_and_inflated_coverage_fail(self):
        for change in (
            "type",
            "file",
            "counter",
            "impossible",
            "critical",
            "low-line",
            "low-branch",
            "duplicate",
            "outside",
        ):
            document = copy.deepcopy(self.document)
            files = document["data"][0]["files"]
            if change == "type":
                document["type"] = "wrong"
            elif change == "file":
                files.pop()
            elif change == "counter":
                del files[0]["summary"]["branches"]
            elif change == "impossible":
                files[0]["summary"]["lines"]["covered"] = 21
            elif change == "critical":
                files[0]["summary"]["branches"]["covered"] = 18
            elif change == "low-line":
                files[1]["summary"]["lines"]["covered"] = 16
            elif change == "low-branch":
                files[1]["summary"]["branches"]["covered"] = 15
            elif change == "duplicate":
                files.append(copy.deepcopy(files[0]))
            elif change == "outside":
                files[0]["filename"] = "/outside.rs"
            with self.subTest(change=change), self.assertRaises(ValueError):
                validate(document, self.root)
