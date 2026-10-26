#!/usr/bin/env python3
from pathlib import Path
import subprocess

root = Path(__file__).resolve().parents[1]
for name in ('pre-commit', 'pre-push'):
    (root / '.githooks' / name).chmod(0o755)
subprocess.run(['git', 'config', '--local', 'core.hooksPath', '.githooks'], cwd=root, check=True)
print('Installed project-local branch, check and review guards. Server rules remain authoritative.')
