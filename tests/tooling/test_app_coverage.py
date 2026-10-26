import tempfile
import unittest
from pathlib import Path

from check_app_coverage import check, read_lcov


class ApplicationCoverage(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.package = self.root / "crates/domain/src"
        self.package.mkdir(parents=True)
        self.source = self.package / "money.rs"
        self.source.write_text("// independent coverage fixture\n" * 20)
        self.report = self.root / "actual.lcov"

    def lcov(self, line_hits=20, branch_hits=20, source=None):
        source = source or self.source.relative_to(self.root)
        rows = [f"SF:{source}"]
        rows += [f"DA:{i},{int(i <= line_hits)}" for i in range(1, 21)]
        rows += [f"BRDA:{i},0,0,{1 if i <= branch_hits else '-'}" for i in range(1, 21)]
        return "\n".join(
            rows + ["LF:20", f"LH:{line_hits}", "BRF:20", f"BRH:{branch_hits}", "end_of_record"]
        )

    def run_gate(self, text, critical=()):
        self.report.write_text(text)
        return check(self.report, "crates/domain/src", critical, root=self.root)

    def test_independent_package_and_critical_thresholds(self):
        self.assertEqual(self.run_gate(self.lcov(17, 16)), 1)
        for lines, branches in ((16, 20), (20, 15)):
            with (
                self.subTest(lines=lines, branches=branches),
                self.assertRaisesRegex(ValueError, "below"),
            ):
                self.run_gate(self.lcov(lines, branches))
        critical = ["crates/domain/src/money.rs"]
        self.assertEqual(self.run_gate(self.lcov(19, 19), critical), 1)
        with self.assertRaisesRegex(ValueError, "below 95"):
            self.run_gate(self.lcov(19, 18), critical)

    def test_missing_source_cannot_hide_in_aggregate(self):
        (self.package / "inventory.rs").write_text("// must be measured\n")
        with self.assertRaisesRegex(ValueError, "Unmeasured"):
            self.run_gate(self.lcov())

    def test_zero_branches_are_not_full_branch_coverage(self):
        text = self.lcov()
        text = "\n".join(line for line in text.splitlines() if not line.startswith("BRDA:"))
        text = text.replace("BRF:20", "BRF:0").replace("BRH:20", "BRH:0")
        with self.assertRaisesRegex(ValueError, "both required"):
            self.run_gate(text)

    def test_absolute_in_repo_paths_from_llvm_cov_are_supported(self):
        self.assertEqual(self.run_gate(self.lcov(source=self.source)), 1)

    def test_unrelated_coverage_cannot_raise_package_results(self):
        other = self.root / "other.ts"
        other.write_text("// other\n" * 20)
        with self.assertRaisesRegex(ValueError, "below"):
            self.run_gate(self.lcov(16, 20) + "\n" + self.lcov(source="other.ts"))

    def test_corrupt_or_forged_records_fail_closed(self):
        valid = self.lcov()
        for text in (
            "",
            "TN:empty",
            valid.removesuffix("end_of_record"),
            valid + "\n" + valid,
            valid.replace("LH:20", "LH:19"),
            valid.replace("DA:1,1", "DA:1,1\nDA:1,1"),
            valid.replace("BRDA:1,0,0,1", "BRDA:1,0,0,1\nBRDA:1,0,0,1"),
            valid.replace("DA:1,1", "DA:0,1"),
            valid.replace("DA:1,1", "DA:21,1"),
            valid.replace("DA:1,1", "DA:1,-1"),
            valid.replace("DA:1,1", "DA:1"),
            valid.replace("BRDA:1,0,0,1", "BRDA:1,0"),
            valid.replace("LF:20", "LF:20\nLF:20"),
            valid.replace("LF:20\n", ""),
            "end_of_record",
            "DA:1,1",
            valid.replace("end_of_record", "SF:crates/domain/src/money.rs"),
        ):
            with self.subTest(text=text[:80]), self.assertRaises(ValueError):
                read_lcov(text, self.root)

    def test_scope_cannot_escape_or_be_empty(self):
        for package in ("absent", "..", "."):
            if package == ".":
                self.source.unlink()
            with self.subTest(package=package), self.assertRaises(ValueError):
                check(self.report, package, [], root=self.root)
        with self.assertRaisesRegex(ValueError, "outside repository"):
            read_lcov("SF:../outside.rs\nend_of_record", self.root)

    def test_critical_file_must_belong_to_package(self):
        other = self.root / "other.ts"
        other.write_text("// other\n" * 20)
        with self.assertRaisesRegex(ValueError, "outside the measured package"):
            self.run_gate(self.lcov(), ["other.ts"])
