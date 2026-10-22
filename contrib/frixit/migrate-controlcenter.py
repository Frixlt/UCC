#!/usr/bin/env python3
"""Prepare an offline UCC import. Never install settings or contact hardware.

The user's Data/ values win over matching UCC/TCC profiles. Full TCC fan/power
profiles must first be exported/converted; unresolved profiles are reported and
never replaced by guessed defaults. Unsupported features remain in pending.json.
"""
import argparse
import copy
import json
import os
from pathlib import Path

CPU = {
    'performance': ('performance', 'performance'),
    'balanced': ('powersave', 'balance_performance'),
    'power-saver': ('powersave', 'power'),
}


def read(path):
    return json.loads(Path(path).read_text(encoding='utf-8'))


def convert(data, settings, capabilities, brightness=None):
    result = copy.deepcopy(settings)
    zones = capabilities['zones']
    maximum = capabilities['maxBrightness']
    if type(zones) is not int or not 1 <= zones <= 1024:
        raise ValueError('Invalid keyboard zone count')
    if type(maximum) is not int or not 1 <= maximum <= 65535:
        raise ValueError('Invalid hardware brightness range')
    leds = capabilities.get('ledIds', list(range(zones)))
    if (len(leds) != zones or any(type(led) is not int or led < 0 for led in leds)
            or len(set(leds)) != zones):
        raise ValueError('Invalid physical LED identities')
    if brightness is None:
        old = settings.get('keyboardBacklightStates', [])
        brightness = old[0].get('brightness', maximum) if old else maximum
    if type(brightness) is not int or not 0 <= brightness <= maximum:
        raise ValueError('Brightness must use hardware units')
    old_states = result.get('keyboardBacklightStates', [])
    states = [copy.deepcopy(old_states[i]) if i < len(old_states) else
              {'red': 0, 'green': 0, 'blue': 0} for i in range(zones)]
    by_led = {led: index for index, led in enumerate(leds)}
    colors = data['keyboard_colors']
    if not isinstance(colors, dict) or not colors:
        raise ValueError('Missing per-key colors')
    for led, rgb in colors.items():
        led_id = int(led)
        if led_id not in by_led:
            raise ValueError(f'Saved LED {led_id} is absent from target capabilities')
        if not isinstance(rgb, list) or len(rgb) != 3 or any(type(c) is not int or not 0 <= c <= 255 for c in rgb):
            raise ValueError(f'Invalid RGB value for LED {led_id}')
        states[by_led[led_id]].update(zip(('red', 'green', 'blue'), rgb))
    for state in states:
        state.update(brightness=brightness, mode=0)
    keyboard_id = 'frixit-keyboard'
    result['keyboardLayout'] = 'frixit-mechrevo'
    result['keyboardBacklightStates'] = states
    result.setdefault('customKeyboardProfiles', {})[keyboard_id] = {
        'id': keyboard_id, 'name': 'Frixit keyboard',
        'json': {'brightness': brightness, 'states': states},
    }
    profiles = result.setdefault('profiles', {})
    mappings = result.setdefault('stateMap', {})
    config = data['config']
    report = {'importedKeys': len(colors), 'importedDevices': len(config.get('devices', [])), 'brightness': brightness,
              'ignored': ['cpu_temp_limit'], 'profiles': {}, 'unresolvedProfiles': []}
    turbo = next((dev for dev in config.get('devices', [])
                  if dev.get('path') == '/sys/devices/system/cpu/intel_pstate/no_turbo'), {})
    for mode in ('ac', 'type_c', 'bat'):
        source = config['settings'][mode]
        source_id = source.get('tuxedo_profile_id')
        base = profiles.get(source_id)
        if base is None:
            report['unresolvedProfiles'].append({'mode': mode, 'tccProfileId': source_id})
            continue
        if isinstance(base, str):
            base = json.loads(base)
        profile = copy.deepcopy(base)
        profile_id = f'frixit-{mode}'
        profile['id'] = profile_id
        profile['name'] = config.get('meta', {}).get('tuxedo_profiles', {}).get(source_id, base.get('name', mode))
        governor, epp = CPU[source.get('cpu_profile', 'balanced')]
        cpu = profile.setdefault('cpu', {})
        cpu.update(governor=governor, energyPerformancePreference=epp)
        if f'{mode}_val' in turbo:
            cpu['noTurbo'] = turbo[f'{mode}_val'] == '1'
        profile['selectedKeyboardProfile'] = keyboard_id
        profiles[profile_id] = profile
        mappings[f'power_{mode}'] = profile_id
        report['profiles'][mode] = profile_id
    per_mode = {}
    for mode in ('ac', 'type_c', 'bat'):
        cpu_name = config['settings'][mode].get('cpu_profile', 'balanced')
        governor, epp = CPU[cpu_name]
        per_mode[mode] = {'governor': governor, 'epp': epp,
                          'aspm': 'powersupersave' if cpu_name == 'power-saver' else 'default'}
    result['powerDevices'] = {
        'enabled': True, 'cpu': per_mode,
        'keepNvidiaAwake': any('GB203' in dev.get('name', '') or 'RTX 5080' in dev.get('name', '')
                              for dev in config.get('devices', [])),
        'devices': copy.deepcopy(config.get('devices', [])),
    }
    result['lightbar'] = copy.deepcopy(data.get('lightbar', {}))
    result['lightbar']['managed'] = True
    pending = {
        'description': 'Preserved source values for features not implemented yet; not daemon settings.',
        'gui': copy.deepcopy(data.get('gui_settings', {})),
        'gpu_run_mode': data.get('gpu_run_mode', 'auto'),
    }
    return result, report, pending


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('data', type=Path)
    parser.add_argument('output', type=Path, help='New private directory; existing paths are refused')
    parser.add_argument('--settings', type=Path, help='Exported/converted UCC settings (optional)')
    parser.add_argument('--capabilities', required=True, type=Path, help='Keyboard capabilities JSON including maxBrightness and zones/ledIds')
    parser.add_argument('--brightness', type=int, help='Hardware brightness; otherwise preserve exported brightness')
    args = parser.parse_args()
    try:
        data = {key: read(args.data / f'{key}.json') for key in
                ('config', 'keyboard_colors', 'lightbar', 'gui_settings')}
        mode_file = args.data / 'gpu_run_mode'
        if mode_file.exists():
            data['gpu_run_mode'] = mode_file.read_text().strip()
        output = convert(data, read(args.settings) if args.settings else {},
                         read(args.capabilities), args.brightness)
        args.output.mkdir(mode=0o700, parents=False, exist_ok=False)
        for name, value in zip(('settings', 'report.json', 'pending.json'), output):
            path = args.output / name
            with path.open('x', encoding='utf-8') as file:
                os.chmod(path, 0o600)
                json.dump(value, file, indent=2, ensure_ascii=False)
                file.write('\n')
        print(json.dumps(output[1], ensure_ascii=False, indent=2))
    except (ValueError, KeyError, OSError, TypeError) as error:
        parser.exit(1, f'Import failed: {error}\n')


if __name__ == '__main__':
    main()
