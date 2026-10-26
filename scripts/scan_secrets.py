#!/usr/bin/env python3
"""Run the pinned upstream Gitleaks CLI, verifying the release archive hash."""
from pathlib import Path
import hashlib
import io
import platform
import subprocess
import tarfile

ROOT = Path(__file__).resolve().parents[1]
VERSION = '8.30.1'
ARCHIVES = {
    ('Linux', 'x86_64'): ('linux_x64', '551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb'),
    ('Linux', 'aarch64'): ('linux_arm64', 'e4a487ee7ccd7d3a7f7ec08657610aa3606637dab924210b3aee62570fb4b080'),
    ('Darwin', 'arm64'): ('darwin_arm64', 'b40ab0ae55c505963e365f271a8d3846efbc170aa17f2607f13df610a9aeb6a5'),
    ('Darwin', 'x86_64'): ('darwin_x64', 'dfe101a4db2255fc85120ac7f3d25e4342c3c20cf749f2c20a18081af1952709'),
}

if __name__ == '__main__':
    target, expected = ARCHIVES[(platform.system(), platform.machine())]
    url = f'https://github.com/gitleaks/gitleaks/releases/download/v{VERSION}/gitleaks_{VERSION}_{target}.tar.gz'
    archive = ROOT / '.local/tools' / f'gitleaks-{VERSION}-{target}.tar.gz'
    archive.parent.mkdir(parents=True, exist_ok=True)
    if not archive.exists():
        subprocess.run(['curl', '--fail', '--silent', '--show-error', '--location', '--max-time', '120', url, '-o', str(archive)], check=True)
    data = archive.read_bytes()
    if hashlib.sha256(data).hexdigest() != expected:
        raise SystemExit('Gitleaks archive checksum mismatch; refusing execution')
    binary = archive.parent / f'gitleaks-{VERSION}'
    with tarfile.open(fileobj=io.BytesIO(data)) as tar:
        member = tar.extractfile('gitleaks')
        if member is None:
            raise SystemExit('Release archive missing binary')
        binary.write_bytes(member.read())
    binary.chmod(0o700)
    subprocess.run([str(binary), 'git', '--redact', '--no-banner', '--log-opts=--all', '.'], cwd=ROOT, check=True)
    subprocess.run([str(binary), 'dir', '--redact', '--no-banner', '.'], cwd=ROOT, check=True)
