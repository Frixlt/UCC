"""Failure-injection tests for the installer; no host commands or root writes."""
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

SOURCE = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('deploy_system', SOURCE / 'contrib/frixit/deploy-system.py')
deploy = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(deploy)


class FakeSystem:
    def __init__(self, root):
        self.root = root
        self.calls = []
        self.active = {name: 'active' if name != 'tccd-sleep.service' else 'inactive' for name in deploy.OLD_UNITS}
        self.enabled = {name: 'enabled' for name in deploy.OLD_UNITS}
        self.installed = False
        self.failure = None

    def __call__(self, args, *, check=True):
        args = tuple(map(str, args))
        self.calls.append(args)
        if self.failure and args[:len(self.failure)] == self.failure:
            self.failure = None
            raise RuntimeError('injected command failure')
        output, code = '', 0
        if args[:2] == ('pacman', '-Q'):
            code = 0 if self.installed else 1
        elif args[:2] == ('pacman', '-U'):
            self.installed = True
            settings = self.root / 'etc/ucc/settings'
            settings.parent.mkdir(parents=True)
            settings.write_text('{}')
        elif args[:2] == ('pacman', '-R'):
            self.installed = False
        elif args[:2] == ('systemctl', 'is-active'):
            output = self.active.get(args[2], 'inactive')
        elif args[:2] == ('systemctl', 'is-enabled'):
            output = self.enabled.get(args[2], 'not-found')
        elif args[:2] in (('systemctl', 'start'), ('systemctl', 'stop')):
            for name in args[2:]:
                self.active[name] = 'active' if args[1] == 'start' else 'inactive'
        elif args[:2] in (('systemctl', 'enable'), ('systemctl', 'disable')):
            for name in args[2:]:
                self.enabled[name] = 'enabled' if args[1] == 'enable' else 'disabled'
        elif args[0] == 'busctl':
            output = 'b true'
        elif args[0] == 'ucc-cli':
            output = '{"current_mode":"ac"}'
        return subprocess.CompletedProcess(args, code, output, '')


class DeploymentTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / 'root'
        self.bundle = Path(self.temp.name) / 'bundle'
        self.bundle.mkdir()
        source = self.root / 'home/test/Data/config.json'
        source.parent.mkdir(parents=True)
        source.write_text('{"original":true}')
        package = self.bundle / 'frixit-control-center-0.2.0-1-x86_64.pkg.tar.zst'
        package.write_bytes(b'fake package for injected package manager')
        self.manifest = {'package': package.name, 'packageSha256': deploy.digest(package),
                         'inputs': [{'path': '/home/test/Data/config.json', 'sha256': deploy.digest(source)}]}
        (self.bundle / 'deployment.json').write_text(json.dumps(self.manifest))
        self.system = FakeSystem(self.root)
        self.install = deploy.Deployment(self.bundle, self.root, self.system)

    def test_install_validates_before_stopping_and_keeps_recovery_inputs(self):
        backup = self.install.install()
        self.assertEqual(self.install.state['phase'], 'installed')
        self.assertEqual((backup / 'inputs/home/test/Data/config.json').read_text(), '{"original":true}')
        calls = self.system.calls
        validation = next(i for i, call in enumerate(calls) if call[0].endswith('ucc-import-check'))
        sleep_stop = calls.index(('systemctl', 'stop', 'tccd-sleep.service'))
        old_stop = calls.index(('systemctl', 'stop', 'tccd.service', 'tuxedo-power.service'))
        new_start = calls.index(('systemctl', 'start', 'uccd.service'))
        self.assertLess(validation, sleep_stop)
        self.assertLess(sleep_stop, old_stop)
        self.assertLess(old_stop, new_start)
        self.install.rollback()
        self.assertFalse(self.system.installed)
        self.assertEqual(self.system.active['tccd.service'], 'active')
        self.assertEqual(self.system.active['tuxedo-power.service'], 'active')
        self.assertTrue((backup / 'retired-ucc-settings/settings').is_file())

    def test_changed_source_or_package_never_touches_services(self):
        for what in ('source', 'package'):
            with self.subTest(what=what):
                path = (self.root / 'home/test/Data/config.json') if what == 'source' else self.bundle / self.manifest['package']
                original = path.read_bytes()
                path.write_bytes(b'changed')
                with self.assertRaises(ValueError):
                    self.install.install()
                self.assertFalse(self.system.calls)
                path.write_bytes(original)

    def test_failed_new_daemon_restores_old_services_and_removes_package(self):
        self.system.failure = ('systemctl', 'start', 'uccd.service')
        with self.assertRaisesRegex(RuntimeError, 'injected'):
            self.install.install()
        self.assertEqual(self.install.state['phase'], 'rolled-back')
        self.assertFalse(self.system.installed)
        for unit in ('tccd.service', 'tuxedo-power.service'):
            self.assertEqual(self.system.active[unit], 'active')
            self.assertEqual(self.system.enabled[unit], 'enabled')

    def test_validation_failure_leaves_existing_controllers_running(self):
        self.system.failure = ('/usr/lib/ucc/ucc-import-check',)
        with self.assertRaises(RuntimeError):
            self.install.install()
        self.assertFalse(self.system.installed)
        self.assertNotIn(('systemctl', 'stop', 'tccd.service', 'tuxedo-power.service'), self.system.calls)
        self.assertEqual(self.system.active['tccd.service'], 'active')

    def test_rollback_does_not_enable_or_start_previously_disabled_controllers(self):
        self.system.active['tuxedo-power.service'] = 'inactive'
        self.system.enabled['tuxedo-power.service'] = 'disabled'
        self.system.failure = ('systemctl', 'start', 'uccd.service')
        with self.assertRaises(RuntimeError):
            self.install.install()
        self.assertNotIn(('systemctl', 'start', 'tuxedo-power.service'), self.system.calls)
        self.assertNotIn(('systemctl', 'enable', 'tuxedo-power.service'), self.system.calls)

    def test_package_manager_failure_after_install_is_also_rolled_back(self):
        def partially_failed(args, *, check=True):
            result = self.system(args, check=check)
            if tuple(args[:2]) == ('pacman', '-U'):
                raise RuntimeError('failure after package commit')
            return result
        self.install.run = partially_failed
        with self.assertRaisesRegex(RuntimeError, 'after package commit'):
            self.install.install()
        self.assertFalse(self.system.installed)
        self.assertEqual(self.install.state['phase'], 'rolled-back')


if __name__ == '__main__':
    unittest.main()
