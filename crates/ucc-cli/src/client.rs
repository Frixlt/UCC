//! D-Bus client wrapper for UCC CLI commands.

use std::collections::HashMap;
use serde_json::Value;
use ucc_dbus::{UccdProxy, DBUS_SERVICE, DBUS_PATH, DBUS_INTERFACE};
use zbus::Connection;

pub const DAEMON_CONNECT_ERR: &str =
    "Error: Cannot connect to uccd daemon (com.uniwill.uccd on system bus).\nMake sure uccd is running: systemctl status uccd";

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

pub struct CliClient {
    _conn: Connection,
    proxy: UccdProxy<'static>,
}

impl CliClient {
    /// Connect to system D-Bus and create UccdProxy.
    /// Exits with code 2 if connection to daemon fails.
    pub async fn connect() -> Result<Self, i32> {
        let conn = match Connection::system().await {
            Ok(c) => c,
            Err(_) => {
                eprintln!("{}", DAEMON_CONNECT_ERR);
                return Err(2);
            }
        };

        // Check if service exists / is owned on the bus
        let has_owner = match conn.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "NameHasOwner",
            &(DBUS_SERVICE),
        ).await {
            Ok(msg) => msg.body().deserialize::<bool>().unwrap_or(false),
            Err(_) => false,
        };

        if !has_owner {
            eprintln!("{}", DAEMON_CONNECT_ERR);
            return Err(2);
        }

        let proxy = match UccdProxy::builder(&conn)
            .destination(DBUS_SERVICE).map_err(|_| 2)?
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
        if let Ok(raw) = self.proxy.get_system_info_json().await {
            serde_json::from_str(&raw).ok()
        } else {
            None
        }
    }

    pub async fn get_power_state(&self) -> Option<String> {
        self.proxy.get_power_state().await.ok()
    }

    pub async fn get_active_profile_json(&self) -> Option<Value> {
        if let Ok(raw) = self.proxy.get_active_profile_json().await {
            serde_json::from_str(&raw).ok()
        } else {
            None
        }
    }

    pub async fn get_power_policy_json(&self) -> Option<Value> {
        if let Ok(raw) = self.proxy.get_power_policy_json().await {
            serde_json::from_str(&raw).ok()
        } else {
            None
        }
    }

    pub async fn get_settings_json(&self) -> Option<Value> {
        if let Ok(raw) = self.proxy.get_settings_json().await {
            serde_json::from_str(&raw).ok()
        } else {
            None
        }
    }

    pub async fn get_cpu_metrics(&self) -> CpuMetrics {
        let mut m = CpuMetrics::default();

        // Temperature and fan speed from GetFanDataCPU or sensors
        if let Ok(raw_fan) = self.proxy.get_fan_data_cpu().await {
            if let Some(temp) = raw_fan.get("temp").and_then(|v| v.downcast_ref::<i32>().ok()) {
                if temp >= 0 {
                    m.temperature = Some(temp);
                }
            }
            if let Some(spd) = raw_fan.get("speed").and_then(|v| v.downcast_ref::<i32>().ok()) {
                if spd >= 0 {
                    m.fan_percent = Some(spd);
                    m.fan_rpm = Some(spd * 60);
                }
            }
        }

        // Frequency
        if let Ok(freq) = self.proxy.get_cpu_frequency_mhz().await {
            if freq > 0 {
                m.frequency = Some(freq);
            }
        }

        // Power from GetCpuPowerValuesJSON
        if let Ok(raw_pow) = self.proxy.get_cpu_power_values_json().await {
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
        if let Ok(raw) = self.proxy.get_dgpu_info_values_json().await {
            if let Ok(val) = serde_json::from_str::<Value>(&raw) {
                if let Some(t) = val.get("temp").and_then(|v| v.as_i64()) {
                    if t >= 0 { m.temperature = Some(t as i32); }
                }
                if let Some(f) = val.get("coreFrequency").or_else(|| val.get("coreFreq")).and_then(|v| v.as_i64()) {
                    if f >= 0 { m.frequency = Some(f as i32); }
                }
                if let Some(p) = val.get("powerDraw").and_then(|v| v.as_f64()) {
                    if p >= 0.0 { m.power = Some(p); }
                }
            }
        }

        // Fall back to iGPU temp/power if dGPU temp is absent
        if m.temperature.is_none() {
            if let Ok(raw_i) = self.proxy.get_igpu_info_values_json().await {
                if let Ok(val) = serde_json::from_str::<Value>(&raw_i) {
                    if let Some(t) = val.get("temp").and_then(|v| v.as_i64()) {
                        if t >= 0 { m.temperature = Some(t as i32); }
                    }
                }
            }
        }

        // Fan speeds from GPU1 and GPU2
        let gpu1_spd = self.proxy.get_fan_data_gpu1().await.ok().and_then(|map| {
            map.get("speed").and_then(|v| v.downcast_ref::<i32>().ok())
        });
        let gpu2_spd = self.proxy.get_fan_data_gpu2().await.ok().and_then(|map| {
            map.get("speed").and_then(|v| v.downcast_ref::<i32>().ok())
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
        if let Ok(raw) = self.proxy.get_igpu_info_values_json().await {
            if let Ok(val) = serde_json::from_str::<Value>(&raw) {
                if let Some(t) = val.get("temp").and_then(|v| v.as_i64()) {
                    if t >= 0 { m.temperature = Some(t as i32); }
                }
                if let Some(f) = val.get("coreFrequency").and_then(|v| v.as_i64()) {
                    if f >= 0 { m.frequency = Some(f as i32); }
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
            display_brightness: self.proxy.get_display_brightness().await.ok(),
            webcam_enabled: self.proxy.get_webcam_sw_status().await.ok(),
            fn_lock: self.proxy.get_fn_lock_status().await.ok(),
        }
    }

    pub async fn get_water_cooler_metrics(&self) -> WaterCoolerMetrics {
        let supported = self.proxy.get_water_cooler_supported().await.unwrap_or(false);
        if !supported {
            return WaterCoolerMetrics { supported: false, ..Default::default() };
        }

        let enabled = self.proxy.is_water_cooler_enabled().await.ok();
        let fan_speed = self.proxy.get_water_cooler_fan_speed().await.ok();
        let pump_level = self.proxy.get_water_cooler_pump_level().await.ok();

        WaterCoolerMetrics {
            supported: true,
            enabled,
            fan_speed: fan_speed.filter(|&v| v >= 0),
            pump_level: pump_level.filter(|&v| v >= 0),
        }
    }

    pub async fn get_charging_metrics(&self) -> ChargingMetrics {
        let profiles_avail = self.proxy.get_charging_profiles_available().await.ok();
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

        let profile = self.proxy.get_current_charging_profile().await.ok().filter(|s| !s.is_empty());
        let priority = self.proxy.get_current_charging_priority().await.ok().filter(|s| !s.is_empty());
        let charge_type = self.proxy.get_charge_type().await.ok().filter(|s| !s.is_empty() && s != "Unknown" && s != "N/A");

        let end_avail = self.proxy.get_charge_end_available_thresholds().await.ok();
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
                self.proxy.get_charge_start_threshold().await.ok().filter(|&v| v >= 0),
                self.proxy.get_charge_end_threshold().await.ok().filter(|&v| v >= 0),
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
