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

    def test_inventory_and_stock_each_require_critical_coverage(self):
        files = self.document["data"][0]["files"]
        for name, zero_allowed in (
            ("crates/domain/src/inventory.rs", False),
            ("crates/infrastructure/src/stock.rs", False),
            ("crates/domain/src/holds.rs", False),
            ("crates/infrastructure/src/holds/create.rs", False),
            ("crates/infrastructure/src/holds/mod.rs", True),
            ("crates/application/src/staff_access.rs", False),
            ("crates/infrastructure/src/staff_access.rs", False),
            ("crates/infrastructure/src/staff_operations.rs", False),
            ("crates/infrastructure/src/catalogue_prices.rs", False),
            ("crates/domain/src/catalogue_authoring.rs", False),
            ("crates/domain/src/factual_review.rs", False),
            ("crates/infrastructure/src/factual_review/mod.rs", False),
            ("crates/infrastructure/src/factual_review/read.rs", False),
            ("crates/infrastructure/src/factual_review/write.rs", False),
            ("crates/infrastructure/src/catalogue_authoring/mod.rs", False),
            ("crates/infrastructure/src/catalogue_authoring/apply.rs", False),
            ("crates/infrastructure/src/catalogue_authoring/events.rs", False),
            ("crates/domain/src/specifications.rs", False),
            ("crates/infrastructure/src/specifications.rs", False),
            ("crates/domain/src/translations.rs", False),
            ("crates/infrastructure/src/translations/mod.rs", False),
            ("crates/infrastructure/src/translations/read.rs", False),
            ("crates/infrastructure/src/translations/review.rs", False),
            ("crates/infrastructure/src/translations/write.rs", False),
            ("crates/domain/src/media.rs", False),
            ("crates/infrastructure/src/media/mod.rs", False),
            ("crates/infrastructure/src/media/register.rs", False),
            ("crates/infrastructure/src/media/rights.rs", False),
            ("crates/infrastructure/src/image_ingestion/mod.rs", False),
            ("crates/infrastructure/src/image_ingestion/process.rs", False),
            ("crates/infrastructure/src/image_ingestion/framing.rs", False),
            ("crates/infrastructure/src/image_ingestion/store.rs", False),
        ):
            source = self.root / name
            source.parent.mkdir(parents=True, exist_ok=True)
            source.write_text("// critical fixture\n" * 20)
            item = copy.deepcopy(files[0])
            item["filename"] = str(source)
            files.append(item)
            self.assertEqual(len(validate(self.document, self.root)), 5)
            for counter in ("lines", "branches"):
                item["summary"][counter]["covered"] = 18
                with self.assertRaisesRegex(ValueError, "Critical Rust"):
                    validate(self.document, self.root)
                item["summary"][counter]["covered"] = 19
                self.assertEqual(len(validate(self.document, self.root)), 5)
                item["summary"][counter]["covered"] = 20
            item["summary"]["branches"] = {"count": 0, "covered": 0}
            if not zero_allowed:
                with self.assertRaisesRegex(ValueError, "Critical Rust"):
                    validate(self.document, self.root)
                item["summary"]["branches"] = {"count": 20, "covered": 20}
            self.assertEqual(len(validate(self.document, self.root)), 5)
            if zero_allowed:
                item["summary"]["regions"]["covered"] = 18
                with self.assertRaisesRegex(ValueError, "Critical Rust"):
                    validate(self.document, self.root)
            files.pop()
            with self.assertRaisesRegex(ValueError, "Critical Rust source unmeasured"):
                validate(self.document, self.root)
            source.unlink()

    def test_money_and_preview_authorization_still_reject_zero_branches(self):
        for index in (0, 3):
            document = copy.deepcopy(self.document)
            document["data"][0]["files"][index]["summary"]["branches"] = {"count": 0, "covered": 0}
            with self.assertRaisesRegex(ValueError, "Critical Rust"):
                validate(document, self.root)

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

    def lcov(self, document=None):
        document = document or self.document
        records = []
        for item in document["data"][0]["files"]:
            records.append(f"SF:{item['filename']}")
            records += [f"DA:{line},1" for line in range(1, 21)]
            records += [f"BRDA:{line},0,0,1" for line in range(1, 21)]
            records += ["LF:20", "LH:20", "BRF:20", "BRH:20", "end_of_record"]
        return "\n".join(records)

    def test_rust_sonar_import_is_populated_and_matches_native(self):
        from rust_coverage import validate_lcov

        self.assertEqual(validate_lcov(self.lcov(), self.document, self.root), 5)
        # Native LLVM may count macro/async regions beyond physical DA records.
        native = copy.deepcopy(self.document)
        native["data"][0]["files"][0]["summary"]["lines"] = {"count": 22, "covered": 22}
        text = self.lcov().replace("LF:20\nLH:20", "LF:22\nLH:22", 1)
        self.assertEqual(validate_lcov(text, native, self.root), 5)

    def test_empty_absent_unrelated_and_inconsistent_rust_imports_fail(self):
        from rust_coverage import validate_lcov

        valid = self.lcov()
        first = self.document["data"][0]["files"][0]["filename"]
        for text in (
            "",
            "TN:empty",
            valid.split("end_of_record")[0],
            valid.replace("SF:" + first, "SF:/unrelated.rs", 1),
            valid.replace("LH:20", "LH:19", 1),
            valid.replace("DA:1,1", "DA:1,1\nDA:1,1", 1),
            valid.replace("BRDA:1,0,0,1\n", "", 1),
            valid.replace("DA:1,1", "DA:21,1", 1),
            valid + "\n" + valid,
        ):
            with self.subTest(text=text[:60]), self.assertRaises(ValueError):
                validate_lcov(text, self.document, self.root)
        no_lines = "\n".join(line for line in valid.splitlines() if not line.startswith("DA:"))
        with self.assertRaisesRegex(ValueError, "line records missing"):
            validate_lcov(no_lines, self.document, self.root)
        with self.assertRaises(ValueError):
            validate_lcov(
                valid.split("end_of_record")[0] + "\nend_of_record", self.document, self.root
            )
