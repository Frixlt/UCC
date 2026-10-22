# SPDX-License-Identifier: GPL-3.0-or-later
"""Read-only conversion of local TCC settings; no JavaScript is executed.

Fan selection and modifier semantics follow TUXEDO Control Center
FanControlBaseClass, FanControlLogic and FanUtils (TUXEDO Computers GmbH,
GPL-3.0-or-later). Runtime preset data is read from the selected installation.
"""
import copy
import json
import math
from pathlib import Path
import re
import struct


def validate_points(points):
    if not isinstance(points, list) or not 2 <= len(points) <= 512:
        raise ValueError('Invalid TCC fan table size')
    previous = -1
    for point in points:
        temp, speed = point['temp'], point['speed']
        if (type(temp) is not int or not previous < temp <= 120 or
                type(speed) is not int or not 0 <= speed <= 100):
            raise ValueError('Invalid TCC fan temperature/speed or point order')
        previous = temp
    return points


def read_fan_presets(archive):
    """Read the installed static preset literal, without evaluating package code.

    ASAR's two Pickle headers precede JSON; entry offsets start at 8+headerSize.
    Only this known packed file is read. Paths from the archive are never written.
    """
    target = 'dist/tuxedo-control-center/service-app/common/models/TccFanTable.js'
    with Path(archive).open('rb') as file:
        prefix = file.read(16)
        if len(prefix) != 16:
            raise ValueError('Truncated TCC archive')
        size, header_size, _, json_size = struct.unpack('<4I', prefix)
        if size != 4 or not 0 < json_size <= 8*1024*1024 or not json_size+8 <= header_size <= json_size+32:
            raise ValueError('Invalid TCC archive header')
        node = json.loads(file.read(json_size))
        for part in target.split('/'):
            node = node['files'][part]
        length, offset = node['size'], int(node['offset'])
        if node.get('unpacked') or type(length) is not int or not 0 < length <= 2*1024*1024 or offset < 0:
            raise ValueError('Invalid TCC fan preset entry')
        file.seek(8+header_size+offset)
        source = file.read(length).decode('utf-8')
        if len(source.encode('utf-8')) != length:
            raise ValueError('Truncated TCC fan presets')
    match = re.search(r'export\s+const\s+defaultFanProfiles\s*=\s*(\[.*?\n\]);', source, re.S)
    if not match:
        raise ValueError('Unsupported TCC fan preset format')
    literal = re.sub(r'\b(name|tableCPU|tableGPU|temp|speed)\s*:', r'"\1":', match[1])
    literal = re.sub(r"'([A-Za-z0-9 _-]+)'", r'"\1"', literal)
    literal = re.sub(r',\s*([}\]])', r'\1', literal)
    presets = {}
    for profile in json.loads(literal):
        name = profile['name']
        if not isinstance(name, str) or name in presets:
            raise ValueError('Invalid or duplicate TCC fan preset name')
        if name == 'Custom' and set(profile) == {'name'}:
            continue
        presets[name] = {key: validate_points(profile[key]) for key in ('tableCPU', 'tableGPU')}
    custom = re.search(r'export\s+const\s+customFanPreset\s*=\s*(\{.*?\n\});', source, re.S)
    if custom:
        literal = re.sub(r'\b(tableCPU|tableGPU|temp|speed)\s*:', r'"\1":', custom[1])
        profile = json.loads(re.sub(r',\s*([}\]])', r'\1', literal))
        presets['Custom'] = {key: validate_points(profile[key]) for key in ('tableCPU', 'tableGPU')}
    if not presets:
        raise ValueError('No TCC fan presets found')
    return presets


def effective_table(points, minimum=0, maximum=100, offset=0):
    """Bake TCC's round(interpolation)+offset/clamp at every integer degree.

    Keeping a 1-degree table preserves clamp boundaries and named-preset steps.
    Runtime filtering, hysteresis and critical-temperature overrides stay UCC's.
    """
    validate_points(points)
    for value, lower, upper in ((minimum, 0, 100), (maximum, 0, 100), (offset, -100, 100)):
        if type(value) is not int or not lower <= value <= upper:
            raise ValueError('Invalid TCC fan modifier')
    result = []
    for temp in range(101):
        if temp <= points[0]['temp']:
            speed = points[0]['speed']
        elif temp >= points[-1]['temp']:
            speed = points[-1]['speed']
        else:
            high = next(i for i, p in enumerate(points) if p['temp'] >= temp)
            a, b = points[high-1], points[high]
            speed = math.floor(a['speed']+(b['speed']-a['speed'])*(temp-a['temp'])/(b['temp']-a['temp'])+0.5)
        result.append({'temp': temp, 'speed': max(0, min(100, max(minimum, min(maximum, speed+offset))))})
    return result


def merge_tcc(settings, tcc_settings, profiles, presets):
    if not isinstance(settings, dict) or not isinstance(tcc_settings, dict) or not isinstance(profiles, list):
        raise ValueError('Invalid TCC settings/profile container')
    result = copy.deepcopy(tcc_settings)
    result.update(copy.deepcopy(settings))
    result['keyboardBacklightControlSupported'] = settings.get(
        'keyboardBacklightControlSupported', tcc_settings.get('keyboardBacklightControlEnabled', True))
    result['profiles'] = {}
    custom_fans = {}
    report = {'profiles': {}, 'runtimeCooling': 'UCC filtering, hysteresis and critical-temperature overrides'}
    for original in profiles:
        profile = copy.deepcopy(original)
        identifier = profile['id']
        if not isinstance(identifier, str) or not identifier or identifier in result['profiles']:
            raise ValueError('Invalid or duplicate TCC profile ID')
        fan = profile['fan']
        name = fan['fanProfile']
        custom = name == 'Custom'
        source = fan.get('customFanCurve', presets.get('Custom')) if custom else presets.get(name)
        if not isinstance(source, dict):
            raise ValueError(f'TCC fan preset {name!r} is unavailable; provide --tcc-archive')
        modifiers = (0, 100, 0) if custom else (fan.get('minimumFanspeed', 0), fan.get('maximumFanspeed', 100), fan.get('offsetFanspeed', 0))
        fan_id = f'frixit-tcc-{identifier}'
        tables = {key: effective_table(source[key], *modifiers) for key in ('tableCPU', 'tableGPU')}
        profile['fan'] = {key: copy.deepcopy(fan[key]) for key in ('useControl', 'sameSpeed', 'autoControlWC', 'enableWaterCooler', 'manualFanSpeed', 'manualPumpVoltage', 'tablePump', 'tableWaterCoolerFan') if key in fan}
        profile['fan'].update(fanProfile=fan_id, **tables)
        profile['fan'].setdefault('sameSpeed', True)
        profile['fan'].setdefault('autoControlWC', False)
        profile['fan'].setdefault('enableWaterCooler', False)
        custom_fans[fan_id] = {'id': fan_id, 'name': f'{profile.get("name", identifier)} (TCC {name})', 'json': tables}
        display = profile.get('display', {})
        if display.get('refreshRate') is None:
            display['refreshRate'] = -1
        elif type(display.get('refreshRate')) is float:
            display['refreshRate'] = math.floor(display['refreshRate']+0.5)
        if 'nvidiaPowerCTRLProfile' in profile:
            offset = profile.pop('nvidiaPowerCTRLProfile').get('cTGPOffset')
            if offset is not None:
                if type(offset) is not int or not 0 <= offset <= 1000:
                    raise ValueError('Invalid TCC cTGP offset')
                profile['nvidiaCTGPOffset'] = offset
        result['profiles'][identifier] = profile
        report['profiles'][identifier] = {'name': profile.get('name'), 'fanPreset': name,
            'fanModifiers': {'minimum': modifiers[0], 'maximum': modifiers[1], 'offset': modifiers[2]},
            'tdpValues': profile.get('odmPowerLimits', {}).get('tdpValues', []),
            'nvidiaCTGPOffset': profile.get('nvidiaCTGPOffset')}
    # Explicit exported UCC profiles and their cooler settings take precedence.
    result['profiles'].update(copy.deepcopy(settings.get('profiles', {})))
    custom_fans.update(copy.deepcopy(settings.get('customFanProfiles', {})))
    result['customFanProfiles'] = custom_fans
    return result, report
