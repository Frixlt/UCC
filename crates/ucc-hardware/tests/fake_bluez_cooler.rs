use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use ucc_hardware::cooler::*;
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
}

#[interface(name = "org.bluez.GattCharacteristic1")]
impl MockRxCharacteristic {
    async fn start_notify(&self) -> zbus::fdo::Result<()> {
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
