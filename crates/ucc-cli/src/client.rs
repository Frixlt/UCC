//! D-Bus client wrapper for UCC CLI commands.

use std::collections::HashMap;
use serde_json::Value;
use ucc_dbus::{UccdProxy, DBUS_SERVICE, DBUS_PATH, DBUS_INTERFACE};
use zbus::Connection;

pub const DAEMON_CONNECT_ERR: &str =
    "Error: Cannot connect to uccd daemon (com.uniwill.uccd on system bus).\nMake sure uccd is running: systemctl status uccd";

pub const DEFAULT_RPC_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);

pub async fn with_rpc_timeout<T>(
    fut: impl std::future::Future<Output = zbus::Result<T>>,
) -> zbus::Result<T> {
    match tokio::time::timeout(DEFAULT_RPC_TIMEOUT, fut).await {
        Ok(res) => res,
        Err(_) => Err(zbus::Error::Failure("RPC call deadline exceeded".to_string())),
    }
}

#[derive(Debug, Default, Clone)]
pub struct LocalAssignments {
    pub profile_states: HashMap<String, Vec<String>>,
    pub fan_states: HashMap<String, Vec<String>>,
    pub kb_states: HashMap<String, Vec<String>>,
}

#[derive(Debug, Default, Clone)]
pub struct CpuMetrics {
    pub temperature: Option<i32>,
    pub frequency: Option<i32>,
    pub power: Option<f64>,
    pub fan_percent: Option<i32>,
    pub fan_rpm: Option<i32>,
}

#[derive(Debug, Default, Clone)]
pub struct GpuMetrics {
    pub temperature: Option<i32>,
    pub frequency: Option<i32>,
    pub power: Option<f64>,
    pub fan_percent: Option<i32>,
    pub fan_rpm: Option<i32>,
}

#[derive(Debug, Default, Clone)]
pub struct IGpuMetrics {
    pub temperature: Option<i32>,
    pub frequency: Option<i32>,
    pub power: Option<f64>,
}

#[derive(Debug, Default, Clone)]
pub struct HardwareMetrics {
    pub display_brightness: Option<i32>,
    pub webcam_enabled: Option<bool>,
    pub fn_lock: Option<bool>,
}

#[derive(Debug, Default, Clone)]
pub struct WaterCoolerMetrics {
    pub supported: bool,
    pub connected: bool,
    pub enabled: Option<bool>,
    pub fan_speed: Option<i32>,
    pub pump_level: Option<i32>,
}

#[derive(Debug, Default, Clone)]
pub struct ChargingMetrics {
    pub available: bool,
    pub profile: Option<String>,
    pub priority: Option<String>,
    pub charge_type: Option<String>,
    pub start_threshold: Option<i32>,
    pub end_threshold: Option<i32>,
}

fn json_number_as_i32(val: &Value) -> Option<i32> {
    val.as_i64()
        .and_then(|n| i32::try_from(n).ok())
        .or_else(|| {
            val.as_f64()
                .filter(|f| f.is_finite())
                .map(|f| f.round() as i32)
        })
}

fn value_to_i64(v: &zbus::zvariant::Value<'_>) -> Option<i64> {
    i64::try_from(v)
        .ok()
        .or_else(|| u64::try_from(v).ok().and_then(|u| i64::try_from(u).ok()))
        .or_else(|| i32::try_from(v).ok().map(i64::from))
        .or_else(|| u32::try_from(v).ok().map(i64::from))
}

fn owned_value_to_i64(v: &zbus::zvariant::OwnedValue) -> Option<i64> {
    v.downcast_ref::<i64>()
        .ok()
        .or_else(|| v.downcast_ref::<u64>().ok().and_then(|u| i64::try_from(u).ok()))
        .or_else(|| v.downcast_ref::<i32>().ok().map(i64::from))
        .or_else(|| v.downcast_ref::<u32>().ok().map(i64::from))
}

fn fan_map_entry_is_stale(map: &HashMap<String, zbus::zvariant::OwnedValue>, field: &str) -> bool {
    if let Some(ts) = map.get("timestamp").and_then(owned_value_to_i64) {
        if ts <= 0 {
            return true;
        }
    }
    if let Some(entry) = map.get(field) {
        if let Ok(sub) = <HashMap<String, zbus::zvariant::OwnedValue>>::try_from(entry.clone()) {
            if let Some(ts) = sub.get("timestamp").and_then(owned_value_to_i64) {
                if ts <= 0 {
                    return true;
                }
            }
        } else if let zbus::zvariant::Value::Dict(ref dict) = **entry {
            for (k, val) in dict.iter() {
                if matches!(<&str>::try_from(k), Ok("timestamp")) {
                    if let Some(ts) = value_to_i64(val) {
                        if ts <= 0 {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

fn extract_variant_i32(v: &zbus::zvariant::OwnedValue) -> Option<i32> {
    if let Ok(val) = v.downcast_ref::<i32>() {
        return Some(val);
    }
    if let Ok(val) = v.downcast_ref::<f64>() {
        if val.is_finite() {
            return Some(val.round() as i32);
        }
    }
    if let Ok(val) = v.downcast_ref::<u32>() {
        return i32::try_from(val).ok();
    }
    if let Ok(val) = v.downcast_ref::<i64>() {
        return i32::try_from(val).ok();
    }
    if let Ok(val) = v.downcast_ref::<u64>() {
        return i32::try_from(val).ok();
    }
    if let Ok(val) = v.downcast_ref::<u8>() {
        return Some(val as i32);
    }
    if let Ok(val) = v.downcast_ref::<i16>() {
        return Some(val as i32);
    }
    if let Ok(val) = v.downcast_ref::<u16>() {
        return Some(val as i32);
    }
    if let Ok(map) = <std::collections::HashMap<String, zbus::zvariant::OwnedValue>>::try_from(v.clone()) {
        if let Some(data_val) = map.get("data") {
            return extract_variant_i32(data_val);
        }
    }
    if let Ok(map) = <std::collections::HashMap<String, i32>>::try_from(v.clone()) {
        if let Some(&data_val) = map.get("data") {
            return Some(data_val);
        }
    }
    if let zbus::zvariant::Value::Dict(ref dict) = **v {
        for (k, val) in dict.iter() {
            if let Ok(key_str) = <&str>::try_from(k) {
                if key_str == "data" {
                    if let Ok(i) = i32::try_from(val) {
                        return Some(i);
                    }
                    if let Ok(u) = u32::try_from(val) {
                        return Some(u as i32);
                    }
                    if let Ok(ov) = zbus::zvariant::OwnedValue::try_from(val) {
                        return extract_variant_i32(&ov);
                    }
                }
            }
        }
    }
    if let zbus::zvariant::Value::Structure(ref s) = **v {
        if let Some(last) = s.fields().last() {
            if let Ok(val) = i32::try_from(last) {
                return Some(val);
            }
        }
    }
    None
}

pub struct CliClient {
    _conn: Connection,
    proxy: UccdProxy<'static>,
}

impl CliClient {
    /// Connect to system D-Bus (or session bus if UCCD_DBUS_SERVICE is set) and create UccdProxy.
    /// Exits with code 2 if connection to daemon fails.
    pub async fn connect() -> Result<Self, i32> {
        let service_name = std::env::var("UCCD_DBUS_SERVICE").unwrap_or_else(|_| DBUS_SERVICE.to_string());
        let use_session = std::env::var("UCCD_DBUS_SERVICE").is_ok() || std::env::var("UCCD_DBUS_SESSION").is_ok();
        let conn = if use_session {
            match Connection::session().await {
                Ok(c) => c,
                Err(_) => {
                    eprintln!("{}", DAEMON_CONNECT_ERR);
                    return Err(2);
                }
            }
        } else {
            match Connection::system().await {
                Ok(c) => c,
                Err(_) => {
                    eprintln!("{}", DAEMON_CONNECT_ERR);
                    return Err(2);
                }
            }
        };

        // Check if service exists / is owned on the bus
        let has_owner = match conn.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "NameHasOwner",
            &(&service_name),
        ).await {
            Ok(msg) => msg.body().deserialize::<bool>().unwrap_or(false),
            Err(_) => false,
        };

        if !has_owner {
            eprintln!("{}", DAEMON_CONNECT_ERR);
            return Err(2);
        }

        let proxy = match UccdProxy::builder(&conn)
            .destination(service_name).map_err(|_| 2)?
            .path(DBUS_PATH).map_err(|_| 2)?
            .interface(DBUS_INTERFACE).map_err(|_| 2)?
            .build()
            .await
        {
            Ok(p) => p,
            Err(_) => {
                eprintln!("{}", DAEMON_CONNECT_ERR);
                return Err(2);
            }
        };

        // Verify that the destination actually exposes UCC API and is responsive
        let api_check = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            proxy.get_device_name(),
        ).await;

        if !matches!(api_check, Ok(Ok(_))) {
            eprintln!("{}", DAEMON_CONNECT_ERR);
            return Err(2);
        }

        Ok(Self {
            _conn: conn,
            proxy,
        })
    }

    /// Access the typed UccdProxy.
    pub fn proxy(&self) -> &UccdProxy<'static> {
        &self.proxy
    }

    pub async fn get_system_info_json(&self) -> Option<Value> {
        if let Ok(raw) = with_rpc_timeout(self.proxy.get_system_info_json()).await {
            serde_json::from_str(&raw).ok()
        } else {
            None
        }
    }

    pub async fn get_power_state(&self) -> Option<String> {
        with_rpc_timeout(self.proxy.get_power_state()).await.ok()
    }

    pub async fn get_active_profile_json(&self) -> Option<Value> {
        if let Ok(raw) = with_rpc_timeout(self.proxy.get_active_profile_json()).await {
            serde_json::from_str(&raw).ok()
        } else {
            None
        }
    }

    pub async fn get_power_policy_json(&self) -> Option<Value> {
        if let Ok(raw) = with_rpc_timeout(self.proxy.get_power_policy_json()).await {
            serde_json::from_str(&raw).ok()
        } else {
            None
        }
    }

    pub async fn get_settings_json(&self) -> Option<Value> {
        if let Ok(raw) = with_rpc_timeout(self.proxy.get_settings_json()).await {
            serde_json::from_str(&raw).ok()
        } else {
            None
        }
    }

    pub async fn get_cpu_metrics(&self) -> CpuMetrics {
        let mut m = CpuMetrics::default();

        // Temperature and fan speed from GetFanDataCPU or sensors
        if let Ok(raw_fan) = with_rpc_timeout(self.proxy.get_fan_data_cpu()).await {
            if !fan_map_entry_is_stale(&raw_fan, "temp") {
                if let Some(temp) = raw_fan.get("temp").and_then(extract_variant_i32) {
                    if temp >= 0 {
                        m.temperature = Some(temp);
                    }
                }
            }
            if !fan_map_entry_is_stale(&raw_fan, "speed") {
                if let Some(spd) = raw_fan.get("speed").and_then(extract_variant_i32) {
                    if spd >= 0 {
                        m.fan_percent = Some(spd);
                        m.fan_rpm = Some(spd * 60);
                    }
                }
            }
        }

        // Frequency
        if let Ok(freq) = with_rpc_timeout(self.proxy.get_cpu_frequency_mhz()).await {
            if freq > 0 {
                m.frequency = Some(freq);
            }
        }

        // Power from GetCpuPowerValuesJSON
        if let Ok(raw_pow) = with_rpc_timeout(self.proxy.get_cpu_power_values_json()).await {
            if let Ok(val) = serde_json::from_str::<Value>(&raw_pow) {
                if let Some(p) = val.get("powerDraw").and_then(|v| v.as_f64()) {
                    if p >= 0.0 {
                        m.power = Some(p);
                    }
                }
            }
        }

        m
    }

    pub async fn get_gpu_metrics(&self) -> GpuMetrics {
        let mut m = GpuMetrics::default();

        // Live dGPU data
        if let Ok(raw) = with_rpc_timeout(self.proxy.get_dgpu_info_values_json()).await {
            if let Ok(val) = serde_json::from_str::<Value>(&raw) {
                if let Some(t) = val.get("temp").and_then(json_number_as_i32) {
                    if t >= 0 { m.temperature = Some(t); }
                }
                if let Some(f) = val.get("coreFrequency").or_else(|| val.get("coreFreq")).and_then(json_number_as_i32) {
                    if f >= 0 { m.frequency = Some(f); }
                }
                if let Some(p) = val.get("powerDraw").and_then(|v| v.as_f64()) {
                    if p >= 0.0 { m.power = Some(p); }
                }
            }
        }

        // Fan speeds from GPU1 and GPU2
        let gpu1_spd = with_rpc_timeout(self.proxy.get_fan_data_gpu1()).await.ok().and_then(|map| {
            if fan_map_entry_is_stale(&map, "speed") {
                None
            } else {
                map.get("speed").and_then(extract_variant_i32)
            }
        });
        let gpu2_spd = with_rpc_timeout(self.proxy.get_fan_data_gpu2()).await.ok().and_then(|map| {
            if fan_map_entry_is_stale(&map, "speed") {
                None
            } else {
                map.get("speed").and_then(extract_variant_i32)
            }
        });

        match (gpu1_spd, gpu2_spd) {
            (Some(s1), Some(s2)) if s1 >= 0 && s2 >= 0 => {
                let avg = (s1 + s2) / 2;
                m.fan_percent = Some(avg);
                m.fan_rpm = Some(avg * 60);
            }
            (Some(s1), _) if s1 >= 0 => {
                m.fan_percent = Some(s1);
                m.fan_rpm = Some(s1 * 60);
            }
            (_, Some(s2)) if s2 >= 0 => {
                m.fan_percent = Some(s2);
                m.fan_rpm = Some(s2 * 60);
            }
            _ => {}
        }

        m
    }

    pub async fn get_igpu_metrics(&self) -> IGpuMetrics {
        let mut m = IGpuMetrics::default();
        if let Ok(raw) = with_rpc_timeout(self.proxy.get_igpu_info_values_json()).await {
            if let Ok(val) = serde_json::from_str::<Value>(&raw) {
                if let Some(t) = val.get("temp").and_then(json_number_as_i32) {
                    if t >= 0 { m.temperature = Some(t); }
                }
                if let Some(f) = val.get("coreFrequency").or_else(|| val.get("coreFreq")).and_then(json_number_as_i32) {
                    if f >= 0 { m.frequency = Some(f); }
                }
                if let Some(p) = val.get("powerDraw").and_then(|v| v.as_f64()) {
                    if p >= 0.0 { m.power = Some(p); }
                }
            }
        }
        m
    }

    pub async fn get_hardware_metrics(&self) -> HardwareMetrics {
        HardwareMetrics {
            display_brightness: with_rpc_timeout(self.proxy.get_display_brightness()).await.ok(),
            webcam_enabled: with_rpc_timeout(self.proxy.get_webcam_sw_status()).await.ok(),
            fn_lock: with_rpc_timeout(self.proxy.get_fn_lock_status()).await.ok(),
        }
    }

    pub async fn get_water_cooler_metrics(&self) -> WaterCoolerMetrics {
        let supported = with_rpc_timeout(self.proxy.get_water_cooler_supported()).await.unwrap_or(false);
        let connected = with_rpc_timeout(self.proxy.get_water_cooler_connected()).await.unwrap_or(false);
        let enabled = with_rpc_timeout(self.proxy.is_water_cooler_enabled()).await.ok();
        let fan_speed = with_rpc_timeout(self.proxy.get_water_cooler_fan_speed()).await.ok();
        let pump_level = with_rpc_timeout(self.proxy.get_water_cooler_pump_level()).await.ok();

        WaterCoolerMetrics {
            supported,
            connected,
            enabled,
            fan_speed: if connected { fan_speed.filter(|&v| v >= 0) } else { None },
            pump_level: if connected { pump_level.filter(|&v| v >= 0) } else { None },
        }
    }

    pub async fn get_charging_metrics(&self) -> ChargingMetrics {
        let profiles_avail = with_rpc_timeout(self.proxy.get_charging_profiles_available()).await.ok();
        let has_hw = if let Some(ref raw) = profiles_avail {
            if let Ok(val) = serde_json::from_str::<Value>(raw) {
                val.as_array().is_some_and(|a| !a.is_empty())
            } else {
                false
            }
        } else {
            false
        };

        if !has_hw {
            return ChargingMetrics { available: false, ..Default::default() };
        }

        let profile = with_rpc_timeout(self.proxy.get_current_charging_profile()).await.ok().filter(|s| !s.is_empty());
        let priority = with_rpc_timeout(self.proxy.get_current_charging_priority()).await.ok().filter(|s| !s.is_empty());
        let charge_type = with_rpc_timeout(self.proxy.get_charge_type()).await.ok().filter(|s| !s.is_empty() && s != "Unknown" && s != "N/A");

        let end_avail = with_rpc_timeout(self.proxy.get_charge_end_available_thresholds()).await.ok();
        let has_thresholds = if let Some(ref raw) = end_avail {
            if let Ok(val) = serde_json::from_str::<Value>(raw) {
                val.as_array().is_some_and(|a| !a.is_empty())
            } else {
                false
            }
        } else {
            false
        };

        let (start_threshold, end_threshold) = if has_thresholds {
            (
                with_rpc_timeout(self.proxy.get_charge_start_threshold()).await.ok().filter(|&v| v >= 0),
                with_rpc_timeout(self.proxy.get_charge_end_threshold()).await.ok().filter(|&v| v >= 0),
            )
        } else {
            (None, None)
        };

        ChargingMetrics {
            available: true,
            profile,
            priority,
            charge_type,
            start_threshold,
            end_threshold,
        }
    }

    pub async fn load_local_assignments(&self) -> LocalAssignments {
        let mut assignments = LocalAssignments::default();
        let settings = match self.get_settings_json().await {
            Some(s) => s,
            None => return assignments,
        };

        // stateMap: { "power_ac": "uuid", ... }
        if let Some(map) = settings.get("stateMap").and_then(|v| v.as_object()) {
            for (state, prof_val) in map {
                if let Some(prof_id) = prof_val.as_str() {
                    if !prof_id.is_empty() {
                        let list = assignments.profile_states.entry(prof_id.to_string()).or_default();
                        if !list.contains(state) {
                            list.push(state.clone());
                        }
                    }
                }
            }
        }

        // profiles: can be EITHER { "power_ac": "profile_id" } OR { "profile_id": { ...UccProfile... } }
        if let Some(profiles_obj) = settings.get("profiles").and_then(|v| v.as_object()) {
            for (k, v) in profiles_obj {
                if let Some(prof_id) = v.as_str() {
                    if !prof_id.is_empty() {
                        let list = assignments.profile_states.entry(prof_id.to_string()).or_default();
                        if !list.contains(k) {
                            list.push(k.clone());
                        }
                    }
                } else if v.is_object() {
                    let states = assignments.profile_states.get(k).cloned().unwrap_or_default();
                    if !states.is_empty() {
                        if let Some(fan_id) = v.get("fan").and_then(|f| f.get("fanProfile")).and_then(|s| s.as_str()) {
                            if !fan_id.is_empty() {
                                let flist = assignments.fan_states.entry(fan_id.to_string()).or_default();
                                for s in &states {
                                    if !flist.contains(s) {
                                        flist.push(s.clone());
                                    }
                                }
                            }
                        }
                        if let Some(kb_id) = v
                            .get("selectedKeyboardProfile")
                            .and_then(|s| s.as_str())
                            .filter(|s| !s.is_empty())
                            .or_else(|| {
                                v.get("keyboard")
                                    .and_then(|kb| kb.get("keyboardProfileId"))
                                    .and_then(|s| s.as_str())
                                    .filter(|s| !s.is_empty())
                            })
                        {
                            let klist = assignments.kb_states.entry(kb_id.to_string()).or_default();
                            for s in &states {
                                if !klist.contains(s) {
                                    klist.push(s.clone());
                                }
                            }
                        }
                    }
                }
            }
        }

        // customProfiles: fan and keyboard profiles linked to assigned main profiles
        if let Some(custom_profiles) = settings.get("customProfiles").and_then(|v| v.as_array()) {
            for prof in custom_profiles {
                if let Some(prof_id) = prof.get("id").and_then(|v| v.as_str()) {
                    if let Some(states) = assignments.profile_states.get(prof_id).cloned() {
                        // Fan profile
                        if let Some(fan_id) = prof.get("fan").and_then(|f| f.get("fanProfile")).and_then(|v| v.as_str()) {
                            if !fan_id.is_empty() {
                                let flist = assignments.fan_states.entry(fan_id.to_string()).or_default();
                                for s in &states {
                                    if !flist.contains(s) {
                                        flist.push(s.clone());
                                    }
                                }
                            }
                        }

                        // Keyboard profile
                        if let Some(kb_id) = prof.get("selectedKeyboardProfile").and_then(|v| v.as_str()) {
                            if !kb_id.is_empty() {
                                let klist = assignments.kb_states.entry(kb_id.to_string()).or_default();
                                for s in &states {
                                    if !klist.contains(s) {
                                        klist.push(s.clone());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        assignments
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::{OwnedValue, Value};

    #[test]
    fn test_json_number_as_i32_parses_int_and_float() {
        assert_eq!(json_number_as_i32(&serde_json::json!(49)), Some(49));
        assert_eq!(json_number_as_i32(&serde_json::json!(49.0)), Some(49));
        assert_eq!(json_number_as_i32(&serde_json::json!(950.4)), Some(950));
        assert_eq!(json_number_as_i32(&serde_json::json!(-1.0)), Some(-1));
        assert_eq!(json_number_as_i32(&serde_json::json!("bad")), None);
    }

    #[test]
    fn test_extract_variant_i32_nested_and_flat() {
        let flat = OwnedValue::from(55i32);
        assert_eq!(extract_variant_i32(&flat), Some(55));

        let mut inner = std::collections::HashMap::<String, OwnedValue>::new();
        inner.insert("timestamp".to_string(), OwnedValue::from(1234567i64));
        inner.insert("data".to_string(), OwnedValue::from(42i32));
        let nested = OwnedValue::try_from(Value::from(inner)).unwrap();
        assert_eq!(extract_variant_i32(&nested), Some(42));
    }
}
