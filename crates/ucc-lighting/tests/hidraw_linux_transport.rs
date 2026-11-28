// SPDX-License-Identifier: GPL-3.0-or-later

//! Unit and mock integration tests for Linux `HidrawDeviceTransport`, extended `UniwillHidTransport`
//! (Lightbar + lid logo zone 5 + EC power hook), and `048D:600B` calibration preservation (`M14`).
//!
//! All tests run deterministically in memory or against a temporary mock sysfs directory with zero
//! host `/dev` or `/sys` I/O.

use serde_json::json;
use std::fs;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use ucc_core::KeyboardBacklightState;
use ucc_lighting::{
    hidiocgfeature_request, hidiocsfeature_request, parse_hid_id_from_uevent,
    parse_hid_report_descriptor_usage_page, select_best_lighting_transport, EcLightbarPowerHook,
    HidDeviceInfo, HidDeviceTransport, HidrawDeviceTransport, KeyboardBacklightController,
    KeyboardCapabilities, LightbarAnimation, LightbarCapabilities, LightbarController,
    LightingError, LightingTransport, MockHidTransport, MockLightingTransport, RgbColor,
    UniwillHidTransport, EC_LIGHTBAR_POWER_REG_PRIMARY, EC_LIGHTBAR_POWER_REG_SECONDARY,
    ITE_KEYBOARD_MAX_HW_BRIGHTNESS, ITE_KEYBOARD_PRODUCT_ID, ITE_LIGHTBAR_PRODUCT_ID_4ZONE,
    ITE_LIGHTBAR_PRODUCT_ID_5ZONE, ITE_VENDOR_ID, LIGHTBAR_LID_LOGO_ZONE_ID, MECHREVO_ENTER_LED_ID,
    MECHREVO_EXPECTED_ZONES, MECHREVO_LCTRL_LED_ID, MECHREVO_LSHIFT_LED_ID,
};

#[cfg(unix)]
fn create_mock_sysfs_hidraw_parent_iface(
    root: &std::path::Path,
    hidraw_name: &str,
    usb_iface_rel: &str,
    hid_dev_name: &str,
    uevent: &str,
    iface_hex: &str,
    report_desc: Option<&[u8]>,
) {
    use std::os::unix::fs::symlink;

    let usb_iface = root.join(usb_iface_rel);
    let hid_dev = usb_iface.join(hid_dev_name);
    fs::create_dir_all(&hid_dev).expect("create hid_dev");
    fs::write(usb_iface.join("bInterfaceNumber"), format!("{iface_hex}\n"))
        .expect("write parent bInterfaceNumber");
    fs::write(hid_dev.join("uevent"), uevent).expect("write uevent");
    if let Some(desc) = report_desc {
        fs::write(hid_dev.join("report_descriptor"), desc).expect("write report_descriptor");
    }

    let hidraw_dir = root.join(hidraw_name);
    fs::create_dir_all(&hidraw_dir).expect("create hidraw dir");
    symlink(&hid_dev, hidraw_dir.join("device")).expect("symlink device");
}

#[test]
fn hidraw_ioctl_numbers_and_descriptor_parsers() {
    // Linux <linux/hidraw.h>:
    // HIDIOCSFEATURE(len) = _IOC(_IOC_WRITE|_IOC_READ, 'H', 0x06, len)
    // HIDIOCGFEATURE(len) = _IOC(_IOC_WRITE|_IOC_READ, 'H', 0x07, len)
    assert_eq!(hidiocsfeature_request(8), Some(0xC008_4806));
    assert_eq!(hidiocgfeature_request(8), Some(0xC008_4807));
    assert_eq!(hidiocsfeature_request(51), Some(0xC033_4806));
    assert_eq!(hidiocgfeature_request(64), Some(0xC040_4807));
    assert_eq!(hidiocsfeature_request(0), None);
    assert_eq!(hidiocgfeature_request(0), None);
    assert_eq!(hidiocsfeature_request(0x4000), None);
    assert_eq!(hidiocgfeature_request(0x4000), None);

    assert_eq!(
        parse_hid_id_from_uevent("DRIVER=hid-generic\nHID_ID=0003:0000048D:0000600B\n"),
        Some((ITE_VENDOR_ID, ITE_KEYBOARD_PRODUCT_ID))
    );
    assert_eq!(
        parse_hid_id_from_uevent("HID_ID=0003:0000048d:00007001\n"),
        Some((ITE_VENDOR_ID, ITE_LIGHTBAR_PRODUCT_ID_4ZONE))
    );
    assert_eq!(
        parse_hid_id_from_uevent("HID_ID=0003:0000048D:00007000\n"),
        Some((ITE_VENDOR_ID, ITE_LIGHTBAR_PRODUCT_ID_5ZONE))
    );
    assert_eq!(parse_hid_id_from_uevent("HID_NAME=NoIdHere\n"), None);
    assert_eq!(parse_hid_id_from_uevent("HID_ID=0003:ZZZZ:600B\n"), None);

    // 1-byte Generic Desktop Usage Page (0x01)
    assert_eq!(
        parse_hid_report_descriptor_usage_page(&[0x05, 0x01, 0x09, 0x06]),
        Some(0x0001)
    );
    // 2-byte vendor-defined Usage Page (0xFF89, little-endian)
    assert_eq!(
        parse_hid_report_descriptor_usage_page(&[0x06, 0x89, 0xFF, 0x09, 0x01]),
        Some(0xFF89)
    );
    // 4-byte Usage Page after a long item (0xFE)
    assert_eq!(
        parse_hid_report_descriptor_usage_page(&[
            0xFE, 0x02, 0x01, 0xAA, 0xBB, 0x07, 0x00, 0xFF, 0x00, 0x00,
        ]),
        Some(0xFF00)
    );
    // Truncated descriptor returns None without panicking
    assert_eq!(parse_hid_report_descriptor_usage_page(&[0x06, 0x89]), None);
    assert_eq!(parse_hid_report_descriptor_usage_page(&[]), None);
}

#[test]
fn hidraw_linux_enumerates_mock_sysfs_and_filters_interfaces() {
    let tmp = TempDir::new().expect("temp dir");
    let root = tmp.path();

    // Non-ITE device on hidraw0 (must be ignored by UniwillHidTransport)
    let hidraw0_dev = root.join("hidraw0/device");
    fs::create_dir_all(&hidraw0_dev).expect("create hidraw0");
    fs::write(
        hidraw0_dev.join("uevent"),
        "HID_ID=0003:0000046D:0000C077\nHID_NAME=Logitech Mouse\n",
    )
    .expect("write hidraw0 uevent");
    fs::write(hidraw0_dev.join("bInterfaceNumber"), "00\n").expect("write hidraw0 iface");

    // ITE 048D:600B interface 00 (standard boot keyboard interface, must be ignored)
    #[cfg(unix)]
    create_mock_sysfs_hidraw_parent_iface(
        root,
        "hidraw1",
        "usb/1-1:1.0",
        "0003:048D:600B.0001",
        "HID_ID=0003:0000048D:0000600B\n",
        "00",
        Some(&[0x05, 0x01, 0x09, 0x06]),
    );
    #[cfg(not(unix))]
    {
        let dev = root.join("hidraw1/device");
        fs::create_dir_all(&dev).expect("create hidraw1");
        fs::write(dev.join("uevent"), "HID_ID=0003:0000048D:0000600B\n").expect("write uevent");
        fs::write(dev.join("bInterfaceNumber"), "00\n").expect("write iface");
        fs::write(dev.join("report_descriptor"), [0x05u8, 0x01, 0x09, 0x06]).expect("write desc");
    }

    // ITE 048D:600B interface 01 (vendor lighting interface, must be selected)
    #[cfg(unix)]
    create_mock_sysfs_hidraw_parent_iface(
        root,
        "hidraw2",
        "usb/1-1:1.1",
        "0003:048D:600B.0002",
        "HID_ID=0003:0000048D:0000600B\n",
        "01",
        Some(&[0x06, 0x89, 0xFF, 0x09, 0x01]),
    );
    #[cfg(not(unix))]
    {
        let dev = root.join("hidraw2/device");
        fs::create_dir_all(&dev).expect("create hidraw2");
        fs::write(dev.join("uevent"), "HID_ID=0003:0000048D:0000600B\n").expect("write uevent");
        fs::write(dev.join("bInterfaceNumber"), "01\n").expect("write iface");
        fs::write(
            dev.join("report_descriptor"),
            [0x06u8, 0x89, 0xFF, 0x09, 0x01],
        )
        .expect("write desc");
    }

    // ITE 048D:7001 fallback device/bInterfaceNumber = 01 (4-zone front Lightbar)
    let hidraw3_dev = root.join("hidraw3/device");
    fs::create_dir_all(&hidraw3_dev).expect("create hidraw3");
    fs::write(
        hidraw3_dev.join("uevent"),
        "HID_ID=0003:0000048D:00007001\n",
    )
    .expect("write hidraw3 uevent");
    fs::write(hidraw3_dev.join("bInterfaceNumber"), "01\n").expect("write hidraw3 iface");

    // ITE 048D:7000 on hidraw10 (tests numeric sorting after hidraw3)
    #[cfg(unix)]
    create_mock_sysfs_hidraw_parent_iface(
        root,
        "hidraw10",
        "usb/1-2:1.1",
        "0003:048D:7000.0003",
        "HID_ID=0003:0000048D:00007000\n",
        "01",
        Some(&[0x06, 0x00, 0xFF, 0x09, 0x01]),
    );
    #[cfg(not(unix))]
    {
        let dev = root.join("hidraw10/device");
        fs::create_dir_all(&dev).expect("create hidraw10");
        fs::write(dev.join("uevent"), "HID_ID=0003:0000048D:00007000\n").expect("write uevent");
        fs::write(dev.join("bInterfaceNumber"), "01\n").expect("write iface");
        fs::write(
            dev.join("report_descriptor"),
            [0x06u8, 0x00, 0xFF, 0x09, 0x01],
        )
        .expect("write desc");
    }

    let hidraw = Arc::new(HidrawDeviceTransport::with_sysfs_root(root));
    assert!(!hidraw.uses_host_sysfs());
    assert!(HidrawDeviceTransport::new().uses_host_sysfs());

    let devices = hidraw.enumerate_devices().expect("enumerate mock sysfs");
    assert_eq!(devices.len(), 5);
    assert_eq!(devices[0].path, "/dev/hidraw0");
    assert_eq!(devices[0].vendor_id, 0x046D);
    assert_eq!(devices[1].path, "/dev/hidraw1");
    assert_eq!(devices[1].product_id, ITE_KEYBOARD_PRODUCT_ID);
    assert_eq!(devices[2].path, "/dev/hidraw2");
    assert_eq!(devices[2].product_id, ITE_KEYBOARD_PRODUCT_ID);
    assert_eq!(devices[3].path, "/dev/hidraw3");
    assert_eq!(devices[3].product_id, ITE_LIGHTBAR_PRODUCT_ID_4ZONE);
    assert_eq!(devices[4].path, "/dev/hidraw10");
    assert_eq!(devices[4].product_id, ITE_LIGHTBAR_PRODUCT_ID_5ZONE);

    assert_eq!(hidraw.device_interface_number("/dev/hidraw1"), Some(0));
    assert_eq!(hidraw.device_interface_number("/dev/hidraw2"), Some(1));
    assert_eq!(hidraw.device_interface_number("/dev/hidraw3"), Some(1));
    assert_eq!(hidraw.device_interface_number("/dev/hidraw10"), Some(1));
    assert_eq!(hidraw.device_usage_page("/dev/hidraw1"), Some(0x0001));
    assert_eq!(hidraw.device_usage_page("/dev/hidraw2"), Some(0xFF89));
    assert_eq!(hidraw.device_usage_page("/dev/hidraw10"), Some(0xFF00));

    // UniwillHidTransport detects 126-zone keyboard (hidraw2) and 4-zone lightbar (hidraw3)
    let transport = UniwillHidTransport::new(hidraw);
    let kbd_caps = transport
        .detect_keyboard()
        .expect("detect keyboard")
        .expect("keyboard present");
    assert_eq!(kbd_caps.zones, MECHREVO_EXPECTED_ZONES);
    let lb_caps = transport
        .detect_lightbar()
        .expect("detect lightbar")
        .expect("lightbar present");
    assert_eq!(lb_caps.zones, 4);

    // When only non-ITE + interface 00 ITE keyboard exist, UniwillHidTransport ignores both
    let empty_tmp = TempDir::new().expect("temp dir");
    let iface0_dev = empty_tmp.path().join("hidraw0/device");
    fs::create_dir_all(&iface0_dev).expect("create iface0");
    fs::write(iface0_dev.join("uevent"), "HID_ID=0003:0000048D:0000600B\n").expect("uevent");
    fs::write(iface0_dev.join("bInterfaceNumber"), "00\n").expect("iface0");
    let non_ite_dev = empty_tmp.path().join("hidraw1/device");
    fs::create_dir_all(&non_ite_dev).expect("create non-ite");
    fs::write(
        non_ite_dev.join("uevent"),
        "HID_ID=0003:00001234:00005678\n",
    )
    .expect("uevent");
    fs::write(non_ite_dev.join("bInterfaceNumber"), "01\n").expect("iface1");

    let filtered = UniwillHidTransport::new(Arc::new(HidrawDeviceTransport::with_sysfs_root(
        empty_tmp.path(),
    )));
    assert!(filtered.detect_keyboard().expect("detect").is_none());
    assert!(filtered.detect_lightbar().expect("detect").is_none());
}

#[test]
fn mock_hid_transport_is_isolated_from_host_sysfs() {
    // 1. MockHidTransport never queries host /sys/class/hidraw even for `/dev/hidraw0`.
    let mock = Arc::new(MockHidTransport::new(vec![
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
    ]));
    assert!(!mock.uses_host_sysfs());
    assert_eq!(mock.device_interface_number("/dev/hidraw0"), None);
    assert_eq!(mock.device_usage_page("/dev/hidraw0"), None);

    // Explicit mock interface disambiguation selects hidraw1 (iface 01) over hidraw0 (iface 00).
    mock.set_device_interface_number("/dev/hidraw0", 0);
    mock.set_device_interface_number("/dev/hidraw1", 1);
    let transport = UniwillHidTransport::new(Arc::clone(&mock) as _);
    transport.write_keyboard_brightness(128).expect("write");
    let reports = mock.recorded_reports();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].0, "/dev/hidraw1");

    // 2. Any minimal custom HidDeviceTransport impl inherits safe non-host-sysfs defaults.
    struct MinimalCustomMock;
    impl HidDeviceTransport for MinimalCustomMock {
        fn enumerate_devices(&self) -> Result<Vec<HidDeviceInfo>, LightingError> {
            Ok(vec![HidDeviceInfo {
                vendor_id: ITE_VENDOR_ID,
                product_id: ITE_KEYBOARD_PRODUCT_ID,
                path: "/dev/hidraw0".to_string(),
            }])
        }
        fn send_feature_report(&self, _path: &str, _report: &[u8]) -> Result<(), LightingError> {
            Ok(())
        }
        fn get_feature_report(&self, _path: &str, len: usize) -> Result<Vec<u8>, LightingError> {
            Ok(vec![0u8; len])
        }
    }

    let custom = MinimalCustomMock;
    assert!(!custom.uses_host_sysfs());
    assert_eq!(custom.device_interface_number("/dev/hidraw0"), None);
    assert_eq!(custom.device_usage_page("/dev/hidraw0"), None);
}

#[test]
fn ite_600b_preserves_physical_calibration_and_caps_brightness_to_50() {
    assert_eq!(MECHREVO_LCTRL_LED_ID, 0);
    assert_eq!(MECHREVO_LSHIFT_LED_ID, 21);
    assert_eq!(MECHREVO_ENTER_LED_ID, 56);
    assert_eq!(ITE_KEYBOARD_MAX_HW_BRIGHTNESS, 50);

    // Brightness scaling edge cases
    assert_eq!(UniwillHidTransport::scale_keyboard_brightness(0), 0);
    assert_eq!(UniwillHidTransport::scale_keyboard_brightness(128), 25);
    assert_eq!(UniwillHidTransport::scale_keyboard_brightness(255), 50);
    assert_eq!(UniwillHidTransport::scale_keyboard_brightness(999), 50);

    let mock_hid = Arc::new(MockHidTransport::new(vec![HidDeviceInfo {
        vendor_id: ITE_VENDOR_ID,
        product_id: ITE_KEYBOARD_PRODUCT_ID,
        path: "/dev/hidraw2".to_string(),
    }]));
    mock_hid.set_device_usage_page("/dev/hidraw2", 0xFF89);

    let transport = Arc::new(UniwillHidTransport::new(Arc::clone(&mock_hid) as _));
    let mut kbd = KeyboardBacklightController::new(Arc::clone(&transport) as _);
    kbd.init().expect("init keyboard");

    let caps = kbd.capabilities().expect("keyboard capabilities");
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

    let reports = mock_hid.recorded_reports();
    // 1 buffer enable + 1 brightness + 8 zone chunks (ceil(126/16)) + 1 buffer disable = 11
    assert_eq!(reports.len(), 11);
    assert_eq!(reports[0].1, vec![0x09, 0x02, 0x01, 0, 0, 0, 0, 0]);
    assert_eq!(reports[1].1, vec![0x08, 0x02, 0x01, 0x05, 50, 0, 0, 0]);
    assert_eq!(reports[10].1, vec![0x09, 0x02, 0x00, 0, 0, 0, 0, 0]);

    // Verify every chunked 0x14 frame is <= 64 bytes and preserves calibrated LED offsets
    let zone_frames: Vec<&Vec<u8>> = reports[2..10].iter().map(|(_, r)| r).collect();
    for frame in &zone_frames {
        assert!(frame.len() <= 64);
        assert_eq!(frame[0], 0x14);
    }

    // LCtrl = LED 0 -> chunk 0, key offset 0
    assert_eq!(&zone_frames[0][3..6], &[11, 22, 33]);
    // LShift = LED 21 -> chunk 1 (16..32), key offset 5 => byte offset 3 + 5*3 = 18..21
    assert_eq!(&zone_frames[1][18..21], &[44, 55, 66]);
    // Enter = LED 56 -> chunk 3 (48..64), key offset 8 => byte offset 3 + 8*3 = 27..30
    assert_eq!(&zone_frames[3][27..30], &[77, 88, 99]);

    assert_eq!(
        transport.read_keyboard_brightness().expect("read b"),
        Some(255)
    );
}

#[test]
fn ite_lightbar_zones_lid_logo_and_ec_power_hook() {
    assert_eq!(EC_LIGHTBAR_POWER_REG_PRIMARY, 0x0748);
    assert_eq!(EC_LIGHTBAR_POWER_REG_SECONDARY, 0x0748);
    assert_eq!(LIGHTBAR_LID_LOGO_ZONE_ID, 5);

    // 1. 4-zone front Lightbar (048D:7001): no lid logo LED
    let mock_7001 = Arc::new(MockHidTransport::new(vec![HidDeviceInfo {
        vendor_id: ITE_VENDOR_ID,
        product_id: ITE_LIGHTBAR_PRODUCT_ID_4ZONE,
        path: "/dev/hidraw3".to_string(),
    }]));
    let transport_7001 = UniwillHidTransport::new(Arc::clone(&mock_7001) as _);
    assert_eq!(
        transport_7001
            .detect_lightbar()
            .expect("detect")
            .expect("present")
            .zones,
        4
    );
    assert!(!transport_7001.has_lid_logo().expect("has_lid_logo"));
    assert!(matches!(
        transport_7001.write_lid_logo_color(RgbColor::WHITE),
        Err(LightingError::Unavailable)
    ));

    let four_zones = [
        RgbColor::new(255, 0, 0),
        RgbColor::new(0, 255, 0),
        RgbColor::new(0, 0, 255),
        RgbColor::new(255, 255, 0),
    ];
    transport_7001
        .write_lightbar_zones(&four_zones)
        .expect("write 4 zones");
    let reports_7001 = mock_7001.recorded_reports();
    assert_eq!(reports_7001.len(), 4);
    assert_eq!(reports_7001[0].1, vec![0x14, 1, 255, 0, 0, 0, 0, 0]);
    assert_eq!(reports_7001[1].1, vec![0x14, 2, 0, 255, 0, 0, 0, 0]);
    assert_eq!(reports_7001[2].1, vec![0x14, 3, 0, 0, 255, 0, 0, 0]);
    assert_eq!(reports_7001[3].1, vec![0x14, 4, 255, 255, 0, 0, 0, 0]);

    // Passing 5 zones to a 4-zone controller must be rejected
    assert!(matches!(
        transport_7001.write_lightbar_zones(&[RgbColor::WHITE; 5]),
        Err(LightingError::InvalidConfig(_))
    ));

    // 2. 5-zone Lightbar + lid logo (048D:7000) with EC power hook (0x767/0x768)
    let ec_events: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(Vec::new()));
    let ec_events_cb = Arc::clone(&ec_events);
    let hook: EcLightbarPowerHook = Arc::new(move |enabled| {
        ec_events_cb
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(enabled);
        Ok(())
    });

    let mock_7000 = Arc::new(MockHidTransport::new(vec![HidDeviceInfo {
        vendor_id: ITE_VENDOR_ID,
        product_id: ITE_LIGHTBAR_PRODUCT_ID_5ZONE,
        path: "/dev/hidraw4".to_string(),
    }]));
    let transport_7000 =
        UniwillHidTransport::new(Arc::clone(&mock_7000) as _).with_ec_lightbar_power_hook(hook);

    assert_eq!(
        transport_7000
            .detect_lightbar()
            .expect("detect")
            .expect("present")
            .zones,
        5
    );
    assert!(transport_7000.has_lid_logo().expect("has_lid_logo"));

    // Turning brightness > 0 enables EC power rail, brightness == 0 disables EC power rail
    transport_7000
        .write_lightbar_brightness(200)
        .expect("brightness on");
    transport_7000
        .write_lid_logo_color(RgbColor::new(12, 34, 56))
        .expect("lid logo write");
    transport_7000
        .write_lightbar_brightness(0)
        .expect("brightness off");

    assert_eq!(
        *ec_events.lock().unwrap_or_else(|e| e.into_inner()),
        vec![true, false]
    );
    let reports_7000 = mock_7000.recorded_reports();
    assert_eq!(reports_7000.len(), 3);
    assert_eq!(reports_7000[0].1, vec![0x08, 0x01, 200, 0, 0, 0, 0, 0]);
    assert_eq!(reports_7000[1].1, vec![0x14, 5, 12, 34, 56, 0, 0, 0]);
    assert_eq!(reports_7000[2].1, vec![0x08, 0x01, 0, 0, 0, 0, 0, 0]);

    // Writing all 5 zones (4 front + lid logo) re-enables EC power automatically for non-black
    mock_7000.clear_reports();
    let five_zones = [
        RgbColor::new(1, 2, 3),
        RgbColor::new(4, 5, 6),
        RgbColor::new(7, 8, 9),
        RgbColor::new(10, 11, 12),
        RgbColor::new(200, 100, 50),
    ];
    transport_7000
        .write_lightbar_zones(&five_zones)
        .expect("write 5 zones");
    assert_eq!(
        *ec_events.lock().unwrap_or_else(|e| e.into_inner()),
        vec![true, false, true]
    );
    let zone_reports = mock_7000.recorded_reports();
    assert_eq!(zone_reports.len(), 5);
    assert_eq!(zone_reports[4].1, vec![0x14, 5, 200, 100, 50, 0, 0, 0]);
}

#[test]
fn lightbar_controller_over_hid_preserves_all_5_animations_and_flash_stays_off() {
    let mock_hid = Arc::new(MockHidTransport::new(vec![HidDeviceInfo {
        vendor_id: ITE_VENDOR_ID,
        product_id: ITE_LIGHTBAR_PRODUCT_ID_5ZONE,
        path: "/dev/hidraw4".to_string(),
    }]));
    let ec_events: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(Vec::new()));
    let ec_cb = Arc::clone(&ec_events);
    let transport = Arc::new(
        UniwillHidTransport::new(Arc::clone(&mock_hid) as _).with_ec_lightbar_power_hook(Arc::new(
            move |enabled| {
                ec_cb
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(enabled);
                Ok(())
            },
        )),
    );

    let mut lb = LightbarController::new(Arc::clone(&transport) as _);
    assert_eq!(lb.status_json()["available"], true);
    assert_eq!(lb.status_json()["zones"], 5);

    let base_rgb = RgbColor::new(0, 160, 240);
    for anim in LightbarAnimation::ALL {
        mock_hid.clear_reports();
        lb.configure(&json!({
            "managed": true,
            "silent_mode": false,
            "welcome_anim": false,
            "power_settings": {
                "ac": {
                    "enabled": true,
                    "brightness_val": 200,
                    "mode": anim.as_str(),
                    "color_mode": [base_rgb.r, base_rgb.g, base_rgb.b]
                }
            }
        }))
        .expect("configure animation");

        if anim == LightbarAnimation::Flash {
            assert!(lb.is_flash_off());
        } else {
            assert!(!lb.is_flash_off());
        }

        mock_hid.clear_reports();
        assert!(lb.trigger(true), "animation {:?} must trigger", anim);
        assert!(lb.is_animating());

        // Step mid-animation and verify 5-zone HID color reports are emitted
        lb.advance(anim.duration_ms() / 2);
        assert!(lb.is_animating());
        let mid_reports = mock_hid.recorded_reports();
        assert!(
            mid_reports.iter().any(|(_, r)| r.first() == Some(&0x14)),
            "animation {:?} must emit 0x14 zone reports",
            anim
        );

        // Complete animation
        mock_hid.clear_reports();
        lb.advance(anim.duration_ms());
        assert!(!lb.is_animating());

        let final_reports = mock_hid.recorded_reports();
        // 5 zones written on completion (4 front + lid logo zone 5)
        assert_eq!(final_reports.len(), 5);
        if anim == LightbarAnimation::Flash {
            assert!(lb.is_flash_off());
            for (idx, (_, r)) in final_reports.iter().enumerate() {
                assert_eq!(r, &vec![0x14, (idx + 1) as u8, 0, 0, 0, 0, 0, 0]);
            }
        } else {
            assert!(!lb.is_flash_off());
            for (idx, (_, r)) in final_reports.iter().enumerate() {
                assert_eq!(
                    r,
                    &vec![
                        0x14,
                        (idx + 1) as u8,
                        base_rgb.r,
                        base_rgb.g,
                        base_rgb.b,
                        0,
                        0,
                        0
                    ]
                );
            }
        }
    }

    assert!(
        ec_events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&true),
        "EC power hook must be invoked when Lightbar is active"
    );
}

#[test]
fn select_best_lighting_transport_respects_prefer_hidraw() {
    let hidraw_mock = Arc::new(MockLightingTransport::new());
    hidraw_mock.set_keyboard_caps(Some(KeyboardCapabilities {
        zones: 126,
        max_brightness: 255,
        max_red: 255,
        max_green: 255,
        max_blue: 255,
        led_ids: (0..126).collect(),
    }));

    let sysfs_mock = Arc::new(MockLightingTransport::new());
    sysfs_mock.set_lightbar_caps(Some(LightbarCapabilities {
        zones: 4,
        max_brightness: 255,
    }));

    // When prefer_hidraw = true and both detect hardware, hidraw wins
    let chosen = select_best_lighting_transport(
        true,
        Arc::clone(&hidraw_mock) as _,
        Arc::clone(&sysfs_mock) as _,
    );
    assert!(chosen.detect_keyboard().expect("detect").is_some());
    assert!(chosen.detect_lightbar().expect("detect").is_none());

    // When prefer_hidraw = false and both detect hardware, sysfs wins
    let chosen_sysfs = select_best_lighting_transport(
        false,
        Arc::clone(&hidraw_mock) as _,
        Arc::clone(&sysfs_mock) as _,
    );
    assert!(chosen_sysfs.detect_lightbar().expect("detect").is_some());
    assert!(chosen_sysfs.detect_keyboard().expect("detect").is_none());

    // Fallback when preferred transport finds no hardware
    let empty_hidraw = Arc::new(MockLightingTransport::new());
    let fallback = select_best_lighting_transport(
        true,
        Arc::clone(&empty_hidraw) as _,
        Arc::clone(&sysfs_mock) as _,
    );
    assert!(fallback.detect_lightbar().expect("detect").is_some());
}
