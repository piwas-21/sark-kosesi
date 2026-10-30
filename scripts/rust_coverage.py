"""Validate native LLVM summaries; LCOV loses macro/async line-count semantics."""

from pathlib import Path


def counter(summary, kind):
    value = summary.get(kind)
    if not isinstance(value, dict):
        raise ValueError(f"Missing Rust {kind} evidence")
    total, covered = value.get("count"), value.get("covered")
    if type(total) is not int or type(covered) is not int or not 0 <= covered <= total:
        raise ValueError("Invalid Rust coverage counts")
    return total, covered


def validate(document, root, branches=True):
    if document.get("type") != "llvm.coverage.json.export" or not document.get("data"):
        raise ValueError("Missing native LLVM coverage")
    files = {}
    for unit in document["data"]:
        for item in unit.get("files", []):
            path = Path(item["filename"]).resolve()
            if not path.is_relative_to(root):
                raise ValueError("Rust coverage outside checkout")
            if path in files:
                raise ValueError("Duplicate Rust coverage source")
            for kind in ("lines", "regions", "branches"):
                counter(item["summary"], kind)
            files[path] = item["summary"]
    critical = [root / "crates/domain/src/lib.rs", root / "crates/api/src/lib.rs"]
    critical += [
        path
        for pattern in (
            "crates/*/src/inventory.rs",
            "crates/*/src/quotes/*.rs",
            "crates/*/src/stock.rs",
            "crates/*/src/holds.rs",
            "crates/*/src/holds/*.rs",
            "crates/*/src/staff_access.rs",
            "crates/*/src/staff_operations.rs",
            "crates/*/src/catalogue_prices.rs",
            "crates/*/src/catalogue_authoring.rs",
            "crates/*/src/catalogue_authoring/*.rs",
            "crates/*/src/specifications.rs",
            "crates/*/src/publication.rs",
            "crates/*/src/publication/*.rs",
            "crates/*/src/published_catalogue/read.rs",
            "crates/*/src/published_catalogue/image.rs",
            "crates/*/src/published_catalogue/stock.rs",
            "crates/api/src/published.rs",
            "crates/api/src/published_types.rs",
            "crates/*/src/image_review.rs",
            "crates/*/src/image_review/*.rs",
            "crates/*/src/factual_review.rs",
            "crates/*/src/factual_review/*.rs",
            "crates/*/src/translations.rs",
            "crates/*/src/translations/*.rs",
            "crates/*/src/media.rs",
            "crates/*/src/media/*.rs",
            "crates/*/src/image_ingestion/*.rs",
        )
        for path in root.glob(pattern)
    ]
    for path in critical:
        summary = files.get(path)
        if not summary:
            raise ValueError("Critical Rust source unmeasured")
        for kind in ("lines", "branches") if branches else ("lines",):
            total, covered = counter(summary, kind)
            if (
                kind == "branches"
                and total == 0
                and path.relative_to(root).as_posix() in ("crates/infrastructure/src/holds/mod.rs",)
            ):
                total, covered = counter(summary, "regions")
            if total <= 0 or covered * 100 < total * 95:
                raise ValueError(
                    f"Critical Rust money/inventory/authorization coverage below 95%: {path.name}"
                )
    results = []
    for name in ("domain", "application", "infrastructure", "api", "worker"):
        sources = set((root / f"crates/{name}/src").rglob("*.rs"))
        if not sources or not sources <= files.keys():
            raise ValueError(f"Unmeasured Rust package: {name}")
        totals = {}
        for kind in ("lines", "regions", "branches"):
            values = [counter(files[path], kind) for path in sources]
            totals[kind] = (sum(v[0] for v in values), sum(v[1] for v in values))
        for kind in ("lines", "branches") if branches else ("lines",):
            total, covered = totals[kind]
            minimum = 95 if name == "domain" else 85 if kind == "lines" else 80
            if kind == "branches" and total == 0:
                # LLVM currently does not branch-instrument match/Result. Require
                # region coverage as well, record zero explicitly, never claim 100%.
                total, covered = totals["regions"]
                minimum = 95
            if total <= 0 or covered * 100 < total * minimum:
                raise ValueError(f"Rust {name} {kind} evidence below {minimum}%")
        results.append({"package": name, "counts": totals})
    return results


def validate_lcov(text, document, root):
    """Check the Sonar import against native counters, preserving LLVM semantics."""
    native = {
        Path(item["filename"]).resolve(): item["summary"]
        for unit in document.get("data", [])
        for item in unit.get("files", [])
    }
    records, current = {}, None
    for line in text.splitlines():
        if line.startswith("SF:"):
            path = (root / line[3:]).resolve()
            if current is not None or path in records or path not in native or not path.is_file():
                raise ValueError("Unrelated, duplicate or incomplete Rust LCOV source")
            current = {"path": path, "lines": {}, "branches": {}, "totals": {}}
        elif line == "end_of_record":
            if current is None:
                raise ValueError("Rust LCOV record has no source")
            records[current["path"]] = current
            current = None
        elif line.startswith(("DA:", "BRDA:", "LF:", "LH:", "BRF:", "BRH:")):
            if current is None:
                raise ValueError("Rust LCOV data has no source")
            key, value = line.split(":", 1)
            parts = value.split(",")
            if key == "DA":
                if len(parts) not in (2, 3):
                    raise ValueError("Invalid Rust LCOV line")
                number, hits = int(parts[0]), int(parts[1])
                if number in current["lines"] or hits < 0:
                    raise ValueError("Duplicate or invalid Rust LCOV line")
                current["lines"][number] = hits
            elif key == "BRDA":
                if len(parts) != 4:
                    raise ValueError("Invalid Rust LCOV branch")
                identity = tuple(int(part) for part in parts[:3])
                hits = 0 if parts[3] == "-" else int(parts[3])
                if (
                    identity in current["branches"]
                    or hits < 0
                    or any(value < 0 for value in identity)
                ):
                    raise ValueError("Duplicate or invalid Rust LCOV branch")
                current["branches"][identity] = hits
            else:
                if key in current["totals"] or not value.isdecimal():
                    raise ValueError("Duplicate or invalid Rust LCOV total")
                current["totals"][key] = int(value)
    if current is not None or not records:
        raise ValueError("Missing or incomplete Rust LCOV import")
    sources = {
        path
        for name in ("domain", "application", "infrastructure", "api", "worker")
        for path in (root / f"crates/{name}/src").rglob("*.rs")
    }
    if not sources or not sources <= records.keys():
        raise ValueError("Rust LCOV import omits maintained source")
    for path, record in records.items():
        summary = native[path]
        lines, covered_lines = counter(summary, "lines")
        branches, covered_branches = counter(summary, "branches")
        expected = {"LF": lines, "LH": covered_lines, "BRF": branches, "BRH": covered_branches}
        if record["totals"] != expected:
            raise ValueError("Rust LCOV/native coverage summaries disagree")
        maximum = len(path.read_text().splitlines())
        locations = [*record["lines"], *(key[0] for key in record["branches"])]
        if not record["lines"] or any(number < 1 or number > maximum for number in locations):
            raise ValueError("Rust LCOV line records missing or outside source")
        if (
            len(record["lines"]) > lines
            or sum(value > 0 for value in record["lines"].values()) > covered_lines
        ):
            raise ValueError("Rust LCOV line population exceeds native summaries")
        if (
            len(record["branches"]) != branches
            or sum(value > 0 for value in record["branches"].values()) != covered_branches
        ):
            raise ValueError("Rust LCOV branch population differs from native evidence")
    return len(sources)
