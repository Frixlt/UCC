// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::Arc;
use ucc_lighting::{
    HidDeviceInfo, LightingTransport, MockHidTransport, RgbColor, UniwillHidTransport,
    ITE_KEYBOARD_PRODUCT_ID, ITE_VENDOR_ID,
};

#[test]
fn uniwill_hid_transport_handles_keyboard_lightbar_and_logo_zones() {
    let hid = Arc::new(MockHidTransport::new(vec![
        HidDeviceInfo {
            vendor_id: ITE_VENDOR_ID,
            product_id: ITE_KEYBOARD_PRODUCT_ID,
            path: "/dev/hidraw0".to_string(),
        },
        HidDeviceInfo {
            vendor_id: ITE_VENDOR_ID,
            product_id: 0x7000,
            path: "/dev/hidraw1".to_string(),
        },
    ]));
    let transport = UniwillHidTransport::new(Arc::clone(&hid) as _);

    let kbd = transport.detect_keyboard().unwrap().unwrap();
    assert_eq!(kbd.zones, 126);

    let lb = transport.detect_lightbar().unwrap().unwrap();
    assert_eq!(lb.zones, 5); // 4 front zones + lid logo LED (048D:7000)

    transport.set_keyboard_buffer_input(true).unwrap();
    transport.write_keyboard_brightness(200).unwrap();
    transport
        .write_keyboard_zones(&[RgbColor::new(10, 20, 30)])
        .unwrap();
    assert_eq!(
        transport.read_keyboard_colors().unwrap(),
        vec![RgbColor::new(10, 20, 30)]
    );

    transport.write_lightbar_brightness(180).unwrap();
    transport
        .write_lightbar_color(RgbColor::new(255, 64, 0))
        .unwrap();
    assert_eq!(hid.recorded_reports().len(), 9);

    hid.set_fail_writes(true);
    assert!(transport.write_lightbar_color(RgbColor::default()).is_err());
}
