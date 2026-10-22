import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

SOURCE = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('desktop_transition', SOURCE / 'contrib/frixit/desktop-transition.py')
desktop = importlib.util.module_from_spec(spec)
spec.loader.exec_module(desktop)


class DesktopTest(unittest.TestCase):
    def test_backup_quiesce_activate_restore_and_unrelated_apps_stay_untouched(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            home = root / 'home'
            autostart = home / '.config/autostart/control-center.desktop'
            autostart.parent.mkdir(parents=True)
            original = '[Desktop Entry]\nExec=original\n'
            autostart.write_text(original)
            calls = []

            def run(args, check=True):
                args = list(map(str, args))
                calls.append(args)
                if 'list-units' in args:
                    output = json.dumps([{'unit': 'tuxedo-power-tray.service', 'active': 'active'},
                                         {'unit': 'browser.service', 'active': 'active'}])
                else:
                    output = 'active'
                return subprocess.CompletedProcess(args, 0, output, '')

            transition = desktop.DesktopTransition(root / 'backup', home, run)
            transition.quiesce(root / 'source')
            self.assertIn(['systemctl', '--user', 'stop', 'tuxedo-power-tray.service'], calls)
            self.assertFalse(any('browser.service' in call for call in calls))
            self.assertEqual(autostart.read_text(), original)
            transition.activate(root / 'preferences.json')
            self.assertIn('Hidden=true', autostart.read_text())
            for unit in desktop.UNITS:
                self.assertTrue(any('--unit=' + unit in call for call in calls))
            transition.restore()
            self.assertEqual(autostart.read_text(), original)
            self.assertEqual(json.loads((root / 'backup/state.json').read_text())['phase'], 'restored')
            self.assertFalse((home / '.config/UniwillControlCenter/desktop.conf').exists())


if __name__ == '__main__':
    unittest.main()
