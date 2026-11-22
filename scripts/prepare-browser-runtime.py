#!/usr/bin/env python3
"""Prepare an optional, self-contained browser runtime from pinned official releases."""
import hashlib
import json
import os
import platform
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / 'desktop/src-tauri/browser-runtime'
NODE = '24.14.0'
PLAYWRIGHT = '1.65.1'


def install_worker(runtime):
    worker = runtime / 'worker'
    source = ROOT / 'executors/browser'
    shutil.copytree(source / 'src', worker / 'src', dirs_exist_ok=True)
    shutil.copy2(source / 'package.json', worker / 'package.json')
    for module in ('playwright', 'playwright-core'):
        shutil.copytree(ROOT / 'node_modules' / module,
                        worker / 'node_modules' / module, dirs_exist_ok=True)
    env = {**os.environ, 'PLAYWRIGHT_BROWSERS_PATH': str(runtime / 'browsers')}
    subprocess.run([str(runtime / 'node'), str(worker / 'node_modules/playwright/cli.js'),
                    'install', 'chromium'], env=env, check=True)


def main():
    system = {'Darwin': 'darwin', 'Linux': 'linux'}.get(platform.system())
    arch = {'arm64': 'arm64', 'aarch64': 'arm64', 'x86_64': 'x64'}.get(platform.machine())
    if not system or not arch:
        raise SystemExit('This preparation script currently supports macOS and Linux arm64/x64 only')
    package = ROOT / 'node_modules/playwright/package.json'
    if not package.exists() or json.loads(package.read_text())['version'] != PLAYWRIGHT:
        raise SystemExit('Run npm ci from the repository root first (the lockfile pins Playwright)')
    manifest = {'node': NODE, 'playwright': PLAYWRIGHT, 'platform': system, 'arch': arch}
    installed = OUTPUT / 'runtime.json'
    if installed.is_file() and json.loads(installed.read_text()) == manifest:
        subprocess.run([str(OUTPUT / 'node'), '--version'], check=True, capture_output=True)
        if not (OUTPUT / 'node.LICENSE').is_file():
            urllib.request.urlretrieve(f'https://raw.githubusercontent.com/nodejs/node/v{NODE}/LICENSE', OUTPUT / 'node.LICENSE')
        install_worker(OUTPUT)
        print(f'Browser components refreshed: {OUTPUT}')
        return
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='jeeves-browser-', dir=OUTPUT.parent) as temporary:
        stage = Path(temporary)
        name = f'node-v{NODE}-{system}-{arch}'
        archive = f'{name}.tar.gz'
        base = f'https://nodejs.org/dist/v{NODE}/'
        sums = urllib.request.urlopen(base + 'SHASUMS256.txt', timeout=60).read().decode()
        expected = next(line.split()[0] for line in sums.splitlines() if line.endswith('  ' + archive))
        target = stage / archive
        urllib.request.urlretrieve(base + archive, target)
        if hashlib.sha256(target.read_bytes()).hexdigest() != expected:
            raise SystemExit('Node archive checksum mismatch')
        with tarfile.open(target) as bundle:
            member = bundle.getmember(f'{name}/bin/node')
            if not member.isfile():
                raise SystemExit('Unexpected Node archive entry')
            (stage / 'node').write_bytes(bundle.extractfile(member).read())
            license_entry = bundle.getmember(f'{name}/LICENSE')
            if not license_entry.isfile():
                raise SystemExit('Unexpected Node license entry')
            (stage / 'node.LICENSE').write_bytes(bundle.extractfile(license_entry).read())
        (stage / 'node').chmod(0o755)
        target.unlink()
        install_worker(stage)
        (stage / 'runtime.json').write_text(json.dumps(manifest) + '\n')
        # Stage fully before replacing an old installation. Never include developer cache paths.
        old = OUTPUT.with_name('browser-runtime.old')
        if old.exists():
            shutil.rmtree(old)
        if OUTPUT.exists():
            OUTPUT.rename(old)
        stage.rename(OUTPUT)
        if old.exists():
            shutil.rmtree(old)
    print(f'Browser components ready: {OUTPUT}')


if __name__ == '__main__':
    main()
