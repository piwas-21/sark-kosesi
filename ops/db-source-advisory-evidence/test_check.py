import copy
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "advisory_check", Path(__file__).with_name("check.py")
)
check = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(check)

IMAGE = "sha256:" + "a" * 64
REVISION = "c" * 40


def manifest():
    return {
        "postgresql_version": "18.7",
        "source_archive_sha256": check.ARCHIVE,
        "source_archive": "https://ftp.postgresql.org/pub/source/v18.7/postgresql-18.7.tar.bz2",
        "composed_image_id": IMAGE,
        "source_build_image_id": "sha256:" + "b" * 64,
        "source_revision": REVISION,
        "file_sha256": {path: "d" * 64 for path in check.PATHS},
    }


def row(cve="CVE-2026-12345", affected="18, 17", fixed="18.7, 17.12"):
    return f"""<tr><td><a href="/support/security/{cve}/">{cve}</a><br>Announcement</td>
    <td>{affected}</td><td>{fixed}</td><td>core server<br>8.8</td>
    <td>description <a href="/support/security/{cve}/">more details</a></td></tr>"""


def page(*rows):
    headers = "".join(f"<th>{v}</th>" for v in check.HEADERS)
    return f"<html><h3>Known PostgreSQL Security Vulnerabilities in PostgreSQL 18</h3><table><tr>{headers}</tr>{''.join(rows)}</table></html>".encode()


class SourceAdvisoryTest(unittest.TestCase):
    def test_same_and_earlier_fixed_versions_are_not_affected(self):
        records = check.parse_page(page(row(), row("CVE-2026-12346", fixed="18.4, 17.10")))
        self.assertEqual([r["status"] for r in records], ["fixed_in_source", "fixed_in_source"])
        self.assertEqual([r["fixed_in_18"] for r in records], ["18.7", "18.4"])

    def test_later_minor_version_requires_review_numerically(self):
        records = check.parse_page(page(row(fixed="18.10, 17.12")))
        self.assertEqual(records[0]["status"], "review_required")

    def test_missing_and_unsupported_versions_fail_closed(self):
        for affected, fixed in [
            ("18, 17", "17.12"),
            ("18, 17", "18.7, unknown"),
            ("17", "17.12"),
            ("18, 17", "18.7, 18.8"),
            ("18, 17", "18.7, 17.12, 16.9"),
            ("18-17", "18.7"),
        ]:
            with self.subTest(affected=affected, fixed=fixed), self.assertRaises(ValueError):
                check.parse_page(page(row(affected=affected, fixed=fixed)))

    def test_table_drift_duplicates_truncation_and_extra_cve_fail(self):
        valid = page(row())
        cases = [
            page(),
            valid.replace(b"Affected", b"Changed"),
            page(row(), row()),
            valid.replace(b"</table>", b""),
            valid.replace(
                b"</html>", b'<a href="/support/security/CVE-2026-99999/">extra</a></html>'
            ),
            valid.replace(b"<td>core server", b"<td>extra</td><td>core server"),
            b"x" * (check.MAX_PAGE + 1),
            b"\xff",
        ]
        for content in cases:
            with self.subTest(content=content[:80]), self.assertRaises((ValueError, UnicodeError)):
                check.parse_page(content)

    def test_cli_writes_replayable_evidence_and_fails_for_later_fix(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "source.json"
            source.write_text(json.dumps(manifest()))
            for number, fixed, expected in [(1, "18.7, 17.12", 0), (2, "18.8, 17.12", 1)]:
                output = root / str(number)
                raw = page(row(fixed=fixed))
                argv = [
                    "check.py",
                    "--manifest",
                    str(source),
                    "--image-id",
                    IMAGE,
                    "--revision",
                    REVISION,
                    "--output-dir",
                    str(output),
                ]
                with patch.object(check, "fetch", return_value=raw), patch("sys.argv", argv):
                    self.assertEqual(check.main(), expected)
                evidence = json.loads((output / "assessment.json").read_text())
                self.assertEqual((output / "advisory.html").read_bytes(), raw)
                self.assertEqual(evidence["composed_image_id"], IMAGE)
                self.assertEqual(evidence["advisory_sha256"], check.hashlib.sha256(raw).hexdigest())
                self.assertEqual(
                    evidence["manifest_sha256"],
                    check.hashlib.sha256(source.read_bytes()).hexdigest(),
                )
                self.assertEqual(evidence["records"], check.parse_page(raw))
                self.assertEqual(
                    evidence["status"],
                    "no_listed_affected_rows" if expected == 0 else "review_required",
                )

    def test_cli_failure_does_not_publish_evidence(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "source.json"
            source.write_text(json.dumps(manifest()))
            output = root / "evidence"
            argv = [
                "check.py",
                "--manifest",
                str(source),
                "--image-id",
                IMAGE,
                "--revision",
                REVISION,
                "--output-dir",
                str(output),
            ]
            with (
                patch.object(check, "fetch", return_value=page(row(fixed="unknown"))),
                patch("sys.argv", argv),
            ):
                self.assertEqual(check.main(), 1)
            self.assertFalse(output.exists())

    def test_publication_collision_and_second_write_failure_leave_no_evidence(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            output = root / "evidence"
            output.mkdir()
            with self.assertRaises(OSError):
                check.publish_evidence(output, b"raw", {"status": "success"})
            self.assertEqual(list(output.iterdir()), [])
            self.assertEqual(list(root.iterdir()), [output])
            output.rmdir()
            with patch.object(Path, "write_text", side_effect=OSError("second write failed")):
                with self.assertRaises(OSError):
                    check.publish_evidence(output, b"raw", {"status": "success"})
            self.assertFalse(output.exists())
            self.assertEqual(list(root.iterdir()), [])

    def test_manifest_binding_rejects_wrong_image_and_identity(self):
        valid = manifest()
        raw = json.dumps(valid).encode()
        self.assertEqual(check.validate_manifest(raw, IMAGE, REVISION), valid)
        for key, bad in [
            ("postgresql_version", "18.6"),
            ("source_archive_sha256", "0" * 64),
            ("source_revision", "bad"),
            ("source_build_image_id", "bad"),
            ("composed_image_id", "sha256:" + "e" * 64),
            ("file_sha256", {}),
        ]:
            altered = copy.deepcopy(valid)
            altered[key] = bad
            with self.subTest(key=key), self.assertRaises(ValueError):
                check.validate_manifest(json.dumps(altered).encode(), IMAGE, REVISION)
        with self.assertRaises(ValueError):
            check.validate_manifest(raw, "sha256:" + "e" * 64, REVISION)
        with self.assertRaises(ValueError):
            check.validate_manifest(raw, IMAGE, "d" * 40)
        with self.assertRaises(ValueError):
            check.validate_manifest(raw, IMAGE, "invalid")
        numeric = manifest()
        numeric["source_revision"] = int("1" * 40)
        with self.assertRaises(ValueError):
            check.validate_manifest(json.dumps(numeric).encode(), IMAGE, "1" * 40)
        with self.assertRaises(ValueError):
            check.validate_manifest(
                b'{"postgresql_version":"18.7","postgresql_version":"18.7"}', IMAGE, REVISION
            )


if __name__ == "__main__":
    unittest.main()
