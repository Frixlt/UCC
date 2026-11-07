use std::sync::Arc;
use ucc_dbus::*;

#[test]
fn test_all_122_methods_accounted_for_in_status_json() {
    let json_bytes = include_bytes!("../../../docs/rust/execution/dbus-status.json");
    let json: serde_json::Value =
        serde_json::from_slice(json_bytes).expect("Failed to parse dbus-status.json");
    let methods = json["methods"]
        .as_array()
        .expect("methods field is not an array");
    assert_eq!(
        methods.len(),
        122,
        "dbus-status.json must contain exactly 122 baseline methods"
    );
}

#[tokio::test]
async fn test_dbus_server_and_client_proxies_live() {
    // Unique service path and service name on session bus for test isolation
    let unique_id = std::process::id();
    let service_name = format!("com.uniwill.uccd.test{unique_id}");
    let path = DBUS_PATH;

    let server_conn = zbus::connection::Builder::session()
        .unwrap()
        .name(service_name.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();

    let backend = Arc::new(DefaultUccdBackend);
    let authorizer = Arc::new(AllowAllAuthorizer);

    // Register both com.uniwill.uccd and de.uniwill.uccd
    register_uccd_service(&server_conn, backend, authorizer)
        .await
        .unwrap();

    let client_conn = zbus::connection::Builder::session()
        .unwrap()
        .build()
        .await
        .unwrap();

    // 1. Test Primary Proxy (com.uniwill.uccd)
    let primary_proxy = UccdProxy::builder(&client_conn)
        .destination(service_name.as_str())
        .unwrap()
        .path(path)
        .unwrap()
        .build()
        .await
        .unwrap();

    let dev_name = primary_proxy.get_device_name().await.unwrap();
    assert_eq!(dev_name, "Frixit Control Center Laptop");

    let is_supp = primary_proxy.is_device_supported().await.unwrap();
    assert!(is_supp);

    let pwr_state = primary_proxy.get_power_state().await.unwrap();
    assert_eq!(pwr_state, "power_ac");

    let disp_set = primary_proxy.set_display_brightness(75).await.unwrap();
    assert!(disp_set);

    let charge_start = primary_proxy.set_charge_start_threshold(45).await.unwrap();
    assert!(charge_start);

    let charge_end = primary_proxy.set_charge_end_threshold(85).await.unwrap();
    assert!(charge_end);

    let charge_type = primary_proxy.set_charge_type("Standard").await.unwrap();
    assert!(charge_type);

    let pump_volt = primary_proxy.set_water_cooler_pump_voltage(3).await.unwrap();
    assert!(pump_volt);

    let fan_spd = primary_proxy.set_water_cooler_fan_speed(60).await.unwrap();
    assert!(fan_spd);

    let odm_profs = primary_proxy.odm_profiles_available().await.unwrap();
    assert!(odm_profs.is_empty());

    let cpu_freq = primary_proxy.get_cpu_frequency_mhz().await.unwrap();
    assert_eq!(cpu_freq, 0);

    // 2. Test Legacy Proxy (de.uniwill.uccd alias)
    let legacy_proxy = UccdLegacyProxy::builder(&client_conn)
        .destination(service_name.as_str())
        .unwrap()
        .path(path)
        .unwrap()
        .build()
        .await
        .unwrap();

    let dev_name_leg = legacy_proxy.get_device_name().await.unwrap();
    assert_eq!(dev_name_leg, "Frixit Control Center Laptop");

    let is_supp_leg = legacy_proxy.is_device_supported().await.unwrap();
    assert!(is_supp_leg);

    let disp_set_leg = legacy_proxy.set_display_brightness(60).await.unwrap();
    assert!(disp_set_leg);

    let charge_start_leg = legacy_proxy.set_charge_start_threshold(50).await.unwrap();
    assert!(charge_start_leg);
}

#[tokio::test]
async fn test_dbus_server_authorization_rejection() {
    let unique_id = std::process::id();
    let service_name = format!("com.uniwill.uccd.deny{unique_id}");
    let path = DBUS_PATH;

    let server_conn = zbus::connection::Builder::session()
        .unwrap()
        .name(service_name.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();

    let backend = Arc::new(DefaultUccdBackend);
    let authorizer = Arc::new(DenyAllAuthorizer);

    register_uccd_service(&server_conn, backend, authorizer)
        .await
        .unwrap();

    let client_conn = zbus::connection::Builder::session()
        .unwrap()
        .build()
        .await
        .unwrap();

    let proxy = UccdProxy::builder(&client_conn)
        .destination(service_name.as_str())
        .unwrap()
        .path(path)
        .unwrap()
        .build()
        .await
        .unwrap();

    // Rejection on Control method -> returns false
    let disp = proxy.set_display_brightness(50).await.unwrap();
    assert!(!disp, "Denied Control method must return false");

    // Rejection on ManageHardware method -> returns false
    let charge = proxy.set_charge_start_threshold(50).await.unwrap();
    assert!(!charge, "Denied ManageHardware method must return false");

    // Rejection on Read JSON method -> returns default "{}"
    let prof_json = proxy.get_active_profile_json().await.unwrap();
    assert_eq!(prof_json, "{}", "Denied Read JSON method must return empty json object");
}

#[tokio::test]
async fn test_dbus_introspection_contains_all_122_methods_and_7_signals() {
    let unique_id = std::process::id();
    let service_name = format!("com.uniwill.uccd.intro{unique_id}");
    let path = DBUS_PATH;

    let server_conn = zbus::connection::Builder::session()
        .unwrap()
        .name(service_name.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();

    let backend = Arc::new(DefaultUccdBackend);
    let authorizer = Arc::new(AllowAllAuthorizer);

    register_uccd_service(&server_conn, backend, authorizer)
        .await
        .unwrap();

    let client_conn = zbus::connection::Builder::session()
        .unwrap()
        .build()
        .await
        .unwrap();

    let introspect_proxy = zbus::fdo::IntrospectableProxy::builder(&client_conn)
        .destination(service_name.as_str())
        .unwrap()
        .path(path)
        .unwrap()
        .build()
        .await
        .unwrap();

    let xml = introspect_proxy.introspect().await.unwrap();

    // Verify com.uniwill.uccd interface exists
    assert!(
        xml.contains("interface name=\"com.uniwill.uccd\""),
        "Missing com.uniwill.uccd interface in introspection"
    );

    // Verify de.uniwill.uccd interface exists
    assert!(
        xml.contains("interface name=\"de.uniwill.uccd\""),
        "Missing de.uniwill.uccd interface in introspection"
    );

    // Verify all 7 signals are in com.uniwill.uccd
    let signals = [
        "ProfileChanged",
        "ModeReapplyPendingChanged",
        "PowerStateChanged",
        "PowerPolicyChanged",
        "PowerDeviceStatusChanged",
        "LightbarSettingsChanged",
        "WaterCoolerStatusChanged",
    ];

    for sig in signals {
        assert!(
            xml.contains(&format!("signal name=\"{sig}\"")),
            "Missing signal '{sig}' in introspection XML"
        );
    }

    // Verify all 122 methods from dbus-status.json are in introspection XML
    let json_bytes = include_bytes!("../../../docs/rust/execution/dbus-status.json");
    let json: serde_json::Value =
        serde_json::from_slice(json_bytes).expect("Failed to parse dbus-status.json");
    let methods = json["methods"]
        .as_array()
        .expect("methods field is not an array");

    for m in methods {
        let m_name = m["name"].as_str().expect("name must be string");
        assert!(
            xml.contains(&format!("method name=\"{m_name}\"")),
            "Missing method '{m_name}' in introspection XML"
        );
    }
}
