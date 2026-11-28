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

#[test]
fn p0_10_hid_keyboard_does_not_send_380_byte_feature_report() {
    use ucc_lighting::{
        HidDeviceInfo, MockHidTransport, UniwillHidTransport, ITE_KEYBOARD_PRODUCT_ID,
        ITE_VENDOR_ID,
    };

    let mock_hid = Arc::new(MockHidTransport::new(vec![HidDeviceInfo {
        vendor_id: ITE_VENDOR_ID,
        product_id: ITE_KEYBOARD_PRODUCT_ID,
        path: "/dev/hidraw0".to_string(),
    }]));
    let transport = UniwillHidTransport::new(Arc::clone(&mock_hid) as _);

    let colors = vec![RgbColor::new(255, 128, 64); 126];
    transport.write_keyboard_zones(&colors).unwrap();

    let reports = mock_hid.recorded_reports();
    assert!(
        !reports.is_empty(),
        "P0-10: write_keyboard_zones must emit HID feature reports"
    );
    for (idx, (_, report)) in reports.iter().enumerate() {
        assert!(
            report.len() <= 64,
            "P0-10: HID keyboard report #{idx} is {} bytes (> 64-byte USB HID limit; 126-zone write must be chunked, not a single 380-byte frame)",
            report.len()
        );
    }
}

#[test]
fn p0_11_hid_lightbar_addresses_all_zones_and_caps_600b_brightness_to_50() {
    use ucc_lighting::{
        HidDeviceInfo, MockHidTransport, UniwillHidTransport, ITE_KEYBOARD_PRODUCT_ID,
        ITE_LIGHTBAR_PRODUCT_IDS, ITE_VENDOR_ID,
    };

    let mock_hid = Arc::new(MockHidTransport::new(vec![
        HidDeviceInfo {
            vendor_id: ITE_VENDOR_ID,
            product_id: ITE_KEYBOARD_PRODUCT_ID,
            path: "/dev/hidraw0".to_string(),
        },
        HidDeviceInfo {
            vendor_id: ITE_VENDOR_ID,
            product_id: ITE_LIGHTBAR_PRODUCT_IDS[0], // 0x7001 (4 zones)
            path: "/dev/hidraw1".to_string(),
        },
    ]));
    let transport = UniwillHidTransport::new(Arc::clone(&mock_hid) as _);

    // 1. ITE 048D:600B hardware brightness register max is 50 (0x32), not 255.
    transport.write_keyboard_brightness(255).unwrap();
    let kbd_reports: Vec<Vec<u8>> = mock_hid
        .recorded_reports()
        .into_iter()
        .filter(|(p, _)| p == "/dev/hidraw0")
        .map(|(_, r)| r)
        .collect();
    assert!(!kbd_reports.is_empty());
    let brightness_byte = kbd_reports[0].get(4).copied().unwrap_or(255);
    assert!(
        brightness_byte <= 50,
        "P0-11: ITE 048D:600B keyboard brightness byte must be capped/scaled to <= 50, got {brightness_byte}"
    );

    // 2. Lightbar 0x7001 static mode (0x08 ... 0x01) reads 1-based slot 1; write_lightbar_color writes slots 0 and 1.
    transport
        .write_lightbar_color(RgbColor::new(10, 20, 30))
        .unwrap();
    let lb_reports: Vec<Vec<u8>> = mock_hid
        .recorded_reports()
        .into_iter()
        .filter(|(p, _)| p == "/dev/hidraw1")
        .map(|(_, r)| r)
        .collect();
    assert_eq!(
        lb_reports.len(),
        2,
        "P0-11: write_lightbar_color on 048D:7001 must write slots 0 and 1, sent {} report(s)",
        lb_reports.len()
    );
}

#[test]
fn p0_12_hid_find_device_filters_vendor_usage_page_or_interface() {
    use std::path::Path;

    let hid_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/hid_transport.rs");
    let source = fs::read_to_string(&hid_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {e}", hid_path.display()));

    assert!(
        source.contains("usage_page") || source.contains("interface_number"),
        "P0-12: hid_transport.rs must filter HID interfaces by vendor usage_page or interface_number instead of matching only VID/PID"
    );
}

#[test]
fn m_lgt_06_brightness_cache_not_poisoned_on_write_error() {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use ucc_hardware::{MockSysfs, Sysfs};

    struct FailBrightnessSysfs {
        inner: MockSysfs,
        fail_brightness: AtomicBool,
    }

    impl Sysfs for FailBrightnessSysfs {
        fn read(&self, path: &Path) -> Result<String, std::io::Error> {
            self.inner.read(path)
        }

        fn write(&self, path: &Path, content: &str) -> Result<(), std::io::Error> {
            if self.fail_brightness.load(Ordering::SeqCst)
                && path.file_name().and_then(|n| n.to_str()) == Some("brightness")
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected brightness write failure",
                ));
            }
            self.inner.write(path, content)
        }

        fn exists(&self, path: &Path) -> bool {
            self.inner.exists(path)
        }

        fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
            self.inner.read_dir(path)
        }
    }

    let sysfs = Arc::new(FailBrightnessSysfs {
        inner: MockSysfs::new(),
        fail_brightness: AtomicBool::new(false),
    });

    // Seed a 4-zone HID keyboard so detect_keyboard succeeds on 1d07126 regardless of N17.
    let base = Path::new("/sys/bus/hid/drivers/ite_8291/0003:048D:600B.0001/leds");
    for (idx, suffix) in ["", "_1", "_2", "_3"].iter().enumerate() {
        let led = base.join(format!("rgb:kbd_backlight{suffix}"));
        sysfs.inner.set_file(led.join("max_brightness"), "255");
        sysfs.inner.set_file(led.join("brightness"), "100");
        sysfs
            .inner
            .set_file(led.join("multi_intensity"), "10 20 30");
        let _ = idx;
    }

    let transport = SysfsLedTransport::new(Arc::clone(&sysfs) as Arc<dyn Sysfs>, Path::new("/"));
    assert!(transport.detect_keyboard().unwrap().is_some());

    // Fail `write_keyboard_brightness(200)` — cache must NOT be poisoned with 200.
    sysfs.fail_brightness.store(true, Ordering::SeqCst);
    let write_err = transport.write_keyboard_brightness(200);
    assert!(
        write_err.is_err(),
        "expected injected brightness write failure"
    );

    // Re-enable writes and update zone colors; brightness on hardware must remain 100, NOT 200.
    sysfs.fail_brightness.store(false, Ordering::SeqCst);
    transport
        .write_keyboard_zones(&[RgbColor::new(40, 50, 60); 4])
        .unwrap();

    let actual_brightness = sysfs
        .read(&base.join("rgb:kbd_backlight/brightness"))
        .unwrap();
    assert_eq!(
        actual_brightness.trim(),
        "100",
        "M-LGT-06: failed write_keyboard_brightness(200) poisoned last_kbd_brightness cache and overwrote hardware brightness with {actual_brightness}"
    );
}

#[test]
fn m_lgt_08_compact_partial_update_preserves_current_states_len() {
    let mock = Arc::new(MockLightingTransport::new());
    mock.set_keyboard_caps(Some(KeyboardCapabilities {
        zones: 4,
        max_brightness: 255,
        max_red: 255,
        max_green: 255,
        max_blue: 255,
        led_ids: vec![0, 1, 2, 3],
    }));

    let mut controller = KeyboardBacklightController::new(Arc::clone(&mock) as _);
    controller.init().unwrap();

    let full_states = vec![
        KeyboardBacklightState {
            brightness: 128,
            red: 10,
            green: 20,
            blue: 30,
        },
        KeyboardBacklightState {
            brightness: 128,
            red: 40,
            green: 50,
            blue: 60,
        },
        KeyboardBacklightState {
            brightness: 128,
            red: 70,
            green: 80,
            blue: 90,
        },
        KeyboardBacklightState {
            brightness: 128,
            red: 100,
            green: 110,
            blue: 120,
        },
    ];
    controller.apply_states(&full_states).unwrap();
    assert_eq!(controller.current_states().len(), 4);

    // Apply a compact 1-zone partial update: untouched zones 1..3 must remain in `current_states`.
    let partial = [KeyboardBacklightState {
        brightness: 200,
        red: 9,
        green: 8,
        blue: 7,
    }];
    controller.apply_states(&partial).unwrap();

    assert_eq!(
        controller.current_states().len(),
        4,
        "M-LGT-08: compact partial apply_states truncated current_states from 4 to {}",
        controller.current_states().len()
    );
}

#[test]
fn m_lgt_11_disco_animation_no_overflow_on_u64_max_and_hue_edge() {
    use std::path::Path;
    use ucc_lighting::LightbarAnimation;

    // On 1d07126 debug builds, `elapsed_ms / 90 * 137 + 41` panics with arithmetic overflow at u64::MAX.
    let base = RgbColor::new(255, 255, 255);
    let _ = LightbarAnimation::Disco.frame(u64::MAX, base);
    let _ = LightbarAnimation::Disco.frame(u64::MAX - 30, base);
    let _ = LightbarAnimation::Disco.frame(0, base);
    let _ = LightbarAnimation::Rainbow.frame(u64::MAX, base);

    // Also guard against overflow in release builds where wrapping hides the bug silently.
    let lb_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lightbar.rs");
    let source = fs::read_to_string(&lb_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {e}", lb_path.display()));
    assert!(
        !source.contains("(elapsed_ms / 90 * 137 + 41) % 360"),
        "M-LGT-11: Disco animation must reduce modulo 360 (or use wrapping/u128 arithmetic) before multiplying by 137"
    );
}
