#!/usr/bin/env python3
"""Prepare a private, tested Arch package and fresh configuration import offline."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import time
import xml.etree.ElementTree as ET

REPOSITORY = Path(__file__).resolve().parents[2]


def run(args, **kwargs):
    return subprocess.run(list(map(str, args)), check=True, **kwargs)


def sha(path):
    with Path(path).open('rb') as file:
        return hashlib.file_digest(file, 'sha256').hexdigest()


def test_report(path, allow_gui_failure=False):
    root = ET.parse(path).getroot()
    cases = list(root.iter('testcase'))
    failed = [case.get('name') for case in cases if list(case.iter('failure')) or list(case.iter('error'))]
    if not cases or (failed and not (allow_gui_failure and failed == ['test_gui_audit'])):
        raise ValueError('Missing or failing test results: ' + str(path))
    return len(cases)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--build', required=True, type=Path)
    parser.add_argument('--data', required=True, type=Path)
    parser.add_argument('--tcc-dir', type=Path, default=Path('/etc/tcc'))
    parser.add_argument('--tcc-archive', type=Path, default=Path('/opt/tuxedo-control-center/resources/app.asar'))
    parser.add_argument('--normal-tests', required=True, type=Path)
    parser.add_argument('--preview-tests', required=True, type=Path)
    parser.add_argument('--allow-gui-audit-failure', action='store_true',
                        help='Explicitly launch despite the known GUI audit failure; retain failing reports in the manifest')
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    if os.geteuid() == 0:
        parser.error('Prepare the bundle as the desktop user')
    counts = [test_report(args.normal_tests, args.allow_gui_audit_failure),
              test_report(args.preview_tests, args.allow_gui_audit_failure)]
    cache = (args.build / 'CMakeCache.txt').read_text()
    for required in ('UCC_READ_ONLY_PREVIEW:BOOL=OFF', 'CMAKE_INSTALL_PREFIX:PATH=/usr',
                     'BUILD_GUI:BOOL=ON', 'BUILD_CLI:BOOL=ON', 'BUILD_DESKTOP_TRAY:BOOL=ON',
                     'BUILD_SESSION_AGENT:BOOL=ON'):
        if required not in cache.splitlines():
            raise ValueError('Unsuitable installation build: ' + required)
    args.output.mkdir(mode=0o700, parents=False)
    bundle = args.output.resolve()
    nodes = []
    for path in Path('/sys/class/leds').iterdir():
        match = re.fullmatch(r'rgb:kbd_backlight(?:_(\d+))?', path.name)
        if match and (path / 'multi_intensity').exists():
            nodes.append((int(match[1] or 0), path))
    nodes.sort()
    if not nodes or len({n for n, _ in nodes}) != len(nodes):
        raise ValueError('Cannot identify physical RGB keyboard LEDs')
    maxima = {int((p / 'max_brightness').read_text()) for _, p in nodes}
    if len(maxima) != 1:
        raise ValueError('Inconsistent keyboard brightness limits')
    capabilities = {'zones': len(nodes), 'maxBrightness': maxima.pop(), 'ledIds': [n for n, _ in nodes]}
    (bundle / 'capabilities.json').write_text(json.dumps(capabilities, indent=2) + '\n')
    snapshot = bundle / 'source'
    snapshot.mkdir(mode=0o700)
    origins = [args.data / (key + '.json') for key in ('config', 'keyboard_colors', 'lightbar', 'gui_settings')]
    origins += [args.tcc_dir / 'settings', args.tcc_dir / 'profiles', args.tcc_archive]
    fingerprints = [{'path': str(path.resolve()), 'sha256': sha(path)} for path in origins]
    data = snapshot / 'Data'
    data.mkdir(mode=0o700)
    for origin in origins[:4]:
        shutil.copy2(origin, data / origin.name)
    tcc = snapshot / 'tcc'
    tcc.mkdir(mode=0o700)
    for origin in origins[4:6]:
        shutil.copy2(origin, tcc / origin.name)
    shutil.copy2(args.tcc_archive, snapshot / 'app.asar')
    run(['python3', REPOSITORY / 'contrib/frixit/migrate-controlcenter.py', data, bundle / 'import',
         '--capabilities', bundle / 'capabilities.json', '--tcc-dir', tcc,
         '--tcc-archive', snapshot / 'app.asar', '--brightness', int((nodes[0][1] / 'brightness').read_text())],
        stdout=(bundle / 'migration.log').open('w'))
    report = json.loads((bundle / 'import/report.json').read_text())
    if report['unresolvedProfiles']:
        raise ValueError('Some original profiles remain unresolved')
    run([args.build / 'bin/ucc-import-check', bundle / 'import/settings', bundle / 'capabilities.json'])
    stage = bundle / 'stage'
    with (bundle / 'install.log').open('w') as log:
        run(['cmake', '--install', args.build], env={**os.environ, 'DESTDIR': str(stage)}, stdout=log, stderr=subprocess.STDOUT)
    config = stage / 'etc/ucc'
    config.mkdir(mode=0o755, parents=True, exist_ok=True)
    shutil.copyfile(bundle / 'import/settings', config / 'settings')
    (config / 'settings').chmod(0o600)
    commit = subprocess.check_output(['git', '-C', REPOSITORY, 'rev-parse', 'HEAD'], text=True).strip()
    revision = subprocess.check_output(['git', '-C', REPOSITORY, 'rev-list', '--count', 'HEAD'], text=True).strip()
    version = '0.2.0.r' + revision + '.g' + commit[:8] + '-1'
    metadata = ['pkgname = frixit-control-center', 'pkgbase = frixit-control-center', 'pkgver = ' + version,
                'pkgdesc = Frixit Control Center fork with original personal configuration',
                'builddate = ' + str(int(time.time())), 'packager = Local Frixit build',
                'size = ' + str(sum(p.stat().st_size for p in stage.rglob('*') if p.is_file())),
                'arch = x86_64', 'license = GPL-3.0-or-later', 'backup = etc/ucc/settings',
                'provides = uniwill-control-center=0.2.0', 'conflict = uniwill-control-center']
    metadata += ['depend = ' + p for p in ('qt6-base', 'qt6-charts', 'qt6-connectivity', 'qt6-declarative',
                                         'dbus', 'polkit', 'systemd-libs', 'hicolor-icon-theme')]
    (stage / '.PKGINFO').write_text('\n'.join(metadata) + '\n')
    package = bundle / ('frixit-control-center-' + version + '-x86_64.pkg.tar.zst')
    run(['bsdtar', '--create', '--zstd', '--uid', '0', '--gid', '0', '--uname', 'root', '--gname', 'root',
         '--file', package, '-C', stage, *sorted(p.name for p in stage.iterdir())])
    package.chmod(0o600)
    run(['pacman', '-Qip', package], stdout=(bundle / 'package-info.txt').open('w'))
    for name, origin in (('normal-tests.xml', args.normal_tests), ('preview-tests.xml', args.preview_tests)):
        shutil.copyfile(origin, bundle / name)
    manifest = {'commit': commit, 'version': version, 'package': package.name, 'packageSha256': sha(package),
                'inputs': fingerprints, 'tests': {'normal': counts[0], 'preview': counts[1]},
                'migration': report}
    manifest['testFailures'] = {name: [case.get('name') for case in ET.parse(path).getroot().iter('testcase')
                                     if list(case.iter('failure')) or list(case.iter('error'))]
                                for name, path in [('normal', args.normal_tests), ('preview', args.preview_tests)]}
    manifest['guiAuditFailureAcceptedForLaunch'] = args.allow_gui_audit_failure
    (bundle / 'deployment.json').write_text(json.dumps(manifest, indent=2) + '\n')
    shutil.copyfile(REPOSITORY / 'contrib/frixit/deploy-system.py', bundle / 'deploy-system.py')
    print(json.dumps({'bundle': str(bundle), 'package': package.name, 'tests': counts}))


if __name__ == '__main__':
    main()
