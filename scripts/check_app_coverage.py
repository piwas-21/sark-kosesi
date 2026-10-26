#!/usr/bin/env python3
"""Gate actual LCOV records against owned source, including branch evidence."""

import argparse
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE_SUFFIXES = {".rs", ".ts", ".tsx", ".js", ".jsx"}


def owned_path(value, root):
    path = (root / value).resolve()
    if not path.is_relative_to(root) or not path.is_file():
        raise ValueError(f"Coverage source missing or outside repository: {value}")
    return path


def number(value):
    if not re.fullmatch(r"\d+", value):
        raise ValueError("Coverage counts must be nonnegative integers")
    return int(value)


def read_lcov(text, root):
    records, current = {}, None
    for raw in text.splitlines():
        if raw.startswith("SF:"):
            if current is not None:
                raise ValueError("Unterminated coverage record")
            path = owned_path(raw[3:], root)
            if path in records:
                raise ValueError("Duplicate coverage source record")
            current = {"path": path, "lines": {}, "branches": {}, "totals": {}}
        elif raw == "end_of_record":
            if current is None:
                raise ValueError("Coverage record has no source")
            validate_record(current)
            records[current["path"]] = current
            current = None
        elif raw.startswith(("DA:", "BRDA:", "LF:", "LH:", "BRF:", "BRH:")):
            if current is None:
                raise ValueError("Coverage data has no source")
            key, value = raw.split(":", 1)
            if key == "DA":
                parts = value.split(",")
                if len(parts) not in (2, 3):
                    raise ValueError("Malformed line coverage")
                line, hits = number(parts[0]), number(parts[1])
                if line in current["lines"]:
                    raise ValueError("Duplicate covered line")
                current["lines"][line] = hits
            elif key == "BRDA":
                parts = value.split(",")
                if len(parts) != 4:
                    raise ValueError("Malformed branch coverage")
                line, block, branch = map(number, parts[:3])
                identity = (line, block, branch)
                if identity in current["branches"]:
                    raise ValueError("Duplicate covered branch")
                current["branches"][identity] = 0 if parts[3] == "-" else number(parts[3])
            else:
                if key in current["totals"]:
                    raise ValueError("Duplicate coverage total")
                current["totals"][key] = number(value)
    if current is not None or not records:
        raise ValueError("Missing or incomplete coverage records")
    return records


def validate_record(record):
    lines, branches = record["lines"], record["branches"]
    source_lines = len(record["path"].read_text().splitlines())
    locations = [*lines, *(key[0] for key in branches)]
    if not lines or any(line < 1 or line > source_lines for line in locations):
        raise ValueError("Empty coverage or location outside source")
    expected = {
        "LF": len(lines),
        "LH": sum(hits > 0 for hits in lines.values()),
        "BRF": len(branches),
        "BRH": sum(hits > 0 for hits in branches.values()),
    }
    if record["totals"] != expected:
        raise ValueError("Missing or inconsistent coverage totals")


def assert_threshold(records, line_minimum, branch_minimum):
    lines = [hits for record in records for hits in record["lines"].values()]
    branches = [hits for record in records for hits in record["branches"].values()]
    if not lines or not branches:
        raise ValueError("Line and branch evidence are both required")
    for label, hits, minimum in (
        ("line", lines, line_minimum),
        ("branch", branches, branch_minimum),
    ):
        covered = sum(count > 0 for count in hits)
        if covered * 100 < minimum * len(hits):
            raise ValueError(f"{label} coverage below {minimum}%: {covered}/{len(hits)}")


def check(report, package, critical, root=ROOT):
    root = root.resolve()
    package = (root / package).resolve()
    if not package.is_relative_to(root) or not package.is_dir():
        raise ValueError("Coverage package missing or outside repository")
    sources = {
        path.resolve()
        for path in package.rglob("*")
        if path.is_file() and path.suffix in SOURCE_SUFFIXES
    }
    if not sources:
        raise ValueError("Coverage package has no maintained source")
    records = read_lcov(report.read_text(), root)
    missing = sources - records.keys()
    if missing:
        raise ValueError(f"Unmeasured maintained source: {sorted(str(p) for p in missing)}")
    assert_threshold([records[path] for path in sources], 85, 80)
    for value in critical:
        path = owned_path(value, root)
        if path not in sources:
            raise ValueError("Critical coverage source is outside the measured package")
        assert_threshold([records[path]], 95, 95)
    return len(sources)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--package", required=True, help="Owned source directory, not build output")
    parser.add_argument("--critical", action="append", default=[])
    args = parser.parse_args()
    try:
        count = check(args.report, args.package, args.critical)
    except (OSError, ValueError) as error:
        parser.exit(1, f"Application coverage rejected: {error}\n")
    print(f"Coverage accepted for {count} maintained sources; line and branch minima met.")


if __name__ == "__main__":
    main()
