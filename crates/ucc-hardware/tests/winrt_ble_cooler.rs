// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::Arc;
use ucc_hardware::bluez_cooler::{LctDeviceModel, SavedCoolerIdentity};
use ucc_hardware::cooler::{
    BleTransport, CoolerError, CoolerPacketEncoder, PumpVoltage, RgbState, CMD_PUMP,
    NORDIC_UART_CHAR_RX, NORDIC_UART_CHAR_TX, NORDIC_UART_SERVICE_UUID, PACKET_FOOTER,
    PACKET_HEADER,
};
use ucc_hardware::winrt_cooler::{
    format_ble_address_u64, nordic_uart_guids, parse_ble_address_u64, parse_winrt_guid,
    validate_cooler_tx_packet, MockWinRtGattBridge, WinRtBleDeviceInfo, WinRtBleError,
    WinRtCoolerTransport, WinRtGattBridge, WinRtGuid, NORDIC_UART_RX_CHAR_GUID,
    NORDIC_UART_SERVICE_GUID, NORDIC_UART_TX_CHAR_GUID,
};

#[test]
fn ble_mac_address_round_trips_with_winrt_u64() {
    let addr = parse_ble_address_u64("AA:BB:CC:DD:EE:FF").unwrap();
    assert_eq!(addr, 0x0000_AABB_CCDD_EEFF);
    assert_eq!(format_ble_address_u64(addr), "AA:BB:CC:DD:EE:FF");

    let lower = parse_ble_address_u64("01-23-45-67-89-ab").unwrap();
    assert_eq!(lower, 0x0000_0123_4567_89AB);
    assert_eq!(format_ble_address_u64(lower), "01:23:45:67:89:AB");

    assert!(matches!(
        parse_ble_address_u64("AA:BB:CC:DD:EE"),
        Err(WinRtBleError::InvalidAddress(_))
    ));
    assert!(matches!(
        parse_ble_address_u64("GG:BB:CC:DD:EE:FF"),
        Err(WinRtBleError::InvalidAddress(_))
    ));
    assert!(matches!(
        parse_ble_address_u64("AABBCCDDEEFF"),
        Err(WinRtBleError::InvalidAddress(_))
    ));
}

#[test]
fn winrt_guid_parses_nordic_uart_service_and_characteristics() {
    let (svc, tx, rx) = nordic_uart_guids().unwrap();
    assert_eq!(svc, NORDIC_UART_SERVICE_GUID);
    assert_eq!(tx, NORDIC_UART_TX_CHAR_GUID);
    assert_eq!(rx, NORDIC_UART_RX_CHAR_GUID);

    assert_eq!(svc.to_uuid_string(), NORDIC_UART_SERVICE_UUID);
    assert_eq!(tx.to_uuid_string(), NORDIC_UART_CHAR_TX);
    assert_eq!(rx.to_uuid_string(), NORDIC_UART_CHAR_RX);

    let braced = parse_winrt_guid("{6E400001-B5A3-F393-E0A9-E50E24DCCA9E}").unwrap();
    assert_eq!(braced, NORDIC_UART_SERVICE_GUID);

    assert!(matches!(
        parse_winrt_guid("6e400001-b5a3-f393-e0a9"),
        Err(WinRtBleError::InvalidGuid(_))
    ));
    assert!(matches!(
        parse_winrt_guid("{6e400001-b5a3-f393-e0a9-e50e24dcca9e"),
        Err(WinRtBleError::InvalidGuid(_))
    ));
}

#[test]
fn winrt_cooler_discovery_filters_non_lct_nordic_uart_peripherals() {
    let bridge = Arc::new(MockWinRtGattBridge::new());
    bridge.add_lct_device("11:22:33:44:55:66", "LCT21001").unwrap();
    bridge.add_lct_device("AA:BB:CC:DD:EE:02", "LCT22002-Unit").unwrap();
    bridge.add_device(
        WinRtBleDeviceInfo {
            bluetooth_address: parse_ble_address_u64("DE:AD:BE:EF:00:01").unwrap(),
            name: "Nordic_UART_Watch".to_string(),
            rssi_dbm: -60,
            advertised_services: vec![NORDIC_UART_SERVICE_GUID],
        },
        true,
        true,
        true,
    );

    let transport = WinRtCoolerTransport::new(Arc::clone(&bridge) as _);
    let found = transport.discover_lct_coolers().unwrap();
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].lct_model(), Some(LctDeviceModel::Lct21001));
    assert_eq!(found[1].lct_model(), Some(LctDeviceModel::Lct22002));
}

#[test]
fn winrt_cooler_connects_writes_valid_packets_and_disconnects() {
    let bridge = Arc::new(MockWinRtGattBridge::new());
    let addr = bridge
        .add_lct_device("AA:11:BB:22:CC:33", "LCT22002")
        .unwrap();
    let transport = WinRtCoolerTransport::new(Arc::clone(&bridge) as _);

    assert!(!transport.is_connected());
    assert_eq!(
        transport.write_packet(CoolerPacketEncoder::reset()),
        Err(CoolerError::NotConnected)
    );

    let handles = transport.connect_mac("AA:11:BB:22:CC:33").unwrap();
    assert_eq!(handles.bluetooth_address, addr);
    assert!(handles.rx_char_handle.is_some());
    assert!(transport.is_connected());

    let fan_pkt = CoolerPacketEncoder::fan_speed(75).unwrap();
    let pump_pkt = CoolerPacketEncoder::pump_voltage(PumpVoltage::V11).unwrap();
    let rgb_pkt = CoolerPacketEncoder::rgb(10, 128, 255, RgbState::Static);

    transport.write_packet(fan_pkt).unwrap();
    transport.write_packet(pump_pkt).unwrap();
    transport.write_packet(rgb_pkt).unwrap();

    let writes = bridge.written_packets();
    assert_eq!(writes.len(), 3);
    assert_eq!(writes[0], (addr, handles.tx_char_handle, fan_pkt.to_vec()));
    assert_eq!(writes[1], (addr, handles.tx_char_handle, pump_pkt.to_vec()));
    assert_eq!(writes[2], (addr, handles.tx_char_handle, rgb_pkt.to_vec()));

    transport.disconnect().unwrap();
    assert!(!transport.is_connected());
    assert!(transport.active_session().is_none());
}

#[test]
fn winrt_cooler_forbids_pump_voltage_v12_raw_packets_invariant_4() {
    let bridge = Arc::new(MockWinRtGattBridge::new());
    bridge
        .add_lct_device("AA:BB:CC:DD:EE:FF", "LCT21001")
        .unwrap();
    let transport = WinRtCoolerTransport::new(Arc::clone(&bridge) as _);
    transport.connect_mac("AA:BB:CC:DD:EE:FF").unwrap();

    // Raw CMD_PUMP frame carrying forbidden voltage code 1 (V12) must be rejected before BLE TX.
    let raw_v12_packet = [PACKET_HEADER, CMD_PUMP, 0x01, 60, 0x01, 0x00, 0x00, PACKET_FOOTER];
    assert_eq!(
        validate_cooler_tx_packet(&raw_v12_packet),
        Err(CoolerError::V12Forbidden)
    );
    assert_eq!(
        transport.write_packet(raw_v12_packet),
        Err(CoolerError::V12Forbidden)
    );
    assert!(bridge.written_packets().is_empty());

    // Valid pump voltages (V11=0, V7=2, V8=3, Off=4) are permitted.
    for voltage in [
        PumpVoltage::V11,
        PumpVoltage::V7,
        PumpVoltage::V8,
        PumpVoltage::Off,
    ] {
        let pkt = CoolerPacketEncoder::pump_voltage(voltage).unwrap();
        transport.write_packet(pkt).unwrap();
    }
    assert_eq!(bridge.written_packets().len(), 4);

    // Out-of-range pump voltage code (e.g. 9) is also rejected.
    let invalid_voltage_pkt = [PACKET_HEADER, CMD_PUMP, 0x01, 60, 0x09, 0x00, 0x00, PACKET_FOOTER];
    assert_eq!(
        transport.write_packet(invalid_voltage_pkt),
        Err(CoolerError::InvalidVoltageCode(9))
    );
}

#[test]
fn winrt_cooler_handles_missing_gatt_service_and_disconnects_cleanly() {
    let bridge = Arc::new(MockWinRtGattBridge::new());
    let addr = parse_ble_address_u64("12:34:56:78:9A:BC").unwrap();
    bridge.add_device(
        WinRtBleDeviceInfo {
            bluetooth_address: addr,
            name: "LCT21001".to_string(),
            rssi_dbm: -55,
            advertised_services: vec![WinRtGuid {
                data1: 0x0000_1800,
                data2: 0x0000,
                data3: 0x1000,
                data4: [0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34, 0xfb],
            }],
        },
        false,
        false,
        false,
    );

    let transport = WinRtCoolerTransport::new(Arc::clone(&bridge) as _);
    let err = transport.connect_address(addr).unwrap_err();
    assert!(matches!(err, WinRtBleError::ServiceNotFound { .. }));
    assert!(!transport.is_connected());
    assert!(!bridge.is_connected(addr));
}

#[test]
fn winrt_cooler_discover_and_connect_prefers_saved_identity() {
    let bridge = Arc::new(MockWinRtGattBridge::new());
    bridge
        .add_lct_device("00:11:22:33:44:01", "LCT21001")
        .unwrap();
    let target_addr = bridge
        .add_lct_device("00:11:22:33:44:02", "LCT22002")
        .unwrap();

    let transport = WinRtCoolerTransport::new(Arc::clone(&bridge) as _);
    let saved = SavedCoolerIdentity {
        address: Some("00:11:22:33:44:02".to_string()),
        name: Some("LCT22002".to_string()),
    };

    let connected = transport.discover_and_connect(Some(&saved)).unwrap();
    assert_eq!(connected.bluetooth_address, target_addr);
    assert!(transport.is_connected());

    // Simulated link drop is immediately reflected by BleTransport::is_connected and write_packet.
    bridge.simulate_disconnect(target_addr);
    assert!(!transport.is_connected());
    assert_eq!(
        transport.write_packet(CoolerPacketEncoder::reset()),
        Err(CoolerError::NotConnected)
    );
}
