// SPDX-License-Identifier: GPL-3.0-or-later

//! Regression tests for V2-P2-13 in `ucc-lighting`:
//! - `read_sysfs_hidraw_interface_number` checks `device/../bInterfaceNumber` first
//!   (matching real Linux USB HID sysfs layout) and falls back to `device/bInterfaceNumber`.

use std::fs;
use tempfile::TempDir;
use ucc_lighting::hid_transport::read_sysfs_hidraw_interface_number;

#[test]
fn v2_p2_13_read_sysfs_hidraw_interface_number_checks_parent_then_device() {
    let tmp = TempDir::new().expect("temp dir");

    // 1. Real Linux USB HID layout: hidraw0/device -> .../usb_iface/hid_dev
    //    where usb_iface/bInterfaceNumber is "01".
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let usb_iface = tmp.path().join("usb_bus/1-1:1.1");
        let hid_dev = usb_iface.join("0003:048D:600B.0001");
        fs::create_dir_all(&hid_dev).expect("create hid_dev");
        fs::write(usb_iface.join("bInterfaceNumber"), "01\n").expect("write parent iface");
        // Put a different value inside hid_dev to prove device/../bInterfaceNumber takes priority.
        fs::write(hid_dev.join("bInterfaceNumber"), "00\n").expect("write child iface");

        let hidraw0 = tmp.path().join("hidraw0");
        fs::create_dir_all(&hidraw0).expect("create hidraw0");
        symlink(&hid_dev, hidraw0.join("device")).expect("symlink device");

        assert_eq!(
            read_sysfs_hidraw_interface_number(tmp.path(), "hidraw0"),
            Some(1),
            "V2-P2-13: device/../bInterfaceNumber must be checked before device/bInterfaceNumber"
        );
    }

    // 2. Fallback layout: only hidraw1/device/bInterfaceNumber exists.
    let hidraw1_dev = tmp.path().join("hidraw1/device");
    fs::create_dir_all(&hidraw1_dev).expect("create hidraw1/device");
    fs::write(hidraw1_dev.join("bInterfaceNumber"), "01\n").expect("write fallback iface");
    assert_eq!(
        read_sysfs_hidraw_interface_number(tmp.path(), "hidraw1"),
        Some(1),
        "V2-P2-13: must fall back to device/bInterfaceNumber when parent does not exist"
    );

    // 3. Missing bInterfaceNumber returns None.
    let hidraw2_dev = tmp.path().join("hidraw2/device");
    fs::create_dir_all(&hidraw2_dev).expect("create hidraw2/device");
    assert_eq!(
        read_sysfs_hidraw_interface_number(tmp.path(), "hidraw2"),
        None
    );
}
