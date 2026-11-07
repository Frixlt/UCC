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

// ==========================================================================
// Comprehensive Contract Test Stand for Methods, Signals, and Fault-Injection
// ==========================================================================

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Mutex;
use std::time::SystemTime;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedValue;

#[derive(Clone)]
pub struct MockContractBackend {
    pub active_profile_id: Arc<Mutex<String>>,
    pub active_profile_json: Arc<Mutex<String>>,
    pub power_state: Arc<Mutex<String>>,
    pub power_policy_json: Arc<Mutex<String>>,
    pub display_brightness: Arc<AtomicI32>,
    pub water_cooler_connected: Arc<AtomicBool>,
    pub water_cooler_fan_speed: Arc<AtomicI32>,
    pub water_cooler_pump_level: Arc<AtomicI32>,
    pub water_cooler_enabled: Arc<AtomicBool>,
    pub water_cooler_auto_control: Arc<AtomicBool>,
    pub charging_profile: Arc<Mutex<String>>,
    pub charge_start_threshold: Arc<AtomicI32>,
    pub charge_end_threshold: Arc<AtomicI32>,
    pub charge_type: Arc<Mutex<String>>,
    pub lightbar_settings: Arc<Mutex<String>>,
    pub lightbar_status: Arc<Mutex<String>>,
    pub custom_profiles: Arc<Mutex<HashMap<String, String>>>,
    pub state_map: Arc<Mutex<HashMap<String, String>>>,
    pub power_requests: Arc<Mutex<HashMap<String, String>>>,
}

impl Default for MockContractBackend {
    fn default() -> Self {
        let mut custom = HashMap::new();
        custom.insert(
            "custom-01".to_string(),
            r#"{"id":"custom-01","name":"Custom Gaming"}"#.to_string(),
        );
        Self {
            active_profile_id: Arc::new(Mutex::new("office".to_string())),
            active_profile_json: Arc::new(Mutex::new(
                r#"{"id":"office","name":"Office Profile"}"#.to_string(),
            )),
            power_state: Arc::new(Mutex::new("power_ac".to_string())),
            power_policy_json: Arc::new(Mutex::new(r#"{"policy":"balanced"}"#.to_string())),
            display_brightness: Arc::new(AtomicI32::new(75)),
            water_cooler_connected: Arc::new(AtomicBool::new(false)),
            water_cooler_fan_speed: Arc::new(AtomicI32::new(40)),
            water_cooler_pump_level: Arc::new(AtomicI32::new(2)),
            water_cooler_enabled: Arc::new(AtomicBool::new(true)),
            water_cooler_auto_control: Arc::new(AtomicBool::new(true)),
            charging_profile: Arc::new(Mutex::new("balanced".to_string())),
            charge_start_threshold: Arc::new(AtomicI32::new(60)),
            charge_end_threshold: Arc::new(AtomicI32::new(80)),
            charge_type: Arc::new(Mutex::new("Standard".to_string())),
            lightbar_settings: Arc::new(Mutex::new(
                r#"{"mode":"static","brightness":60}"#.to_string(),
            )),
            lightbar_status: Arc::new(Mutex::new(r#"{"active":true}"#.to_string())),
            custom_profiles: Arc::new(Mutex::new(custom)),
            state_map: Arc::new(Mutex::new(HashMap::new())),
            power_requests: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl UccdBackend for MockContractBackend {
    fn get_device_name(&self) -> String {
        "Frixit LAPTOP Test Stand".to_string()
    }

    fn get_system_info_json(&self) -> String {
        r#"{"cpu":"AMD Ryzen 7 7840HS","gpu":"NVIDIA RTX 4060","ramGb":32}"#.to_string()
    }

    fn is_device_supported(&self) -> bool {
        true
    }

    fn uccd_version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }

    fn get_power_state(&self) -> String {
        self.power_state.lock().unwrap().clone()
    }

    fn get_active_profile_json(&self) -> String {
        self.active_profile_json.lock().unwrap().clone()
    }

    fn get_power_policy_json(&self) -> String {
        self.power_policy_json.lock().unwrap().clone()
    }

    fn get_settings_json(&self) -> String {
        r#"{"version":2,"profiles":{}}"#.to_string()
    }

    fn set_manual_power_mode(&self, mode: &str) -> bool {
        *self.power_state.lock().unwrap() = mode.to_string();
        true
    }

    fn register_power_request(&self, name: &str, _reason: &str, mode: &str) -> bool {
        self.power_requests
            .lock()
            .unwrap()
            .insert(name.to_string(), mode.to_string());
        true
    }

    fn unregister_power_request(&self, name: &str) -> bool {
        self.power_requests.lock().unwrap().remove(name).is_some()
    }

    fn set_state_map(&self, state: &str, profile_id: &str) -> bool {
        self.state_map
            .lock()
            .unwrap()
            .insert(state.to_string(), profile_id.to_string());
        true
    }

    fn set_batch_state_map(&self, state_map_json: &str) -> bool {
        let map: Result<HashMap<String, String>, _> = serde_json::from_str(state_map_json);
        match map {
            Ok(m) => {
                let mut current = self.state_map.lock().unwrap();
                for (k, v) in m {
                    current.insert(k, v);
                }
                true
            }
            Err(_) => false,
        }
    }

    fn set_active_profile(&self, profile_id: &str) -> bool {
        let known = ["office", "balanced", "performance"];
        let is_custom = self.custom_profiles.lock().unwrap().contains_key(profile_id);
        if known.contains(&profile_id) || is_custom {
            *self.active_profile_id.lock().unwrap() = profile_id.to_string();
            *self.active_profile_json.lock().unwrap() = format!(r#"{{"id":"{profile_id}"}}"#);
            true
        } else {
            false
        }
    }

    fn apply_profile(&self, profile_json: &str) -> bool {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(profile_json) {
            if val.is_object() {
                *self.active_profile_json.lock().unwrap() = profile_json.to_string();
                return true;
            }
        }
        false
    }

    fn save_custom_profile(&self, profile_json: &str) -> bool {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(profile_json) {
            if let Some(id) = val.get("id").and_then(|i| i.as_str()) {
                self.custom_profiles
                    .lock()
                    .unwrap()
                    .insert(id.to_string(), profile_json.to_string());
                return true;
            }
        }
        false
    }

    fn delete_custom_profile(&self, profile_id: &str) -> bool {
        self.custom_profiles
            .lock()
            .unwrap()
            .remove(profile_id)
            .is_some()
    }

    fn get_fan_data_cpu(&self) -> HashMap<String, OwnedValue> {
        let mut map = HashMap::new();
        map.insert("temp".to_string(), OwnedValue::from(55));
        map.insert("fanSpeedRPM".to_string(), OwnedValue::from(2400));
        map.insert("fanSpeedPercent".to_string(), OwnedValue::from(45));
        map
    }

    fn get_fan_data_gpu1(&self) -> HashMap<String, OwnedValue> {
        let mut map = HashMap::new();
        map.insert("temp".to_string(), OwnedValue::from(48));
        map.insert("fanSpeedRPM".to_string(), OwnedValue::from(2200));
        map.insert("fanSpeedPercent".to_string(), OwnedValue::from(40));
        map
    }

    fn get_fan_data_gpu2(&self) -> HashMap<String, OwnedValue> {
        HashMap::new()
    }

    fn get_display_brightness(&self) -> i32 {
        self.display_brightness.load(Ordering::SeqCst)
    }

    fn set_display_brightness(&self, brightness: i32) -> bool {
        if (0..=100).contains(&brightness) {
            self.display_brightness.store(brightness, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    fn get_water_cooler_connected(&self) -> bool {
        self.water_cooler_connected.load(Ordering::SeqCst)
    }

    fn get_water_cooler_fan_speed(&self) -> i32 {
        self.water_cooler_fan_speed.load(Ordering::SeqCst)
    }

    fn get_water_cooler_pump_level(&self) -> i32 {
        self.water_cooler_pump_level.load(Ordering::SeqCst)
    }

    fn set_water_cooler_fan_speed(&self, duty_cycle: i32) -> bool {
        if (0..=100).contains(&duty_cycle) {
            self.water_cooler_fan_speed
                .store(duty_cycle, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    fn set_water_cooler_pump_voltage(&self, voltage: i32) -> bool {
        // Discrete pump codes allowed on LCT hardware: 0 (11V), 2 (7V), 3 (8V), 4 (Off)
        // Code 1 (12V) is forbidden on hardware.
        if [0, 2, 3, 4].contains(&voltage) {
            self.water_cooler_pump_level.store(voltage, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    fn turn_off_water_cooler_fan(&self) -> bool {
        self.water_cooler_fan_speed.store(0, Ordering::SeqCst);
        true
    }

    fn turn_off_water_cooler_pump(&self) -> bool {
        self.water_cooler_pump_level.store(4, Ordering::SeqCst); // 4 = Off
        true
    }

    fn turn_off_water_cooler_led(&self) -> bool {
        true
    }

    fn get_current_charging_profile(&self) -> String {
        self.charging_profile.lock().unwrap().clone()
    }

    fn set_charging_profile(&self, profile: &str) -> bool {
        *self.charging_profile.lock().unwrap() = profile.to_string();
        true
    }

    fn get_charge_start_threshold(&self) -> i32 {
        self.charge_start_threshold.load(Ordering::SeqCst)
    }

    fn get_charge_end_threshold(&self) -> i32 {
        self.charge_end_threshold.load(Ordering::SeqCst)
    }

    fn set_charge_start_threshold(&self, value: i32) -> bool {
        if (0..=100).contains(&value) {
            self.charge_start_threshold.store(value, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    fn set_charge_end_threshold(&self, value: i32) -> bool {
        if (0..=100).contains(&value) {
            self.charge_end_threshold.store(value, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    fn get_charge_type(&self) -> String {
        self.charge_type.lock().unwrap().clone()
    }

    fn set_charge_type(&self, charge_type: &str) -> bool {
        *self.charge_type.lock().unwrap() = charge_type.to_string();
        true
    }

    fn get_profiles_json(&self) -> String {
        r#"[{"id":"office","name":"Office"},{"id":"balanced","name":"Balanced"},{"id":"performance","name":"Performance"}]"#.to_string()
    }

    fn get_default_profiles_json(&self) -> String {
        r#"[{"id":"office","name":"Office"},{"id":"balanced","name":"Balanced"}]"#.to_string()
    }

    fn get_custom_profiles_json(&self) -> String {
        let map = self.custom_profiles.lock().unwrap();
        let list: Vec<serde_json::Value> = map
            .values()
            .filter_map(|s| serde_json::from_str(s).ok())
            .collect();
        serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_string())
    }

    fn get_fan_profile(&self, name: &str) -> String {
        format!(r#"{{"name":"{name}","points":[{{"temp":40,"speed":30}},{{"temp":80,"speed":100}}]}}"#)
    }

    fn get_fan_profile_names(&self) -> String {
        r#"["office","balanced","performance"]"#.to_string()
    }

    fn set_fan_profile(&self, _name: &str, json: &str) -> bool {
        serde_json::from_str::<serde_json::Value>(json).is_ok()
    }

    fn apply_fan_profiles(&self, fan_profiles_json: &str) -> bool {
        serde_json::from_str::<serde_json::Value>(fan_profiles_json).is_ok()
    }

    fn revert_fan_profiles(&self) -> bool {
        true
    }

    fn get_fans_off_available(&self) -> bool {
        true
    }

    fn get_lightbar_settings_json(&self) -> String {
        self.lightbar_settings.lock().unwrap().clone()
    }

    fn get_lightbar_status_json(&self) -> String {
        self.lightbar_status.lock().unwrap().clone()
    }

    fn set_lightbar_settings_json(&self, json: &str) -> bool {
        if serde_json::from_str::<serde_json::Value>(json).is_ok() {
            *self.lightbar_settings.lock().unwrap() = json.to_string();
            true
        } else {
            false
        }
    }

    fn trigger_lightbar_animation(&self, _force: bool) -> bool {
        true
    }

    fn get_power_device_settings_json(&self) -> String {
        r#"[{"id":"device-1","enabled":true}]"#.to_string()
    }

    fn get_power_device_status_json(&self) -> String {
        r#"[{"id":"device-1","online":true}]"#.to_string()
    }

    fn set_power_device_settings_json(&self, json: &str) -> bool {
        serde_json::from_str::<serde_json::Value>(json).is_ok()
    }

    fn scan_power_devices(&self) -> bool {
        true
    }

    fn get_water_cooler_available(&self) -> bool {
        true
    }

    fn get_water_cooler_supported(&self) -> bool {
        true
    }

    fn enable_water_cooler(&self, enable: bool) -> bool {
        self.water_cooler_enabled.store(enable, Ordering::SeqCst);
        true
    }

    fn is_water_cooler_enabled(&self) -> bool {
        self.water_cooler_enabled.load(Ordering::SeqCst)
    }

    fn set_water_cooler_auto_control(&self, enabled: bool) -> bool {
        self.water_cooler_auto_control
            .store(enabled, Ordering::SeqCst);
        true
    }

    fn is_water_cooler_auto_control_enabled(&self) -> bool {
        self.water_cooler_auto_control.load(Ordering::SeqCst)
    }

    fn get_driver_health_json(&self) -> String {
        r#"{"status":"healthy","errors":[]}"#.to_string()
    }
}

async fn setup_contract_test_stand(
    backend: MockContractBackend,
) -> (
    String,
    zbus::connection::Connection,
    zbus::connection::Connection,
    UccdProxy<'static>,
) {
    let unique_suffix = format!(
        "{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let bus_name = format!("com.uniwill.uccd.contract_{unique_suffix}");

    let server_conn = zbus::connection::Builder::session()
        .expect("session builder")
        .name(bus_name.as_str())
        .expect("bus name")
        .build()
        .await
        .expect("server connection");

    register_uccd_service(
        &server_conn,
        Arc::new(backend),
        Arc::new(AllowAllAuthorizer),
    )
    .await
    .expect("register_uccd_service");

    let client_conn = zbus::connection::Builder::session()
        .expect("client session builder")
        .build()
        .await
        .expect("client connection");

    let dest_name: zbus::names::BusName<'static> =
        zbus::names::BusName::try_from(bus_name.clone()).expect("bus name into owned");

    let proxy = UccdProxy::builder(&client_conn)
        .destination(dest_name)
        .expect("dest")
        .path("/com/uniwill/uccd")
        .expect("path")
        .build()
        .await
        .expect("proxy build");

    (bus_name, server_conn, client_conn, proxy)
}

#[tokio::test]
async fn test_contract_profiles_group() {
    let backend = MockContractBackend::default();
    let (_bus, _server, _client, proxy) = setup_contract_test_stand(backend.clone()).await;

    // 1. GetActiveProfileJSON
    let active_json = proxy.get_active_profile_json().await.unwrap();
    assert!(active_json.contains("Office Profile"));

    // 2. SetActiveProfile valid & invalid
    let ok = proxy.set_active_profile("balanced").await.unwrap();
    assert!(ok);
    assert_eq!(*backend.active_profile_id.lock().unwrap(), "balanced");

    let bad = proxy.set_active_profile("non_existent_profile").await.unwrap();
    assert!(!bad);

    // 3. ApplyProfile valid & malformed
    let apply_ok = proxy
        .apply_profile(r#"{"id":"temp-gaming","name":"Temp Gaming"}"#)
        .await
        .unwrap();
    assert!(apply_ok);
    assert!(proxy.get_active_profile_json().await.unwrap().contains("Temp Gaming"));

    let apply_bad = proxy.apply_profile("NOT_VALID_JSON{:::").await.unwrap();
    assert!(!apply_bad);

    // 4. SaveCustomProfile & DeleteCustomProfile
    let save_ok = proxy
        .save_custom_profile(r#"{"id":"custom-user-1","name":"User Profile"}"#)
        .await
        .unwrap();
    assert!(save_ok);

    let custom_list = proxy.get_custom_profiles_json().await.unwrap();
    assert!(custom_list.contains("custom-user-1"));

    let del_ok = proxy.delete_custom_profile("custom-user-1").await.unwrap();
    assert!(del_ok);

    let del_bad = proxy.delete_custom_profile("custom-user-1").await.unwrap();
    assert!(!del_bad); // already deleted

    // 5. Inventory listings
    let all_profiles = proxy.get_profiles_json().await.unwrap();
    assert!(all_profiles.contains("office") && all_profiles.contains("performance"));

    let def_profiles = proxy.get_default_profiles_json().await.unwrap();
    assert!(def_profiles.contains("office") && def_profiles.contains("balanced"));
}

#[tokio::test]
async fn test_contract_power_and_state_group() {
    let backend = MockContractBackend::default();
    let (_bus, _server, _client, proxy) = setup_contract_test_stand(backend.clone()).await;

    // 1. PowerState & Policy
    let pstate = proxy.get_power_state().await.unwrap();
    assert_eq!(pstate, "power_ac");

    let policy = proxy.get_power_policy_json().await.unwrap();
    assert!(policy.contains("balanced"));

    // 2. Manual power mode
    let mode_ok = proxy.set_manual_power_mode("power_bat").await.unwrap();
    assert!(mode_ok);
    assert_eq!(proxy.get_power_state().await.unwrap(), "power_bat");

    // 3. Register & unregister power request
    let req_ok = proxy
        .register_power_request("game.exe", "High performance demand", "power_ac")
        .await
        .unwrap();
    assert!(req_ok);
    assert!(backend.power_requests.lock().unwrap().contains_key("game.exe"));

    let unreq_ok = proxy.unregister_power_request("game.exe").await.unwrap();
    assert!(unreq_ok);

    // 4. State Map & Batch State Map
    let sm_ok = proxy.set_state_map("ac", "performance").await.unwrap();
    assert!(sm_ok);

    let batch_ok = proxy
        .set_batch_state_map(r#"{"bat":"office","wc":"performance"}"#)
        .await
        .unwrap();
    assert!(batch_ok);
    assert_eq!(
        backend.state_map.lock().unwrap().get("wc").map(String::as_str),
        Some("performance")
    );

    let batch_bad = proxy.set_batch_state_map("BROKEN_JSON").await.unwrap();
    assert!(!batch_bad);

    // 5. Settings JSON
    let settings = proxy.get_settings_json().await.unwrap();
    assert!(settings.contains("version"));
}

#[tokio::test]
async fn test_contract_fans_and_sensors_group() {
    let backend = MockContractBackend::default();
    let (_bus, _server, _client, proxy) = setup_contract_test_stand(backend).await;

    // Fan data maps
    let cpu_map = proxy.get_fan_data_cpu().await.unwrap();
    assert!(cpu_map.contains_key("temp"));
    assert!(cpu_map.contains_key("fanSpeedRPM"));
    assert!(cpu_map.contains_key("fanSpeedPercent"));

    let gpu1_map = proxy.get_fan_data_gpu1().await.unwrap();
    assert!(gpu1_map.contains_key("temp"));
    assert!(gpu1_map.contains_key("fanSpeedRPM"));

    let gpu2_map = proxy.get_fan_data_gpu2().await.unwrap();
    assert!(gpu2_map.is_empty());

    // Fan profiles
    let fan_names = proxy.get_fan_profile_names().await.unwrap();
    assert!(fan_names.contains("office"));

    let fan_prof = proxy.get_fan_profile("office").await.unwrap();
    assert!(fan_prof.contains("points"));

    let set_ok = proxy
        .set_fan_profile("office", r#"{"points":[{"temp":50,"speed":50}]}"#)
        .await
        .unwrap();
    assert!(set_ok);

    let set_bad = proxy.set_fan_profile("office", "INVALID").await.unwrap();
    assert!(!set_bad);

    let apply_ok = proxy
        .apply_fan_profiles(r#"{"cpu":[{"temp":40,"speed":30}]}"#)
        .await
        .unwrap();
    assert!(apply_ok);

    let rev_ok = proxy.revert_fan_profiles().await.unwrap();
    assert!(rev_ok);

    assert_eq!(proxy.get_fans_min_speed().await.unwrap(), 0);
    assert!(proxy.get_fans_off_available().await.unwrap());
}

#[tokio::test]
async fn test_contract_water_cooler_group() {
    let backend = MockContractBackend::default();
    let (_bus, _server, _client, proxy) = setup_contract_test_stand(backend.clone()).await;

    // Status queries
    assert!(!proxy.get_water_cooler_connected().await.unwrap());
    assert!(proxy.get_water_cooler_available().await.unwrap());
    assert!(proxy.get_water_cooler_supported().await.unwrap());
    assert_eq!(proxy.get_water_cooler_fan_speed().await.unwrap(), 40);
    assert_eq!(proxy.get_water_cooler_pump_level().await.unwrap(), 2);

    // Controls: Fan speed
    assert!(proxy.set_water_cooler_fan_speed(80).await.unwrap());
    assert_eq!(backend.water_cooler_fan_speed.load(Ordering::SeqCst), 80);
    assert!(!proxy.set_water_cooler_fan_speed(150).await.unwrap()); // out of bounds
    assert!(!proxy.set_water_cooler_fan_speed(-5).await.unwrap());

    // Controls: Discrete pump voltage codes (0: 11V, 2: 7V, 3: 8V, 4: Off)
    assert!(proxy.set_water_cooler_pump_voltage(0).await.unwrap());
    assert!(proxy.set_water_cooler_pump_voltage(3).await.unwrap());
    assert!(proxy.set_water_cooler_pump_voltage(4).await.unwrap());
    // Code 1 (12V) is forbidden on hardware and must be rejected
    assert!(!proxy.set_water_cooler_pump_voltage(1).await.unwrap());
    assert!(!proxy.set_water_cooler_pump_voltage(99).await.unwrap());

    // Turn off
    assert!(proxy.turn_off_water_cooler_fan().await.unwrap());
    assert_eq!(backend.water_cooler_fan_speed.load(Ordering::SeqCst), 0);

    assert!(proxy.turn_off_water_cooler_pump().await.unwrap());
    assert_eq!(backend.water_cooler_pump_level.load(Ordering::SeqCst), 4); // 4 = Off

    assert!(proxy.turn_off_water_cooler_led().await.unwrap());

    // Enable and auto control
    assert!(proxy.enable_water_cooler(true).await.unwrap());
    assert!(proxy.is_water_cooler_enabled().await.unwrap());
    assert!(proxy.set_water_cooler_auto_control(false).await.unwrap());
    assert!(!proxy.is_water_cooler_auto_control_enabled().await.unwrap());
}

#[tokio::test]
async fn test_contract_lightbar_and_display_group() {
    let backend = MockContractBackend::default();
    let (_bus, _server, _client, proxy) = setup_contract_test_stand(backend.clone()).await;

    // Display brightness
    assert_eq!(proxy.get_display_brightness().await.unwrap(), 75);
    assert!(proxy.set_display_brightness(90).await.unwrap());
    assert_eq!(backend.display_brightness.load(Ordering::SeqCst), 90);

    // Bounds checking [0, 100]
    assert!(!proxy.set_display_brightness(-1).await.unwrap());
    assert!(!proxy.set_display_brightness(101).await.unwrap());

    // Lightbar settings & animation
    let lb_settings = proxy.get_lightbar_settings_json().await.unwrap();
    assert!(lb_settings.contains("static"));

    let lb_status = proxy.get_lightbar_status_json().await.unwrap();
    assert!(lb_status.contains("active"));

    assert!(proxy
        .set_lightbar_settings_json(r#"{"mode":"breathing","speed":3}"#)
        .await
        .unwrap());
    assert!(!proxy.set_lightbar_settings_json("CORRUPT").await.unwrap());

    assert!(proxy.trigger_lightbar_animation(true).await.unwrap());
}

#[tokio::test]
async fn test_contract_devices_and_charging_group() {
    let backend = MockContractBackend::default();
    let (_bus, _server, _client, proxy) = setup_contract_test_stand(backend.clone()).await;

    // Device & System info
    assert_eq!(
        proxy.get_device_name().await.unwrap(),
        "Frixit LAPTOP Test Stand"
    );
    assert!(proxy.get_system_info_json().await.unwrap().contains("7840HS"));
    assert!(proxy.is_device_supported().await.unwrap());
    assert_eq!(
        proxy.uccd_version().await.unwrap(),
        env!("CARGO_PKG_VERSION")
    );

    // Charging profile
    assert_eq!(
        proxy.get_current_charging_profile().await.unwrap(),
        "balanced"
    );
    assert!(proxy.set_charging_profile("stationary").await.unwrap());
    assert_eq!(
        backend.charging_profile.lock().unwrap().as_str(),
        "stationary"
    );

    // Charging thresholds bounds
    assert_eq!(proxy.get_charge_start_threshold().await.unwrap(), 60);
    assert_eq!(proxy.get_charge_end_threshold().await.unwrap(), 80);

    assert!(proxy.set_charge_start_threshold(50).await.unwrap());
    assert!(proxy.set_charge_end_threshold(85).await.unwrap());
    assert!(!proxy.set_charge_start_threshold(-10).await.unwrap());
    assert!(!proxy.set_charge_start_threshold(110).await.unwrap());
    assert!(!proxy.set_charge_end_threshold(150).await.unwrap());

    // Charge type
    assert_eq!(proxy.get_charge_type().await.unwrap(), "Standard");
    assert!(proxy.set_charge_type("Express").await.unwrap());
    assert_eq!(backend.charge_type.lock().unwrap().as_str(), "Express");

    // Power devices
    let pdev_set = proxy.get_power_device_settings_json().await.unwrap();
    assert!(pdev_set.contains("device-1"));
    let pdev_stat = proxy.get_power_device_status_json().await.unwrap();
    assert!(pdev_stat.contains("online"));
    assert!(proxy
        .set_power_device_settings_json(r#"[{"id":"device-2"}]"#)
        .await
        .unwrap());
    assert!(proxy.scan_power_devices().await.unwrap());
}

#[tokio::test]
async fn test_contract_signals_emission_and_delivery() {
    let backend = MockContractBackend::default();
    let (_bus, server_conn, _client_conn, proxy) =
        setup_contract_test_stand(backend).await;

    let emitter = SignalEmitter::new(&server_conn, "/com/uniwill/uccd").unwrap();

    // Verify emission of all 7 signals succeeds without error
    UccdInterface::<MockContractBackend>::profile_changed(
        &emitter,
        "performance",
        "kb-default",
        "fan-turbo",
    )
    .await
    .unwrap();

    UccdInterface::<MockContractBackend>::power_state_changed(&emitter, "power_bat")
        .await
        .unwrap();

    UccdInterface::<MockContractBackend>::power_policy_changed(
        &emitter,
        r#"{"policy":"powersave"}"#,
    )
    .await
    .unwrap();

    UccdInterface::<MockContractBackend>::water_cooler_status_changed(&emitter, "connected")
        .await
        .unwrap();

    UccdInterface::<MockContractBackend>::lightbar_settings_changed(
        &emitter,
        r#"{"mode":"wave"}"#,
    )
    .await
    .unwrap();

    UccdInterface::<MockContractBackend>::power_device_status_changed(
        &emitter,
        r#"{"devices":[]}"#,
    )
    .await
    .unwrap();

    UccdInterface::<MockContractBackend>::mode_reapply_pending_changed(&emitter, true)
        .await
        .unwrap();

    // Verify proxy remains responsive after signal emissions
    assert!(proxy.is_device_supported().await.unwrap());
}

#[tokio::test]
async fn test_contract_error_handling_and_fault_injection() {
    let backend = MockContractBackend::default();
    let (_bus, _server, _client, proxy) = setup_contract_test_stand(backend.clone()).await;

    // 1. Malformed JSON handling across all JSON-accepting methods
    assert!(!proxy.apply_profile("NOT_JSON").await.unwrap());
    assert!(!proxy.save_custom_profile("{unclosed").await.unwrap());
    assert!(!proxy.set_batch_state_map("INVALID").await.unwrap());
    assert!(!proxy.set_fan_profile("office", "{:::").await.unwrap());
    assert!(!proxy.apply_fan_profiles("").await.unwrap());
    assert!(!proxy.set_lightbar_settings_json("").await.unwrap());
    assert!(!proxy.set_power_device_settings_json("{").await.unwrap());

    // 2. Non-existent IDs
    assert!(!proxy.set_active_profile("non_existent_12345").await.unwrap());
    assert!(!proxy.delete_custom_profile("non_existent_12345").await.unwrap());

    // 3. Out-of-bounds parameters
    assert!(!proxy.set_display_brightness(-50).await.unwrap());
    assert!(!proxy.set_display_brightness(250).await.unwrap());
    assert!(!proxy.set_water_cooler_fan_speed(-1).await.unwrap());
    assert!(!proxy.set_water_cooler_fan_speed(101).await.unwrap());
    assert!(!proxy.set_water_cooler_pump_voltage(1).await.unwrap()); // forbidden
    assert!(!proxy.set_water_cooler_pump_voltage(5).await.unwrap()); // out of range
    assert!(!proxy.set_charge_start_threshold(-1).await.unwrap());
    assert!(!proxy.set_charge_start_threshold(101).await.unwrap());
    assert!(!proxy.set_charge_end_threshold(-1).await.unwrap());
    assert!(!proxy.set_charge_end_threshold(101).await.unwrap());
}

