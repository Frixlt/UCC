use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use ucc_dbus::*;
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
        let is_custom = self
            .custom_profiles
            .lock()
            .unwrap()
            .contains_key(profile_id);
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
            self.water_cooler_pump_level
                .store(voltage, Ordering::SeqCst);
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
        format!(
            r#"{{"name":"{name}","points":[{{"temp":40,"speed":30}},{{"temp":80,"speed":100}}]}}"#
        )
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

pub(crate) async fn setup_contract_test_stand(
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
