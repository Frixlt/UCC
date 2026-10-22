import copy
import importlib.util
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('frixit_migration', ROOT/'contrib/frixit/migrate-controlcenter.py')
migration = importlib.util.module_from_spec(spec)
spec.loader.exec_module(migration)

class MigrationTest(unittest.TestCase):
    def setUp(self):
        self.data = {'keyboard_colors': {'0': [1, 2, 3], '21': [4, 5, 6], '56': [7, 8, 9]},
          'config': {'settings': {mode: {'tuxedo_profile_id': 'shared', 'cpu_profile': cpu, 'cpu_temp_limit': 85}
             for mode, cpu in zip(('ac', 'type_c', 'bat'), ('performance', 'balanced', 'power-saver'))},
             'devices': [{'path': '/sys/devices/system/cpu/intel_pstate/no_turbo', 'ac_val': '0', 'type_c_val': '0', 'bat_val': '1'}]},
          'lightbar': {'silent_mode': True}, 'gui_settings': {'theme': 'system'}}
        self.base = {'profiles': {'shared': {'id': 'shared', 'name': 'Original', 'fan': {'fanProfile': 'user-curve'},
                    'odmPowerLimits': {'tdpValues': [55, 65, 75]}}}, 'stateMap': {'power_wc': 'cooler'},
                    'keyboardBacklightStates': [{'brightness': 17, 'red': 10, 'green': 20, 'blue': 30} for _ in range(4)]}
        self.caps = {'zones': 4, 'maxBrightness': 50, 'ledIds': [0, 21, 56, 66]}

    def test_user_colors_win_without_shifting_missing_leds(self):
        before = copy.deepcopy((self.base, self.data))
        settings, report, pending = migration.convert(self.data, self.base, self.caps)
        states = settings['customKeyboardProfiles']['frixit-keyboard']['json']['states']
        self.assertEqual([s['red'] for s in states], [1, 4, 7, 10])
        self.assertEqual([s['brightness'] for s in states], [17]*4)
        self.assertEqual(settings['keyboardLayout'], 'frixit-mechrevo')
        self.assertEqual((self.base, self.data), before)
        self.assertEqual(report['ignored'], ['cpu_temp_limit'])
        self.assertEqual(pending['lightbar'], self.data['lightbar'])
        self.assertEqual(settings['powerDevices']['devices'], self.data['config']['devices'])
        self.assertEqual(settings['powerDevices']['cpu']['bat']['aspm'], 'powersupersave')
        self.assertNotIn('cpu_temp_limit', str(settings['powerDevices']))

    def test_three_modes_keep_fan_and_tdp_but_use_personal_cpu_policy(self):
        settings, report, _ = migration.convert(self.data, self.base, self.caps)
        self.assertEqual(settings['stateMap']['power_type_c'], 'frixit-type_c')
        self.assertEqual(settings['stateMap']['power_wc'], 'cooler')
        self.assertFalse(report['unresolvedProfiles'])
        for mode, governor, epp, turbo in [('ac','performance','performance',False),
            ('type_c','powersave','balance_performance',False), ('bat','powersave','power',True)]:
            profile = settings['profiles'][f'frixit-{mode}']
            self.assertEqual(profile['cpu'], {'governor': governor, 'energyPerformancePreference': epp, 'noTurbo': turbo})
            self.assertEqual(profile['fan'], self.base['profiles']['shared']['fan'])
            self.assertEqual(profile['odmPowerLimits'], self.base['profiles']['shared']['odmPowerLimits'])
            self.assertNotIn('cpu_temp_limit', str(profile))

    def test_absent_tcc_profiles_are_reported_without_guessing_hardware_limits(self):
        settings, report, _ = migration.convert(self.data, {}, self.caps)
        self.assertEqual(len(report['unresolvedProfiles']), 3)
        self.assertEqual(settings['profiles'], {})
        self.assertEqual(settings['stateMap'], {})

    def test_invalid_or_missing_led_is_rejected(self):
        for colors in ({'55': [1, 2, 3]}, {'0': [256, 0, 0]}, {'0': [True, 0, 0]}):
            with self.subTest(colors=colors), self.assertRaises(ValueError):
                migration.convert({**self.data, 'keyboard_colors': colors}, self.base, self.caps)
        with self.assertRaises(ValueError):
            migration.convert(self.data, self.base, {**self.caps, 'ledIds': [0, 21, 21, 56]})

if __name__ == '__main__':
    unittest.main()
