#!/usr/bin/env python3
"""Install a tested first-release package, with service-state backup and rollback.

Run through pkexec after preparing the bundle. Never runs a shell or suspends the
machine. Package installation uses pacman, so installed files remain tracked.
"""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import uuid

OLD_UNITS = ('tccd-sleep.service', 'tccd.service', 'tuxedo-power.service')
NEW_UNITS = ('uccd.service', 'uccd-sleep.service')
PACKAGE = 'frixit-control-center'


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def execute(args, *, check=True):
    print('+ ' + ' '.join(map(str, args)), flush=True)
    result = subprocess.run(list(map(str, args)), check=False, text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=600)
    if check and result.returncode:
        raise RuntimeError('Command failed: ' + str(args) + '\n' + result.stdout[-4000:] + result.stderr[-4000:])
    return result


class Deployment:
    def __init__(self, bundle, root=Path('/'), run=execute):
        self.bundle = Path(bundle).resolve()
        self.root = Path(root)
        self.run = run
        self.backup = None
        self.state = None

    def path(self, absolute):
        return self.root / str(absolute).lstrip('/')

    def save(self):
        target = self.backup / 'state.json'
        temporary = target.with_suffix('.tmp')
        temporary.write_text(json.dumps(self.state, indent=2) + '\n')
        temporary.chmod(0o600)
        temporary.replace(target)

    def verify_bundle(self):
        manifest = json.loads((self.bundle / 'deployment.json').read_text())
        name = manifest['package']
        if Path(name).name != name or not name.startswith(PACKAGE + '-'):
            raise ValueError('Invalid package filename')
        package = self.bundle / name
        if package.is_symlink() or digest(package) != manifest['packageSha256']:
            raise ValueError('Package checksum mismatch')
        for item in manifest['inputs']:
            if digest(self.path(item['path'])) != item['sha256']:
                raise ValueError('Source configuration changed: ' + item['path'])
        if self.run(['pacman', '-Q', PACKAGE], check=False).returncode == 0:
            raise ValueError('This first-install tool refuses to overwrite an installed UCC package')
        if self.path('/usr/bin/uccd').exists() or self.path('/etc/ucc/settings').exists():
            raise ValueError('Existing unmanaged UCC installation requires a separate upgrade migration')
        return manifest, package

    def install(self):
        manifest, package = self.verify_bundle()
        stamp = datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ')
        self.backup = self.path('/var/lib/ucc/deployments') / (stamp + '-' + uuid.uuid4().hex[:8])
        self.backup.mkdir(mode=0o700, parents=True)
        self.backup.chmod(0o700)
        self.state = {'phase': 'prepared', 'package': PACKAGE, 'manifest': manifest,
                      'packageInstalled': False, 'packageInstallStarted': False, 'servicesTouched': False, 'units': {}}
        for unit in OLD_UNITS + NEW_UNITS:
            self.state['units'][unit] = {
                'active': self.run(['systemctl', 'is-active', unit], check=False).stdout.strip(),
                'enabled': self.run(['systemctl', 'is-enabled', unit], check=False).stdout.strip()}
        for name in ('/etc/tcc', '/etc/ucc'):
            source = self.path(name)
            if source.is_dir():
                shutil.copytree(source, self.backup / name.lstrip('/'), symlinks=True)
        for item in manifest['inputs']:
            destination = self.backup / 'inputs' / item['path'].lstrip('/')
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(self.path(item['path']), destination)
        shutil.copy2(__file__, self.backup / 'deploy-system.py')
        self.save()
        try:
            # Dependencies are resolved from configured signed repositories; no
            # signature policy changes or full distribution upgrade are made.
            self.run(['pacman', '-S', '--needed', '--noconfirm', 'qt6-charts'])
            self.state['packageInstallStarted'] = True
            self.save()
            self.run(['pacman', '-U', '--noconfirm', package])
            self.state['packageInstalled'] = True
            self.save()
            self.run(['/usr/lib/ucc/ucc-import-check', '/etc/ucc/settings', self.bundle / 'capabilities.json'])
            self.run(['systemd-analyze', '--man=no', 'verify', 'uccd.service', 'uccd-sleep.service'])
            # Stop the old sleep hook first: its ExecStop may start tccd.
            self.state['servicesTouched'] = True
            self.save()
            self.run(['systemctl', 'stop', OLD_UNITS[0]])
            self.run(['systemctl', 'stop', *OLD_UNITS[1:]])
            self.run(['systemctl', 'disable', *OLD_UNITS])
            self.run(['systemctl', 'mask', '--runtime', 'tccd.service', 'tccd-sleep.service'])
            self.run(['systemctl', 'daemon-reload'])
            self.run(['systemctl', 'reload', 'dbus.service'])
            self.run(['systemctl', 'enable', *NEW_UNITS])
            self.run(['systemctl', 'start', 'uccd.service'])
            self.health()
            # Exercise the real stop/save/restart path without suspending the PC.
            previous = json.loads(self.run(['ucc-cli', '--json', 'power', 'status']).stdout)
            self.run(['/usr/lib/ucc/ucc-sleep', 'prepare'])
            self.run(['/usr/lib/ucc/ucc-sleep', 'resume'])
            self.health()
            resumed = json.loads(self.run(['ucc-cli', '--json', 'power', 'status']).stdout)
            if resumed.get('manual_mode') != previous.get('manual_mode'):
                raise RuntimeError('Resume lifecycle did not preserve the manual power mode')
            self.state['phase'] = 'installed'
            self.save()
        except BaseException:
            try:
                self.rollback()
            except Exception as recovery_error:
                print('ROLLBACK ERROR: ' + str(recovery_error), flush=True)
            raise
        print(json.dumps({'installed': True, 'backup': str(self.backup)}), flush=True)
        return self.backup

    def health(self):
        supported = self.run(['busctl', '--timeout=15', 'call', 'com.uniwill.uccd',
                              '/com/uniwill/uccd', 'com.uniwill.uccd', 'IsDeviceSupported']).stdout.strip()
        if supported != 'b true':
            raise RuntimeError('UCC does not report a supported device: ' + supported)
        policy = json.loads(self.run(['ucc-cli', '--json', 'power', 'status']).stdout)
        if policy.get('current_mode') not in ('ac', 'type_c', 'bat'):
            raise RuntimeError('Invalid live power policy')
        self.run(['systemctl', 'is-active', 'uccd.service'])

    def rollback(self):
        if self.state.get('packageInstallStarted') and not self.state['packageInstalled']:
            self.state['packageInstalled'] = self.run(['pacman', '-Q', PACKAGE], check=False).returncode == 0
            self.save()
        if self.state['servicesTouched']:
            self.run(['systemctl', 'stop', 'uccd-sleep.service'], check=False)
            self.run(['systemctl', 'stop', 'uccd.service'], check=False)
            self.run(['systemctl', 'disable', *NEW_UNITS], check=False)
        if self.state['packageInstalled']:
            self.run(['pacman', '-R', '--noconfirm', PACKAGE])
            # pacman may preserve the imported settings as .pacsave; keep them
            # in the recovery directory so the original /etc/ucc state is restored.
            settings_dir = self.path('/etc/ucc')
            if settings_dir.exists():
                shutil.move(str(settings_dir), self.backup / 'retired-ucc-settings')
            previous = self.backup / 'etc/ucc'
            if previous.exists():
                shutil.copytree(previous, settings_dir, symlinks=True)
        if self.state['servicesTouched']:
            self.run(['systemctl', 'unmask', '--runtime', 'tccd.service', 'tccd-sleep.service'], check=False)
            self.run(['systemctl', 'daemon-reload'])
            self.run(['systemctl', 'reload', 'dbus.service'], check=False)
            # Sleep hooks must be restored before the main daemon: their start
            # commands stop tccd, which must then be started if previously active.
            for unit in OLD_UNITS:
                saved = self.state['units'][unit]
                if saved['enabled'] in ('enabled', 'enabled-runtime'):
                    args = ['systemctl', 'enable']
                    if saved['enabled'] == 'enabled-runtime':
                        args.append('--runtime')
                    self.run([*args, unit])
                if saved['active'] == 'active':
                    self.run(['systemctl', 'start', unit])
        self.state['phase'] = 'rolled-back'
        self.save()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument('--install', type=Path, metavar='BUNDLE')
    group.add_argument('--rollback', type=Path, metavar='BACKUP')
    args = parser.parse_args()
    if os.geteuid() != 0:
        parser.error('System installation requires root (use pkexec)')
    os.umask(0o077)
    try:
        if args.install:
            Deployment(args.install).install()
        else:
            backup = args.rollback.resolve()
            if backup.parent != Path('/var/lib/ucc/deployments') or backup.stat().st_uid != 0:
                raise ValueError('Untrusted recovery directory')
            deployment = Deployment(backup)
            deployment.backup = backup
            deployment.state = json.loads((backup / 'state.json').read_text())
            if deployment.state['phase'] == 'rolled-back':
                raise ValueError('This deployment was already rolled back')
            deployment.rollback()
    except Exception as error:
        print('Deployment failed: ' + str(error), flush=True)
        raise SystemExit(1)


if __name__ == '__main__':
    main()
