#!/usr/bin/env python3
"""Install only committed, checksum-pinned free CLI binaries into task-local storage."""

import hashlib
import io
import json
import os
import platform
import tarfile
from pathlib import Path
from urllib.request import urlopen
from zipfile import ZipFile

ROOT = Path(__file__).resolve().parents[1]


def install():
    target = ROOT / ".local/bin"
    target.mkdir(parents=True, exist_ok=True)
    system = f"{platform.system()}-{platform.machine()}"
    for name, tool in json.loads((ROOT / "config/tools.json").read_text()).items():
        asset = tool["assets"][system]
        with urlopen(asset["url"], timeout=60) as response:
            payload = response.read(150_000_000)
        if hashlib.sha256(payload).hexdigest() != asset["sha256"]:
            raise ValueError(f"Checksum mismatch: {name}")
        if asset["url"].endswith(".zip"):
            with ZipFile(io.BytesIO(payload)) as archive:
                matches = [p for p in archive.namelist() if Path(p).name == name]
                if len(matches) != 1:
                    raise ValueError(f"Ambiguous tool archive: {name}")
                content = archive.read(matches[0])
        else:
            with tarfile.open(fileobj=io.BytesIO(payload), mode="r:gz") as archive:
                matches = [
                    p for p in archive.getmembers() if p.isfile() and Path(p.name).name == name
                ]
                if len(matches) != 1:
                    raise ValueError(f"Ambiguous tool archive: {name}")
                stream = archive.extractfile(matches[0])
                if stream is None:
                    raise ValueError(f"Tool executable missing: {name}")
                content = stream.read()
        executable = target / name
        executable.write_bytes(content)
        executable.chmod(0o700)
        print(f"Installed {name} {tool['version']} with verified SHA-256")
    if os.environ.get("GITHUB_PATH"):
        with Path(os.environ["GITHUB_PATH"]).open("a") as output:
            output.write(str(target) + "\n")


if __name__ == "__main__":
    install()
