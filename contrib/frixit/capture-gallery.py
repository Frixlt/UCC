#!/usr/bin/env python3
"""Capture current Qt pages against an isolated fake daemon; never use the live bus."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[2]
PAGES = ('overview', 'profiles', 'cooler-settings', 'watercool-settings', 'monitor',
         'keyboard-hardware', 'about', 'power-devices', 'lightbar', 'battery-processes', 'driver-warning')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--build', type=Path, required=True)
    parser.add_argument('--output', type=Path, default=ROOT / 'screenshots/current')
    args = parser.parse_args()
    build, output = args.build.resolve(), args.output.resolve()
    logs = ROOT / 'artifacts/gallery-0.3.0'
    logs.mkdir(parents=True, exist_ok=True)
    version = re.search(r'#define UCC_VERSION_FULL\s+"([^"]+)"', (build / 'include/version.h').read_text())
    for theme in ('light', 'dark'):
        folder = output / theme
        folder.mkdir(parents=True, exist_ok=True)
        env = {**os.environ, 'UCC_AUDIT_THEME': theme, 'UCC_SCREENSHOT_DIR': str(folder)}
        with (logs / f'{theme}.log').open('w') as log:
            subprocess.run(['dbus-run-session', '--', 'bash', str(ROOT / 'tests/run_gui_audit_test.sh'),
                            str(build / 'bin/test_gui_audit'), 'renderReleaseGallery'],
                           env=env, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=75)
        actual = {p.stem for p in folder.glob('*.png')}
        if actual != set(PAGES):
            raise ValueError(f'Unexpected gallery files for {theme}: {actual}')
    images = {str(p.relative_to(output)): hashlib.sha256(p.read_bytes()).hexdigest()
              for p in sorted(output.rglob('*.png'))}
    manifest = {'capturedOn': '2026-10-22', 'release': '0.3.0',
                'buildVersion': version[1] if version else (build / 'include/version.h').read_text().strip(),
                'sourceCommit': subprocess.check_output(['git', '-C', str(ROOT), 'rev-parse', 'HEAD'], text=True).strip(),
                'qtVersion': subprocess.check_output(['pkg-config', '--modversion', 'Qt6Core'], text=True).strip(),
                'viewport': [1440, 1000], 'platform': 'offscreen', 'data': 'isolated test fixture; no live hardware writes',
                'files': images}
    (output / 'manifest.json').write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + '\n')
    print(f'Captured and verified {len(images)} images at {output}')


if __name__ == '__main__':
    main()
