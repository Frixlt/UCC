// SPDX-License-Identifier: GPL-3.0-or-later

use serde_json::json;
use std::sync::Arc;
use ucc_hardware::{MockSysfs, RealSysfs};
use ucc_lighting::{
    KeyboardCapabilities, LightbarCapabilities, LightbarController, LightbarPowerMode, LightingOp,
    LightingTransport, MockLightingTransport, RgbColor, SysfsLedTransport,
};

#[test]
fn t_hw_14a_sysfs_parse_zone_id_handles_hid_prefixed_kbd_backlight_names() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let base = root.join("sys/bus/hid/drivers/ite_8291/0003:048D:CE00.0001/leds");
    for suffix in ["_10", "_2", "_21", "_5"] {
        let dir = base.join(format!("0003:048D:CE00.0001:rgb:kbd_backlight{suffix}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("max_brightness"), "255\n").unwrap();
        std::fs::write(dir.join("brightness"), "100\n").unwrap();
        std::fs::write(dir.join("multi_intensity"), "0 0 0\n").unwrap();
    }

    let transport = SysfsLedTransport::new(Arc::new(RealSysfs), root);
    let caps = transport
        .detect_keyboard()
        .expect("detect_keyboard must succeed")
        .expect("HID-prefixed per-key keyboard should be detected");
    assert_eq!(caps.zones, 4);
    assert_eq!(
        caps.led_ids,
        vec![2, 5, 10, 21],
        "HID/ITE-prefixed zone IDs must parse numeric suffix rather than fallback indices"
    );
}

#[test]
fn t_hw_14b_detect_lightbar_prefers_rgb_lightbar_over_plain_lightbar() {
    let sysfs = Arc::new(MockSysfs::new());
    for name in ["lightbar", "rgb:lightbar"] {
        let dir = format!("/sys/class/leds/{name}");
        sysfs.set_file(format!("{dir}/max_brightness"), "255\n");
        sysfs.set_file(format!("{dir}/brightness"), "0\n");
        sysfs.set_file(format!("{dir}/multi_intensity"), "0 0 0\n");
    }

    let transport = SysfsLedTransport::new(sysfs.clone(), "/");
    let caps = transport.detect_lightbar().unwrap().unwrap();
    assert_eq!(caps.zones, 1);

    transport
        .write_lightbar_color(RgbColor::new(10, 20, 30))
        .unwrap();
    assert_eq!(
        sysfs
            .get_file("/sys/class/leds/rgb:lightbar/multi_intensity")
            .as_deref(),
        Some("10 20 30"),
        "canonical 'rgb:lightbar' must take priority over plain 'lightbar'"
    );
}

#[test]
fn t_hw_14c_lightbar_reconfigure_resets_animation_state() {
    let mock = Arc::new(MockLightingTransport::new());
    mock.set_keyboard_caps(Some(KeyboardCapabilities {
        zones: 1,
        max_brightness: 255,
        max_red: 255,
        max_green: 255,
        max_blue: 255,
        led_ids: vec![0],
    }));
    mock.set_lightbar_caps(Some(LightbarCapabilities {
        zones: 1,
        max_brightness: 255,
    }));

    let mut lb = LightbarController::new(mock.clone());
    lb.configure(&json!({
        "managed": true,
        "welcome_anim": false,
        "power_settings": {
            "ac": { "enabled": true, "brightness_val": 200, "mode": "Pulse (stays on)", "color_mode": [255, 0, 0] }
        }
    }))
    .unwrap();
    assert!(lb.trigger(true));
    assert!(lb.is_animating());

    // Switching power mode or reconfiguring stops active animation cleanly
    lb.set_power_mode(LightbarPowerMode::Bat);
    assert!(!lb.is_animating());
    let ops = mock.ops();
    assert!(ops
        .iter()
        .any(|op| matches!(op, LightingOp::WriteLightbarBrightness(_))));
}
