// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::Arc;
use ucc_lighting::{
    frame_windows_feature_report, unframe_windows_feature_report, HidDeviceTransport,
    LightingError, LightingTransport, MockWinHidSysApi, RgbColor, UniwillHidTransport,
    WinHidDeviceRecord, WindowsHidTransport, ITE_KEYBOARD_PRODUCT_ID, ITE_VENDOR_ID,
    VENDOR_USAGE_PAGE_MIN,
};

const WIN_KBD_BOOT_PATH: &str = r"\\?\hid#vid_048d&pid_600b&mi_00#7&1a2b3c4d&0&0000#{4d1e55b2}";
const WIN_KBD_VENDOR_PATH: &str = r"\\?\hid#vid_048d&pid_600b&mi_01#7&1a2b3c4d&0&0000#{4d1e55b2}";
const WIN_LIGHTBAR_PATH: &str = r"\\?\hid#vid_048d&pid_7000&mi_01#7&5e6f7a8b&0&0000#{4d1e55b2}";

fn sample_ite_windows_devices() -> Vec<WinHidDeviceRecord> {
    vec![
        // Boot keyboard interface (UsagePage 0x0001) must be skipped by UniwillHidTransport.
        WinHidDeviceRecord {
            path: WIN_KBD_BOOT_PATH.to_string(),
            vendor_id: ITE_VENDOR_ID,
            product_id: ITE_KEYBOARD_PRODUCT_ID,
            usage_page: 0x0001,
            usage: 0x0006,
            feature_report_byte_length: 0,
        },
        // Vendor lighting interface (UsagePage 0xFF89 >= 0xFF00) for ITE 048D:600B.
        WinHidDeviceRecord {
            path: WIN_KBD_VENDOR_PATH.to_string(),
            vendor_id: ITE_VENDOR_ID,
            product_id: ITE_KEYBOARD_PRODUCT_ID,
            usage_page: 0xFF89,
            usage: 0x00CC,
            feature_report_byte_length: 65,
        },
        // Vendor lighting interface for ITE 048D:7000 (4 front lightbar zones + lid logo LED).
        WinHidDeviceRecord {
            path: WIN_LIGHTBAR_PATH.to_string(),
            vendor_id: ITE_VENDOR_ID,
            product_id: 0x7000,
            usage_page: VENDOR_USAGE_PAGE_MIN,
            usage: 0x0001,
            feature_report_byte_length: 9,
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
    // 3 keyboard reports + 1 lightbar brightness + 5 lightbar/logo zone reports = 9 total.
    assert_eq!(reports.len(), 9);

    // Every keyboard report targets the vendor lighting interface, never the boot keyboard.
    for (path, buf) in &reports[..3] {
        assert_eq!(path, WIN_KBD_VENDOR_PATH);
        assert_eq!(buf.len(), 65);
        assert_eq!(buf[0], 0x00);
    }
    // Brightness report: [0x00, 0x08, 0x02, 0x01, 0x05, 50, ...]
    assert_eq!(&reports[1].1[1..6], &[0x08, 0x02, 0x01, 0x05, 50]);
    // Zone chunk report: [0x00, 0x14, 0, 2, 10, 20, 30, 40, 50, 60, ...]
    assert_eq!(&reports[2].1[1..10], &[0x14, 0, 2, 10, 20, 30, 40, 50, 60]);

    // Every lightbar report targets 048D:7000 with exact 9-byte framed buffer.
    for (path, buf) in &reports[3..] {
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
    assert_eq!(read_back, vec![0x08, 0x01, 0xAA, 0x00, 0x00, 0x00, 0x00, 0x00]);

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
