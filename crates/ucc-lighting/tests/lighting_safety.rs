// SPDX-License-Identifier: GPL-3.0-or-later

use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use ucc_core::KeyboardBacklightState;
use ucc_hardware::{MockSysfs, RealSysfs};
use ucc_lighting::{
    KeyboardBacklightController, KeyboardCapabilities, LightbarCapabilities, LightbarController,
    LightbarPowerMode, LightingError, LightingOp, LightingTransport, MockLightingTransport,
    RgbColor, SysfsLedTransport,
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

    let transport = SysfsLedTransport::new(Arc::clone(&sysfs) as _, "/");
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

    let mut lb = LightbarController::new(Arc::clone(&mock) as _);
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

struct CountingTransport {
    inner: MockLightingTransport,
    color_reads: AtomicUsize,
    brightness_reads: AtomicUsize,
}

impl LightingTransport for CountingTransport {
    fn detect_keyboard(&self) -> Result<Option<KeyboardCapabilities>, LightingError> {
        self.inner.detect_keyboard()
    }
    fn set_keyboard_buffer_input(&self, enabled: bool) -> Result<(), LightingError> {
        self.inner.set_keyboard_buffer_input(enabled)
    }
    fn read_keyboard_brightness(&self) -> Result<Option<u32>, LightingError> {
        self.brightness_reads.fetch_add(1, Ordering::SeqCst);
        self.inner.read_keyboard_brightness()
    }
    fn write_keyboard_brightness(&self, brightness: u32) -> Result<(), LightingError> {
        self.inner.write_keyboard_brightness(brightness)
    }
    fn write_keyboard_zones(&self, colors: &[RgbColor]) -> Result<(), LightingError> {
        self.inner.write_keyboard_zones(colors)
    }
    fn read_keyboard_colors(&self) -> Result<Vec<RgbColor>, LightingError> {
        self.color_reads.fetch_add(1, Ordering::SeqCst);
        self.inner.read_keyboard_colors()
    }
    fn detect_lightbar(&self) -> Result<Option<LightbarCapabilities>, LightingError> {
        self.inner.detect_lightbar()
    }
    fn write_lightbar_brightness(&self, brightness: u32) -> Result<(), LightingError> {
        self.inner.write_lightbar_brightness(brightness)
    }
    fn write_lightbar_color(&self, rgb: RgbColor) -> Result<(), LightingError> {
        self.inner.write_lightbar_color(rgb)
    }
}

#[test]
fn t_lighting_snapshot_reads_live_hardware_after_external_change() {
    let inner = MockLightingTransport::new();
    inner.set_keyboard_caps(Some(KeyboardCapabilities {
        zones: 3,
        max_brightness: 50,
        max_red: 255,
        max_green: 255,
        max_blue: 255,
        led_ids: vec![0, 1, 2],
    }));
    let transport = Arc::new(CountingTransport {
        inner,
        color_reads: AtomicUsize::new(0),
        brightness_reads: AtomicUsize::new(0),
    });

    // Cold controller performs live hardware readback.
    let mut controller = KeyboardBacklightController::new(Arc::clone(&transport) as _);
    assert!(controller.snapshot_hardware_state().is_some());
    assert_eq!(transport.color_reads.load(Ordering::SeqCst), 1);
    assert_eq!(transport.brightness_reads.load(Ordering::SeqCst), 1);

    // Warm controller must still read live hardware so external changes are reflected accurately (N05).
    let states = vec![
        KeyboardBacklightState {
            brightness: 32,
            red: 10,
            green: 20,
            blue: 30,
        },
        KeyboardBacklightState {
            brightness: 32,
            red: 40,
            green: 50,
            blue: 60,
        },
        KeyboardBacklightState {
            brightness: 32,
            red: 70,
            green: 80,
            blue: 90,
        },
    ];
    controller.apply_states(&states).unwrap();
    transport.inner.write_keyboard_brightness(18).unwrap();
    transport.inner.set_keyboard_colors(vec![
        RgbColor::new(1, 2, 3),
        RgbColor::new(4, 5, 6),
        RgbColor::new(7, 8, 9),
    ]);
    transport.color_reads.store(0, Ordering::SeqCst);
    transport.brightness_reads.store(0, Ordering::SeqCst);

    let snap = controller.snapshot_hardware_state().unwrap();
    assert_eq!(transport.color_reads.load(Ordering::SeqCst), 1);
    assert_eq!(transport.brightness_reads.load(Ordering::SeqCst), 1);
    assert_eq!(snap.brightness, Some(18));
    assert_eq!(
        snap.colors,
        vec![
            RgbColor::new(1, 2, 3),
            RgbColor::new(4, 5, 6),
            RgbColor::new(7, 8, 9),
        ]
    );
}
