# SPDX-License-Identifier: GPL-3.0-or-later
import copy
import importlib.util
import json
from pathlib import Path
import struct
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('tcc_import', ROOT/'contrib/frixit/tcc_import.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class TccImportTest(unittest.TestCase):
    def setUp(self):
        self.points = [{'temp': 0, 'speed': 0}, {'temp': 100, 'speed': 100}]
        self.presets = {'Silent': {key: self.points for key in ('tableCPU', 'tableGPU')}}
        self.profile = {'id': 'user', 'name': 'Actual profile',
            'cpu': {'onlineCores': 8, 'scalingMaxFrequency': 3400000},
            'display': {'useRefRate': False, 'refreshRate': 59.99},
            'fan': {'useControl': True, 'fanProfile': 'Silent',
                'minimumFanspeed': 15, 'maximumFanspeed': 68, 'offsetFanspeed': 10,
                'customFanCurve': {key: [{'temp': 0, 'speed': 99}, {'temp': 100, 'speed': 99}] for key in ('tableCPU', 'tableGPU')}},
            'odmPowerLimits': {'tdpValues': [11, 20, 32]}, 'nvidiaPowerCTRLProfile': {'cTGPOffset': 95}}

    def test_active_named_curve_wins_over_unused_custom_curve_and_bakes_modifiers(self):
        before = copy.deepcopy(self.profile)
        merged, report = module.merge_tcc({}, {'chargingProfile': 'stationary'}, [self.profile], self.presets)
        profile = merged['profiles']['user']
        self.assertEqual([profile['fan']['tableCPU'][t]['speed'] for t in (0, 20, 58, 90)], [15, 30, 68, 68])
        self.assertEqual(profile['odmPowerLimits'], self.profile['odmPowerLimits'])
        self.assertEqual(profile['cpu'], self.profile['cpu'])
        self.assertEqual(profile['display']['refreshRate'], 60)
        self.assertEqual(profile['nvidiaCTGPOffset'], 95)
        self.assertEqual(merged['chargingProfile'], 'stationary')
        self.assertEqual(report['profiles']['user']['fanPreset'], 'Silent')
        self.assertEqual(self.profile, before)
        self.assertEqual(merged['customFanProfiles'][profile['fan']['fanProfile']]['json']['tableGPU'], profile['fan']['tableGPU'])

    def test_custom_curve_ignores_named_modifiers_and_uses_half_up_rounding(self):
        self.profile['fan']['fanProfile'] = 'Custom'
        self.profile['fan']['customFanCurve'] = {key: [{'temp': 0, 'speed': 0}, {'temp': 100, 'speed': 50}] for key in ('tableCPU', 'tableGPU')}
        merged, _ = module.merge_tcc({}, {}, [self.profile], {})
        values = merged['profiles']['user']['fan']['tableCPU']
        self.assertEqual([values[t]['speed'] for t in (0, 1, 3, 100)], [0, 1, 2, 50])

    def test_exported_ucc_profile_and_cooler_settings_are_preserved(self):
        exported = {'profiles': {'user': {'id': 'user', 'fan': {'autoControlWC': True, 'manualPumpVoltage': 4}}}, 'stateMap': {'power_wc': 'user'}}
        merged, _ = module.merge_tcc(exported, {'keyboardBacklightControlEnabled': False}, [self.profile], self.presets)
        self.assertEqual(merged['profiles']['user'], exported['profiles']['user'])
        self.assertEqual(merged['stateMap'], exported['stateMap'])
        self.assertFalse(merged['keyboardBacklightControlSupported'])

    def test_missing_named_curve_and_invalid_tables_fail_instead_of_substituting(self):
        with self.assertRaisesRegex(ValueError, 'unavailable'):
            module.merge_tcc({}, {}, [self.profile], {})
        for table in ([], [{'temp': 1, 'speed': 1}]*2, [{'temp': 0, 'speed': 101}, {'temp': 100, 'speed': 100}]):
            with self.subTest(table=table), self.assertRaises(ValueError):
                module.effective_table(table)

    def test_installed_preset_literal_is_read_without_running_javascript(self):
        literal = b"export const defaultFanProfiles = [\n{name: 'Silent', tableCPU: [{temp: 0, speed: 0}, {temp: 100, speed: 100},], tableGPU: [{temp: 0, speed: 10}, {temp: 100, speed: 90}]},\n{name: 'Custom'},\n];\nthrow new Error('This code must never execute');\n"
        tree = {'size': len(literal), 'offset': '0'}
        for name in reversed('dist/tuxedo-control-center/service-app/common/models/TccFanTable.js'.split('/')):
            tree = {'files': {name: tree}}
        header = json.dumps(tree).encode()
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)/'app.asar'
            path.write_bytes(struct.pack('<4I', 4, len(header)+8, len(header)+4, len(header))+header+literal)
            presets = module.read_fan_presets(path)
            self.assertEqual(presets['Silent']['tableGPU'][0]['speed'], 10)
            self.assertNotIn('Custom', presets)
            path.write_bytes(b'bad')
            with self.assertRaises(ValueError):
                module.read_fan_presets(path)

if __name__ == '__main__':
    unittest.main()
