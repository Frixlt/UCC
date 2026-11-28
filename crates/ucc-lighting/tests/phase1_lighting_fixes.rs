// SPDX-License-Identifier: GPL-3.0-or-later

//! Regression tests for Phase 1.5 `ucc-lighting` fixes:
//! - HID interface filtering (`0xFF89` exclusion, `0xFF03 > 0xFF12 > 0xFF02` ranking,
//!   `048D:7001` `usage == 0x0002`, multi-collection descriptor parsing, no `true` fallback)
//! - `048D:7001` Zone 2 power-on/off packets and forced `power_on` on startup/`on_resume()`
//! - `048D:7000` `0xA2` topology probing & safe fallback
//! - `048D:CE00` 4-zone (`0xFF12`) vs per-key rev `0.03` (`version_number` / `output_report_byte_length`)
//! - Unified framing, transient `EAGAIN`/`ETIMEDOUT` retry, Windows cache invalidation,
//!   `33 ms` animation interval, `advance()` error reporting, and `KeyboardBacklightController::on_resume()`

use serde_json::json;
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use ucc_core::KeyboardBacklightState;
use ucc_lighting::{
    decode_ite7000_topology, frame_windows_feature_report, parse_hid_report_descriptor_usage_page,
    parse_hid_report_descriptor_usage_pages, usage_page_priority, HidDeviceInfo,
    HidDeviceTransport, HidrawDeviceTransport, Ite7000Zone, KeyboardBacklightController,
    KeyboardCapabilities, LightbarAnimation, LightbarCapabilities, LightbarController,
    LightingError, LightingOp, LightingTransport, MockHidTransport, MockLightingTransport,
    MockWinHidSysApi, RgbColor, UniwillHidTransport, WinHidDeviceRecord, WinHidSysApi,
    WindowsHidTransport, ITE_KEYBOARD_PRODUCT_ID, ITE_KEYBOARD_PRODUCT_ID_CE00,
    ITE_LIGHTBAR_PRODUCT_ID_4ZONE, ITE_LIGHTBAR_PRODUCT_ID_5ZONE, ITE_USAGE_PAGE_4ZONE,
    ITE_USAGE_PAGE_EXCLUDED_FF89, ITE_USAGE_PAGE_LEGACY, ITE_USAGE_PAGE_PER_KEY, ITE_VENDOR_ID,
    LIGHTBAR_FRAME_INTERVAL,
};

#[test]
fn ff89_excluded_and_usage_page_priority_ranked() {
    assert_eq!(usage_page_priority(ITE_USAGE_PAGE_EXCLUDED_FF89), 0);
    assert_eq!(usage_page_priority(0x0001), 0);
    assert!(
        usage_page_priority(ITE_USAGE_PAGE_PER_KEY) > usage_page_priority(ITE_USAGE_PAGE_4ZONE)
    );
    assert!(usage_page_priority(ITE_USAGE_PAGE_4ZONE) > usage_page_priority(ITE_USAGE_PAGE_LEGACY));
    assert!(usage_page_priority(ITE_USAGE_PAGE_LEGACY) > usage_page_priority(0xFF00));

    // Only 0xFF89 present on 048D:600B -> must be rejected!
    let mock_only_ff89 = Arc::new(MockHidTransport::new(vec![HidDeviceInfo {
        vendor_id: ITE_VENDOR_ID,
        product_id: ITE_KEYBOARD_PRODUCT_ID,
        path: "/dev/hidraw0".to_string(),
    }]));
    mock_only_ff89.set_device_usage_page("/dev/hidraw0", ITE_USAGE_PAGE_EXCLUDED_FF89);
    let t_ff89 = UniwillHidTransport::new(Arc::clone(&mock_only_ff89) as _);
    assert!(t_ff89.detect_keyboard().unwrap().is_none());

    // Ranked selection: 0xFF89, 0xFF02, 0xFF12, 0xFF03 -> must select 0xFF03 regardless of order.
    let mock_ranked = Arc::new(MockHidTransport::new(vec![
        HidDeviceInfo {
            vendor_id: ITE_VENDOR_ID,
            product_id: ITE_KEYBOARD_PRODUCT_ID,
            path: "/dev/hidraw0".to_string(),
        },
        HidDeviceInfo {
            vendor_id: ITE_VENDOR_ID,
            product_id: ITE_KEYBOARD_PRODUCT_ID,
            path: "/dev/hidraw1".to_string(),
        },
        HidDeviceInfo {
            vendor_id: ITE_VENDOR_ID,
            product_id: ITE_KEYBOARD_PRODUCT_ID,
            path: "/dev/hidraw2".to_string(),
        },
        HidDeviceInfo {
            vendor_id: ITE_VENDOR_ID,
            product_id: ITE_KEYBOARD_PRODUCT_ID,
            path: "/dev/hidraw3".to_string(),
        },
    ]));
    mock_ranked.set_device_usage_page("/dev/hidraw0", ITE_USAGE_PAGE_EXCLUDED_FF89);
    mock_ranked.set_device_usage_page("/dev/hidraw1", ITE_USAGE_PAGE_LEGACY);
    mock_ranked.set_device_usage_page("/dev/hidraw2", ITE_USAGE_PAGE_4ZONE);
    mock_ranked.set_device_usage_page("/dev/hidraw3", ITE_USAGE_PAGE_PER_KEY);

    let t_ranked = UniwillHidTransport::new(Arc::clone(&mock_ranked) as _);
    t_ranked.write_keyboard_brightness(255).unwrap();
    assert_eq!(mock_ranked.recorded_reports()[0].0, "/dev/hidraw3");

    // Windows 048D:7001 prefers usage == 0x0002 over usage == 0x0001 on 0xFF03, and excludes 0xFF89.
    let win_records = vec![
        WinHidDeviceRecord {
            path: r"\\?\hid#vid_048d&pid_7001&col01".to_string(),
            vendor_id: ITE_VENDOR_ID,
            product_id: ITE_LIGHTBAR_PRODUCT_ID_4ZONE,
            version_number: 0,
            usage_page: ITE_USAGE_PAGE_EXCLUDED_FF89,
            usage: 0x0002,
            feature_report_byte_length: 9,
            output_report_byte_length: 0,
        },
        WinHidDeviceRecord {
            path: r"\\?\hid#vid_048d&pid_7001&col02".to_string(),
            vendor_id: ITE_VENDOR_ID,
            product_id: ITE_LIGHTBAR_PRODUCT_ID_4ZONE,
            version_number: 0,
            usage_page: ITE_USAGE_PAGE_PER_KEY,
            usage: 0x0001,
            feature_report_byte_length: 9,
            output_report_byte_length: 0,
        },
        WinHidDeviceRecord {
            path: r"\\?\hid#vid_048d&pid_7001&col03".to_string(),
            vendor_id: ITE_VENDOR_ID,
            product_id: ITE_LIGHTBAR_PRODUCT_ID_4ZONE,
            version_number: 0,
            usage_page: ITE_USAGE_PAGE_PER_KEY,
            usage: 0x0002,
            feature_report_byte_length: 9,
            output_report_byte_length: 0,
        },
    ];
    assert!(!win_records[0].is_vendor_lighting_interface());
    assert!(win_records[2].is_vendor_lighting_interface());

    let win_api = Arc::new(MockWinHidSysApi::new(win_records));
    let win_hid = Arc::new(WindowsHidTransport::new(Arc::clone(&win_api) as _));
    let t_win = UniwillHidTransport::new(win_hid);
    t_win.write_lightbar_brightness(255).unwrap();
    let sent = win_api.sent_reports();
    assert!(!sent.is_empty());
    assert!(sent
        .iter()
        .all(|(p, _)| p == r"\\?\hid#vid_048d&pid_7001&col03"));
}

#[test]
fn hidraw_parses_all_collections_and_rejects_unreadable_descriptor_without_iface() {
    // Descriptor with 3 top-level collections: 0x0001 (boot kbd), 0xFF89 (excluded), 0xFF03 (LED).
    let multi_desc = [
        0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0xC0, // Generic Desktop / Keyboard
        0x06, 0x89, 0xFF, 0x09, 0x01, 0xA1, 0x01, 0xC0, // Excluded 0xFF89
        0x06, 0x03, 0xFF, 0x09, 0x02, 0xA1, 0x01, 0xC0, // Lighting 0xFF03
    ];
    assert_eq!(
        parse_hid_report_descriptor_usage_pages(&multi_desc),
        vec![0x0001, 0xFF89, 0xFF03]
    );
    assert_eq!(
        parse_hid_report_descriptor_usage_page(&multi_desc),
        Some(0xFF03)
    );

    // Missing report_descriptor AND missing bInterfaceNumber on HidrawDeviceTransport must NOT
    // fall back to `true` (prevents opening boot keyboard 0x0001 on read failure).
    let tmp = TempDir::new().expect("temp dir");
    let dev_dir = tmp.path().join("hidraw0/device");
    fs::create_dir_all(&dev_dir).expect("create dir");
    fs::write(dev_dir.join("uevent"), "HID_ID=0003:0000048D:0000600B\n").expect("uevent");

    let hidraw = Arc::new(HidrawDeviceTransport::with_sysfs_root(tmp.path()));
    let transport = UniwillHidTransport::new(hidraw);
    assert!(
        transport.detect_keyboard().unwrap().is_none(),
        "HidrawDeviceTransport must reject device when report_descriptor and bInterfaceNumber are both missing"
    );
}

#[test]
fn ite7001_zone2_power_on_off_and_resume_force_power_on() {
    let mock = Arc::new(MockHidTransport::new(vec![HidDeviceInfo {
        vendor_id: ITE_VENDOR_ID,
        product_id: ITE_LIGHTBAR_PRODUCT_ID_4ZONE,
        path: "/dev/hidraw1".to_string(),
    }]));
    let transport = Arc::new(UniwillHidTransport::new(Arc::clone(&mock) as _));

    // Startup color write must send power_on, color (0x14 0x02 0x01), and effect (0x08 0x22).
    transport
        .write_lightbar_color(RgbColor::new(10, 20, 30))
        .unwrap();
    assert_eq!(
        mock.recorded_reports()
            .into_iter()
            .map(|(_, r)| r)
            .collect::<Vec<_>>(),
        vec![
            vec![0x1A, 0x02, 0x01, 0x04, 0, 0, 0, 0x01],
            vec![0x14, 0x02, 0x01, 10, 20, 30, 0, 0],
            vec![0x08, 0x22, 0x01, 0x01, 100, 0x01, 0, 0],
        ]
    );

    // Brightness 128 -> lb_level = 50 with ctrl = 0x22.
    mock.clear_reports();
    transport.write_lightbar_brightness(128).unwrap();
    assert_eq!(
        mock.recorded_reports()
            .into_iter()
            .map(|(_, r)| r)
            .collect::<Vec<_>>(),
        vec![vec![0x08, 0x22, 0x01, 0x01, 50, 0x01, 0, 0]]
    );

    // Brightness 0 -> full 4-packet power-off sequence.
    mock.clear_reports();
    transport.write_lightbar_brightness(0).unwrap();
    assert_eq!(
        mock.recorded_reports()
            .into_iter()
            .map(|(_, r)| r)
            .collect::<Vec<_>>(),
        vec![
            vec![0x12, 0x00, 0x03, 0, 0, 0, 0, 0],
            vec![0x08, 0x05, 0, 0, 0, 0, 0, 0],
            vec![0x08, 0x01, 0x02, 0, 0, 0, 0, 0],
            vec![0x1A, 0x02, 0x00, 0, 0, 0, 0, 0x01],
        ]
    );

    // LightbarController::on_resume() must force power_on again after wake from sleep.
    let mut lb = LightbarController::new(Arc::clone(&transport) as _);
    lb.configure(&json!({
        "managed": true,
        "welcome_anim": false,
        "power_settings": {
            "ac": {
                "enabled": true,
                "brightness_val": 200,
                "mode": "Pulse (stays on)",
                "color_mode": [255, 0, 128]
            }
        }
    }))
    .unwrap();
    mock.clear_reports();
    lb.on_resume().unwrap();
    let resume_pkts: Vec<Vec<u8>> = mock
        .recorded_reports()
        .into_iter()
        .map(|(_, r)| r)
        .collect();
    assert_eq!(
        resume_pkts.first(),
        Some(&vec![0x1A, 0x02, 0x01, 0x04, 0, 0, 0, 0x01]),
        "on_resume must force 048D:7001 Zone 2 power_on"
    );
}

#[test]
fn ite7000_a2_topology_probe_and_fallback() {
    assert_eq!(decode_ite7000_topology(0), Some(vec![Ite7000Zone::Hinge]));
    assert_eq!(
        decode_ite7000_topology(1),
        Some(vec![Ite7000Zone::Hinge, Ite7000Zone::Logo])
    );
    assert_eq!(
        decode_ite7000_topology(4),
        Some(vec![
            Ite7000Zone::Hinge,
            Ite7000Zone::Base,
            Ite7000Zone::Logo
        ])
    );
    assert_eq!(decode_ite7000_topology(5), Some(vec![Ite7000Zone::Hinge]));
    assert_eq!(
        decode_ite7000_topology(6),
        Some(vec![Ite7000Zone::Hinge, Ite7000Zone::Base])
    );
    assert_eq!(decode_ite7000_topology(99), None);

    // Topology 6: (hinge, base) — no logo!
    let mock_topo6 = Arc::new(MockHidTransport::new(vec![HidDeviceInfo {
        vendor_id: ITE_VENDOR_ID,
        product_id: ITE_LIGHTBAR_PRODUCT_ID_5ZONE,
        path: "/dev/hidraw2".to_string(),
    }]));
    mock_topo6.set_feature_response("/dev/hidraw2", 0xA2, vec![0xA2, 6, 0, 0, 0, 0, 0, 0]);
    let t6 = UniwillHidTransport::new(Arc::clone(&mock_topo6) as _);
    let caps6 = t6.detect_lightbar().unwrap().unwrap();
    assert_eq!(caps6.zones, 2);
    assert!(!t6.has_lid_logo().unwrap());
    assert!(matches!(
        t6.write_lid_logo(true, RgbColor::WHITE, 255),
        Err(LightingError::Unavailable)
    ));

    mock_topo6.clear_reports();
    t6.write_lightbar_color(RgbColor::new(11, 22, 33)).unwrap();
    t6.write_lightbar_brightness(255).unwrap();
    assert_eq!(
        mock_topo6
            .recorded_reports()
            .into_iter()
            .map(|(_, r)| r)
            .collect::<Vec<_>>(),
        vec![
            vec![0x14, 0x01, 0x01, 11, 22, 33, 0, 0],
            vec![0x14, 0x02, 0x01, 11, 22, 33, 0, 0],
            vec![0x08, 0x21, 0x01, 0x01, 50, 0x01, 0, 0],
            vec![0x08, 0x22, 0x01, 0x01, 50, 0x01, 0, 0],
        ]
    );

    // Topology 1: (hinge, logo) — has logo, no base!
    let mock_topo1 = Arc::new(MockHidTransport::new(vec![HidDeviceInfo {
        vendor_id: ITE_VENDOR_ID,
        product_id: ITE_LIGHTBAR_PRODUCT_ID_5ZONE,
        path: "/dev/hidraw2".to_string(),
    }]));
    mock_topo1.set_feature_response("/dev/hidraw2", 0xA2, vec![0xA2, 1, 0, 0, 0, 0, 0, 0]);
    let t1 = UniwillHidTransport::new(Arc::clone(&mock_topo1) as _);
    assert_eq!(t1.detect_lightbar().unwrap().unwrap().zones, 2);
    assert!(t1.has_lid_logo().unwrap());
    mock_topo1.clear_reports();
    t1.write_lid_logo(true, RgbColor::new(44, 55, 66), 255)
        .unwrap();
    assert_eq!(
        mock_topo1
            .recorded_reports()
            .into_iter()
            .map(|(_, r)| r)
            .collect::<Vec<_>>(),
        vec![
            vec![0x14, 0x03, 0x01, 44, 55, 66, 0, 0],
            vec![0x08, 0x23, 0x01, 0x01, 50, 0x01, 0, 0],
        ]
    );
}

#[test]
fn ce00_distinguishes_four_zone_ff12_vs_per_key_rev003() {
    // 1. 4-zone 048D:CE00 on 0xFF12
    let mock_4z = Arc::new(MockHidTransport::new(vec![HidDeviceInfo {
        vendor_id: ITE_VENDOR_ID,
        product_id: ITE_KEYBOARD_PRODUCT_ID_CE00,
        path: "/dev/hidraw4".to_string(),
    }]));
    mock_4z.set_device_usage_page("/dev/hidraw4", ITE_USAGE_PAGE_4ZONE);
    let t_4z = UniwillHidTransport::new(Arc::clone(&mock_4z) as _);
    let caps_4z = t_4z.detect_keyboard().unwrap().unwrap();
    assert_eq!(caps_4z.zones, 4);
    t_4z.write_keyboard_brightness(255).unwrap();
    t_4z.write_keyboard_zones(&[
        RgbColor::new(255, 0, 0),
        RgbColor::new(0, 255, 0),
        RgbColor::new(0, 0, 255),
        RgbColor::new(255, 255, 0),
    ])
    .unwrap();
    let r_4z: Vec<Vec<u8>> = mock_4z
        .recorded_reports()
        .into_iter()
        .map(|(_, r)| r)
        .collect();
    assert_eq!(r_4z[0], vec![0x08, 0x02, 0x01, 0x00, 50, 0x08, 0, 0]);
    assert_eq!(r_4z[1], vec![0x14, 0x00, 1, 255, 0, 0, 0, 0]);
    assert_eq!(r_4z[4], vec![0x14, 0x00, 4, 255, 255, 0, 0, 0]);
    assert_eq!(r_4z[5], vec![0x08, 0x02, 0x01, 0x00, 50, 0x08, 0, 0]);

    // 2. Per-key 048D:CE00 rev 0.03 via Windows WinHidDeviceRecord (version_number == 3, output_len == 65)
    let win_api = Arc::new(MockWinHidSysApi::new(vec![WinHidDeviceRecord {
        path: r"\\?\hid#vid_048d&pid_ce00&rev_0003".to_string(),
        vendor_id: ITE_VENDOR_ID,
        product_id: ITE_KEYBOARD_PRODUCT_ID_CE00,
        version_number: 3,
        usage_page: ITE_USAGE_PAGE_4ZONE,
        usage: 0x0001,
        feature_report_byte_length: 9,
        output_report_byte_length: 65,
    }]));
    let win_hid = Arc::new(WindowsHidTransport::new(Arc::clone(&win_api) as _));
    let t_pk = UniwillHidTransport::new(win_hid);
    let caps_pk = t_pk.detect_keyboard().unwrap().unwrap();
    assert_eq!(caps_pk.zones, 126);
    t_pk.write_keyboard_brightness(255).unwrap();
    assert_eq!(
        &win_api.sent_reports()[0].1[1..6],
        &[0x08, 0x02, 0x33, 0x00, 50]
    );
}

#[test]
fn framing_transient_retry_windows_cache_invalidation_and_resume() {
    // 1. Unified feature report framing: always prepend 0x00 even when payload[0] == 0x00.
    let framed = frame_windows_feature_report(&[0x00, 0x11, 0x22], 9).unwrap();
    assert_eq!(framed, vec![0x00, 0x00, 0x11, 0x22, 0, 0, 0, 0, 0]);

    // 2. Transient EAGAIN/WouldBlock retries once even on cold cache (!was_cached).
    struct TransientOnceHid {
        attempts: AtomicUsize,
    }
    impl HidDeviceTransport for TransientOnceHid {
        fn enumerate_devices(&self) -> Result<Vec<HidDeviceInfo>, LightingError> {
            Ok(vec![HidDeviceInfo {
                vendor_id: ITE_VENDOR_ID,
                product_id: ITE_KEYBOARD_PRODUCT_ID,
                path: "/dev/hidraw0".to_string(),
            }])
        }
        fn send_feature_report(&self, _path: &str, _report: &[u8]) -> Result<(), LightingError> {
            let n = self.attempts.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                return Err(LightingError::Io(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "EAGAIN",
                )));
            }
            Ok(())
        }
        fn get_feature_report(&self, _path: &str, len: usize) -> Result<Vec<u8>, LightingError> {
            Ok(vec![0u8; len])
        }
    }
    let transient = Arc::new(TransientOnceHid {
        attempts: AtomicUsize::new(0),
    });
    let t_transient = UniwillHidTransport::new(Arc::clone(&transient) as _);
    // Cold cache (detect_keyboard not called beforehand): must retry once and succeed!
    t_transient.write_keyboard_brightness(128).unwrap();
    assert_eq!(transient.attempts.load(Ordering::SeqCst), 2);

    // 3. WindowsHidTransport invalidates device_cache on write error.
    struct CountingWinApi {
        inner: MockWinHidSysApi,
        enum_calls: AtomicUsize,
    }
    impl WinHidSysApi for CountingWinApi {
        fn enumerate_hid_devices(&self) -> Result<Vec<WinHidDeviceRecord>, LightingError> {
            self.enum_calls.fetch_add(1, Ordering::SeqCst);
            self.inner.enumerate_hid_devices()
        }
        fn set_feature_report(&self, path: &str, report: &[u8]) -> Result<(), LightingError> {
            self.inner.set_feature_report(path, report)
        }
        fn get_feature_report(&self, path: &str, buf: &mut [u8]) -> Result<(), LightingError> {
            self.inner.get_feature_report(path, buf)
        }
    }
    let api = Arc::new(CountingWinApi {
        inner: MockWinHidSysApi::new(vec![WinHidDeviceRecord {
            path: r"\\?\hid#vid_048d&pid_600b&mi_01".to_string(),
            vendor_id: ITE_VENDOR_ID,
            product_id: ITE_KEYBOARD_PRODUCT_ID,
            version_number: 0,
            usage_page: ITE_USAGE_PAGE_PER_KEY,
            usage: 1,
            feature_report_byte_length: 9,
            output_report_byte_length: 65,
        }]),
        enum_calls: AtomicUsize::new(0),
    });
    let win_t = WindowsHidTransport::new(Arc::clone(&api) as _);
    win_t.enumerate_devices().unwrap();
    assert_eq!(api.enum_calls.load(Ordering::SeqCst), 1);
    api.inner.set_fail_writes(true);
    assert!(win_t
        .send_feature_report(r"\\?\hid#vid_048d&pid_600b&mi_01", &[0x08, 0x01])
        .is_err());
    api.inner.set_fail_writes(false);
    // Next send must re-enumerate because device_cache was invalidated on write error.
    win_t
        .send_feature_report(r"\\?\hid#vid_048d&pid_600b&mi_01", &[0x08, 0x01])
        .unwrap();
    assert_eq!(api.enum_calls.load(Ordering::SeqCst), 2);

    // 4. LIGHTBAR_FRAME_INTERVAL is 33 ms (~30 Hz) and advance() records write errors.
    assert_eq!(LIGHTBAR_FRAME_INTERVAL, Duration::from_millis(33));
    let lb_mock = Arc::new(MockLightingTransport::new());
    lb_mock.set_lightbar_caps(Some(LightbarCapabilities {
        zones: 4,
        max_brightness: 255,
    }));
    let mut lb = LightbarController::new(Arc::clone(&lb_mock) as _);
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
    .unwrap();
    assert!(lb.trigger(true));
    lb_mock.set_fail_on_lightbar_write(true);
    lb.advance(LightbarAnimation::Pulse.duration_ms());
    assert_eq!(lb.status_json()["ok"], false);
    assert!(!lb.status_json()["error"].as_str().unwrap().is_empty());

    // 5. KeyboardBacklightController::on_resume() re-applies current_states after wake.
    let kbd_mock = Arc::new(MockLightingTransport::new());
    kbd_mock.set_keyboard_caps(Some(KeyboardCapabilities {
        zones: 4,
        max_brightness: 255,
        max_red: 255,
        max_green: 255,
        max_blue: 255,
        led_ids: vec![0, 1, 2, 3],
    }));
    let mut kbd = KeyboardBacklightController::new(Arc::clone(&kbd_mock) as _);
    kbd.init().unwrap();
    let states = vec![
        KeyboardBacklightState {
            brightness: 180,
            red: 10,
            green: 20,
            blue: 30,
        };
        4
    ];
    kbd.apply_states(&states).unwrap();
    kbd_mock.clear_ops();

    kbd.on_resume().unwrap();
    let ops = kbd_mock.ops();
    assert!(ops.contains(&LightingOp::WriteKeyboardBrightness(180)));
    assert!(ops.contains(&LightingOp::WriteKeyboardZones(vec![
        RgbColor::new(
            10, 20, 30
        );
        4
    ])));
}
