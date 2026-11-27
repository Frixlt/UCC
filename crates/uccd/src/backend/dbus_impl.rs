// SPDX-License-Identifier: GPL-3.0-or-later

//! `UccdBackend` D-Bus trait implementation delegating cleanly to modular services inside `LiveUccdBackend`.

use super::cooling::builtin_fan_profiles;
use super::profile_tx::is_builtin_profile_id;
use super::{unix_ms, ApplyOutcome, DbusSignalEvent, LiveUccdBackend};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use ucc_config::{
    default_high_performance_profile, default_max_energy_save_profile, default_office_profile,
    default_quiet_profile,
};
use ucc_core::{ProfileState, UccProfile};
use ucc_dbus::UccdBackend;
use ucc_hardware::{CoolerMode, CoolerState, DeviceVendor};
use zbus::zvariant::OwnedValue;

impl UccdBackend for LiveUccdBackend {
    fn get_device_name(&self) -> String {
        self.get_device_name_impl()
    }

    fn get_system_info_json(&self) -> String {
        self.get_system_info_json_impl()
    }

    fn is_device_supported(&self) -> bool {
        !self.is_vendor_unsupported()
    }

    fn get_display_modes_json(&self) -> String {
        self.get_display_modes_json_impl()
    }

    fn get_is_x11(&self) -> bool {
        self.get_is_x11_impl()
    }

    fn get_force_yuv420_output_switch_available(&self) -> bool {
        self.get_force_yuv420_output_switch_available_impl()
    }

    fn set_display_refresh_rate(&self, _display: &str, _refresh_rate: i32) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        false
    }

    fn get_prime_state(&self) -> String {
        self.get_prime_state_impl()
    }

    fn get_driver_health_json(&self) -> String {
        self.get_driver_health_json_impl()
    }

    fn tuxedo_wmi_available(&self) -> bool {
        self.platform.is_available()
    }

    fn fan_hwmon_available(&self) -> bool {
        self.capabilities.device.number_of_fans > 0
    }

    fn get_fan_data_cpu(&self) -> HashMap<String, OwnedValue> {
        let cached = self.read_rw(&self.latest_fans).clone();
        if let Some(tick) = cached {
            let temp = tick.temps_c.first().copied().flatten().unwrap_or(-1);
            let speed = tick
                .speeds_percent
                .first()
                .copied()
                .flatten()
                .map(i32::from)
                .unwrap_or(-1);
            return Self::fan_map(tick.timestamp_ms, temp, speed);
        }
        let temp = self.platform.read_fan_temp(0).unwrap_or(-1);
        let speed = self
            .platform
            .read_fan_speed_percent(0)
            .map(i32::from)
            .unwrap_or(-1);
        Self::fan_map(unix_ms(), temp, speed)
    }

    fn get_fan_data_gpu1(&self) -> HashMap<String, OwnedValue> {
        let cached = self.read_rw(&self.latest_fans).clone();
        if let Some(tick) = cached {
            let temp = tick.temps_c.get(1).copied().flatten().unwrap_or(-1);
            let speed = tick
                .speeds_percent
                .get(1)
                .copied()
                .flatten()
                .map(i32::from)
                .unwrap_or(-1);
            return Self::fan_map(tick.timestamp_ms, temp, speed);
        }
        let temp = self.platform.read_fan_temp(1).unwrap_or(-1);
        let speed = self
            .platform
            .read_fan_speed_percent(1)
            .map(i32::from)
            .unwrap_or(-1);
        Self::fan_map(unix_ms(), temp, speed)
    }

    fn get_fan_data_gpu2(&self) -> HashMap<String, OwnedValue> {
        let cached = self.read_rw(&self.latest_fans).clone();
        if let Some(tick) = cached {
            let temp = tick.temps_c.get(2).copied().flatten().unwrap_or(-1);
            let speed = tick
                .speeds_percent
                .get(2)
                .copied()
                .flatten()
                .map(i32::from)
                .unwrap_or(-1);
            return Self::fan_map(tick.timestamp_ms, temp, speed);
        }
        let temp = self.platform.read_fan_temp(2).unwrap_or(-1);
        let speed = self
            .platform
            .read_fan_speed_percent(2)
            .map(i32::from)
            .unwrap_or(-1);
        Self::fan_map(unix_ms(), temp, speed)
    }

    fn webcam_sw_available(&self) -> bool {
        self.webcam_sw_available_impl()
    }

    fn get_webcam_sw_status(&self) -> bool {
        self.webcam_sw_available_impl() && self.platform.read_webcam().unwrap_or(false)
    }

    fn set_webcam(&self, enabled: bool) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        self.set_webcam_internal(enabled)
    }

    fn get_display_brightness(&self) -> i32 {
        self.get_display_brightness_impl()
    }

    fn get_keyboard_backlight_control_supported(&self) -> bool {
        let mut ctrl = self.lock_mutex(&self.keyboard_ctrl);
        if !ctrl.is_available() {
            let _ = ctrl.init();
        }
        ctrl.is_available()
    }

    fn set_display_brightness(&self, brightness: i32) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        self.set_display_brightness_internal(brightness)
    }

    fn get_dgpu_info_values_json(&self) -> String {
        self.get_dgpu_info_values_json_impl()
    }

    fn get_igpu_info_values_json(&self) -> String {
        self.get_igpu_info_values_json_impl()
    }

    fn get_cpu_power_values_json(&self) -> String {
        if !self.sensor_data_collection.load(Ordering::SeqCst) {
            return json!({ "powerDraw": -1.0 }).to_string();
        }
        if let Some(ref snap) = *self.read_rw(&self.latest_telemetry) {
            let mut obj = json!({
                "powerDraw": snap.cpu.power_w.unwrap_or(-1.0),
            });
            if let Some(lim) = snap.cpu.max_power_limit_w {
                obj["maxPowerLimit"] = json!(lim);
            }
            return obj.to_string();
        }
        json!({ "powerDraw": -1.0 }).to_string()
    }

    fn consume_mode_reapply_pending(&self) -> bool {
        let prev = self.mode_reapply_pending.swap(false, Ordering::SeqCst);
        if prev {
            self.emit_signal(DbusSignalEvent::ModeReapplyPendingChanged { pending: false });
        }
        prev
    }

    fn get_active_profile_json(&self) -> String {
        let p = self.read_rw(&self.active_profile).clone();
        serde_json::to_string(&p).unwrap_or_else(|_| "{}".to_string())
    }

    fn get_power_state(&self) -> String {
        self.current_decision().profile_state.as_str().to_string()
    }

    fn get_power_policy_json(&self) -> String {
        self.current_decision().status.to_string()
    }

    fn get_power_device_settings_json(&self) -> String {
        self.power_devices.settings().to_string()
    }

    fn get_power_device_status_json(&self) -> String {
        self.power_devices.status().to_string()
    }

    fn set_power_device_settings_json(&self, json_str: &str) -> bool {
        self.set_power_device_settings_json_impl(json_str)
    }

    fn scan_power_devices(&self) -> bool {
        self.scan_power_devices_impl()
    }

    fn get_lightbar_settings_json(&self) -> String {
        self.lock_mutex(&self.lightbar_ctrl)
            .settings_json()
            .to_string()
    }

    fn get_lightbar_status_json(&self) -> String {
        self.lock_mutex(&self.lightbar_ctrl)
            .status_json()
            .to_string()
    }

    fn set_lightbar_settings_json(&self, json_str: &str) -> bool {
        self.set_lightbar_settings_json_impl(json_str)
    }

    fn trigger_lightbar_animation(&self, force: bool) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        self.lock_mutex(&self.lightbar_ctrl).trigger(force)
    }

    fn set_manual_power_mode(&self, mode_val: &str) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let ok = self.write_rw(&self.power_policy).set_manual_mode(mode_val);
        if ok {
            self.reconcile_power_state(false);
        }
        ok
    }

    fn register_power_request(&self, name: &str, reason: &str, mode_val: &str) -> bool {
        self.register_power_request_with_sender("local", name, reason, mode_val)
    }

    fn register_power_request_with_sender(
        &self,
        sender: &str,
        name: &str,
        reason: &str,
        mode_val: &str,
    ) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let owner = if sender.is_empty() { "local" } else { sender };
        if owner.starts_with(':')
            && self
                .lock_mutex(&self.disconnected_senders)
                .0
                .contains(owner)
        {
            return false;
        }
        let ok = self
            .write_rw(&self.power_policy)
            .register_request(owner, name, reason, mode_val);
        if ok {
            self.reconcile_power_state(false);
        }
        ok
    }

    fn unregister_power_request(&self, name: &str) -> bool {
        self.unregister_power_request_with_sender("local", name)
    }

    fn unregister_power_request_with_sender(&self, sender: &str, name: &str) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let owner = if sender.is_empty() { "local" } else { sender };
        let ok = self
            .write_rw(&self.power_policy)
            .unregister_request(owner, name);
        if ok {
            self.reconcile_power_state(false);
        }
        ok
    }

    fn set_temp_profile(&self, profile_name: &str) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        if profile_name.is_empty() {
            *self.write_rw(&self.temp_profile_name) = None;
            return true;
        }
        for _ in 0..3 {
            let found = {
                let settings = self.read_rw(&self.settings);
                settings.get_profile(profile_name).cloned()
            };
            let Some(prof) = found else {
                return false;
            };
            match self.apply_profile_internal(&prof, true, None, None, None, Some(&prof)) {
                ApplyOutcome::Committed => {
                    *self.write_rw(&self.temp_profile_name) = Some(prof.id);
                    return true;
                }
                ApplyOutcome::SupersededReconciled => return true,
                ApplyOutcome::StaleRevision => continue,
                ApplyOutcome::Failed => return false,
            }
        }
        false
    }

    fn set_active_profile(&self, id: &str) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        for _ in 0..3 {
            let prof = {
                let settings = self.read_rw(&self.settings);
                settings.get_profile(id).cloned()
            };
            let Some(prof) = prof else {
                return false;
            };
            let state_key = self.get_power_state();
            let updates = [(state_key.clone(), prof.id.clone())];
            match self.apply_profile_internal(
                &prof,
                true,
                Some(&updates),
                Some(&state_key),
                None,
                Some(&prof),
            ) {
                ApplyOutcome::Committed | ApplyOutcome::SupersededReconciled => return true,
                ApplyOutcome::StaleRevision => continue,
                ApplyOutcome::Failed => return false,
            }
        }
        false
    }

    fn apply_profile(&self, profile_json: &str) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        if profile_json.len() > 1024 * 1024 {
            return false;
        }
        let Ok(profile) = serde_json::from_str::<UccProfile>(profile_json) else {
            return false;
        };
        let initial_state = self.get_power_state();
        match self.apply_profile_internal(&profile, true, None, Some(&initial_state), None, None) {
            ApplyOutcome::Committed => {
                if self.get_power_state() == initial_state {
                    *self.lock_mutex(&self.last_applied_profile_state) = Some(initial_state);
                } else {
                    *self.lock_mutex(&self.last_applied_profile_state) = None;
                }
                true
            }
            ApplyOutcome::SupersededReconciled => true,
            ApplyOutcome::StaleRevision | ApplyOutcome::Failed => false,
        }
    }

    fn get_profiles_json(&self) -> String {
        let mut list: Vec<UccProfile> = self
            .read_rw(&self.settings)
            .profiles
            .values()
            .cloned()
            .collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_string())
    }

    fn get_custom_profiles_json(&self) -> String {
        let mut list: Vec<UccProfile> = self
            .read_rw(&self.settings)
            .profiles
            .values()
            .filter(|p| !is_builtin_profile_id(&p.id))
            .cloned()
            .collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_string())
    }

    fn set_fan_profile_cpu(&self, points_json: &str) -> bool {
        self.set_fan_profile_cpu_impl(points_json)
    }

    fn set_fan_profile_dgpu(&self, points_json: &str) -> bool {
        self.set_fan_profile_dgpu_impl(points_json)
    }

    fn apply_fan_profiles(&self, fan_profiles_json: &str) -> bool {
        self.apply_fan_profiles_impl(fan_profiles_json)
    }

    fn revert_fan_profiles(&self) -> bool {
        self.revert_fan_profiles_impl()
    }

    fn get_default_profiles_json(&self) -> String {
        serde_json::to_string(&[
            default_quiet_profile(),
            default_office_profile(),
            default_high_performance_profile(),
            default_max_energy_save_profile(),
        ])
        .unwrap_or_else(|_| "[]".to_string())
    }

    fn get_cpu_frequency_limits_json(&self) -> String {
        let Some(first) = self.discover_cpu_dirs().into_iter().next() else {
            return "{}".to_string();
        };
        let cpufreq = first.join("cpufreq");
        let min_khz = self
            .sysfs
            .read(&cpufreq.join("cpuinfo_min_freq"))
            .ok()
            .and_then(|s| s.trim().parse::<i64>().ok());
        let max_khz = self
            .sysfs
            .read(&cpufreq.join("cpuinfo_max_freq"))
            .ok()
            .and_then(|s| s.trim().parse::<i64>().ok());
        match (min_khz, max_khz) {
            (Some(min), Some(max)) => json!({ "min": min, "max": max }).to_string(),
            (Some(min), None) => json!({ "min": min }).to_string(),
            (None, Some(max)) => json!({ "max": max }).to_string(),
            (None, None) => "{}".to_string(),
        }
    }

    fn get_default_values_profile_json(&self) -> String {
        serde_json::to_string(&default_office_profile()).unwrap_or_else(|_| "{}".to_string())
    }

    fn add_custom_profile(&self, profile_json: &str) -> bool {
        self.persist_or_reapply_custom_profile(profile_json, false)
    }

    fn save_custom_profile(&self, profile_json: &str) -> bool {
        self.persist_or_reapply_custom_profile(profile_json, false)
    }

    fn delete_custom_profile(&self, profile_id: &str) -> bool {
        self.delete_custom_profile_impl(profile_id)
    }

    fn update_custom_profile(&self, profile_json: &str) -> bool {
        self.persist_or_reapply_custom_profile(profile_json, true)
    }

    fn get_fan_profile(&self, name: &str) -> String {
        let builtins = builtin_fan_profiles();
        if let Some(fp) = builtins
            .iter()
            .find(|fp| fp.id == name)
            .or_else(|| builtins.iter().find(|fp| fp.name == name))
        {
            return serde_json::to_string(fp).unwrap_or_else(|_| "{}".to_string());
        }
        let custom = self.read_rw(&self.custom_fan_profiles);
        custom
            .get(name)
            .or_else(|| custom.values().find(|fp| fp.name == name))
            .and_then(|fp| serde_json::to_string(fp).ok())
            .unwrap_or_else(|| "{}".to_string())
    }

    fn get_fan_profile_names(&self) -> String {
        let entries: Vec<Value> = builtin_fan_profiles()
            .into_iter()
            .map(|fp| json!({ "id": fp.id, "name": fp.name }))
            .collect();
        serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string())
    }

    fn set_fan_profile(&self, name: &str, json_str: &str) -> bool {
        self.save_custom_fan_profile_impl(name, name, json_str)
    }

    fn get_settings_json(&self) -> String {
        let settings = self.read_rw(&self.settings);
        let Ok(mut val) = serde_json::to_value(&*settings) else {
            return settings
                .to_json_pretty()
                .unwrap_or_else(|_| "{}".to_string());
        };
        if let Some(obj) = val.as_object_mut() {
            obj.insert(
                "hwpDynamicBoostSupported".to_string(),
                json!(self.capabilities.device.hwp_dynamic_boost_supported),
            );
            obj.insert(
                "waterCoolerSupported".to_string(),
                json!(self.capabilities.device.water_cooler_supported),
            );
            obj.insert(
                "ctgpSupported".to_string(),
                json!(self.capabilities.device.ctgp_supported),
            );
        }
        serde_json::to_string_pretty(&val).unwrap_or_else(|_| "{}".to_string())
    }

    fn set_state_map(&self, state: &str, profile_id: &str) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let Ok(parsed) = state.parse::<ProfileState>() else {
            return false;
        };
        let key = parsed.as_str().to_string();
        if profile_id.trim().is_empty() {
            let cleared = self
                .update_settings(|s| {
                    s.state_map.remove(&key);
                    Some(())
                })
                .is_some();
            if cleared {
                let dec = self.current_decision();
                if dec.profile_state.as_str() == key.as_str()
                    || dec.effective.as_str() == key.as_str()
                {
                    *self.lock_mutex(&self.last_applied_profile_state) = None;
                }
            }
            return cleared;
        }
        for _ in 0..3 {
            let resolved = {
                let s = self.read_rw(&self.settings);
                s.get_profile(profile_id).cloned()
            };
            let Some(prof) = resolved else {
                return false;
            };
            if prof.validate().is_err() {
                return false;
            }
            let updates = [(key.clone(), prof.id.clone())];
            if self.get_power_state() == key {
                match self.apply_profile_internal(
                    &prof,
                    true,
                    Some(&updates),
                    Some(&key),
                    None,
                    Some(&prof),
                ) {
                    ApplyOutcome::Committed => return true,
                    ApplyOutcome::StaleRevision => continue,
                    ApplyOutcome::SupersededReconciled | ApplyOutcome::Failed => return false,
                }
            } else {
                return self
                    .update_settings(|s| {
                        if !s.profiles.contains_key(&prof.id) {
                            return None;
                        }
                        s.state_map.insert(key.clone(), prof.id.clone());
                        Some(())
                    })
                    .is_some();
            }
        }
        false
    }

    fn set_batch_state_map(&self, state_map_json: &str) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let Ok(map) = serde_json::from_str::<HashMap<String, String>>(state_map_json) else {
            return false;
        };
        for _ in 0..3 {
            let mut updates: Vec<(String, String)> = Vec::with_capacity(map.len());
            let mut resolved_profiles: Vec<(String, UccProfile)> = Vec::with_capacity(map.len());
            {
                let s = self.read_rw(&self.settings);
                for (k, v) in &map {
                    let Ok(st) = k.parse::<ProfileState>() else {
                        return false;
                    };
                    let key = st.as_str().to_string();
                    if v.trim().is_empty() {
                        updates.push((key, String::new()));
                        continue;
                    }
                    let Some(prof) = s.get_profile(v).cloned() else {
                        return false;
                    };
                    if prof.validate().is_err() {
                        return false;
                    }
                    updates.push((key.clone(), prof.id.clone()));
                    resolved_profiles.push((key, prof));
                }
            }
            let cur_state = self.get_power_state();
            if let Some((_, active_prof)) =
                resolved_profiles.into_iter().find(|(k, _)| k == &cur_state)
            {
                match self.apply_profile_internal(
                    &active_prof,
                    true,
                    Some(&updates),
                    Some(&cur_state),
                    None,
                    Some(&active_prof),
                ) {
                    ApplyOutcome::Committed => return true,
                    ApplyOutcome::StaleRevision => continue,
                    ApplyOutcome::SupersededReconciled | ApplyOutcome::Failed => return false,
                }
            } else {
                let updated = self
                    .update_settings(|s| {
                        if updates.iter().any(|(_, prof_id)| {
                            !prof_id.is_empty() && !s.profiles.contains_key(prof_id)
                        }) {
                            return None;
                        }
                        for (k, prof_id) in &updates {
                            if prof_id.is_empty() {
                                s.state_map.remove(k);
                            } else {
                                s.state_map.insert(k.clone(), prof_id.clone());
                            }
                        }
                        Some(())
                    })
                    .is_some();
                if updated {
                    let dec = self.current_decision();
                    let cleared_active = updates.iter().any(|(k, prof_id)| {
                        prof_id.is_empty()
                            && (k == &cur_state
                                || k.as_str() == dec.profile_state.as_str()
                                || k.as_str() == dec.effective.as_str())
                    });
                    if cleared_active {
                        *self.lock_mutex(&self.last_applied_profile_state) = None;
                    }
                }
                return updated;
            }
        }
        false
    }

    fn odm_profiles_available(&self) -> Vec<String> {
        let avail = self.capabilities.perf_profiles_available;
        match self.capabilities.device.vendor {
            DeviceVendor::Clevo if avail > 0 => vec![
                "quiet".to_string(),
                "power_saving".to_string(),
                "performance".to_string(),
                "entertainment".to_string(),
            ],
            DeviceVendor::Uniwill if avail >= 2 => {
                let mut v = vec!["power_save".to_string(), "enthusiast".to_string()];
                if avail >= 3 {
                    v.push("overboost".to_string());
                }
                v
            }
            _ => Vec::new(),
        }
    }

    fn odm_power_limits_json(&self) -> String {
        let mut entries = Vec::new();
        for idx in 0..self.capabilities.tdp_channels {
            if let Ok((min_w, max_w)) = self.platform.read_tdp_limits(idx) {
                let cur = self.platform.read_tdp(idx).unwrap_or(-1);
                entries.push(json!({
                    "index": idx,
                    "min": min_w,
                    "max": max_w,
                    "current": cur,
                }));
            }
        }
        serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string())
    }

    fn get_keyboard_backlight_capabilities_json(&self) -> String {
        self.lock_mutex(&self.keyboard_ctrl).capabilities_json()
    }

    fn get_keyboard_backlight_states_json(&self) -> String {
        let mut ctrl = self.lock_mutex(&self.keyboard_ctrl);
        if ctrl.current_states().is_empty() && ctrl.is_available() {
            ctrl.hydrate_from_hardware();
        }
        ctrl.current_states_json()
    }

    fn set_keyboard_backlight_states_json(&self, keyboard_backlight_states_json: &str) -> bool {
        self.set_keyboard_backlight_states_json_impl(keyboard_backlight_states_json)
    }

    fn get_custom_keyboard_profiles_json(&self) -> String {
        let settings = self.read_rw(&self.settings);
        if let Some(ref map) = settings.custom_keyboard_profiles {
            let mut entries: Vec<&ucc_config::CustomKeyboardProfile> = map.values().collect();
            entries.sort_by(|a, b| a.id.cmp(&b.id));
            return serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string());
        }
        "[]".to_string()
    }

    fn save_custom_keyboard_profile(&self, id: &str, name: &str, json_str: &str) -> bool {
        self.save_custom_keyboard_profile_impl(id, name, json_str)
    }

    fn delete_custom_keyboard_profile(&self, id: &str) -> bool {
        if ucc_config::is_read_only_preview() || self.shutting_down.load(Ordering::SeqCst) {
            return false;
        }
        self.update_settings(|s| {
            s.custom_keyboard_profiles
                .as_mut()
                .and_then(|m| m.remove(id))
                .map(|_| ())
        })
        .is_some()
    }

    fn get_custom_fan_profiles_json(&self) -> String {
        let settings = self.read_rw(&self.settings);
        if let Some(ref map) = settings.custom_fan_profiles {
            let mut entries: Vec<&ucc_config::CustomFanProfile> = map.values().collect();
            entries.sort_by(|a, b| a.id.cmp(&b.id));
            return serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string());
        }
        "[]".to_string()
    }

    fn save_custom_fan_profile(&self, id: &str, name: &str, json_str: &str) -> bool {
        self.save_custom_fan_profile_impl(id, name, json_str)
    }

    fn delete_custom_fan_profile(&self, id: &str) -> bool {
        self.delete_custom_fan_profile_impl(id)
    }

    fn get_fans_min_speed(&self) -> i32 {
        self.capabilities.device.fans_min_speed
    }

    fn get_fans_off_available(&self) -> bool {
        self.capabilities.device.fans_off_available
    }

    fn get_charging_profiles_available(&self) -> String {
        let Some((avail_path, _)) = self.charging_profile_paths() else {
            return "[]".to_string();
        };
        let tokens = self.read_whitespace_tokens(&avail_path);
        serde_json::to_string(&tokens).unwrap_or_else(|_| "[]".to_string())
    }

    fn get_current_charging_profile(&self) -> String {
        let Some((_, current_path)) = self.charging_profile_paths() else {
            return String::new();
        };
        self.sysfs
            .read(&current_path)
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    }

    fn set_charging_profile(&self, profile_descriptor: &str) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let desc = profile_descriptor.trim();
        if desc.is_empty() {
            return false;
        }
        let Some((avail_path, current_path)) = self.charging_profile_paths() else {
            return false;
        };
        let avail = self.read_whitespace_tokens(&avail_path);
        if !avail.iter().any(|p| p == desc) {
            return false;
        }
        let Some(prev_hw) = self.active_bracket_token(&current_path) else {
            return false;
        };
        self.commit_direct_charging_token(
            &current_path,
            super::display_gpu::ChargingTokenKind::ChargingProfile,
            desc,
            &prev_hw,
        )
    }

    fn get_charging_priorities_available(&self) -> String {
        let Some((avail_path, _)) = self.charging_priority_paths() else {
            return "[]".to_string();
        };
        let tokens = self.read_whitespace_tokens(&avail_path);
        serde_json::to_string(&tokens).unwrap_or_else(|_| "[]".to_string())
    }

    fn get_current_charging_priority(&self) -> String {
        let Some((_, current_path)) = self.charging_priority_paths() else {
            return String::new();
        };
        self.sysfs
            .read(&current_path)
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    }

    fn set_charging_priority(&self, priority_descriptor: &str) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let desc = priority_descriptor.trim();
        if desc.is_empty() {
            return false;
        }
        let Some((avail_path, current_path)) = self.charging_priority_paths() else {
            return false;
        };
        let avail = self.read_whitespace_tokens(&avail_path);
        if !avail.iter().any(|p| p == desc) {
            return false;
        }
        let Some(prev_hw) = self.active_bracket_token(&current_path) else {
            return false;
        };
        self.commit_direct_charging_token(
            &current_path,
            super::display_gpu::ChargingTokenKind::ChargingPriority,
            desc,
            &prev_hw,
        )
    }

    fn get_charge_start_available_thresholds(&self) -> String {
        self.get_charge_start_available_thresholds_impl()
    }

    fn get_charge_end_available_thresholds(&self) -> String {
        self.get_charge_end_available_thresholds_impl()
    }

    fn get_charge_start_threshold(&self) -> i32 {
        let Some(bat) = self.discover_battery_dir() else {
            return -1;
        };
        self.sysfs
            .read(&bat.join("charge_control_start_threshold"))
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok())
            .unwrap_or(-1)
    }

    fn get_charge_end_threshold(&self) -> i32 {
        let Some(bat) = self.discover_battery_dir() else {
            return -1;
        };
        self.sysfs
            .read(&bat.join("charge_control_end_threshold"))
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok())
            .unwrap_or(-1)
    }

    fn set_charge_start_threshold(&self, value: i32) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        if !(0..=100).contains(&value) {
            return false;
        }
        let Some(bat) = self.discover_battery_dir() else {
            return false;
        };
        let path = bat.join("charge_control_start_threshold");
        if !self.sysfs.exists(&path) {
            return false;
        }
        let end = self.get_charge_end_threshold();
        if end >= 0 && value >= end {
            return false;
        }
        let Some(prev_hw) = self
            .sysfs
            .read(&path)
            .ok()
            .and_then(|s| s.trim().parse::<u8>().ok())
        else {
            return false;
        };
        self.commit_direct_charging_u8(&path, true, value as u8, prev_hw)
    }

    fn set_charge_end_threshold(&self, value: i32) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        if !(1..=100).contains(&value) {
            return false;
        }
        let Some(bat) = self.discover_battery_dir() else {
            return false;
        };
        let path = bat.join("charge_control_end_threshold");
        if !self.sysfs.exists(&path) {
            return false;
        }
        let start = self.get_charge_start_threshold();
        if start >= 0 && value <= start {
            return false;
        }
        let Some(prev_hw) = self
            .sysfs
            .read(&path)
            .ok()
            .and_then(|s| s.trim().parse::<u8>().ok())
        else {
            return false;
        };
        self.commit_direct_charging_u8(&path, false, value as u8, prev_hw)
    }

    fn get_charge_type(&self) -> String {
        let Some(bat) = self.discover_battery_dir() else {
            return String::new();
        };
        self.sysfs
            .read(&bat.join("charge_type"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    }

    fn set_charge_type(&self, type_val: &str) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let trimmed = type_val.trim();
        if trimmed.is_empty() {
            return false;
        }
        let Some(bat) = self.discover_battery_dir() else {
            return false;
        };
        let path = bat.join("charge_type");
        if !self.sysfs.exists(&path) {
            return false;
        }
        let Some(prev_hw) = self.active_bracket_token(&path) else {
            return false;
        };
        self.commit_direct_charging_token(
            &path,
            super::display_gpu::ChargingTokenKind::ChargeType,
            trimmed,
            &prev_hw,
        )
    }

    fn get_fn_lock_supported(&self) -> bool {
        self.discover_fn_lock_path().is_some()
    }

    fn get_fn_lock_status(&self) -> bool {
        let Some(path) = self.discover_fn_lock_path() else {
            return false;
        };
        self.sysfs
            .read(&path)
            .ok()
            .is_some_and(|s| matches!(s.trim(), "1" | "true" | "enabled"))
    }

    fn set_fn_lock_status(&self, status: bool) -> Result<(), String> {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return Err("Hardware control is unavailable".to_string());
        };
        let Some(path) = self.discover_fn_lock_path() else {
            return Err("Fn-Lock control is not supported on this device".to_string());
        };
        if self.write_aborted_by_shutdown() {
            return Err("Daemon shutting down".to_string());
        }
        self.sysfs
            .write(&path, if status { "1" } else { "0" })
            .map_err(|e| format!("Failed to write Fn-Lock status: {e}"))
    }

    fn set_sensor_data_collection_status(&self, status: bool) {
        self.sensor_data_collection.store(status, Ordering::SeqCst);
    }

    fn get_sensor_data_collection_status(&self) -> bool {
        self.sensor_data_collection.load(Ordering::SeqCst)
    }

    fn set_dgpu_d0_metrics(&self, status: bool) {
        self.dgpu_d0_metrics.store(status, Ordering::SeqCst);
        self.lock_mutex(&self.telemetry).set_dgpu_d0_metrics(status);
    }

    fn get_nvidia_power_ctrl_default_power_limit(&self) -> i32 {
        if let Some((_, base_w)) = *self.lock_mutex(&self.nvidia_default_power_limit_w) {
            if base_w.is_finite() && base_w > 0.0 {
                return base_w.round() as i32;
            }
        }
        self.read_rw(&self.latest_telemetry)
            .as_ref()
            .and_then(|s| s.dgpu.as_ref())
            .and_then(|d| {
                d.default_power_limit_w
                    .filter(|w| w.is_finite() && *w > 0.0)
                    .or_else(|| d.max_power_limit_w.filter(|w| w.is_finite() && *w > 0.0))
                    .or_else(|| {
                        d.enforced_power_limit_w
                            .filter(|w| w.is_finite() && *w > 0.0)
                    })
            })
            .map(|w| w.round() as i32)
            .unwrap_or(0)
    }

    fn get_nvidia_power_ctrl_max_power_limit(&self) -> i32 {
        self.read_rw(&self.latest_telemetry)
            .as_ref()
            .and_then(|s| s.dgpu.as_ref())
            .and_then(|d| d.max_power_limit_w.filter(|w| w.is_finite() && *w > 0.0))
            .map(|w| w.round() as i32)
            .unwrap_or(0)
    }

    fn get_nvidia_power_ctrl_available(&self) -> bool {
        if !self.capabilities.device.ctgp_supported {
            return false;
        }
        let has_nvml_limit = self
            .read_rw(&self.latest_telemetry)
            .as_ref()
            .and_then(|s| s.dgpu.as_ref())
            .is_some_and(|d| {
                d.max_power_limit_w
                    .is_some_and(|w| w.is_finite() && w > 0.0)
            });
        has_nvml_limit || self.nvidia_ctgp_sysfs_path().is_some()
    }

    fn set_nvidia_power_offset(&self, offset: i32) -> bool {
        if !self.get_nvidia_power_ctrl_available() {
            return false;
        }
        self.set_nvidia_power_offset_impl(offset)
    }

    fn get_available_governors(&self) -> String {
        let Some(first) = self.discover_cpu_dirs().into_iter().next() else {
            return "[]".to_string();
        };
        let tokens =
            self.read_whitespace_tokens(&first.join("cpufreq/scaling_available_governors"));
        serde_json::to_string(&tokens).unwrap_or_else(|_| "[]".to_string())
    }

    fn get_available_epps(&self) -> String {
        let Some(first) = self.discover_cpu_dirs().into_iter().next() else {
            return "[]".to_string();
        };
        let tokens = self.read_whitespace_tokens(
            &first.join("cpufreq/energy_performance_available_preferences"),
        );
        serde_json::to_string(&tokens).unwrap_or_else(|_| "[]".to_string())
    }

    fn get_cpu_core_count(&self) -> i32 {
        self.read_present_cpu_count()
    }

    fn get_cpu_topology_json(&self) -> String {
        self.read_cpu_topology_cached_json()
    }

    fn get_water_cooler_available(&self) -> bool {
        self.water_cooler_available.load(Ordering::SeqCst)
    }

    fn get_water_cooler_connected(&self) -> bool {
        self.water_cooler_connected.load(Ordering::SeqCst)
            || self.lock_mutex(&self.cooler_controller).state() == CoolerState::Connected
    }

    fn get_water_cooler_fan_speed(&self) -> i32 {
        self.get_water_cooler_fan_speed_impl()
    }

    fn get_water_cooler_pump_level(&self) -> i32 {
        self.get_water_cooler_pump_level_impl()
    }

    fn enable_water_cooler(&self, enable: bool) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        if !self.water_cooler_available.load(Ordering::SeqCst) {
            return false;
        }
        self.water_cooler_user_disabled
            .store(!enable, Ordering::SeqCst);
        let prev = self.water_cooler_enabled.swap(enable, Ordering::SeqCst);
        self.write_rw(&self.active_profile).fan.enable_water_cooler = enable;
        if prev != enable {
            self.emit_signal(DbusSignalEvent::WaterCoolerStatusChanged {
                status: if enable { "enabled" } else { "disabled" }.to_string(),
            });
        }
        true
    }

    fn is_water_cooler_enabled(&self) -> bool {
        self.water_cooler_enabled.load(Ordering::SeqCst)
    }

    fn set_water_cooler_fan_speed(&self, duty_cycle_percent: i32) -> bool {
        self.set_water_cooler_fan_speed_impl(duty_cycle_percent)
    }

    fn set_water_cooler_pump_voltage(&self, voltage: i32) -> bool {
        self.set_water_cooler_pump_voltage_impl(voltage)
    }

    fn set_water_cooler_led_color(&self, red: i32, green: i32, blue: i32, mode_val: i32) -> bool {
        self.set_water_cooler_led_color_impl(red, green, blue, mode_val)
    }

    fn turn_off_water_cooler_led(&self) -> bool {
        self.turn_off_water_cooler_led_impl()
    }

    fn turn_off_water_cooler_fan(&self) -> bool {
        self.turn_off_water_cooler_fan_impl()
    }

    fn turn_off_water_cooler_pump(&self) -> bool {
        self.turn_off_water_cooler_pump_impl()
    }

    fn is_water_cooler_auto_control_enabled(&self) -> bool {
        self.lock_mutex(&self.cooler_controller).mode() == CoolerMode::Auto
    }

    fn set_water_cooler_auto_control(&self, enabled: bool) -> bool {
        self.set_water_cooler_auto_control_impl(enabled)
    }

    fn get_water_cooler_supported(&self) -> bool {
        self.capabilities.device.water_cooler_supported
    }

    fn get_ctgp_adjustment_supported(&self) -> bool {
        self.capabilities.device.ctgp_supported
    }

    fn get_hwp_dynamic_boost_supported(&self) -> bool {
        self.capabilities.device.hwp_dynamic_boost_supported
    }

    fn get_monitor_data_since(&self, since_timestamp_ms: i64) -> Vec<u8> {
        self.history.query_since_binary(since_timestamp_ms)
    }

    fn set_monitor_history_horizon(&self, seconds: i32) {
        self.history.set_horizon_seconds(seconds);
    }

    fn get_monitor_history_horizon(&self) -> i32 {
        self.history.horizon_seconds()
    }

    fn get_cpu_frequency_mhz(&self) -> i32 {
        if !self.sensor_data_collection.load(Ordering::SeqCst) {
            return -1;
        }
        self.read_rw(&self.latest_telemetry)
            .as_ref()
            .and_then(|s| s.cpu.freq_mhz)
            .map(|f| f as i32)
            .unwrap_or(-1)
    }
}
