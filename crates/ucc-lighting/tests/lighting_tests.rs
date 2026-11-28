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
    let mut kbd = KeyboardBacklightController::new(Arc::clone(&transport));
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

    let mut kbd = KeyboardBacklightController::new(Arc::clone(&mock));
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

    let mut lb = LightbarController::new(Arc::clone(&mock));

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

#[test]
fn lightbar_welcome_skip_shutdown_and_keyboard_changed_guards() {
    let mock = Arc::new(MockLightingTransport::new());
    mock.set_lightbar_caps(Some(LightbarCapabilities {
        zones: 4,
        max_brightness: 255,
    }));
    mock.set_keyboard_colors(vec![RgbColor::new(10, 20, 30)]);

    // 1. One-shot welcome_anim triggers on first managed configure, not on subsequent configures.
    let mut lb = LightbarController::new(Arc::clone(&mock));
    lb.configure(&json!({
        "managed": true,
        "welcome_anim": true,
        "power_settings": {
            "ac": {
                "enabled": true,
                "brightness_val": 180,
                "mode": "Pulse (stays on)",
                "color_mode": "Auto"
            }
        }
    }))
    .expect("first managed configure triggers welcome");
    assert!(lb.is_animating());

    lb.advance(LightbarAnimation::Pulse.duration_ms());
    assert!(!lb.is_animating());

    lb.configure(&json!({
        "managed": true,
        "welcome_anim": true,
        "power_settings": {
            "ac": {
                "enabled": true,
                "brightness_val": 180,
                "mode": "Pulse (stays on)",
                "color_mode": "Auto"
            }
        }
    }))
    .expect("second managed configure must not replay welcome");
    assert!(!lb.is_animating());

    // 2. skip_welcome() suppresses welcome_anim even on the first managed configure.
    let mut lb_skipped = LightbarController::new(Arc::clone(&mock));
    lb_skipped.skip_welcome();
    lb_skipped
        .configure(&json!({
            "managed": true,
            "welcome_anim": true
        }))
        .expect("configure after skip_welcome");
    assert!(!lb_skipped.is_animating());

    // 3. keyboard_changed() updates idle color when managed + Auto + idle + not flash_off.
    mock.set_keyboard_colors(vec![RgbColor::new(50, 100, 150)]);
    mock.clear_ops();
    lb.keyboard_changed();
    assert_eq!(
        mock.ops(),
        vec![
            LightingOp::WriteLightbarColor(RgbColor::new(50, 100, 150)),
            LightingOp::WriteLightbarBrightness(180),
        ]
    );

    // Guard: keyboard_changed() is a no-op while animating.
    assert!(lb.trigger(true));
    assert!(lb.is_animating());
    mock.clear_ops();
    lb.keyboard_changed();
    assert!(mock.ops().is_empty());
    lb.advance(LightbarAnimation::Pulse.duration_ms());
    assert!(!lb.is_animating());

    // Guard: keyboard_changed() is a no-op when flash_off is active.
    lb.configure(&json!({
        "managed": true,
        "welcome_anim": false,
        "power_settings": {
            "ac": {
                "enabled": true,
                "brightness_val": 180,
                "mode": "Flash (stays off)",
                "color_mode": "Auto"
            }
        }
    }))
    .expect("configure flash");
    assert!(lb.is_flash_off());
    mock.clear_ops();
    lb.keyboard_changed();
    assert!(mock.ops().is_empty());

    // Guard: keyboard_changed() is a no-op when color_mode is Fixed.
    lb.configure(&json!({
        "managed": true,
        "welcome_anim": false,
        "power_settings": {
            "ac": {
                "enabled": true,
                "brightness_val": 180,
                "mode": "Pulse (stays on)",
                "color_mode": [1, 2, 3]
            }
        }
    }))
    .expect("configure fixed");
    mock.clear_ops();
    lb.keyboard_changed();
    assert!(mock.ops().is_empty());

    // 4. shutdown() stops animation, marks unmanaged, and turns off lightbar once.
    assert!(lb.trigger(true));
    assert!(lb.is_animating());
    mock.clear_ops();
    lb.shutdown().expect("shutdown managed lightbar");
    assert!(!lb.is_animating());
    assert_eq!(lb.status_json()["managed"], false);
    assert_eq!(
        mock.ops(),
        vec![LightingOp::WriteLightbarColor(RgbColor::BLACK)]
    );

    // Guard: second shutdown() and keyboard_changed() after shutdown are no-ops.
    mock.clear_ops();
    lb.shutdown().expect("second shutdown no-op");
    lb.keyboard_changed();
    assert!(mock.ops().is_empty());
}

#[test]
fn lightbar_settings_normalize_json_invalid_types_and_roundtrip() {
    use ucc_lighting::{LightbarColorMode, LightbarSettings};

    let invalid_inputs = [
        json!([]),
        json!("not-an-object"),
        json!(42),
        json!({ "managed": "true" }),
        json!({ "silent_mode": 1 }),
        json!({ "welcome_anim": null }),
        json!({ "power_settings": [] }),
        json!({ "power_settings": { "unknown_mode": {} } }),
        json!({ "power_settings": { "ac": "not-an-object" } }),
        json!({ "power_settings": { "ac": { "enabled": 1 } } }),
        json!({ "power_settings": { "ac": { "brightness_val": true } } }),
        json!({ "power_settings": { "ac": { "brightness_val": "200" } } }),
        json!({ "power_settings": { "ac": { "brightness_val": -1 } } }),
        json!({ "power_settings": { "ac": { "brightness_val": 256 } } }),
        json!({ "power_settings": { "ac": { "brightness_val": 12.5 } } }),
        json!({ "power_settings": { "ac": { "mode": 123 } } }),
        json!({ "power_settings": { "ac": { "mode": "Bogus" } } }),
        json!({ "power_settings": { "ac": { "color_mode": "Manual" } } }),
        json!({ "power_settings": { "ac": { "color_mode": [1, 2] } } }),
        json!({ "power_settings": { "ac": { "color_mode": [1, 2, 3, 4] } } }),
        json!({ "power_settings": { "ac": { "color_mode": [true, 0, 0] } } }),
        json!({ "power_settings": { "ac": { "color_mode": ["255", 0, 0] } } }),
        json!({ "power_settings": { "ac": { "color_mode": [-1, 0, 0] } } }),
        json!({ "power_settings": { "ac": { "color_mode": [0, 256, 0] } } }),
    ];

    for bad in &invalid_inputs {
        assert!(
            matches!(
                LightbarSettings::normalize_json(bad),
                Err(LightingError::InvalidConfig(_))
            ),
            "expected InvalidConfig for {bad}"
        );
    }

    let valid_input = json!({
        "managed": true,
        "silent_mode": true,
        "welcome_anim": true,
        "power_settings": {
            "ac": {
                "enabled": true,
                "brightness_val": 210,
                "mode": "Rainbow",
                "color_mode": [10, 20, 30]
            },
            "type_c": {
                "enabled": true,
                "brightness_val": 120,
                "mode": "Police Strobe",
                "color_mode": "Auto"
            },
            "bat": {
                "enabled": false,
                "brightness_val": 40,
                "mode": "Disco",
                "color_mode": [0, 255, 128]
            }
        }
    });

    let parsed = LightbarSettings::normalize_json(&valid_input).expect("valid settings");
    assert!(parsed.managed);
    assert!(parsed.silent_mode);
    assert!(parsed.welcome_anim);
    assert_eq!(parsed.ac.brightness_val, 210);
    assert_eq!(parsed.ac.mode, LightbarAnimation::Rainbow);
    assert_eq!(
        parsed.ac.color_mode,
        LightbarColorMode::Fixed(RgbColor::new(10, 20, 30))
    );
    assert_eq!(parsed.type_c.mode, LightbarAnimation::PoliceStrobe);
    assert_eq!(parsed.type_c.color_mode, LightbarColorMode::Auto);
    assert!(!parsed.bat.enabled);
    assert_eq!(parsed.bat.mode, LightbarAnimation::Disco);
    assert_eq!(
        parsed.bat.color_mode,
        LightbarColorMode::Fixed(RgbColor::new(0, 255, 128))
    );

    let roundtripped_json = parsed.to_json();
    let reparsed = LightbarSettings::normalize_json(&roundtripped_json).expect("roundtrip parse");
    assert_eq!(parsed, reparsed);
    assert_eq!(roundtripped_json, valid_input);
}
