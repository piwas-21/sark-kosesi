#!/usr/bin/env python3
"""Portable foundation contracts; never a production acceptance substitute."""
from pathlib import Path
from html.parser import HTMLParser
from urllib.parse import unquote, urlsplit
import hashlib
import json
import os
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
REQUIRED = ['AGENTS.md', 'CLAUDE.md', 'ROADMAP.md', 'README.md', 'SECURITY.md',
    'docs/plans/COMMERCE-PLAN.md', 'docs/plans/IMPLEMENTATION-HANDOFF.md',
    'docs/architecture/ADR-001-COMMERCE-FOUNDATION.md', 'docs/engineering/CI-GATES.md',
    'docs/engineering/DEVELOPMENT.md', 'docs/security/BASELINE.md',
    'docs/quality/TEST-STRATEGY.md', 'docs/quality/PERFORMANCE.md',
    'docs/discovery/SEO-AEO-GEO.md', 'docs/runbooks/RELEASE.md',
    'docs/product/LAUNCH-INPUTS.md', 'design/README.md']
APPLICATION_SCRIPTS = ['typecheck', 'lint', 'build', 'test:unit', 'test:integration',
    'test:migrations', 'test:e2e', 'test:accessibility', 'test:discovery', 'test:performance']


def project_files():
    excluded = {'.git', '.local', 'node_modules', '__pycache__', 'test-results', 'playwright-report'}
    for parent, dirs, files in os.walk(ROOT):
        dirs[:] = [d for d in dirs if d not in excluded]
        for name in files:
            yield Path(parent) / name


def local_target(source, target):
    target = target.strip('<>')
    parsed = urlsplit(target)
    if parsed.scheme or parsed.netloc or not parsed.path:
        return
    dest = (source.parent / unquote(parsed.path)).resolve()
    if not dest.is_relative_to(ROOT):
        raise ValueError(f'Nonportable link: {source.relative_to(ROOT)} → {target}')
    if not dest.exists():
        raise ValueError(f'Missing link: {source.relative_to(ROOT)} → {target}')


class Assets(HTMLParser):
    def __init__(self, source):
        super().__init__()
        self.source, self.ids = source, set()

    def handle_starttag(self, tag, attrs):
        values = dict(attrs)
        if 'id' in values:
            if values['id'] in self.ids:
                raise ValueError(f'Duplicate id in {self.source}: {values["id"]}')
            self.ids.add(values['id'])
        for key in ('src', 'href'):
            if key in values:
                local_target(self.source, values[key])


def check_stage(config, package):
    files = list((ROOT / 'apps').rglob('*')) if (ROOT / 'apps').exists() else []
    production = [p for p in files if p.is_file() and p.suffix not in ('.md',)]
    if config['stage'] not in ('foundation', 'application'):
        raise ValueError('Unknown delivery stage')
    if config['stage'] == 'foundation' and production:
        raise ValueError('Production app files require application CI in this PR')
    if config['stage'] == 'application':
        if not production:
            raise ValueError('Application stage has no application files')
        for name in APPLICATION_SCRIPTS:
            command = package.get('scripts', {}).get(name, '')
            if not command or re.search(r'(^echo\b|passWithNoTests|exit 0)', command):
                raise ValueError(f'Missing real application command: {name}')
        if 'Application gate' not in config['requiredChecks']:
            raise ValueError('Application gate must be required in GitHub and CI')
        if 'name: Application gate' not in (ROOT / '.github/workflows/ci.yml').read_text():
            raise ValueError('Application gate job missing')


def main():
    for name in REQUIRED:
        if not (ROOT / name).is_file():
            raise ValueError(f'Missing required document: {name}')
    config = json.loads((ROOT / 'config/delivery.json').read_text())
    rules = json.loads((ROOT / 'config/github-ruleset.json').read_text())
    package = json.loads((ROOT / 'package.json').read_text())
    expected = set(config['requiredChecks'])
    if not expected:
        raise ValueError('Required check inventory cannot be empty')
    actual = {x['context'] for r in rules['rules'] if r['type'] == 'required_status_checks'
              for x in r['parameters']['required_status_checks']}
    if expected != actual:
        raise ValueError('CI check inventory and GitHub rules disagree')
    check_stage(config, package)
    count = 0
    for path in project_files():
        if path.suffix not in ('.md', '.html', '.css', '.js', '.mjs', '.yml', '.json', '.txt'):
            continue
        text = path.read_text()
        if '/Users/' in text or '/home/runner/work/' in text:
            raise ValueError(f'Absolute workstation path in {path.relative_to(ROOT)}')
        if path.suffix == '.md':
            for target in re.findall(r'\]\(([^)\n]+)\)', text):
                local_target(path, target)
        if path.suffix == '.html':
            Assets(path).feed(text)
            if 'name="robots" content="noindex,nofollow"' not in text:
                raise ValueError(f'Prototype must remain noindex: {path}')
        if path.suffix == '.css':
            for target in re.findall(r'url\([\'"]?([^\)\'\"]+)', text):
                local_target(path, target)
        if path.suffix in ('.js', '.mjs'):
            subprocess.run(['node', '--check', str(path)], check=True, capture_output=True)
        if path.parent == ROOT / '.github/workflows':
            for action in re.findall(r'uses:\s*([^\s]+)', text):
                if not re.fullmatch(r'[\w.-]+/[\w./-]+@[0-9a-f]{40}', action):
                    raise ValueError(f'Unpinned action: {action}')
            for container in re.findall(r'image:\s*([^\s]+)', text):
                if not re.fullmatch(r'[^\s]+@sha256:[0-9a-f]{64}', container):
                    raise ValueError(f'Unpinned CI container: {container}')
                if 'mcr.microsoft.com/playwright:' in container:
                    version = package['devDependencies']['@playwright/test']
                    if f'playwright:v{version}-noble@' not in container:
                        raise ValueError('Playwright package and CI browser image versions differ')
            if 'pull_request_target:' in text or 'continue-on-error: true' in text:
                raise ValueError('Unsafe or nonblocking required workflow')
        count += 1
    for asset in json.loads((ROOT / 'design/asset-manifest.json').read_text())['files']:
        path = ROOT / asset['path']
        if not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != asset['sha256']:
            raise ValueError(f'Asset integrity mismatch: {asset["path"]}')
    if os.environ.get('GITHUB_EVENT_NAME') == 'pull_request':
        event = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text())['pull_request']
        base, head = event['base']['ref'], event['head']['ref']
        if base not in ('develop', 'main') or (base == 'main' and head != 'develop'):
            raise ValueError('Feature PRs target develop; only develop releases target main')
    print(f'Foundation contracts passed: {count} source/document files, assets and delivery policy.')

if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f'Foundation check failed: {error}', file=sys.stderr)
        sys.exit(1)
