"""Exercise the actual offline C++ preflight with valid and corrupted imports."""
import copy
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

CHECKER = Path(sys.argv.pop(1))


class ImportCheckTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.states = [{'red': n, 'green': 20, 'blue': 30, 'brightness': 17} for n in (1, 21, 56)]
        self.profiles = {}
        for mode in ('ac', 'type_c', 'bat'):
            self.profiles['frixit-' + mode] = {
                'id': 'frixit-' + mode, 'name': mode,
                'selectedKeyboardProfile': 'frixit-keyboard',
                'cpu': {'governor': 'powersave', 'noTurbo': mode == 'bat', 'onlineCores': 8},
                'odmPowerLimits': {'tdpValues': [10, 20, 30]},
                'fan': {key: [{'temp': 0, 'speed': 20}, {'temp': 100, 'speed': 100}]
                        for key in ('tableCPU', 'tableGPU')}}
        self.settings = {
            'keyboardLayout': 'frixit-mechrevo', 'keyboardBacklightStates': self.states,
            'customKeyboardProfiles': {'frixit-keyboard': {'id': 'frixit-keyboard', 'name': 'Personal', 'json': {'states': self.states}}},
            'powerDevices': {'enabled': True, 'devices': []}, 'lightbar': {'managed': True},
            'profiles': self.profiles, 'stateMap': {'power_' + mode: 'frixit-' + mode for mode in ('ac', 'type_c', 'bat')}}
        (self.root / 'caps').write_text(json.dumps({'zones': 3, 'maxBrightness': 50, 'ledIds': [0, 21, 56]}))

    def check(self, settings):
        file = self.root / 'settings'
        file.write_text(json.dumps(settings))
        return subprocess.run([CHECKER, file, self.root / 'caps'], capture_output=True, text=True)

    def test_all_modes_roundtrip_through_production_parsers(self):
        result = self.check(self.settings)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout.splitlines()[-1]), {'ok': True, 'profiles': 3, 'keyboardZones': 3, 'devices': 0})

    def test_missing_mode_wrong_keyboard_and_invalid_curves_are_rejected(self):
        for corruption in ('mode', 'keyboard', 'brightness', 'curve', 'device'):
            with self.subTest(corruption=corruption):
                settings = copy.deepcopy(self.settings)
                if corruption == 'mode':
                    del settings['stateMap']['power_type_c']
                elif corruption == 'keyboard':
                    settings['profiles']['frixit-bat']['selectedKeyboardProfile'] = 'upstream'
                elif corruption == 'brightness':
                    settings['keyboardBacklightStates'][0]['brightness'] = 51
                elif corruption == 'curve':
                    settings['profiles']['frixit-ac']['fan']['tableCPU'][0]['speed'] = 101
                else:
                    settings['powerDevices']['devices'] = [{'path': '/etc/shadow'}]
                self.assertEqual(self.check(settings).returncode, 1)


if __name__ == '__main__':
    unittest.main()
