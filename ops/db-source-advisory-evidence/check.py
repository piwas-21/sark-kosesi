#!/usr/bin/env python3
"""Capture a bounded PostgreSQL project advisory assessment for a source manifest."""

import argparse
import hashlib
import json
import re
import sys
from datetime import UTC, datetime
from html.parser import HTMLParser
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import HTTPRedirectHandler, Request, build_opener

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "db-sibling-scan-evidence"))
from source_manifest import ARCHIVE, PATHS  # noqa: E402

URL = "https://www.postgresql.org/support/security/18/"
MAX_PAGE = 2 * 1024 * 1024
MAX_MANIFEST = 64 * 1024
HEADERS = ("Reference", "Affected", "Fixed", "Component & CVSS v3 Base Score", "Description")
CVE = re.compile(r"CVE-\d{4}-\d{4,}")
IMAGE = re.compile(r"sha256:[0-9a-f]{64}")
HASH = re.compile(r"[0-9a-f]{64}")


def unique_pairs(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def validate_manifest(raw, image_id):
    if len(raw) > MAX_MANIFEST or not IMAGE.fullmatch(image_id):
        raise ValueError("invalid manifest size or expected image identity")
    value = json.loads(raw, object_pairs_hook=unique_pairs)
    if not isinstance(value, dict):
        raise ValueError("manifest is not an object")
    files = value.get("file_sha256")
    if (
        value.get("postgresql_version") != "18.7"
        or value.get("source_archive_sha256") != ARCHIVE
        or value.get("source_archive")
        != "https://ftp.postgresql.org/pub/source/v18.7/postgresql-18.7.tar.bz2"
        or value.get("composed_image_id") != image_id
        or not IMAGE.fullmatch(str(value.get("source_build_image_id", "")))
        or not re.fullmatch(r"[0-9a-f]{40}", str(value.get("source_revision", "")))
        or not isinstance(files, dict)
        or set(files) != set(PATHS)
        or any(not isinstance(v, str) or not HASH.fullmatch(v) for v in files.values())
    ):
        raise ValueError("source manifest identity or file hashes do not match")
    return value


class TableParser(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.depth = 0
        self.tables = []
        self.rows = None
        self.row = None
        self.cell = None
        self.references = set()
        self.malformed = False
        self.heading = None
        self.headings = []

    def handle_starttag(self, tag, attrs):
        if tag == "h3":
            if self.heading is not None:
                self.malformed = True
            self.heading = []
        if tag == "a":
            href = dict(attrs).get("href", "")
            match = re.fullmatch(r"/support/security/(CVE-\d{4}-\d{4,})/", href)
            if match:
                self.references.add(match.group(1))
        if tag == "table":
            self.depth += 1
            if self.depth == 1:
                self.rows = []
            else:
                self.malformed = True
        elif self.depth == 1 and tag == "tr":
            if self.row is not None:
                self.malformed = True
            self.row = []
        elif self.depth == 1 and tag in ("td", "th"):
            if self.row is None or self.cell is not None:
                self.malformed = True
            self.cell = [tag, []]
        elif tag == "br" and self.cell is not None:
            self.cell[1].append(" ")

    def handle_data(self, data):
        if self.heading is not None:
            self.heading.append(data)
        if self.cell is not None:
            self.cell[1].append(data)

    def handle_endtag(self, tag):
        if tag == "h3":
            if self.heading is None:
                self.malformed = True
            else:
                self.headings.append(" ".join("".join(self.heading).split()))
                self.heading = None
        if self.depth == 1 and tag in ("td", "th"):
            if self.cell is None or self.cell[0] != tag or self.row is None:
                self.malformed = True
            else:
                self.row.append((tag, " ".join("".join(self.cell[1]).split())))
            self.cell = None
        elif self.depth == 1 and tag == "tr":
            if self.row is None or self.cell is not None:
                self.malformed = True
            else:
                self.rows.append(self.row)
            self.row = None
        elif tag == "table":
            if self.depth == 1:
                if self.row is not None or self.cell is not None:
                    self.malformed = True
                self.tables.append(self.rows)
                self.rows = None
            if self.depth == 0:
                self.malformed = True
            else:
                self.depth -= 1


def parse_page(raw):
    if not raw or len(raw) > MAX_PAGE:
        raise ValueError("empty or oversized advisory page")
    parser = TableParser()
    parser.feed(raw.decode("utf-8", errors="strict"))
    parser.close()
    if parser.malformed or parser.depth or parser.heading is not None:
        raise ValueError("malformed advisory HTML table")
    if parser.headings.count("Known PostgreSQL Security Vulnerabilities in PostgreSQL 18") != 1:
        raise ValueError("version-specific advisory heading missing or duplicated")
    matches = [
        table for table in parser.tables if table and tuple(v for _, v in table[0]) == HEADERS
    ]
    if len(matches) != 1 or any(tag != "th" for tag, _ in matches[0][0]):
        raise ValueError("advisory table headers missing, changed or duplicated")
    records = []
    seen = set()
    for row in matches[0][1:]:
        if len(row) != 5 or any(tag != "td" for tag, _ in row):
            raise ValueError("unexpected advisory table row")
        reference, affected, fixed, component, description = (value for _, value in row)
        ids = CVE.findall(reference)
        if len(ids) != 1 or ids[0] in seen or not component or not description:
            raise ValueError("invalid or duplicate advisory row")
        majors = affected.split(", ")
        versions = fixed.split(", ")
        if (
            any(not re.fullmatch(r"[1-9][0-9]*", v) for v in majors)
            or len(set(majors)) != len(majors)
            or "18" not in majors
            or any(not re.fullmatch(r"[1-9][0-9]*\.[0-9]+", v) for v in versions)
            or len({v.split(".")[0] for v in versions}) != len(versions)
            or set(majors) != {v.split(".")[0] for v in versions}
        ):
            raise ValueError(f"unsupported affected or fixed versions: {ids[0]}")
        fixed_18 = next(v for v in versions if v.startswith("18."))
        records.append(
            {
                "cve": ids[0],
                "fixed_in_18": fixed_18,
                "component": component,
                "status": "review_required"
                if int(fixed_18.split(".")[1]) > 7
                else "fixed_in_source",
            }
        )
        seen.add(ids[0])
    if not records or seen != parser.references:
        raise ValueError("empty or incomplete advisory table")
    return records


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ValueError("advisory redirects are not accepted")


def fetch():
    request = Request(
        URL,
        headers={
            "Accept": "text/html",
            "Accept-Encoding": "identity",
            "User-Agent": "Piwas-source-advisory-evidence/1",
        },
    )
    with build_opener(NoRedirect()).open(request, timeout=20) as response:
        if (
            response.status != 200
            or response.geturl() != URL
            or response.headers.get_content_type() != "text/html"
        ):
            raise ValueError("unexpected advisory response")
        length = response.headers.get("Content-Length")
        if length is not None and (not length.isdigit() or int(length) > MAX_PAGE):
            raise ValueError("invalid advisory content length")
        raw = response.read(MAX_PAGE + 1)
        if len(raw) > MAX_PAGE or (length is not None and len(raw) != int(length)):
            raise ValueError("truncated or oversized advisory response")
        return raw


def main():
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("--manifest", type=Path, required=True)
    cli.add_argument("--image-id", required=True)
    cli.add_argument("--output-dir", type=Path, required=True, help="new directory for evidence")
    args = cli.parse_args()
    try:
        with args.manifest.open("rb") as source:
            manifest_raw = source.read(MAX_MANIFEST + 1)
        manifest = validate_manifest(manifest_raw, args.image_id)
        raw = fetch()
        captured_at = datetime.now(UTC).isoformat(timespec="seconds")
        records = parse_page(raw)
        status = (
            "review_required"
            if any(r["status"] == "review_required" for r in records)
            else "no_listed_affected_rows"
        )
        evidence = {
            "schema_version": 1,
            "status": status,
            "retrieved_at_utc": captured_at,
            "advisory_url": URL,
            "advisory_sha256": hashlib.sha256(raw).hexdigest(),
            "manifest_sha256": hashlib.sha256(manifest_raw).hexdigest(),
            "source_revision": manifest["source_revision"],
            "composed_image_id": args.image_id,
            "postgresql_version": manifest["postgresql_version"],
            "records": records,
            "scope": "Published PostgreSQL project version-18 table only; source manifest is same-run evidence, not independent attestation. Does not assess other dependencies, undisclosed issues, deployment, or release readiness.",
        }
        args.output_dir.mkdir(parents=False, exist_ok=False)
        (args.output_dir / "advisory.html").write_bytes(raw)
        (args.output_dir / "assessment.json").write_text(
            json.dumps(evidence, indent=2, sort_keys=True) + "\n"
        )
        print(f"{status}: {len(records)} advisory rows; evidence in {args.output_dir}")
        return 0 if status == "no_listed_affected_rows" else 1
    except (ValueError, OSError, UnicodeError, HTTPError, URLError) as error:
        print(f"source advisory evidence failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
