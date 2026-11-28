// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::Arc;
use ucc_lighting::{
    frame_windows_feature_report, frame_windows_output_report, unframe_windows_feature_report,
    HidDeviceTransport, LightingError, LightingTransport, MockWinHidSysApi, RgbColor,
    UniwillHidTransport, WinHidDeviceRecord, WindowsHidTransport, ITE_KEYBOARD_PRODUCT_ID,
    ITE_VENDOR_ID, VENDOR_USAGE_PAGE_MIN,
};

const WIN_KBD_BOOT_PATH: &str = r"\\?\hid#vid_048d&pid_600b&mi_00#7&1a2b3c4d&0&0000#{4d1e55b2}";
const WIN_KBD_VENDOR_PATH: &str = r"\\?\hid#vid_048d&pid_600b&mi_01#7&1a2b3c4d&0&0000#{4d1e55b2}";
const WIN_LIGHTBAR_PATH: &str = r"\\?\hid#vid_048d&pid_7000&mi_01#7&5e6f7a8b&0&0000#{4d1e55b2}";

fn sample_ite_windows_devices() -> Vec<WinHidDeviceRecord> {
    vec![
        // Boot keyboard interface (Generic Desktop / Keyboard) — must be skipped.
        WinHidDeviceRecord {
            path: WIN_KBD_BOOT_PATH.to_string(),
            vendor_id: ITE_VENDOR_ID,
            product_id: ITE_KEYBOARD_PRODUCT_ID,
            usage_page: 0x0001,
            usage: 0x0006,
            feature_report_byte_length: 0,
            output_report_byte_length: 0,
        },
        // Keyboard vendor lighting interface (UsagePage 0xFF89, 8B feature + 64B row output).
        WinHidDeviceRecord {
            path: WIN_KBD_VENDOR_PATH.to_string(),
            vendor_id: ITE_VENDOR_ID,
            product_id: ITE_KEYBOARD_PRODUCT_ID,
            usage_page: 0xFF89,
            usage: 0x00CC,
            feature_report_byte_length: 9,
            output_report_byte_length: 65,
        },
        // Lightbar + lid logo vendor interface (048D:7000, UsagePage 0xFF00).
        WinHidDeviceRecord {
            path: WIN_LIGHTBAR_PATH.to_string(),
            vendor_id: ITE_VENDOR_ID,
            product_id: 0x7000,
            usage_page: VENDOR_USAGE_PAGE_MIN,
            usage: 0x0001,
            feature_report_byte_length: 9,
            output_report_byte_length: 0,
        },
    ]
}

#[test]
fn windows_hid_feature_report_framing_prepends_report_id_and_pads() {
    let raw = [0x08, 0x02, 0x01, 0x05, 50, 0, 0, 0];
    let framed = frame_windows_feature_report(&raw, 65).unwrap();
    assert_eq!(framed.len(), 65);
    assert_eq!(framed[0], 0x00);
    assert_eq!(&framed[1..9], &raw);
    assert!(framed[9..].iter().all(|&b| b == 0));

    // Already-framed report starting with Report ID 0x00 is not double-prefixed.
    let preframed = [0x00, 0x08, 0x01, 200, 0, 0, 0, 0, 0];
    let exact = frame_windows_feature_report(&preframed, 9).unwrap();
    assert_eq!(exact, preframed);

    // Unframing strips leading Report ID 0x00 and resizes cleanly.
    let unframed = unframe_windows_feature_report(&framed, 8);
    assert_eq!(unframed, raw);
}

#[test]
fn windows_hid_feature_report_framing_rejects_empty_and_oversized_reports() {
    assert!(matches!(
        frame_windows_feature_report(&[], 65),
        Err(LightingError::InvalidConfig(_))
    ));
    assert!(matches!(
        frame_windows_feature_report(&[0x08, 0x01], 0),
        Err(LightingError::InvalidConfig(_))
    ));
    // 9 payload bytes + 1 Report ID byte = 10 > 9 must fail.
    assert!(matches!(
        frame_windows_feature_report(&[0x14; 9], 9),
        Err(LightingError::InvalidConfig(_))
    ));
    // Pre-framed 10-byte report > 9 must fail.
    let mut oversized_preframed = vec![0x00];
    oversized_preframed.extend_from_slice(&[0x14; 9]);
    assert!(matches!(
        frame_windows_feature_report(&oversized_preframed, 9),
        Err(LightingError::InvalidConfig(_))
    ));
}

#[test]
fn windows_hid_transport_selects_vendor_usage_page_over_boot_keyboard() {
    let sys = Arc::new(MockWinHidSysApi::new(sample_ite_windows_devices()));
    let win_hid = Arc::new(WindowsHidTransport::new(Arc::clone(&sys) as _));

    assert_eq!(win_hid.device_usage_page(WIN_KBD_BOOT_PATH), Some(0x0001));
    assert_eq!(win_hid.device_usage_page(WIN_KBD_VENDOR_PATH), Some(0xFF89));
    assert_eq!(win_hid.device_interface_number(WIN_KBD_VENDOR_PATH), None);

    let lighting = UniwillHidTransport::new(Arc::clone(&win_hid) as _);

    let kbd = lighting.detect_keyboard().unwrap().unwrap();
    assert_eq!(kbd.zones, 126);
    let lb = lighting.detect_lightbar().unwrap().unwrap();
    assert_eq!(lb.zones, 5);

    lighting.set_keyboard_buffer_input(true).unwrap();
    lighting.write_keyboard_brightness(255).unwrap();
    lighting
        .write_keyboard_zones(&[RgbColor::new(10, 20, 30), RgbColor::new(40, 50, 60)])
        .unwrap();
    lighting.write_lightbar_brightness(180).unwrap();
    lighting
        .write_lightbar_color(RgbColor::new(255, 128, 0))
        .unwrap();

    let reports = sys.sent_reports();
    // 14 keyboard reports (1 brightness + 6*(0x16 + 64B row) + 1 latch) + 1 lightbar brightness + 5 lightbar/logo zone reports = 20 total.
    assert_eq!(reports.len(), 20);

    // Every keyboard report targets the vendor lighting interface, never the boot keyboard.
    // Feature reports (9 bytes) alternate with 64-byte planar row output reports (65 bytes).
    for (idx, (path, buf)) in reports[..14].iter().enumerate() {
        assert_eq!(path, WIN_KBD_VENDOR_PATH);
        let expected_len = if (1..=12).contains(&idx) && idx % 2 == 0 {
            65
        } else {
            9
        };
        assert_eq!(buf.len(), expected_len);
        assert_eq!(buf[0], 0x00);
    }
    // Brightness report: [0x00, 0x08, 0x02, 0x33, 0x00, 50, ...]
    assert_eq!(&reports[0].1[1..6], &[0x08, 0x02, 0x33, 0x00, 50]);
    // Row 0 select + planar BGR row report (col 0 = 10,20,30; col 1 = 40,50,60).
    assert_eq!(&reports[1].1[1..4], &[0x16, 0x00, 0x00]);
    assert_eq!(
        (reports[2].1[44], reports[2].1[23], reports[2].1[2]),
        (10, 20, 30)
    );
    assert_eq!(
        (reports[2].1[45], reports[2].1[24], reports[2].1[3]),
        (40, 50, 60)
    );

    // Every lightbar report targets 048D:7000 with exact 9-byte framed buffer.
    for (path, buf) in &reports[14..] {
        assert_eq!(path, WIN_LIGHTBAR_PATH);
        assert_eq!(buf.len(), 9);
        assert_eq!(buf[0], 0x00);
    }
}

#[test]
fn windows_hid_transport_supports_get_feature_report_and_propagates_errors() {
    let sys = Arc::new(MockWinHidSysApi::new(sample_ite_windows_devices()));
    sys.seed_feature_buffer(
        WIN_LIGHTBAR_PATH,
        vec![0x00, 0x08, 0x01, 0xAA, 0x00, 0x00, 0x00, 0x00, 0x00],
    );
    let win_hid = WindowsHidTransport::new(Arc::clone(&sys) as _);

    let read_back = win_hid.get_feature_report(WIN_LIGHTBAR_PATH, 8).unwrap();
    assert_eq!(
        read_back,
        vec![0x08, 0x01, 0xAA, 0x00, 0x00, 0x00, 0x00, 0x00]
    );

    // Missing device returns Unavailable.
    assert!(matches!(
        win_hid.send_feature_report(r"\\?\hid#unknown", &[0x08, 0x01]),
        Err(LightingError::Unavailable)
    ));

    // Simulated HidD_SetFeature / HidD_GetFeature failures propagate as WriteFailed.
    sys.set_fail_writes(true);
    assert!(matches!(
        win_hid.send_feature_report(WIN_LIGHTBAR_PATH, &[0x08, 0x01, 100, 0, 0, 0, 0, 0]),
        Err(LightingError::WriteFailed(_))
    ));
    sys.set_fail_reads(true);
    assert!(matches!(
        win_hid.get_feature_report(WIN_LIGHTBAR_PATH, 8),
        Err(LightingError::WriteFailed(_))
    ));
}

#[test]
fn windows_hid_126_key_calibration_lid_logo_zone5_and_ec_hook() {
    use serde_json::json;
    use std::sync::Mutex;
    use ucc_core::KeyboardBacklightState;
    use ucc_lighting::{
        EcLightbarPowerHook, KeyboardBacklightController, LightbarController,
        LIGHTBAR_LID_LOGO_ZONE_ID, MECHREVO_ENTER_LED_ID, MECHREVO_EXPECTED_ZONES,
        MECHREVO_LCTRL_LED_ID, MECHREVO_LSHIFT_LED_ID,
    };

    let sys = Arc::new(MockWinHidSysApi::new(sample_ite_windows_devices()));
    let win_hid = Arc::new(WindowsHidTransport::new(Arc::clone(&sys) as _));

    let ec_events: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(Vec::new()));
    let ec_cb = Arc::clone(&ec_events);
    let hook: EcLightbarPowerHook = Arc::new(move |enabled| {
        ec_cb
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(enabled);
        Ok(())
    });

    let transport = Arc::new(
        UniwillHidTransport::new(Arc::clone(&win_hid) as _).with_ec_lightbar_power_hook(hook),
    );

    // 1. 126-key calibrated KeyboardBacklightController over WindowsHidTransport.
    let mut kbd = KeyboardBacklightController::new(Arc::clone(&transport) as _);
    kbd.init().expect("init keyboard");
    let caps = kbd.capabilities().expect("keyboard caps");
    assert_eq!(caps.zones, MECHREVO_EXPECTED_ZONES);
    assert_eq!(caps.led_ids[MECHREVO_LCTRL_LED_ID as usize], 0);
    assert_eq!(caps.led_ids[MECHREVO_LSHIFT_LED_ID as usize], 21);
    assert_eq!(caps.led_ids[MECHREVO_ENTER_LED_ID as usize], 56);

    let mut states = vec![
        KeyboardBacklightState {
            brightness: 255,
            red: 1,
            green: 2,
            blue: 3,
        };
        MECHREVO_EXPECTED_ZONES
    ];
    states[MECHREVO_LCTRL_LED_ID as usize] = KeyboardBacklightState {
        brightness: 255,
        red: 11,
        green: 22,
        blue: 33,
    };
    states[MECHREVO_LSHIFT_LED_ID as usize] = KeyboardBacklightState {
        brightness: 255,
        red: 44,
        green: 55,
        blue: 66,
    };
    states[MECHREVO_ENTER_LED_ID as usize] = KeyboardBacklightState {
        brightness: 255,
        red: 77,
        green: 88,
        blue: 99,
    };

    kbd.apply_states(&states).expect("apply 126-key profile");

    let kbd_reports = sys.sent_reports();
    // 1 brightness + 6 * (SET_ROW_INDEX 0x16 + 64B row) + 1 latch = 14.
    assert_eq!(kbd_reports.len(), 14);
    for (idx, (path, buf)) in kbd_reports.iter().enumerate() {
        assert_eq!(path, WIN_KBD_VENDOR_PATH);
        let expected_len = if (1..=12).contains(&idx) && idx % 2 == 0 {
            65
        } else {
            9
        };
        assert_eq!(buf.len(), expected_len);
        assert_eq!(buf[0], 0x00);
    }
    assert_eq!(&kbd_reports[0].1[1..6], &[0x08, 0x02, 0x33, 0x00, 50]);
    assert_eq!(&kbd_reports[13].1[1..6], &[0x08, 0x02, 0x33, 0x00, 50]);

    // LCtrl = LED 0 -> row 0, col 0 (buf[0]=0x00 report id, buf[1]=0x00 pad, B=2, G=23, R=44).
    assert_eq!(
        (kbd_reports[2].1[44], kbd_reports[2].1[23], kbd_reports[2].1[2]),
        (11, 22, 33)
    );
    // LShift = LED 21 -> row 1, col 0.
    assert_eq!(
        (kbd_reports[4].1[44], kbd_reports[4].1[23], kbd_reports[4].1[2]),
        (44, 55, 66)
    );
    // Enter = LED 56 -> row 2, col 14.
    assert_eq!(
        (kbd_reports[6].1[44 + 14], kbd_reports[6].1[23 + 14], kbd_reports[6].1[2 + 14]),
        (77, 88, 99)
    );

    // 2. LightbarController + lid logo zone 5 + write_lightbar_zones + EC hook.
    let mut lb = LightbarController::new(Arc::clone(&transport) as _);
    assert_eq!(lb.status_json()["available"], true);
    assert_eq!(lb.status_json()["zones"], 5);
    lb.configure(&json!({
        "managed": true,
        "welcome_anim": false,
        "power_settings": {
            "ac": {
                "enabled": true,
                "brightness_val": 200,
                "mode": "Pulse (stays on)",
                "color_mode": [10, 20, 30]
            }
        }
    }))
    .expect("configure lightbar");
    assert_eq!(lb.status_json()["ok"], true);
    assert!(transport.has_lid_logo().expect("has_lid_logo"));

    // Turning brightness off disables EC power; lid logo color re-enables EC power.
    transport
        .write_lightbar_brightness(0)
        .expect("lightbar off");
    transport
        .write_lid_logo_color(RgbColor::new(12, 34, 56))
        .expect("write lid logo zone 5");

    let five_zones = [
        RgbColor::new(1, 2, 3),
        RgbColor::new(4, 5, 6),
        RgbColor::new(7, 8, 9),
        RgbColor::new(10, 11, 12),
        RgbColor::new(200, 100, 50),
    ];
    transport
        .write_lightbar_zones(&five_zones)
        .expect("write 5 lightbar zones");

    assert_eq!(
        *ec_events.lock().unwrap_or_else(|e| e.into_inner()),
        vec![true, true, false, true]
    );

    let all_reports = sys.sent_reports();
    // 14 kbd + 5 configure color + 1 configure brightness + 1 off + 1 lid logo + 5 zones = 27.
    assert_eq!(all_reports.len(), 27);
    for (path, buf) in &all_reports[14..] {
        assert_eq!(path, WIN_LIGHTBAR_PATH);
        assert_eq!(buf.len(), 9);
        assert_eq!(buf[0], 0x00);
    }
    let expected_logo = vec![
        0x00,
        0x14,
        0x00,
        LIGHTBAR_LID_LOGO_ZONE_ID,
        12,
        34,
        56,
        0,
        0,
    ];
    assert_eq!(all_reports[21].1, expected_logo);
    assert_eq!(
        all_reports[26].1,
        vec![0x00, 0x14, 0x00, 5, 200, 100, 50, 0, 0]
    );
}

#[test]
fn windows_hid_case_insensitive_cache_and_zero_len_get() {
    let sys = Arc::new(MockWinHidSysApi::new(sample_ite_windows_devices()));
    let win_hid = WindowsHidTransport::new(Arc::clone(&sys) as _);

    // Populate device cache via enumeration, then query with uppercased path.
    let devices = win_hid.enumerate_devices().expect("enumerate");
    assert_eq!(devices.len(), 3);

    let upper_lb_path = WIN_LIGHTBAR_PATH.to_ascii_uppercase();
    assert_eq!(
        win_hid.device_usage_page(&upper_lb_path),
        Some(VENDOR_USAGE_PAGE_MIN)
    );
    win_hid
        .send_feature_report(&upper_lb_path, &[0x08, 0x01, 0x55, 0, 0, 0, 0, 0])
        .expect("case-insensitive send");
    let read_back = win_hid
        .get_feature_report(&upper_lb_path, 8)
        .expect("case-insensitive get");
    assert_eq!(read_back, vec![0x08, 0x01, 0x55, 0, 0, 0, 0, 0]);

    let sent = sys.sent_reports();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, WIN_LIGHTBAR_PATH);

    // Boot keyboard reports feature_report_byte_length == 0 -> must reject get and send.
    assert!(matches!(
        win_hid.get_feature_report(WIN_KBD_BOOT_PATH, 8),
        Err(LightingError::InvalidConfig(_))
    ));
    assert!(matches!(
        win_hid.send_feature_report(WIN_KBD_BOOT_PATH, &[0x08, 0x01]),
        Err(LightingError::InvalidConfig(_))
    ));
}

#[test]
fn windows_hid_unframe_edge_cases_empty_single_and_nonzero_id() {
    assert!(unframe_windows_feature_report(&[], 0).is_empty());
    assert_eq!(unframe_windows_feature_report(&[], 4), vec![0, 0, 0, 0]);

    // Single-byte buffers are not stripped even when byte 0 is 0x00.
    assert_eq!(unframe_windows_feature_report(&[0x00], 1), vec![0x00]);
    assert_eq!(
        unframe_windows_feature_report(&[0x00], 3),
        vec![0x00, 0x00, 0x00]
    );
    assert_eq!(unframe_windows_feature_report(&[0xAB], 1), vec![0xAB]);
    assert_eq!(
        unframe_windows_feature_report(&[0xAB], 3),
        vec![0xAB, 0x00, 0x00]
    );

    // Non-zero leading Report ID is preserved verbatim, with truncation or zero-padding.
    let nonzero = [0x02, 0xAA, 0xBB, 0xCC];
    assert_eq!(
        unframe_windows_feature_report(&nonzero, 4),
        vec![0x02, 0xAA, 0xBB, 0xCC]
    );
    assert_eq!(unframe_windows_feature_report(&nonzero, 2), vec![0x02, 0xAA]);
    assert_eq!(
        unframe_windows_feature_report(&nonzero, 6),
        vec![0x02, 0xAA, 0xBB, 0xCC, 0x00, 0x00]
    );
    assert!(unframe_windows_feature_report(&[0x00, 0x11, 0x22], 0).is_empty());
}

#[cfg(windows)]
#[test]
fn open_best_lighting_transport_windows_smoke() {
    use ucc_lighting::open_best_lighting_transport;

    for prefer_hidraw in [true, false] {
        let transport = open_best_lighting_transport(prefer_hidraw);
        let _ = transport.detect_keyboard().expect("detect_keyboard smoke");
        let _ = transport.detect_lightbar().expect("detect_lightbar smoke");
    }
}

#[test]
fn windows_output_report_framing_preserves_leading_zero_pad_and_validates_bounds() {
    let mut row = [0u8; 64];
    row[0] = 0x00;
    row[1] = 0x33;
    row[22] = 0x22;
    row[43] = 0x11;

    let framed = frame_windows_output_report(&row, 65).expect("frame 64-byte row");
    assert_eq!(framed.len(), 65);
    assert_eq!(framed[0], 0x00);
    assert_eq!(framed[1], 0x00);
    assert_eq!((framed[44], framed[23], framed[2]), (0x11, 0x22, 0x33));

    assert!(matches!(
        frame_windows_output_report(&[], 65),
        Err(LightingError::InvalidConfig(_))
    ));
    assert!(matches!(
        frame_windows_output_report(&row, 0),
        Err(LightingError::InvalidConfig(_))
    ));
    assert!(matches!(
        frame_windows_output_report(&row, 64),
        Err(LightingError::InvalidConfig(_))
    ));

    // Device with OutputReportByteLength == 0 rejects output reports; simulated write errors propagate.
    let sys = Arc::new(MockWinHidSysApi::new(sample_ite_windows_devices()));
    let win_hid = WindowsHidTransport::new(Arc::clone(&sys) as _);
    assert!(matches!(
        win_hid.send_output_report(WIN_KBD_BOOT_PATH, &row),
        Err(LightingError::InvalidConfig(_))
    ));
    sys.set_fail_writes(true);
    assert!(matches!(
        win_hid.send_output_report(WIN_KBD_VENDOR_PATH, &row),
        Err(LightingError::WriteFailed(_))
    ));
}
