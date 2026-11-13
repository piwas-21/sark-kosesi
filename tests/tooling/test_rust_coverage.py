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
                    "branches": [[line, 1, line, 2, 1, 1, 0, 0, 4] for line in range(1, 11)],
                    "expansions": [],
                    "summary": {
                        kind: {"count": 20, "covered": 20}
                        for kind in ("lines", "regions", "branches")
                    },
                }
            )
        self.document = {
            "type": "llvm.coverage.json.export",
            "version": "3.1.0",
            "data": [
                {
                    "files": files,
                    "functions": [
                        {
                            "filenames": [item["filename"]],
                            "branches": copy.deepcopy(item["branches"]),
                        }
                        for item in files
                    ],
                }
            ],
        }

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
            ("crates/domain/src/quotes/mod.rs", False),
            ("crates/domain/src/quotes/policy.rs", False),
            ("crates/domain/src/quotes/totals.rs", False),
            ("crates/infrastructure/src/quotes/mod.rs", False),
            ("crates/infrastructure/src/quotes/stock.rs", False),
            ("crates/infrastructure/src/stock.rs", False),
            ("crates/domain/src/order_stock.rs", False),
            ("crates/domain/src/order_shipments.rs", False),
            ("crates/infrastructure/src/order_stock/mod.rs", False),
            ("crates/infrastructure/src/order_stock/source.rs", False),
            ("crates/infrastructure/src/order_stock/rows.rs", False),
            ("crates/infrastructure/src/order_stock/effects.rs", False),
            ("crates/infrastructure/src/order_stock/allocation/history.rs", False),
            ("crates/domain/src/late_stock.rs", False),
            ("crates/domain/src/late_stock/mod.rs", False),
            ("crates/domain/src/late_stock/footprint.rs", False),
            ("crates/infrastructure/src/late_stock/mod.rs", False),
            ("crates/infrastructure/src/late_stock/rows/allocations.rs", False),
            ("crates/domain/src/payment_attempts/callbacks.rs", False),
            ("crates/infrastructure/src/payment_attempts/callbacks/lookup.rs", False),
            ("crates/infrastructure/src/payment_attempts/callbacks/lookup/rows.rs", False),
            ("crates/domain/src/payment_attempts/callbacks/inbox.rs", False),
            ("crates/infrastructure/src/payment_attempts/callbacks/inbox/jobs.rs", False),
            ("crates/infrastructure/src/payment_attempts/callbacks/inbox/rows.rs", False),
            ("crates/worker/src/callbacks.rs", False),
            ("crates/api/src/callbacks/mod.rs", False),
            ("crates/api/src/callbacks/form.rs", False),
            ("crates/api/src/callback_edge/mod.rs", False),
            ("crates/api/src/callback_edge/config.rs", False),
            ("crates/api/src/bin/callback_edge.rs", False),
            ("crates/infrastructure/src/callback_forwarder.rs", False),
            ("crates/api/src/config.rs", False),
            ("crates/api/src/test_login_input.rs", False),
            ("crates/api/src/test_session.rs", False),
            ("crates/api/src/test_favourites.rs", False),
            ("crates/api/src/test_linked_orders.rs", False),
            ("crates/api/src/test_restock.rs", False),
            ("crates/domain/src/holds.rs", False),
            ("crates/infrastructure/src/holds/create.rs", False),
            ("crates/infrastructure/src/holds/mod.rs", True),
            ("crates/application/src/staff_access.rs", False),
            ("crates/infrastructure/src/staff_access.rs", False),
            ("crates/infrastructure/src/staff_operations.rs", False),
            ("crates/infrastructure/src/catalogue_prices.rs", False),
            ("crates/domain/src/catalogue_authoring.rs", False),
            ("crates/domain/src/publication.rs", False),
            ("crates/infrastructure/src/publication/mod.rs", False),
            ("crates/infrastructure/src/publication/sources.rs", False),
            ("crates/infrastructure/src/publication/read.rs", False),
            ("crates/infrastructure/src/publication/write.rs", False),
            ("crates/infrastructure/src/published_catalogue/read.rs", False),
            ("crates/infrastructure/src/published_catalogue/image.rs", False),
            ("crates/infrastructure/src/published_catalogue/stock.rs", False),
            ("crates/api/src/published.rs", False),
            ("crates/api/src/published_types.rs", False),
            ("crates/domain/src/image_review.rs", False),
            ("crates/infrastructure/src/image_review/mod.rs", False),
            ("crates/infrastructure/src/image_review/read.rs", False),
            ("crates/infrastructure/src/image_review/write.rs", False),
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
            records += [f"BRDA:{line},0,{branch},1" for line in range(1, 11) for branch in (0, 1)]
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

    def test_unified_branch_details_can_exceed_native_covered_summary(self):
        from rust_coverage import validate_lcov

        native = copy.deepcopy(self.document)
        first = native["data"][0]["files"][0]
        first["branches"] += [[1, 1, 1, 2, 0, 2, 0, 0, 4]]
        native["data"][0]["functions"][0]["branches"] = copy.deepcopy(first["branches"])
        first["summary"]["branches"]["covered"] = 19
        text = self.lcov().replace("BRDA:1,0,1,1", "BRDA:1,0,1,3", 1)
        text = text.replace("BRH:20", "BRH:19", 1)
        self.assertEqual(validate_lcov(text, native, self.root), 5)
        # Swapping hits preserves both population and positive-hit totals.
        altered = text.replace("BRDA:1,0,0,1\nBRDA:1,0,1,3", "BRDA:1,0,0,3\nBRDA:1,0,1,1", 1)
        with self.assertRaisesRegex(ValueError, "branch population"):
            validate_lcov(altered, native, self.root)
        missing = text.replace("BRDA:1,0,1,3\n", "", 1)
        with self.assertRaisesRegex(ValueError, "branch population"):
            validate_lcov(missing, native, self.root)

    def test_opposite_generic_instances_unify_without_changing_summary(self):
        from rust_coverage import validate_lcov

        native = copy.deepcopy(self.document)
        first = native["data"][0]["files"][0]
        first["branches"][0][5] = 0
        first["branches"].append([1, 1, 1, 2, 0, 1, 0, 0, 4])
        functions = native["data"][0]["functions"]
        functions[0]["branches"] = copy.deepcopy(first["branches"][:-1])
        functions.append({"filenames": [first["filename"]], "branches": [first["branches"][-1]]})
        first["summary"]["branches"]["covered"] = 19
        text = self.lcov().replace("BRH:20", "BRH:19", 1)
        self.assertEqual(validate_lcov(text, native, self.root), 5)

    def test_same_line_column_order_and_uncovered_branch_are_checked(self):
        from rust_coverage import validate_lcov

        native = copy.deepcopy(self.document)
        first = native["data"][0]["files"][0]
        first["branches"].insert(0, [1, 4, 1, 5, 0, 0, 0, 0, 4])
        native["data"][0]["functions"][0]["branches"] = copy.deepcopy(first["branches"])
        first["summary"]["branches"]["count"] = 22
        text = self.lcov().replace("BRDA:1,0,1,1", "BRDA:1,0,1,1\nBRDA:1,1,2,-\nBRDA:1,1,3,-", 1)
        text = text.replace("BRF:20", "BRF:22", 1)
        self.assertEqual(validate_lcov(text, native, self.root), 5)
        with self.assertRaisesRegex(ValueError, "branch population"):
            validate_lcov(text.replace("BRDA:1,1,3,-\n", "", 1), native, self.root)

    def test_joint_loss_of_detail_cannot_pass_with_intact_summaries(self):
        from rust_coverage import validate_lcov

        for drop_pair in (True, False):
            native = copy.deepcopy(self.document)
            rows = native["data"][0]["files"][0]["branches"]
            text = self.lcov()
            if drop_pair:
                rows.pop(0)
                text = text.replace("BRDA:1,0,0,1\nBRDA:1,0,1,1\n", "", 1)
            else:
                rows[0][4] = 0
                text = text.replace("BRDA:1,0,0,1", "BRDA:1,0,0,0", 1)
            native["data"][0]["functions"][0]["branches"] = copy.deepcopy(rows)
            with (
                self.subTest(drop_pair=drop_pair),
                self.assertRaisesRegex(ValueError, "branch population"),
            ):
                validate_lcov(text, native, self.root)

    def test_folded_direction_can_exceed_native_population(self):
        from rust_coverage import validate_lcov

        native = copy.deepcopy(self.document)
        first = native["data"][0]["files"][0]
        # One compiler-folded direction is absent from the native summary but
        # retained in both LLVM detail projections and the LCOV branch records.
        first["branches"][0][4] = 0
        native["data"][0]["functions"][0]["branches"][0][4] = 0
        first["summary"]["branches"] = {"count": 19, "covered": 19}
        text = self.lcov().replace("BRDA:1,0,0,1", "BRDA:1,0,0,-", 1)
        text = text.replace("BRF:20\nBRH:20", "BRF:19\nBRH:19", 1)
        self.assertEqual(validate_lcov(text, native, self.root), 5)

        # A matched removal from all detail exports still cannot fall below
        # either summary lower bound.
        first["branches"].pop(0)
        native["data"][0]["functions"][0]["branches"].pop(0)
        text = text.replace("BRDA:1,0,0,-\nBRDA:1,0,1,1\n", "", 1)
        with self.assertRaisesRegex(ValueError, "branch population"):
            validate_lcov(text, native, self.root)

    def test_function_projection_is_independent_and_fails_closed(self):
        from rust_coverage import validate_lcov

        for change in (
            "missing",
            "altered",
            "missing-row",
            "index",
            "expanded-index",
            "shape",
            "filenames",
        ):
            native = copy.deepcopy(self.document)
            unit = native["data"][0]
            function = unit["functions"][0]
            if change == "missing":
                del unit["functions"]
            elif change == "altered":
                function["branches"][0][4] += 1
            elif change == "missing-row":
                function["branches"].pop()
            elif change == "index":
                function["branches"][0][6] = 1
            elif change == "expanded-index":
                function["branches"][0][7] = 1
            elif change == "shape":
                function["branches"][0].pop()
            else:
                function["filenames"] = []
            with self.subTest(change=change), self.assertRaises(ValueError):
                validate_lcov(self.lcov(), native, self.root)

        # Unexported dependency filenames are permitted, but their rows and
        # filename indices remain subject to validation.
        native = copy.deepcopy(self.document)
        external = {"filenames": ["/dependency.rs"], "branches": [[1, 1, 1, 2, 0, 1, 0, 0, 4]]}
        native["data"][0]["functions"].append(external)
        self.assertEqual(validate_lcov(self.lcov(), native, self.root), 5)
        external["branches"][0][6] = 2
        with self.assertRaisesRegex(ValueError, "filename index"):
            validate_lcov(self.lcov(), native, self.root)

    def test_invalid_native_branch_details_fail_closed(self):
        from rust_coverage import validate_lcov

        for change in ("version", "missing", "expansions", "shape", "overflow"):
            native = copy.deepcopy(self.document)
            item = native["data"][0]["files"][0]
            if change == "version":
                native["version"] = "other"
            elif change == "missing":
                del item["branches"]
            elif change == "expansions":
                item["expansions"] = [{}]
            elif change == "shape":
                item["branches"][0].pop()
            else:
                item["branches"][0][4] = 0x100000000
            with self.subTest(change=change), self.assertRaises(ValueError):
                validate_lcov(self.lcov(), native, self.root)

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
