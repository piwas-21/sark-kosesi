#!/usr/bin/env python3
from pathlib import Path
import json
import subprocess
import sys
from audit_github import ROOT, audit
from gate_policy import validate_release

if __name__ == '__main__':
    try:
        config = json.loads((ROOT / 'config/delivery.json').read_text())
        head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
        path = ROOT / '.local/releases' / head / 'evidence.json'
        evidence = json.loads(path.read_text()) if path.exists() else {}
        validate_release(config, evidence, head, ROOT)
        audit()
        print(f'Release evidence and server protection present for {head}; review evidence content before activation.')
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f'Release blocked: {error}', file=sys.stderr)
        sys.exit(1)
