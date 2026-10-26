#!/usr/bin/env python3
"""Application source structure and dependency inventory; no runtime acceptance claims."""

import json
import re
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ALLOWED = {
    "MIT",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "0BSD",
    "CC0-1.0",
    "CC-BY-4.0",
    "OFL-1.1",
    "MPL-2.0",
    "BlueOak-1.0.0",
    "Python-2.0",
    "Unlicense",
    "MIT-0",
    "LGPL-3.0-or-later",
}


def main():
    for manifest in [
        ROOT / "package.json",
        *(ROOT / "apps").glob("*/package.json"),
        *(ROOT / "packages").glob("*/package.json"),
    ]:
        package = json.loads(manifest.read_text())
        for section in ("dependencies", "devDependencies"):
            for name, version in package.get(section, {}).items():
                if not re.fullmatch(r"\d+\.\d+\.\d+(?:-[\w.-]+)?", version):
                    raise ValueError(f"Direct npm dependency must be exact: {name}")
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["dependencies"]
    for name, dependency in workspace.items():
        version = dependency if isinstance(dependency, str) else dependency.get("version", "")
        if not re.fullmatch(r"=\d+\.\d+\.\d+(?:-[\w.-]+)?", version):
            raise ValueError(f"Direct Cargo dependency must be exact: {name}")
    lock = json.loads((ROOT / "package-lock.json").read_text())
    inventory = []
    for name, package in lock["packages"].items():
        if not name or package.get("link") or name.startswith(("apps/", "packages/")):
            continue
        licence = package.get("license", "")
        identifiers = set(re.findall(r"[A-Za-z0-9.-]+", licence)) - {"OR", "AND"}
        if not identifiers or not identifiers <= ALLOWED:
            raise ValueError(f"Unreviewed npm licence: {name}: {licence}")
        inventory.append({"package": name, "version": package["version"], "licence": licence})
    for parent in ("apps/web/src", "packages/design/src", "crates"):
        for path in (ROOT / parent).rglob("*"):
            if (
                not path.is_file()
                or path.suffix not in (".rs", ".ts", ".tsx", ".css")
                or "tests" in path.parts
            ):
                continue
            text = path.read_text()
            maximum = 300 if path.suffix == ".rs" else 200 if path.name == "page.tsx" else 250
            if len(text.splitlines()) > maximum:
                raise ValueError(
                    f"Source responsibility exceeds {maximum} lines: {path.relative_to(ROOT)}"
                )
            if (
                path.suffix == ".css"
                and path.name != "tokens.css"
                and re.search(r"#[0-9a-fA-F]{3,8}\b|\b(?:rgb|hsl)a?\(", text)
            ):
                raise ValueError(
                    f"Component colour outside semantic tokens: {path.relative_to(ROOT)}"
                )
    if not inventory:
        raise ValueError("Dependency inventory cannot be empty")
    destination = ROOT / ".local/evidence/npm-licences.json"
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(json.dumps(inventory, indent=2) + "\n")
    print(f"Application source bounds and {len(inventory)} dependency licences checked")


if __name__ == "__main__":
    main()
