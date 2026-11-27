use serde_json::json;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use ucc_core::KeyboardBacklightState;
use ucc_hardware::RealSysfs;
use ucc_lighting::{
    KeyboardBacklightController, KeyboardCapabilities, LightbarAnimation, LightbarCapabilities,
    LightbarController, LightbarPowerMode, LightingError, LightingOp, LightingTransport,
    MockLightingTransport, RgbColor, SysfsLedTransport, MECHREVO_ENTER_LED_ID,
    MECHREVO_EXPECTED_ZONES, MECHREVO_LCTRL_LED_ID, MECHREVO_LSHIFT_LED_ID,
    MECHREVO_PHYSICAL_KEY_COUNT,
};

#[test]
fn mechrevo_physical_led_constants_match_calibrated_layout() {
    assert_eq!(MECHREVO_LCTRL_LED_ID, 0);
    assert_eq!(MECHREVO_LSHIFT_LED_ID, 21);
    assert_eq!(MECHREVO_ENTER_LED_ID, 56);
    assert_eq!(MECHREVO_EXPECTED_ZONES, 126);
    assert_eq!(MECHREVO_PHYSICAL_KEY_COUNT, 101);
}

#[test]
fn sysfs_transport_discovers_per_key_kbd_with_numeric_led_ids_and_lightbar_excluding_kbd() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // Fake ite_8291 per-key HID device with unsorted directory names 0, 56, 21
    let leds_base = root.join("sys/bus/hid/drivers/ite_8291/0003:048D:600B.0001/leds");
    for suffix in ["", "_1", "_2", "_21", "_56"] {
        let p = leds_base.join(format!("rgb:kbd_backlight{suffix}"));
        fs::create_dir_all(&p).unwrap();
        fs::write(p.join("max_brightness"), "255\n").unwrap();
        fs::write(p.join("brightness"), "128\n").unwrap();
        fs::write(p.join("multi_intensity"), "0 128 255\n").unwrap();
    }
    let buf_dir = leds_base.join("rgb:kbd_backlight/device/controls");
    fs::create_dir_all(&buf_dir).unwrap();
    fs::write(buf_dir.join("buffer_input"), "0\n").unwrap();

    // Fake /sys/class/leds with keyboard (must be skipped by lightbar) + rgb:lightbar
    let kbd_class = root.join("sys/class/leds/rgb:kbd_backlight");
    fs::create_dir_all(&kbd_class).unwrap();
    fs::write(kbd_class.join("multi_intensity"), "10 20 30\n").unwrap();

    let lb_dir = root.join("sys/class/leds/rgb:lightbar");
    fs::create_dir_all(&lb_dir).unwrap();
    fs::write(lb_dir.join("max_brightness"), "255\n").unwrap();
    fs::write(lb_dir.join("brightness"), "0\n").unwrap();
    fs::write(lb_dir.join("multi_intensity"), "0 0 0 0 0 0 0 0 0\n").unwrap();

    let transport = Arc::new(SysfsLedTransport::new(Arc::new(RealSysfs), root));
    let mut kbd = KeyboardBacklightController::new(transport.clone());
    let caps_json = kbd.init().unwrap();
    let caps = kbd.capabilities().unwrap();
    assert_eq!(caps.zones, 5);
    assert_eq!(caps.led_ids, vec![0, 1, 2, 21, 56]);
    assert!(caps_json.contains("\"ledIds\":[0,1,2,21,56]"));

    let states: Vec<KeyboardBacklightState> = (0..5)
        .map(|i| KeyboardBacklightState {
            brightness: 200,
            red: (i * 20) as u8,
            green: 100,
            blue: 250,
        })
        .collect();
    kbd.apply_states(&states).unwrap();

    assert_eq!(
        fs::read_to_string(buf_dir.join("buffer_input")).unwrap(),
        "0",
        "BufferInputGuard must release buffer_input back to 0"
    );
    assert_eq!(
        fs::read_to_string(leds_base.join("rgb:kbd_backlight_56/multi_intensity")).unwrap(),
        "80 100 250"
    );

    let lb_caps = transport.detect_lightbar().unwrap().unwrap();
    assert_eq!(
        lb_caps,
        LightbarCapabilities {
            zones: 3,
            max_brightness: 255
        }
    );
    transport
        .write_lightbar_color(RgbColor::new(12, 34, 56))
        .unwrap();
    assert_eq!(
        fs::read_to_string(Path::new(&lb_dir).join("multi_intensity")).unwrap(),
        "12 34 56 12 34 56 12 34 56"
    );
}

#[test]
fn buffer_input_guard_resets_even_when_zone_write_fails() {
    let mock = Arc::new(MockLightingTransport::new());
    mock.set_keyboard_caps(Some(KeyboardCapabilities {
        zones: 2,
        max_brightness: 255,
        max_red: 255,
        max_green: 255,
        max_blue: 255,
        led_ids: vec![0, 1],
    }));
    mock.set_fail_on_keyboard_zones(true);

    let mut kbd = KeyboardBacklightController::new(mock.clone());
    kbd.init().unwrap();
    let err = kbd
        .apply_states(&[KeyboardBacklightState {
            brightness: 100,
            red: 255,
            green: 0,
            blue: 0,
        }])
        .unwrap_err();
    assert!(matches!(err, LightingError::WriteFailed(_)));

    assert_eq!(
        mock.ops(),
        vec![
            LightingOp::SetBufferInput(true),
            LightingOp::WriteKeyboardBrightness(100),
            LightingOp::SetBufferInput(false),
        ]
    );
}

#[test]
fn lightbar_controller_lifecycle_flash_silent_and_auto_majority_color() {
    let mock = Arc::new(MockLightingTransport::new());
    mock.set_lightbar_caps(Some(LightbarCapabilities {
        zones: 3,
        max_brightness: 255,
    }));
    mock.set_keyboard_colors(vec![
        RgbColor::BLACK,
        RgbColor::BLACK,
        RgbColor::BLACK,
        RgbColor::new(0, 180, 255),
        RgbColor::new(0, 180, 255),
        RgbColor::new(255, 0, 0),
    ]);

    let mut lb = LightbarController::new(mock.clone());

    // 1. Default managed=false: configure writes NOTHING
    lb.configure(&json!({ "managed": false })).unwrap();
    assert!(mock.ops().is_empty());

    // Auto majority ignores black when non-black colors exist
    assert_eq!(lb.resolve_color(), RgbColor::new(0, 180, 255));

    // 2. Managed=true + Flash (stays off): clears color to BLACK before brightness
    mock.clear_ops();
    lb.configure(&json!({
        "managed": true,
        "silent_mode": true,
        "welcome_anim": false,
        "power_settings": {
            "ac": {
                "enabled": true,
                "brightness_val": 200,
                "mode": "Flash (stays off)",
                "color_mode": "Auto"
            }
        }
    }))
    .unwrap();
    assert!(lb.is_flash_off());
    assert_eq!(
        mock.ops(),
        vec![
            LightingOp::WriteLightbarColor(RgbColor::BLACK),
            LightingOp::WriteLightbarBrightness(200),
        ]
    );

    // 3. Silent mode blocks non-forced notifications, allows forced Test/Welcome trigger
    assert!(!lb.trigger(false));
    mock.clear_ops();
    assert!(lb.trigger(true));
    assert!(lb.is_animating());
    // Overlapping animation trigger blocked while running
    assert!(!lb.trigger(true));
    // Complete Flash animation -> must end on BLACK and restore flash_off
    lb.advance(LightbarAnimation::Flash.duration_ms());
    assert!(!lb.is_animating());
    assert!(lb.is_flash_off());
    assert_eq!(
        mock.ops().last().cloned(),
        Some(LightingOp::WriteLightbarColor(RgbColor::BLACK))
    );

    // 4. Power mode transition + managed true -> false turn-off
    mock.clear_ops();
    lb.set_power_mode(LightbarPowerMode::Bat); // disabled on bat by default -> black color + brightness 50
    assert_eq!(
        mock.ops(),
        vec![
            LightingOp::WriteLightbarColor(RgbColor::BLACK),
            LightingOp::WriteLightbarBrightness(50),
        ]
    );
    mock.clear_ops();
    lb.configure(&json!({ "managed": false })).unwrap();
    assert_eq!(
        mock.ops(),
        vec![LightingOp::WriteLightbarColor(RgbColor::BLACK)]
    );

    // 5. Validation rejects bad brightness or animation
    assert!(matches!(
        lb.configure(&json!({ "power_settings": { "ac": { "brightness_val": 999 } } })),
        Err(LightingError::InvalidConfig(_))
    ));
    assert!(matches!(
        lb.configure(&json!({ "power_settings": { "ac": { "mode": "Bogus" } } })),
        Err(LightingError::InvalidConfig(_))
    ));
}
