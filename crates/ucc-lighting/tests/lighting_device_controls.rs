use serde_json::json;
use std::{fs, sync::Arc};
use ucc_core::KeyboardBacklightState;
use ucc_hardware::RealSysfs;
use ucc_lighting::{
    KeyboardBacklightController, KeyboardCapabilities, LightbarCapabilities, LightbarController,
    LightingError, LightingOp, MockLightingTransport, RgbColor, SysfsLedTransport,
    MECHREVO_ENTER_LED_ID, MECHREVO_EXPECTED_ZONES, MECHREVO_LCTRL_LED_ID, MECHREVO_LSHIFT_LED_ID,
};

#[test]
fn keyboard_reports_a_failed_buffer_release() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir
        .path()
        .join("sys/devices/platform/tuxedo_keyboard/leds/rgb:kbd_backlight");
    let ctrl = base.join("device/controls");
    fs::create_dir_all(&ctrl).unwrap();
    fs::create_dir(ctrl.join("buffer_input")).unwrap();
    fs::write(base.join("max_brightness"), "255").unwrap();
    fs::write(base.join("brightness"), "0").unwrap();
    fs::write(base.join("multi_intensity"), "0 0 0").unwrap();
    let mut controller = KeyboardBacklightController::new(Arc::new(SysfsLedTransport::new(
        Arc::new(RealSysfs),
        dir.path(),
    )));
    controller.init().unwrap();
    let state = KeyboardBacklightState {
        brightness: 99,
        red: 255,
        green: 0,
        blue: 0,
    };
    assert!(
        controller.apply_states(&[state]).is_err(),
        "buffer toggles failed, but apply reported success"
    );
    assert!(controller.current_states().is_empty());
}

#[test]
fn keyboard_propagates_buffer_enable_mid_write_and_release_failures() {
    let caps = Some(KeyboardCapabilities {
        zones: 2,
        max_brightness: 255,
        max_red: 255,
        max_green: 255,
        max_blue: 255,
        led_ids: vec![0, 1],
    });
    let st = KeyboardBacklightState {
        brightness: 120,
        red: 10,
        green: 20,
        blue: 30,
    };

    // 1. Enable failure aborts immediately without zone writes
    let mock = Arc::new(MockLightingTransport::new());
    mock.set_keyboard_caps(caps.clone());
    let mut kbd = KeyboardBacklightController::new(Arc::clone(&mock));
    kbd.init().unwrap();
    mock.set_fail_on_buffer_input(true, false);
    assert!(matches!(
        kbd.apply_states(&[st]),
        Err(LightingError::WriteFailed(_))
    ));
    assert!(mock.ops().is_empty());

    // 2. Final release failure still fails apply_states (and does not record current_states)
    mock.clear_ops();
    mock.set_fail_on_buffer_input(false, true);
    assert!(matches!(
        kbd.apply_states(&[st]),
        Err(LightingError::WriteFailed(_))
    ));
    assert!(kbd.current_states().is_empty());
}

#[test]
fn lightbar_reports_failure_to_apply_managed_settings_and_recovers_on_retry_and_resume() {
    let transport = Arc::new(MockLightingTransport::new());
    transport.set_lightbar_caps(Some(LightbarCapabilities {
        zones: 1,
        max_brightness: 255,
    }));
    transport.set_fail_on_lightbar_write(true);
    let mut controller = LightbarController::new(Arc::clone(&transport));
    let result = controller.configure(&json!({"managed": true, "welcome_anim": true}));
    assert!(
        result.is_err(),
        "{result:?} with status {}",
        controller.status_json()
    );
    assert_eq!(controller.status_json()["ok"], false);

    // Clear fault, retry apply_current -> succeeds & status.ok=true
    transport.set_fail_on_lightbar_write(false);
    transport.clear_ops();
    controller.apply_current().unwrap();
    assert_eq!(controller.status_json()["ok"], true);

    // Trigger welcome / verify resume restores idle without triggering welcome again
    transport.clear_ops();
    controller.on_resume().unwrap();
    assert!(!controller.is_animating());
    assert!(transport
        .ops()
        .iter()
        .any(|op| matches!(op, LightingOp::WriteLightbarBrightness(255))));
}

#[test]
fn mechrevo_126_zone_profile_preserves_physical_led_ids_when_a_zone_is_missing() {
    let mock = Arc::new(MockLightingTransport::new());
    // Simulate hardware with 125 zones where LED ID 20 is missing (between LCtrl 0 and LShift 21 / Enter 56)
    let led_ids: Vec<u32> = (0..MECHREVO_EXPECTED_ZONES as u32)
        .filter(|&id| id != 20)
        .collect();
    mock.set_keyboard_caps(Some(KeyboardCapabilities {
        zones: led_ids.len(),
        max_brightness: 255,
        max_red: 255,
        max_green: 255,
        max_blue: 255,
        led_ids: led_ids.clone(),
    }));

    let mut profile_states = vec![
        KeyboardBacklightState {
            brightness: 200,
            red: 0,
            green: 0,
            blue: 0,
        };
        MECHREVO_EXPECTED_ZONES
    ];
    profile_states[MECHREVO_LCTRL_LED_ID as usize].red = 11;
    profile_states[MECHREVO_LSHIFT_LED_ID as usize].green = 22;
    profile_states[MECHREVO_ENTER_LED_ID as usize].blue = 33;

    let mut kbd = KeyboardBacklightController::new(Arc::clone(&mock));
    kbd.init().unwrap();
    kbd.apply_states(&profile_states).unwrap();

    let written = mock
        .ops()
        .into_iter()
        .find_map(|op| match op {
            LightingOp::WriteKeyboardZones(colors) => Some(colors),
            _ => None,
        })
        .unwrap();

    let pos_lctrl = led_ids
        .iter()
        .position(|&id| id == MECHREVO_LCTRL_LED_ID)
        .unwrap();
    let pos_lshift = led_ids
        .iter()
        .position(|&id| id == MECHREVO_LSHIFT_LED_ID)
        .unwrap();
    let pos_enter = led_ids
        .iter()
        .position(|&id| id == MECHREVO_ENTER_LED_ID)
        .unwrap();

    assert_eq!(written[pos_lctrl], RgbColor::new(11, 0, 0));
    assert_eq!(written[pos_lshift], RgbColor::new(0, 22, 0));
    assert_eq!(written[pos_enter], RgbColor::new(0, 0, 33));
}

#[test]
fn compact_keyboard_update_preserves_unmentioned_zone_colors() {
    let dir = tempfile::tempdir().unwrap();
    let leds = dir.path().join("sys/devices/platform/tuxedo_keyboard/leds");
    for suffix in ["", "_1"] {
        let zone = leds.join(format!("rgb:kbd_backlight{suffix}"));
        fs::create_dir_all(&zone).unwrap();
        fs::write(zone.join("max_brightness"), "255").unwrap();
        fs::write(zone.join("brightness"), "20").unwrap();
        fs::write(zone.join("multi_intensity"), "0 20 0").unwrap();
    }
    let transport = Arc::new(SysfsLedTransport::new(Arc::new(RealSysfs), dir.path()));
    let mut keyboard = KeyboardBacklightController::new(transport);
    keyboard.init().unwrap();
    keyboard
        .apply_states(&[KeyboardBacklightState {
            brightness: 20,
            red: 255,
            green: 0,
            blue: 0,
        }])
        .unwrap();
    assert_eq!(
        fs::read_to_string(leds.join("rgb:kbd_backlight_1/multi_intensity")).unwrap(),
        "0 20 0"
    );
}

#[test]
fn sysfs_keyboard_readback_sorts_zones_numerically_and_hydrates_controller() {
    let dir = tempfile::tempdir().unwrap();
    let hid_leds = dir
        .path()
        .join("sys/bus/hid/drivers/tuxedo-keyboard-ite/0003:048D:600B.0001/leds");
    let sys_leds = dir.path().join("sys/class/leds");
    fs::create_dir_all(&sys_leds).unwrap();
    for id in 0..=10_u32 {
        let name = if id == 0 {
            "rgb:kbd_backlight".to_string()
        } else {
            format!("rgb:kbd_backlight_{id}")
        };
        let zone = hid_leds.join(&name);
        fs::create_dir_all(&zone).unwrap();
        fs::write(zone.join("max_brightness"), "50").unwrap();
        fs::write(zone.join("brightness"), "32").unwrap();
        fs::write(zone.join("multi_intensity"), format!("{id} 10 20")).unwrap();
        // Deliberately populate sys/class/leds out of lexicographical order too (`_10` before `_2`)
        let link = sys_leds.join(&name);
        fs::create_dir_all(&link).unwrap();
        fs::write(link.join("brightness"), "32").unwrap();
        fs::write(link.join("multi_intensity"), format!("{id} 10 20")).unwrap();
    }

    let transport = Arc::new(SysfsLedTransport::new(Arc::new(RealSysfs), dir.path()));
    let mut kb = KeyboardBacklightController::new(transport);
    kb.init().unwrap();
    assert!(kb.current_states().is_empty());
    kb.hydrate_from_hardware();
    let states = kb.current_states();
    assert_eq!(states.len(), 11);
    for (idx, st) in states.iter().enumerate() {
        assert_eq!(st.brightness, 32);
        assert_eq!(
            st.red, idx as u8,
            "zone {idx} order corrupted by lexical sort"
        );
        assert_eq!(st.green, 10);
        assert_eq!(st.blue, 20);
    }
}

#[test]
fn sysfs_write_keyboard_zones_commits_brightness_after_multi_intensity() {
    use ucc_hardware::MockSysfs;

    let sysfs = Arc::new(MockSysfs::new());
    let base = std::path::PathBuf::from("/sys/bus/hid/drivers/ite_8291/0003:048D:600B.0004/leds");
    for suffix in ["", "_1", "_2", "_3"] {
        let p = base.join(format!("rgb:kbd_backlight{suffix}"));
        sysfs.set_file(p.join("max_brightness"), "50\n");
        sysfs.set_file(p.join("brightness"), "32\n");
        sysfs.set_file(p.join("multi_intensity"), "0 0 0\n");
    }
    let buf_path = base.join("rgb:kbd_backlight/device/controls/buffer_input");
    sysfs.set_file(&buf_path, "0\n");

    let transport = Arc::new(SysfsLedTransport::new(Arc::clone(&sysfs), "/"));
    let mut kbd = KeyboardBacklightController::new(transport);
    kbd.init().unwrap();
    sysfs.clear_journal();

    let states: Vec<KeyboardBacklightState> = (0..4)
        .map(|i| KeyboardBacklightState {
            brightness: 31,
            red: (i * 50) as u8,
            green: 10,
            blue: 200,
        })
        .collect();
    kbd.apply_states(&states).unwrap();

    let journal = sysfs.journal();
    let entries: Vec<(std::path::PathBuf, String)> =
        journal.into_iter().map(|w| (w.path, w.content)).collect();

    assert_eq!(entries.first(), Some(&(buf_path.clone(), "1".to_string())));
    assert_eq!(entries.last(), Some(&(buf_path, "0".to_string())));

    // After the initial brightness pass, every zone must write multi_intensity and then commit brightness=31 before buffer_input=0.
    let commit_slice = &entries[5..entries.len() - 1];
    assert_eq!(commit_slice.len(), 8);
    for (idx, chunk) in commit_slice.as_chunks::<2>().0.iter().enumerate() {
        let suffix = if idx == 0 {
            String::new()
        } else {
            format!("_{idx}")
        };
        let zone = base.join(format!("rgb:kbd_backlight{suffix}"));
        assert_eq!(chunk[0].0, zone.join("multi_intensity"));
        assert_eq!(chunk[1], (zone.join("brightness"), "31".to_string()));
    }
}

#[test]
fn keyboard_apply_states_rolls_back_brightness_on_zone_failure() {
    let mock = Arc::new(MockLightingTransport::new());
    mock.set_keyboard_caps(Some(KeyboardCapabilities {
        zones: 2,
        max_brightness: 255,
        max_red: 255,
        max_green: 255,
        max_blue: 255,
        led_ids: vec![0, 1],
    }));

    let mut kbd = KeyboardBacklightController::new(
        Arc::clone(&mock) as Arc<dyn ucc_lighting::LightingTransport>
    );
    kbd.init().expect("init keyboard");

    let initial = [
        KeyboardBacklightState {
            brightness: 80,
            red: 10,
            green: 20,
            blue: 30,
        },
        KeyboardBacklightState {
            brightness: 80,
            red: 40,
            green: 50,
            blue: 60,
        },
    ];
    kbd.apply_states(&initial).expect("apply initial states");
    assert_eq!(mock.read_keyboard_brightness_state(), Some(80));

    mock.clear_ops();
    mock.set_fail_on_keyboard_zones(true);

    let err = kbd
        .apply_states(&[KeyboardBacklightState {
            brightness: 220,
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
            LightingOp::WriteKeyboardBrightness(220),
            LightingOp::WriteKeyboardBrightness(80),
            LightingOp::SetBufferInput(false),
        ]
    );
    assert_eq!(mock.read_keyboard_brightness_state(), Some(80));
    assert_eq!(kbd.current_states(), &initial);
}

#[test]
fn keyboard_apply_states_json_and_profile_json_validation() {
    let mock = Arc::new(MockLightingTransport::new());
    let mut kbd = KeyboardBacklightController::new(
        Arc::clone(&mock) as Arc<dyn ucc_lighting::LightingTransport>
    );

    // Before init (no capabilities): default states JSON is "[]" and apply fails with Unavailable.
    assert_eq!(kbd.build_default_states_json(), "[]");
    assert!(matches!(
        kbd.apply_states_json(r#"[{"brightness":10,"red":1,"green":2,"blue":3}]"#),
        Err(LightingError::Unavailable)
    ));

    mock.set_keyboard_caps(Some(KeyboardCapabilities {
        zones: 2,
        max_brightness: 50,
        max_red: 255,
        max_green: 255,
        max_blue: 255,
        led_ids: vec![0, 1],
    }));
    kbd.init().expect("init keyboard");

    // build_default_states_json produces one state per zone capped at max_brightness.
    let defaults_json = kbd.build_default_states_json();
    let parsed_defaults: serde_json::Value =
        serde_json::from_str(&defaults_json).expect("valid default states json");
    assert_eq!(
        parsed_defaults,
        json!([
            { "brightness": 50, "red": 255, "green": 255, "blue": 255 },
            { "brightness": 50, "red": 255, "green": 255, "blue": 255 }
        ])
    );
    kbd.apply_states_json(&defaults_json)
        .expect("apply default states json");
    assert_eq!(kbd.current_states().len(), 2);

    // Malformed and empty JSON rejected for apply_states_json.
    for bad in ["{not valid", r#"{"brightness":10}"#, "[]"] {
        assert!(matches!(
            kbd.apply_states_json(bad),
            Err(LightingError::InvalidConfig(_))
        ));
    }

    // Out-of-range brightness and RGB channels are clamped cleanly.
    kbd.apply_states_json(
        r#"[
            {"brightness": -10, "red": -5, "green": 300, "blue": 100},
            {"brightness": 999, "red": 500, "green": -1, "blue": 42}
        ]"#,
    )
    .expect("clamped states json");
    assert_eq!(
        kbd.current_states(),
        &[
            KeyboardBacklightState {
                brightness: 0,
                red: 0,
                green: 255,
                blue: 100,
            },
            KeyboardBacklightState {
                brightness: 50,
                red: 255,
                green: 0,
                blue: 42,
            },
        ]
    );

    // Profile JSON validation: malformed, missing "states", empty "states", non-array "states".
    for bad_profile in [
        "not json",
        "{}",
        r#"{"states":[]}"#,
        r#"{"states":"invalid"}"#,
    ] {
        assert!(matches!(
            kbd.apply_profile_keyboard_json(bad_profile),
            Err(LightingError::InvalidConfig(_))
        ));
    }

    // Valid profile JSON applies and clamps brightness to max_brightness (50).
    kbd.apply_profile_keyboard_json(
        r#"{"states":[{"brightness":999,"red":12,"green":34,"blue":56}]}"#,
    )
    .expect("apply valid profile json");
    assert_eq!(
        kbd.current_states()[0],
        KeyboardBacklightState {
            brightness: 50,
            red: 12,
            green: 34,
            blue: 56,
        }
    );
}
