use serde_json::Value;
use std::{collections::HashMap, process::Command, sync::Arc, time::SystemTime};
use ucc_dbus::{register_uccd_service, AllowAllAuthorizer, UccdBackend, DBUS_PATH};
use zbus::{connection::Builder, zvariant::OwnedValue};

struct WideSample;
impl UccdBackend for WideSample {
    fn get_fan_data_cpu(&self) -> HashMap<String, OwnedValue> {
        HashMap::from([("speed".to_string(), OwnedValue::from((1_u64 << 32) + 65))])
    }
}

struct SlowService;
#[zbus::interface(name = "com.uniwill.uccd")]
impl SlowService {
    #[zbus(name = "GetDeviceName")]
    fn device_name(&self) -> &str {
        "audit"
    }

    #[zbus(name = "GetSystemInfoJSON")]
    async fn system_info(&self) -> String {
        std::future::pending().await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_does_not_wrap_out_of_range_dbus_fan_sample() {
    let unique_id = format!(
        "{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let bus_name = format!("com.uniwill.uccd.sample_{unique_id}");
    let conn = Builder::session()
        .unwrap()
        .name(bus_name.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    register_uccd_service(&conn, Arc::new(WideSample), Arc::new(AllowAllAuthorizer))
        .await
        .unwrap();
    let output = Command::new("timeout")
        .args(["5s", env!("CARGO_BIN_EXE_ucc-cli"), "status", "--json"])
        .env("UCCD_DBUS_SERVICE", &bus_name)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let data: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        data["cpu"]["fanPercent"].is_null(),
        "fake reading accepted: {}",
        data["cpu"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_handshake_status_rpc_has_a_deadline() {
    let unique_id = format!(
        "{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let bus_name = format!("com.uniwill.uccd.deadline_{unique_id}");
    let conn = Builder::session()
        .unwrap()
        .name(bus_name.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    conn.object_server()
        .at(DBUS_PATH, SlowService)
        .await
        .unwrap();
    let output = Command::new("timeout")
        .args(["5s", env!("CARGO_BIN_EXE_ucc-cli"), "status", "--json"])
        .env("UCCD_DBUS_SERVICE", &bus_name)
        .output()
        .unwrap();
    assert_ne!(
        output.status.code(),
        Some(124),
        "handshake completed, command blocked waiting for later RPC"
    );
}
