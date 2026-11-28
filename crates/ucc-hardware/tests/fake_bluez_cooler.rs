use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use ucc_hardware::cooler::*;
use ucc_hardware::BluezCoolerClient;
use zbus::zvariant::Value;
use zbus::{connection, interface};

/// Mock implementation of BlueZ Device1 interface
struct MockBluezDevice {
    connected: Arc<AtomicBool>,
    connect_calls: Arc<AtomicU32>,
}

#[interface(name = "org.bluez.Device1")]
impl MockBluezDevice {
    async fn connect(&self) -> zbus::fdo::Result<()> {
        self.connect_calls.fetch_add(1, Ordering::SeqCst);
        self.connected.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn disconnect(&self) -> zbus::fdo::Result<()> {
        self.connected.store(false, Ordering::SeqCst);
        Ok(())
    }

    #[zbus(property)]
    async fn connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    #[zbus(property)]
    async fn name(&self) -> String {
        "LCT21001-TEST".to_string()
    }

    #[zbus(property)]
    async fn uuids(&self) -> Vec<String> {
        vec![NORDIC_UART_SERVICE_UUID.to_string()]
    }
}

/// Mock implementation of BlueZ GattCharacteristic1 for TX (write commands)
struct MockTxCharacteristic {
    written_packets: Arc<Mutex<Vec<Vec<u8>>>>,
}

#[interface(name = "org.bluez.GattCharacteristic1")]
impl MockTxCharacteristic {
    async fn write_value(
        &self,
        value: Vec<u8>,
        _options: HashMap<String, Value<'_>>,
    ) -> zbus::fdo::Result<()> {
        let mut packets = self.written_packets.lock().await;
        packets.push(value);
        Ok(())
    }

    #[zbus(property)]
    async fn uuid(&self) -> String {
        NORDIC_UART_CHAR_TX.to_string()
    }
}

/// Mock implementation of BlueZ GattCharacteristic1 for RX (notifications)
struct MockRxCharacteristic {
    notifying: Arc<AtomicBool>,
    hang_start_notify: bool,
}

#[interface(name = "org.bluez.GattCharacteristic1")]
impl MockRxCharacteristic {
    async fn start_notify(&self) -> zbus::fdo::Result<()> {
        if self.hang_start_notify {
            std::future::pending::<()>().await;
        }
        self.notifying.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn stop_notify(&self) -> zbus::fdo::Result<()> {
        self.notifying.store(false, Ordering::SeqCst);
        Ok(())
    }

    #[zbus(property)]
    async fn notifying(&self) -> bool {
        self.notifying.load(Ordering::SeqCst)
    }

    #[zbus(property)]
    async fn uuid(&self) -> String {
        NORDIC_UART_CHAR_RX.to_string()
    }
}

#[tokio::test]
async fn test_fake_bluez_gatt_service_and_characteristics() {
    let connected = Arc::new(AtomicBool::new(false));
    let connect_calls = Arc::new(AtomicU32::new(0));
    let written_packets = Arc::new(Mutex::new(Vec::new()));
    let notifying = Arc::new(AtomicBool::new(false));

    let mock_dev = MockBluezDevice {
        connected: connected.clone(),
        connect_calls: connect_calls.clone(),
    };
    let mock_tx = MockTxCharacteristic {
        written_packets: written_packets.clone(),
    };
    let mock_rx = MockRxCharacteristic {
        notifying: notifying.clone(),
        hang_start_notify: false,
    };

    // Connect to the session bus with a test service name
    let bus_name = "org.test.bluez.watercooler";
    let server_conn = connection::Builder::session()
        .unwrap()
        .name(bus_name)
        .unwrap()
        .build()
        .await
        .unwrap();

    let client_conn = connection::Builder::session()
        .unwrap()
        .build()
        .await
        .unwrap();

    // Serve mock interfaces on server connection
    server_conn
        .object_server()
        .at("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF", mock_dev)
        .await
        .unwrap();

    server_conn
        .object_server()
        .at(
            "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF/service001/char002",
            mock_tx,
        )
        .await
        .unwrap();

    server_conn
        .object_server()
        .at(
            "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF/service001/char003",
            mock_rx,
        )
        .await
        .unwrap();

    // 1. Client connects to device
    let dev_proxy = zbus::Proxy::new(
        &client_conn,
        bus_name,
        "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF",
        "org.bluez.Device1",
    )
    .await
    .unwrap();

    dev_proxy.call_method("Connect", &()).await.unwrap();
    assert_eq!(connect_calls.load(Ordering::SeqCst), 1);
    assert!(connected.load(Ordering::SeqCst));

    // 2. Client discovers and enables RX notifications
    let rx_proxy = zbus::Proxy::new(
        &client_conn,
        bus_name,
        "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF/service001/char003",
        "org.bluez.GattCharacteristic1",
    )
    .await
    .unwrap();

    rx_proxy.call_method("StartNotify", &()).await.unwrap();
    assert!(notifying.load(Ordering::SeqCst));

    // 3. Client writes fan speed and pump voltage via TX characteristic
    let tx_proxy = zbus::Proxy::new(
        &client_conn,
        bus_name,
        "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF/service001/char002",
        "org.bluez.GattCharacteristic1",
    )
    .await
    .unwrap();

    let fan_cmd = CoolerPacketEncoder::fan_speed(75).unwrap();
    let empty_opts: HashMap<String, Value<'_>> = HashMap::new();
    tx_proxy
        .call_method("WriteValue", &(fan_cmd.to_vec(), empty_opts.clone()))
        .await
        .unwrap();

    let pump_cmd = CoolerPacketEncoder::pump_voltage(PumpVoltage::V7).unwrap();
    tx_proxy
        .call_method("WriteValue", &(pump_cmd.to_vec(), empty_opts))
        .await
        .unwrap();

    let received = written_packets.lock().await;
    assert_eq!(received.len(), 2);
    assert_eq!(received[0], fan_cmd);
    assert_eq!(received[1], pump_cmd);
}

struct MockObjectManager {
    dev_name: String,
    address: String,
    connected: Arc<AtomicBool>,
    require_connected_for_gatt: bool,
    hang_when_connected: bool,
}

#[interface(name = "org.freedesktop.DBus.ObjectManager")]
impl MockObjectManager {
    async fn get_managed_objects(
        &self,
    ) -> HashMap<
        zbus::zvariant::OwnedObjectPath,
        HashMap<String, HashMap<String, zbus::zvariant::OwnedValue>>,
    > {
        if self.hang_when_connected && self.connected.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        use zbus::zvariant::OwnedObjectPath;
        let mut root = HashMap::new();

        let is_connected = self.connected.load(Ordering::SeqCst);
        let mut dev_props: HashMap<String, zbus::zvariant::OwnedValue> = HashMap::new();
        dev_props.insert(
            "Address".into(),
            Value::from(self.address.clone()).try_into().unwrap(),
        );
        dev_props.insert(
            "Name".into(),
            Value::from(self.dev_name.clone()).try_into().unwrap(),
        );
        dev_props.insert(
            "Connected".into(),
            Value::from(is_connected).try_into().unwrap(),
        );
        dev_props.insert(
            "UUIDs".into(),
            Value::from(vec![NORDIC_UART_SERVICE_UUID.to_string()])
                .try_into()
                .unwrap(),
        );
        let mut dev_ifaces = HashMap::new();
        dev_ifaces.insert("org.bluez.Device1".into(), dev_props);
        root.insert(
            OwnedObjectPath::try_from("/org/bluez/hci0/dev_DE_C9_A0_A9_FC_84").unwrap(),
            dev_ifaces,
        );

        if self.require_connected_for_gatt && !is_connected {
            return root;
        }

        let mut tx_props: HashMap<String, zbus::zvariant::OwnedValue> = HashMap::new();
        tx_props.insert(
            "UUID".into(),
            Value::from(NORDIC_UART_CHAR_TX.to_string())
                .try_into()
                .unwrap(),
        );
        let mut tx_ifaces = HashMap::new();
        tx_ifaces.insert("org.bluez.GattCharacteristic1".into(), tx_props);
        root.insert(
            OwnedObjectPath::try_from("/org/bluez/hci0/dev_DE_C9_A0_A9_FC_84/service001/char002")
                .unwrap(),
            tx_ifaces,
        );

        let mut rx_props: HashMap<String, zbus::zvariant::OwnedValue> = HashMap::new();
        rx_props.insert(
            "UUID".into(),
            Value::from(NORDIC_UART_CHAR_RX.to_string())
                .try_into()
                .unwrap(),
        );
        let mut rx_ifaces = HashMap::new();
        rx_ifaces.insert("org.bluez.GattCharacteristic1".into(), rx_props);
        root.insert(
            OwnedObjectPath::try_from("/org/bluez/hci0/dev_DE_C9_A0_A9_FC_84/service001/char003")
                .unwrap(),
            rx_ifaces,
        );

        root
    }
}

#[tokio::test]
async fn production_bluez_client_discovers_connects_and_drives_cooler_controller() {
    let connected = Arc::new(AtomicBool::new(false));
    let connect_calls = Arc::new(AtomicU32::new(0));
    let written_packets = Arc::new(Mutex::new(Vec::new()));
    let notifying = Arc::new(AtomicBool::new(false));

    let bus_name = "org.test.bluez.production.stand";
    let server_conn = connection::Builder::session()
        .unwrap()
        .name(bus_name)
        .unwrap()
        .build()
        .await
        .unwrap();

    let client_conn = connection::Builder::session()
        .unwrap()
        .build()
        .await
        .unwrap();

    server_conn
        .object_server()
        .at(
            "/",
            MockObjectManager {
                dev_name: "CoolingSystem LCT22002".into(),
                address: "DE:C9:A0:A9:FC:84".into(),
                connected: connected.clone(),
                require_connected_for_gatt: false,
                hang_when_connected: false,
            },
        )
        .await
        .unwrap();
    server_conn
        .object_server()
        .at(
            "/org/bluez/hci0/dev_DE_C9_A0_A9_FC_84",
            MockBluezDevice {
                connected: connected.clone(),
                connect_calls: connect_calls.clone(),
            },
        )
        .await
        .unwrap();
    server_conn
        .object_server()
        .at(
            "/org/bluez/hci0/dev_DE_C9_A0_A9_FC_84/service001/char002",
            MockTxCharacteristic {
                written_packets: written_packets.clone(),
            },
        )
        .await
        .unwrap();
    server_conn
        .object_server()
        .at(
            "/org/bluez/hci0/dev_DE_C9_A0_A9_FC_84/service001/char003",
            MockRxCharacteristic {
                notifying: notifying.clone(),
                hang_start_notify: false,
            },
        )
        .await
        .unwrap();

    let bluez = BluezCoolerClient::with_bus_name(client_conn, bus_name);
    let found = bluez.discover_coolers().await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].model, ucc_hardware::LctDeviceModel::Lct22002);

    // Match by rotated MAC address via unique name fallback
    let saved = ucc_hardware::SavedCoolerIdentity {
        address: Some("11:22:33:44:55:66".into()),
        name: Some("CoolingSystem LCT22002".into()),
    };
    let chosen = ucc_hardware::select_cooler(&found, Some(&saved)).unwrap();
    let session = bluez.connect_cooler(chosen).await.unwrap();
    assert!(session.is_connected().await);
    assert!(notifying.load(Ordering::SeqCst));

    let mut controller = CoolerController::new();
    controller.on_connected();
    controller.set_fan_speed(70).unwrap();
    controller.set_pump_voltage(PumpVoltage::V8).unwrap();

    let t0 = std::time::Instant::now();
    assert_eq!(
        session.flush_controller(&mut controller, t0).await.unwrap(),
        1
    );
    let t1 = std::time::Instant::now() + BLE_WRITE_GAP;
    assert_eq!(
        session.flush_controller(&mut controller, t1).await.unwrap(),
        1
    );

    session.disconnect().await.unwrap();
    assert!(!session.is_connected().await);

    let pkts = written_packets.lock().await;
    assert_eq!(pkts.len(), 2);
    assert_eq!(pkts[0], [0xfe, 0x1b, 0x01, 70, 0x00, 0x00, 0x00, 0xef]);
    assert_eq!(pkts[1], [0xfe, 0x1c, 0x01, 60, 0x03, 0x00, 0x00, 0xef]);
}

#[tokio::test]
async fn connects_to_a_cooler_whose_gatt_appears_after_connect() {
    let connected = Arc::new(AtomicBool::new(false));
    let connect_calls = Arc::new(AtomicU32::new(0));
    let written_packets = Arc::new(Mutex::new(Vec::new()));
    let notifying = Arc::new(AtomicBool::new(false));

    let bus_name = "org.test.bluez.postconnect.stand";
    let server_conn = connection::Builder::session()
        .unwrap()
        .name(bus_name)
        .unwrap()
        .build()
        .await
        .unwrap();

    let client_conn = connection::Builder::session()
        .unwrap()
        .build()
        .await
        .unwrap();

    server_conn
        .object_server()
        .at(
            "/",
            MockObjectManager {
                dev_name: "CoolingSystem LCT22002".into(),
                address: "DE:C9:A0:A9:FC:84".into(),
                connected: connected.clone(),
                require_connected_for_gatt: true,
                hang_when_connected: false,
            },
        )
        .await
        .unwrap();
    server_conn
        .object_server()
        .at(
            "/org/bluez/hci0/dev_DE_C9_A0_A9_FC_84",
            MockBluezDevice {
                connected: connected.clone(),
                connect_calls: connect_calls.clone(),
            },
        )
        .await
        .unwrap();
    server_conn
        .object_server()
        .at(
            "/org/bluez/hci0/dev_DE_C9_A0_A9_FC_84/service001/char002",
            MockTxCharacteristic {
                written_packets: written_packets.clone(),
            },
        )
        .await
        .unwrap();
    server_conn
        .object_server()
        .at(
            "/org/bluez/hci0/dev_DE_C9_A0_A9_FC_84/service001/char003",
            MockRxCharacteristic {
                notifying: notifying.clone(),
                hang_start_notify: false,
            },
        )
        .await
        .unwrap();

    let bluez = BluezCoolerClient::with_bus_name(client_conn, bus_name);
    let found = bluez.discover_coolers().await.unwrap();
    assert_eq!(found.len(), 1);
    assert!(found[0].tx_char_path.is_none());

    let session = bluez
        .connect_cooler(&found[0])
        .await
        .expect("Connect must precede GATT resolution");
    assert!(session.is_connected().await);
    assert!(notifying.load(Ordering::SeqCst));

    let mut controller = CoolerController::new();
    controller.on_connected();
    controller.set_fan_speed(70).unwrap();
    assert_eq!(
        session
            .flush_controller(&mut controller, std::time::Instant::now())
            .await
            .unwrap(),
        1
    );
    let pkts = written_packets.lock().await;
    assert_eq!(
        pkts.as_slice(),
        &[[0xfe, 0x1b, 0x01, 70, 0x00, 0x00, 0x00, 0xef]]
    );
}

#[tokio::test]
async fn gatt_resolution_timeout_includes_the_dbus_rpc() {
    let connected = Arc::new(AtomicBool::new(false));
    let connect_calls = Arc::new(AtomicU32::new(0));
    let bus_name = "org.test.bluez.timeout.stand";

    let server_conn = connection::Builder::session()
        .unwrap()
        .name(bus_name)
        .unwrap()
        .build()
        .await
        .unwrap();
    let client_conn = connection::Builder::session()
        .unwrap()
        .build()
        .await
        .unwrap();

    server_conn
        .object_server()
        .at(
            "/",
            MockObjectManager {
                dev_name: "CoolingSystem LCT22002".into(),
                address: "DE:C9:A0:A9:FC:84".into(),
                connected: connected.clone(),
                require_connected_for_gatt: true,
                hang_when_connected: true,
            },
        )
        .await
        .unwrap();
    server_conn
        .object_server()
        .at(
            "/org/bluez/hci0/dev_DE_C9_A0_A9_FC_84",
            MockBluezDevice {
                connected: connected.clone(),
                connect_calls: connect_calls.clone(),
            },
        )
        .await
        .unwrap();

    let bluez = BluezCoolerClient::with_bus_name(client_conn, bus_name);
    let found = bluez.discover_coolers().await.unwrap();
    assert_eq!(found.len(), 1);
    assert!(found[0].tx_char_path.is_none());

    let result = tokio::time::timeout(
        std::time::Duration::from_millis(350),
        bluez.connect_cooler_with_timeout(&found[0], std::time::Duration::from_millis(30)),
    )
    .await;
    assert!(
        matches!(result, Ok(Err(_))),
        "GATT timeout did not bound GetManagedObjects: {result:?}"
    );
}

#[tokio::test]
async fn start_notify_timeout_obeys_gatt_deadline() {
    let connected = Arc::new(AtomicBool::new(false));
    let connect_calls = Arc::new(AtomicU32::new(0));
    let written_packets = Arc::new(Mutex::new(Vec::new()));
    let notifying = Arc::new(AtomicBool::new(false));
    let bus_name = "org.test.bluez.notify_timeout.stand";

    let server_conn = connection::Builder::session()
        .unwrap()
        .name(bus_name)
        .unwrap()
        .build()
        .await
        .unwrap();
    let client_conn = connection::Builder::session()
        .unwrap()
        .build()
        .await
        .unwrap();

    server_conn
        .object_server()
        .at(
            "/",
            MockObjectManager {
                dev_name: "CoolingSystem LCT22002".into(),
                address: "DE:C9:A0:A9:FC:84".into(),
                connected: connected.clone(),
                require_connected_for_gatt: false,
                hang_when_connected: false,
            },
        )
        .await
        .unwrap();
    server_conn
        .object_server()
        .at(
            "/org/bluez/hci0/dev_DE_C9_A0_A9_FC_84",
            MockBluezDevice {
                connected: connected.clone(),
                connect_calls: connect_calls.clone(),
            },
        )
        .await
        .unwrap();
    server_conn
        .object_server()
        .at(
            "/org/bluez/hci0/dev_DE_C9_A0_A9_FC_84/service001/char002",
            MockTxCharacteristic {
                written_packets: written_packets.clone(),
            },
        )
        .await
        .unwrap();
    server_conn
        .object_server()
        .at(
            "/org/bluez/hci0/dev_DE_C9_A0_A9_FC_84/service001/char003",
            MockRxCharacteristic {
                notifying: notifying.clone(),
                hang_start_notify: true,
            },
        )
        .await
        .unwrap();

    let bluez = BluezCoolerClient::with_bus_name(client_conn, bus_name);
    let found = bluez.discover_coolers().await.unwrap();
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(350),
        bluez.connect_cooler_with_timeout(&found[0], std::time::Duration::from_millis(30)),
    )
    .await;
    assert!(
        matches!(result, Ok(Err(_))),
        "GATT timeout did not bound StartNotify: {result:?}"
    );
}
