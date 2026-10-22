#!/usr/bin/env python3
"""Quiesce the old desktop clients and activate/restore the user's UCC session."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

FILES = ('.config/autostart/control-center.desktop',
         '.config/UniwillControlCenter/desktop.conf', '.config/UniwillControlCenter/appearance.conf')
UNITS = ('ucc-desktop-tray.service', 'ucc-session-agent.service', 'ucc-gui.service')


def run(args, check=True):
    return subprocess.run(list(map(str, args)), check=check, capture_output=True, text=True, timeout=30)


class DesktopTransition:
    def __init__(self, state, home=None, runner=run):
        self.directory = Path(state)
        self.home = Path(home) if home else Path.home()
        self.run = runner

    def save(self, state):
        (self.directory / 'state.json').write_text(json.dumps(state, indent=2) + '\n')

    def quiesce(self, source):
        self.directory.mkdir(mode=0o700)
        state = {'source': str(Path(source).resolve()), 'phase': 'quiesced', 'files': {}, 'oldUnits': []}
        for relative in FILES:
            origin = self.home / relative
            state['files'][relative] = origin.exists()
            if origin.exists():
                target = self.directory / 'files' / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(origin, target)
        units = json.loads(self.run(['systemctl', '--user', 'list-units', '--all', '--output=json']).stdout)
        for unit in units:
            name = unit['unit']
            if (name in ('tuxedo-power-tray.service', 'tuxedo-lightbar-tray.service') or
                    name.startswith(('app-tuxedo-control-center-', r'app-tuxedo\x2dcontrol\x2dcenter@'))):
                if unit['active'] in ('active', 'activating'):
                    state['oldUnits'].append(name)
        self.save(state)
        if state['oldUnits']:
            self.run(['systemctl', '--user', 'stop', *state['oldUnits']])

    def activate(self, preferences):
        state = json.loads((self.directory / 'state.json').read_text())
        if self.run(['systemctl', 'is-active', 'uccd.service'], check=False).returncode:
            raise RuntimeError('The system UCC daemon is not active')
        # A user Hidden entry overrides the old login launcher without deleting it.
        target = self.home / '.config/autostart/control-center.desktop'
        target.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.NamedTemporaryFile(mode='w', dir=target.parent, delete=False) as file:
            file.write('[Desktop Entry]\nType=Application\nName=Control Center (replaced by UCC)\nHidden=true\n')
        Path(file.name).replace(target)
        self.run(['/usr/bin/ucc-gui', '--import-preferences', preferences, '--preferences-only'])
        self.run(['systemctl', '--user', 'daemon-reload'])
        for unit in UNITS:
            self.run(['systemd-run', '--user', '--collect', '--unit=' + unit, '/usr/bin/' + unit.removesuffix('.service')])
        for _ in range(10):
            if all(self.run(['systemctl', '--user', 'is-active', unit], check=False).returncode == 0 for unit in UNITS):
                state['phase'] = 'activated'
                self.save(state)
                return
            time.sleep(1)
        raise RuntimeError('One or more UCC desktop clients failed to start')

    def restore(self):
        state = json.loads((self.directory / 'state.json').read_text())
        self.run(['systemctl', '--user', 'stop', *UNITS], check=False)
        for relative, existed in state['files'].items():
            target = self.home / relative
            if existed:
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(self.directory / 'files' / relative, target)
            else:
                target.unlink(missing_ok=True)
        self.run(['systemctl', '--user', 'daemon-reload'])
        if any(name.startswith('tuxedo-') for name in state['oldUnits']):
            if self.run(['systemctl', 'is-active', 'tuxedo-power.service'], check=False).returncode:
                raise RuntimeError('Restore the system deployment before restarting original desktop clients')
            self.run(['bash', Path(state['source']) / 'launch.sh'])
        state['phase'] = 'restored'
        self.save(state)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('quiesce', 'activate', 'restore'))
    parser.add_argument('state', type=Path)
    parser.add_argument('--source', type=Path)
    parser.add_argument('--preferences', type=Path)
    args = parser.parse_args()
    if os.geteuid() == 0:
        parser.error('Desktop transition must run as the normal desktop user')
    transition = DesktopTransition(args.state)
    if args.action == 'quiesce':
        if not args.source:
            parser.error('--source is required')
        transition.quiesce(args.source)
    elif args.action == 'activate':
        if not args.preferences:
            parser.error('--preferences is required')
        transition.activate(args.preferences)
    else:
        transition.restore()


if __name__ == '__main__':
    main()
