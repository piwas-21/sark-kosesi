#!/usr/bin/env python3
"""Record an actual independent review, after findings are resolved."""
from pathlib import Path
import argparse
import hashlib
import json
import subprocess
from datetime import datetime, timezone

ROOT = Path(__file__).resolve().parents[1]

def git(*args):
    return subprocess.check_output(['git', *args], cwd=ROOT, text=True).strip()

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--reviewer', required=True)
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--base', default='origin/develop')
    args = parser.parse_args()
    if git('status', '--porcelain'):
        raise SystemExit('Commit the reviewed source first; dirty source cannot receive a receipt.')
    report = args.report.resolve()
    if not report.is_relative_to(ROOT) or not report.is_file() or report.stat().st_size < 40:
        raise SystemExit('A substantive independent review report inside .local is required.')
    head, base = git('rev-parse', 'HEAD'), git('rev-parse', args.base)
    receipt = {'head': head, 'base': base, 'reviewer': args.reviewer, 'verdict': 'approved',
        'report': report.relative_to(ROOT).as_posix(), 'reportSha256': hashlib.sha256(report.read_bytes()).hexdigest(),
        'recordedAt': datetime.now(timezone.utc).isoformat()}
    target = ROOT / '.local/reviews' / f'{head}.json'
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(json.dumps(receipt, indent=2) + '\n')
    print(f'Recorded independent review for {head}; do not use this command to self-approve.')
