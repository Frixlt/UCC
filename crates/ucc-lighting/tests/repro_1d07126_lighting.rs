// SPDX-License-Identifier: GPL-3.0-or-later
//! Reproduction integration tests for confirmed `ucc-lighting` findings on commit `1d07126`:
//! - N05: `KeyboardBacklightController::snapshot_hardware_state` returns stale cached state
//!   instead of live hardware brightness and RGB values mutated externally.
//! - N17: `SysfsLedTransport::detect_keyboard` ignores 1..=3-zone HID RGB keyboards because
//!   it requires `per_key.len() > 3`.

use std::fs;
use std::sync::Arc;
use tempfile::TempDir;
use ucc_core::KeyboardBacklightState;
use ucc_hardware::RealSysfs;
use ucc_lighting::{
    KeyboardBacklightController, KeyboardCapabilities, LightingTransport, MockLightingTransport,
    RgbColor, SysfsLedTransport,
};

#[test]
fn n05_snapshot_hardware_state_reads_live_hardware_not_stale_cache() {
    let mock = Arc::new(MockLightingTransport::new());
    mock.set_keyboard_caps(Some(KeyboardCapabilities {
        zones: 1,
        max_brightness: 255,
        max_red: 255,
        max_green: 255,
        max_blue: 255,
        led_ids: vec![0],
    }));

    let mut controller = KeyboardBacklightController::new(Arc::clone(&mock) as _);
    controller.init().unwrap();

    // Populate controller cache with brightness=70, RGB=(1, 2, 3).
    controller
        .apply_states(&[KeyboardBacklightState {
            brightness: 70,
            red: 1,
            green: 2,
            blue: 3,
        }])
        .unwrap();

    // Mutate mock hardware externally (outside the controller) to brightness=30, RGB=(4, 5, 6).
    mock.write_keyboard_brightness(30).unwrap();
    mock.set_keyboard_colors(vec![RgbColor::new(4, 5, 6)]);

    let snapshot = controller
        .snapshot_hardware_state()
        .expect("snapshot_hardware_state must return Some when keyboard is available");

    assert_eq!(
        snapshot.brightness,
        Some(30),
        "N05: snapshot brightness must reflect live hardware (30), not stale cache (70)"
    );
    assert_eq!(
        snapshot.colors,
        vec![RgbColor::new(4, 5, 6)],
        "N05: snapshot colors must reflect live hardware RGB(4, 5, 6), not stale cache RGB(1, 2, 3)"
    );
    assert_eq!(
        snapshot.states,
        vec![KeyboardBacklightState {
            brightness: 30,
            red: 4,
            green: 5,
            blue: 6,
        }],
        "N05: snapshot states must reflect live hardware (30, RGB(4, 5, 6)), not stale cached state"
    );
}

#[test]
fn n17_sysfs_detect_keyboard_recognizes_1_to_3_hid_zones() {
    let tmp = TempDir::new().unwrap();
    let led_dir = tmp
        .path()
        .join("sys/bus/hid/drivers/ite_8291/0003:048D:600B.0001/leds/rgb:kbd_backlight");
    fs::create_dir_all(&led_dir).unwrap();
    fs::write(led_dir.join("max_brightness"), "255\n").unwrap();
    fs::write(led_dir.join("brightness"), "128\n").unwrap();
    fs::write(led_dir.join("multi_intensity"), "255 128 0\n").unwrap();

    let transport = SysfsLedTransport::new(Arc::new(RealSysfs), tmp.path());
    let detected = transport.detect_keyboard().unwrap();

    let caps = detected
        .expect("N17: SysfsLedTransport::detect_keyboard must recognize a 1-zone HID RGB keyboard");
    assert_eq!(
        caps.zones, 1,
        "N17: detected HID keyboard capabilities must report zones == 1"
    );
}
