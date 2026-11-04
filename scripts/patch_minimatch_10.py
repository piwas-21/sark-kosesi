#!/usr/bin/env python3
"""Apply the upstream minimatch 10.2.6 backport to an owned install.

The lint launcher applies this before loading ESLint. npm audit remains unchanged.
The edits port isaacs/minimatch commit 77a0f5c4b80bdc641c3c930a1d9edf5ad2db57f8
to the published 10.2.6 JavaScript distributions. A partial write fails closed on
the next invocation; reinstall the original package rather than repairing it here.
"""

import argparse
import hashlib
import os
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PACKAGE_HASH = "bb847ba240909c9a36d4eaeb4e0efdc3e31182aeef208c1611259a48f282ea75"
FILES = {
    "dist/commonjs/index.js": (
        "d224f71f20d7c206fcc5b7e9f747324c7698345a0b2c28d717dd9cf698306783",
        "7e835c0a30f7b4f29f4ad8cf8c3e92b97ab23ed854a7a777f240d6bdf3d19231",
    ),
    "dist/esm/index.js": (
        "59821ec2ce0d3df968292036e5c831ad322b33e9c33a949ecbc9dfe125d165be",
        "37ba4ff4010cda499e890975983b78ebd5f2be2dd8506d938472de4af8e9ed25",
    ),
}
REPLACEMENTS = (
    (
        b"    firstPhasePreProcess(globParts) {\n        let didSomething = false;",
        b"    firstPhasePreProcess(globParts) {\n"
        b"        const { braceExpandMax = 100_000 } = this.options;\n"
        b"        let didSomething = false;",
    ),
    (
        b"                        p2 === '..') {",
        b"                        p2 === '..' ||\n"
        b"                        globParts.length >= braceExpandMax) {",
    ),
    (
        b"    secondPhasePreProcess(globParts) {\n        for (let i = 0;",
        b"    secondPhasePreProcess(globParts) {\n"
        b"        if (globParts.length > 1000) return globParts;\n"
        b"        for (let i = 0;",
    ),
)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def regular_file(root, relative):
    path = root
    for part in Path(relative).parts:
        path = path / part
        if path.is_symlink():
            raise ValueError(f"Symlink is not an owned package file: {relative}")
    if not path.is_file():
        raise ValueError(f"Missing package file: {relative}")
    return path


def read_installation(project):
    if project.is_symlink() or not project.is_dir():
        raise ValueError("Project directory must exist and not be a symlink")
    root = project.resolve()
    metadata = regular_file(root, "node_modules/minimatch/package.json")
    if digest(metadata.read_bytes()) != PACKAGE_HASH:
        raise ValueError("Unexpected minimatch package metadata")
    found = {}
    for relative, hashes in FILES.items():
        path = regular_file(root, f"node_modules/minimatch/{relative}")
        data = path.read_bytes()
        current = digest(data)
        if current not in hashes:
            raise ValueError(f"Unknown minimatch source: {relative}")
        found[relative] = (path, data, hashes.index(current))
    states = {value[2] for value in found.values()}
    if len(states) != 1:
        raise ValueError("Partial patch detected; reinstall the original package")
    return found, states.pop()


def patched_files(found):
    replacements = {}
    for relative, (path, data, _) in found.items():
        for old, new in REPLACEMENTS:
            if data.count(old) != 1:
                raise ValueError(f"Unexpected patch context: {relative}")
            data = data.replace(old, new)
        if digest(data) != FILES[relative][1]:
            raise ValueError(f"Patched digest mismatch: {relative}")
        replacements[path] = data
    return replacements


def install_files(replacements):
    staged = []
    try:
        for path, data in replacements.items():
            with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as file:
                staged.append((Path(file.name), path))
                file.write(data)
            os.chmod(staged[-1][0], path.stat().st_mode & 0o777)
        for temporary, path in staged:
            os.replace(temporary, path)
    finally:
        for temporary, _ in staged:
            temporary.unlink(missing_ok=True)


def check(project, apply=False):
    found, state = read_installation(project)
    if not apply:
        if state != 1:
            raise ValueError("Minimatch backport is not installed")
        return
    if state == 1:
        return
    install_files(patched_files(found))
    check(project)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", type=Path, default=ROOT)
    parser.add_argument("--apply", action="store_true", help="Apply; default only verifies")
    args = parser.parse_args()
    try:
        check(args.project, args.apply)
    except (OSError, ValueError) as error:
        parser.exit(1, f"Minimatch backport rejected: {error}\n")
    print("Minimatch 10.2.6 backport verified; npm audit remains unchanged")


if __name__ == "__main__":
    main()
