// SPDX-License-Identifier: GPL-3.0-or-later
// Standalone Node/CJS reproduction suite for confirmed GNOME extension findings
// on commit 1d07126 (N23, N24, N25).
// Matches tests/test_gnome_refresh.cjs conventions.
'use strict';

const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const assert = require('node:assert/strict');

function resolveRepoRoot() {
    if (process.env.UCC_REPO_DIR && fs.existsSync(path.join(process.env.UCC_REPO_DIR, 'ucc-gnome/extension.js'))) {
        return process.env.UCC_REPO_DIR;
    }
    const dropInRoot = path.resolve(__dirname, '..');
    if (fs.existsSync(path.join(dropInRoot, 'ucc-gnome/extension.js'))) {
        return dropInRoot;
    }
    const siblingReview = path.resolve(__dirname, '../../ucc_review/UCC-rust-migration');
    if (fs.existsSync(path.join(siblingReview, 'ucc-gnome/extension.js'))) {
        return siblingReview;
    }
    throw new Error('Could not locate ucc-gnome/extension.js (set UCC_REPO_DIR or drop into tests/)');
}

const REPO_ROOT = resolveRepoRoot();

function loadExtensionIndicator() {
    const source = fs.readFileSync(path.join(REPO_ROOT, 'ucc-gnome/extension.js'), 'utf8')
        .replace(/^import .*;\n/gm, '')
        .replace('export default class', 'class');
    const context = vm.createContext({
        GObject: {registerClass: klass => klass},
        PanelMenu: {Button: class {destroy() {}}},
        Extension: class {},
        logError: error => {throw error;},
    });
    vm.runInContext(source + '\nglobalThis.Indicator = UccIndicator;', context);
    return context.Indicator;
}

function loadUccdClient() {
    const source = fs.readFileSync(path.join(REPO_ROOT, 'ucc-gnome/uccdClient.js'), 'utf8')
        .replace(/^import .*;\n/gm, '')
        .replace('export class UccdClient', 'class UccdClient');
    const context = vm.createContext({
        Gio: {
            BusType: {SYSTEM: 0},
            DBusCallFlags: {NONE: 0},
            Cancellable: class {
                cancel() {this._cancelled = true;}
                is_cancelled() {return Boolean(this._cancelled);}
            },
            bus_get_sync() {return {};},
            bus_watch_name_on_connection() {return 1;},
        },
        GLib: {
            Variant: class {
                constructor(sig, val) {this.sig = sig; this.val = val;}
            },
        },
    });
    vm.runInContext(source + '\nglobalThis.UccdClient = UccdClient;', context);
    return context.UccdClient;
}

function makeIndicator(Indicator, client) {
    return Object.assign(new Indicator(), {
        _client: client,
        _pendingReads: new Map(),
        _connectionGeneration: 0,
        _destroyed: false,
        _state: {},
        _capabilitiesLoaded: true,
        _profilesLoaded: true,
    });
}

// N23: GetDisplayBrightness returning sentinel -1 (no backlight device) must
// disable/hide the slider instead of displaying "-1%" and setting value = -0.01.
async function n23_display_brightness_minus_one_disables_slider_instead_of_minus_one_percent() {
    const Indicator = loadExtensionIndicator();
    const client = new Proxy({connected: true, getDisplayBrightness: async () => -1}, {
        get(target, name) {
            return name in target ? target[name] : async () => null;
        },
    });
    const ui = makeIndicator(Indicator, client);
    ui._state = {
        displayBrightness: 50,
        webcamEnabled: false,
        fnLock: false,
        wcEnabled: false,
        waterCoolerSupported: false,
    };
    ui._brightnessSlider = {
        value: 0.5,
        reactive: true,
        visible: true,
        sensitive: true,
        set_reactive(v) {this.reactive = v;},
    };
    ui._brightnessValueLabel = {text: '50%'};

    await ui._pollSlowState();

    assert.notEqual(
        ui._brightnessValueLabel.text,
        '-1%',
        'N23: unavailable display brightness (-1) must not render "-1%"',
    );
    assert.ok(
        ui._brightnessSlider.value >= 0,
        `N23: slider value must not become negative (got ${ui._brightnessSlider.value})`,
    );
    const sliderDisabled = !ui._brightnessSlider.reactive ||
        !ui._brightnessSlider.visible ||
        !ui._brightnessSlider.sensitive;
    assert.ok(
        sliderDisabled,
        'N23: slider must be disabled or hidden when display brightness is -1',
    );
}

// N24: _applyKeyboardProfile must inject keyboardProfileId into the JSON payload
// sent to SetKeyboardBacklightStatesJSON and await the D-Bus result.
async function n24_apply_keyboard_profile_injects_keyboardProfileId_and_awaits_result() {
    const Indicator = loadExtensionIndicator();
    let sentPayload = null;
    let resolveSetter;
    const setterPromise = new Promise(resolve => {resolveSetter = resolve;});

    const ui = makeIndicator(Indicator, {
        connected: true,
        setKeyboardBacklight(payload) {
            sentPayload = payload;
            return setterPromise;
        },
    });
    ui._state = {
        keyboardProfileIds: ['kb-neon'],
        keyboardProfilesData: [{
            id: 'kb-neon',
            name: 'Neon',
            json: {brightness: 40, states: [{mode: 0, red: 255, green: 0, blue: 128}]},
        }],
    };

    const result = ui._applyKeyboardProfile('kb-neon');
    assert.equal(typeof sentPayload, 'string', 'D-Bus keyboard setter must receive a JSON string');

    const parsed = JSON.parse(sentPayload);
    assert.equal(
        parsed.keyboardProfileId,
        'kb-neon',
        'N24: _applyKeyboardProfile must inject keyboardProfileId into D-Bus payload',
    );
    assert.equal(
        typeof result?.then,
        'function',
        'N24: _applyKeyboardProfile must be async and await setKeyboardBacklight result',
    );

    resolveSetter(true);
    await result;
}

// N25: getFanSpeedRPM / getGpuFanSpeedRPM must not fabricate RPM as pct * 60
// when no real tachometer RPM value is provided by the daemon.
async function n25_fan_rpm_does_not_fabricate_pct_times_60() {
    const UccdClient = loadUccdClient();
    const client = new UccdClient();
    client._connected = true;
    client._call = async method => {
        if (method === 'GetFanDataCPU') {
            return {
                speed: {timestamp: 1001, data: 50},
                temp: {timestamp: 1001, data: 64},
            };
        }
        if (method === 'GetFanDataGPU1') {
            return {
                speed: {timestamp: 1001, data: 50},
                temp: {timestamp: 1001, data: 58},
            };
        }
        if (method === 'GetFanDataGPU2') {
            return null;
        }
        return null;
    };

    const cpuRpm = await client.getFanSpeedRPM();
    const gpuRpm = await client.getGpuFanSpeedRPM();

    assert.notEqual(
        cpuRpm,
        50 * 60,
        'N25: getFanSpeedRPM() fabricated 3000 RPM from 50% * 60 without tachometer data',
    );
    assert.notEqual(
        gpuRpm,
        50 * 60,
        'N25: getGpuFanSpeedRPM() fabricated 3000 RPM from 50% * 60 without tachometer data',
    );
    assert.equal(cpuRpm, -1, 'N25: missing CPU tachometer must report unavailable (-1)');
    assert.equal(gpuRpm, -1, 'N25: missing GPU tachometer must report unavailable (-1)');
}

// UccdClient must read realRpm from GetFanDataCPU/GPU1/GPU2 when rpm is absent,
// average GPU1+GPU2 realRpm, and preserve realRpm === 0 instead of falling back to -1.
async function gnome_client_reads_realRpm_key_from_fan_data() {
    const UccdClient = loadUccdClient();
    const client = new UccdClient();
    client._connected = true;

    let fanPayloads = {
        GetFanDataCPU: {
            speed: {timestamp: 1001, data: 55},
            temp: {timestamp: 1001, data: 66},
            realRpm: {timestamp: 1001, data: 3250},
        },
        GetFanDataGPU1: {
            speed: {timestamp: 1001, data: 45},
            temp: {timestamp: 1001, data: 60},
            realRpm: {timestamp: 1001, data: 2800},
        },
        GetFanDataGPU2: {
            speed: {timestamp: 1001, data: 50},
            temp: {timestamp: 1001, data: 59},
            realRpm: {timestamp: 1001, data: 3200},
        },
    };
    client._call = async method => fanPayloads[method] ?? null;

    assert.equal(
        await client.getFanSpeedRPM(),
        3250,
        'getFanSpeedRPM() must read realRpm when rpm key is absent',
    );
    assert.equal(
        await client.getGpuFanSpeedRPM(),
        3000,
        'getGpuFanSpeedRPM() must average GPU1 and GPU2 realRpm values',
    );

    fanPayloads = {
        GetFanDataCPU: {
            speed: {timestamp: 1002, data: 0},
            temp: {timestamp: 1002, data: 40},
            realRpm: {timestamp: 1002, data: 0},
        },
        GetFanDataGPU1: {
            speed: {timestamp: 1002, data: 0},
            temp: {timestamp: 1002, data: 38},
            realRpm: {timestamp: 1002, data: 0},
        },
        GetFanDataGPU2: null,
    };

    assert.equal(
        await client.getFanSpeedRPM(),
        0,
        'getFanSpeedRPM() must preserve realRpm === 0 rather than falling back to -1',
    );
    assert.equal(
        await client.getGpuFanSpeedRPM(),
        0,
        'getGpuFanSpeedRPM() must preserve realRpm === 0 rather than falling back to -1',
    );
}

async function main() {
    const tests = [
        n23_display_brightness_minus_one_disables_slider_instead_of_minus_one_percent,
        n24_apply_keyboard_profile_injects_keyboardProfileId_and_awaits_result,
        n25_fan_rpm_does_not_fabricate_pct_times_60,
        gnome_client_reads_realRpm_key_from_fan_data,
    ];
    let failed = 0;
    for (const fn of tests) {
        try {
            await fn();
            console.log(`PASS: ${fn.name}`);
        } catch (error) {
            failed++;
            console.error(`FAIL: ${fn.name}\n  ${error.message}`);
        }
    }
    if (failed > 0) {
        process.exitCode = 1;
    }
}

if (require.main === module) {
    main();
}

module.exports = {
    n23_display_brightness_minus_one_disables_slider_instead_of_minus_one_percent,
    n24_apply_keyboard_profile_injects_keyboardProfileId_and_awaits_result,
    n25_fan_rpm_does_not_fabricate_pct_times_60,
    gnome_client_reads_realRpm_key_from_fan_data,
};
