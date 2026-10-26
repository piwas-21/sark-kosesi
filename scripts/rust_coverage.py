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
    for name in ("crates/domain/src/lib.rs", "crates/api/src/lib.rs"):
        summary = files.get(root / name)
        if not summary:
            raise ValueError("Critical Rust source unmeasured")
        for kind in ("lines", "branches") if branches else ("lines",):
            total, covered = counter(summary, kind)
            if total <= 0 or covered * 100 < total * 95:
                raise ValueError("Critical Rust money/authorization coverage below 95%")
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
