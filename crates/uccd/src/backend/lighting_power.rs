// SPDX-License-Identifier: GPL-3.0-or-later

//! Keyboard RGB, Lightbar animation ticks, PowerPolicy/PowerDevices, and WakeupPolicy (`ISSUE-03`, `ISSUE-13`, `ISSUE-34`).

use super::{DbusSignalEvent, LiveUccdBackend};
use crate::lid::{parse_lid_policy_from_json, LidSourcePolicy, WakeupPolicy};
use serde_json::Value;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use ucc_config::UccSettings;
use ucc_core::{KeyboardBacklightState, PolicyDecision, ProfileState, UccProfile};
use ucc_dbus::UccdBackend;
use ucc_hardware::{CoolerState, PowerDeviceController, RgbTarget, Sysfs};

impl LiveUccdBackend {
    pub(super) fn lock_mutex<'a, T>(&self, m: &'a std::sync::Mutex<T>) -> std::sync::MutexGuard<'a, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(super) fn read_rw<'a, T>(&self, rw: &'a std::sync::RwLock<T>) -> std::sync::RwLockReadGuard<'a, T> {
        rw.read().unwrap_or_else(|e| e.into_inner())
    }

    pub(super) fn write_rw<'a, T>(
        &self,
        rw: &'a std::sync::RwLock<T>,
    ) -> std::sync::RwLockWriteGuard<'a, T> {
        rw.write().unwrap_or_else(|e| e.into_inner())
    }

    /// Serializes read-modify-save-commit under `config_manager` lock (`M8-RECHECK-06`),
    /// ensuring concurrent callers never overwrite one another's changes and RAM never diverges from disk.
    pub(super) fn update_settings<R>(
        &self,
        mutate: impl FnOnce(&mut UccSettings) -> Option<R>,
    ) -> Option<R> {
        let mgr = self.lock_mutex(&self.config_manager);
        let mut candidate = self.read_rw(&self.settings).clone();
        let outcome = mutate(&mut candidate)?;
        if mgr.save(&candidate).is_err() {
            return None;
        }
        *self.write_rw(&self.settings) = candidate;
        Some(outcome)
    }

    pub(super) fn set_mode_reapply_pending(&self, pending: bool) {
        let prev = self.mode_reapply_pending.swap(pending, Ordering::SeqCst);
        if prev != pending {
            self.emit_signal(DbusSignalEvent::ModeReapplyPendingChanged { pending });
        }
    }

    pub(super) fn detect_physical_power_source_for(
        sysfs: &Arc<dyn Sysfs>,
        sys_root: &Path,
    ) -> ProfileState {
        let ps_dir = sys_root.join("sys/class/power_supply");
        let Ok(entries) = sysfs.read_dir(&ps_dir) else {
            return ProfileState::Ac;
        };
        let mut saw_ac = false;
        let mut saw_usb = false;
        let mut known_supply = false;
        for entry in entries {
            let Ok(kind_raw) = sysfs.read(&entry.join("type")) else {
                continue;
            };
            let kind = kind_raw.trim();
            let dir_name = entry
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            let is_usb = kind.starts_with("USB")
                || dir_name.starts_with("ucsi")
                || dir_name.starts_with("USB");
            let is_mains = kind == "Mains";
            known_supply |= is_usb || is_mains || kind == "Battery";

            let online = sysfs
                .read(&entry.join("online"))
                .ok()
                .is_some_and(|v| v.trim() == "1");
            if !online {
                continue;
            }
            saw_usb |= is_usb;
            saw_ac |= is_mains;
        }
        if saw_usb {
            ProfileState::TypeC
        } else if saw_ac {
            ProfileState::Ac
        } else if known_supply {
            ProfileState::Bat
        } else {
            ProfileState::Ac
        }
    }

    pub(super) fn detect_physical_power_source(&self) -> ProfileState {
        Self::detect_physical_power_source_for(&self.sysfs, &self.sys_root)
    }

    pub(super) fn current_decision(&self) -> PolicyDecision {
        let src = self.detect_physical_power_source();
        let wc = self.water_cooler_connected.load(Ordering::SeqCst);
        self.read_rw(&self.power_policy).evaluate(src, wc)
    }

    /// Parses either a flat `Vec<KeyboardBacklightState>` OR an object `{states, keyboardProfileId?, brightness?}` (`ISSUE-13`).
    pub(super) fn parse_keyboard_states_value(
        val: &Value,
    ) -> Option<(Vec<KeyboardBacklightState>, Option<String>)> {
        if let Some(arr) = val.as_array() {
            let states: Vec<KeyboardBacklightState> =
                serde_json::from_value(Value::Array(arr.clone())).ok()?;
            return (!states.is_empty()).then_some((states, None));
        }
        let obj = val.as_object()?;
        let default_brightness = obj.get("brightness").and_then(|v| v.as_i64());
        let kb_profile_id = obj
            .get("keyboardProfileId")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let raw_states = obj.get("states")?.as_array()?;
        let mut normalized_arr = Vec::with_capacity(raw_states.len());
        for entry in raw_states {
            let mut item = entry.as_object()?.clone();
            if !item.contains_key("brightness") {
                if let Some(b) = default_brightness {
                    item.insert("brightness".to_string(), Value::from(b));
                }
            }
            normalized_arr.push(Value::Object(item));
        }
        let states: Vec<KeyboardBacklightState> =
            serde_json::from_value(Value::Array(normalized_arr)).ok()?;
        (!states.is_empty()).then_some((states, kb_profile_id))
    }

    pub(super) fn parse_keyboard_states_payload(
        json_str: &str,
    ) -> Option<(Vec<KeyboardBacklightState>, Option<String>)> {
        let val: Value = serde_json::from_str(json_str).ok()?;
        Self::parse_keyboard_states_value(&val)
    }

    pub(super) fn apply_profile_keyboard(&self, profile: &UccProfile) {
        if self.is_vendor_unsupported() || ucc_config::is_read_only_preview() {
            return;
        }
        let kb_id = profile
            .selected_keyboard_profile
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(profile.keyboard.keyboard_profile_id.trim());
        if kb_id.is_empty() {
            return;
        }
        let saved_json = {
            let settings = self.read_rw(&self.settings);
            settings.custom_keyboard_profiles.as_ref().and_then(|map| {
                map.get(kb_id)
                    .or_else(|| map.values().find(|e| e.name.eq_ignore_ascii_case(kb_id)))
                    .map(|e| e.json.clone())
            })
        };
        let Some(val) = saved_json else {
            return;
        };
        let Some((states, _)) = Self::parse_keyboard_states_value(&val) else {
            return;
        };
        let lid_policy = self.current_lid_source_policy();
        let blanked_under_lid =
            self.is_lid_closed() && lid_policy.turn_off_keyboard_backlight_on_lid_close;
        if blanked_under_lid {
            *self.lock_mutex(&self.lid_saved_keyboard_states) = Some(states.clone());
        }
        let target_states = if blanked_under_lid {
            Self::blanked_keyboard_states(&states)
        } else {
            states
        };
        let applied = {
            let mut ctrl = self.lock_mutex(&self.keyboard_ctrl);
            if !ctrl.is_available() {
                let _ = ctrl.init();
            }
            ctrl.is_available() && ctrl.apply_states(&target_states).is_ok()
        };
        if applied && !blanked_under_lid {
            self.lock_mutex(&self.lightbar_ctrl).keyboard_changed();
        }
    }

    pub(super) fn current_lid_source_policy(&self) -> LidSourcePolicy {
        let pd_val = serde_json::from_str::<Value>(&self.get_power_device_settings_json())
            .unwrap_or(Value::Null);
        parse_lid_policy_from_json(&pd_val)
    }

    pub(super) fn is_water_cooler_connected_now(&self) -> bool {
        self.water_cooler_connected.load(Ordering::SeqCst)
            || self.lock_mutex(&self.cooler_controller).state() == CoolerState::Connected
    }

    fn blanked_keyboard_states(states: &[KeyboardBacklightState]) -> Vec<KeyboardBacklightState> {
        states
            .iter()
            .cloned()
            .map(|mut s| {
                s.brightness = 0;
                s
            })
            .collect()
    }

    pub fn seed_initial_lid_state(&self, closed: bool) {
        self.lid_closed.store(closed, Ordering::SeqCst);
    }

    pub fn is_lid_closed(&self) -> bool {
        self.lid_closed.load(Ordering::SeqCst)
    }

    /// Blanks keyboard backlight and optional water cooler LED in RAM on lid close (`M8.10`),
    /// restoring previous live states when reopened without mutating `settings.json`.
    pub fn on_lid_state_changed(&self, closed: bool) {
        if self.shutting_down.load(Ordering::SeqCst)
            || ucc_config::is_read_only_preview()
            || self.is_vendor_unsupported()
        {
            self.lid_closed.store(closed, Ordering::SeqCst);
            return;
        }
        let prev = self.lid_closed.swap(closed, Ordering::SeqCst);
        if prev == closed {
            return;
        }
        if !closed {
            self.restore_lid_lighting();
            return;
        }

        let policy = self.current_lid_source_policy();
        if policy.turn_off_keyboard_backlight_on_lid_close {
            let mut ctrl = self.lock_mutex(&self.keyboard_ctrl);
            if !ctrl.is_available() {
                let _ = ctrl.init();
            }
            if ctrl.is_available() {
                let current = ctrl.current_states().to_vec();
                if !current.is_empty() && current.iter().any(|s| s.brightness > 0) {
                    {
                        let mut saved = self.lock_mutex(&self.lid_saved_keyboard_states);
                        if saved.is_none() {
                            *saved = Some(current.clone());
                        }
                    }
                    let dark = Self::blanked_keyboard_states(&current);
                    let _ = ctrl.apply_states(&dark);
                }
            }
        }

        if policy.turn_off_water_cooler_led_on_lid_close && self.is_water_cooler_connected_now() {
            let mut cooler = self.lock_mutex(&self.cooler_controller);
            let current_rgb = cooler.desired_rgb_target();
            if matches!(current_rgb, RgbTarget::Color { .. }) {
                {
                    let mut saved = self.lock_mutex(&self.lid_saved_wc_rgb);
                    if saved.is_none() {
                        *saved = Some(current_rgb);
                    }
                }
                let _ = cooler.turn_off_led();
            }
        }
    }

    /// Restores keyboard backlight and water cooler LED previously blanked by lid close (`M8.10`).
    pub fn restore_lid_lighting(&self) {
        if self.shutting_down.load(Ordering::SeqCst)
            || ucc_config::is_read_only_preview()
            || self.is_vendor_unsupported()
        {
            return;
        }
        if let Some(states) = self.lock_mutex(&self.lid_saved_keyboard_states).take() {
            let mut ctrl = self.lock_mutex(&self.keyboard_ctrl);
            if !ctrl.is_available() {
                let _ = ctrl.init();
            }
            if ctrl.is_available() {
                let _ = ctrl.apply_states(&states);
            }
        }
        if let Some(RgbTarget::Color {
            red,
            green,
            blue,
            state,
        }) = self.lock_mutex(&self.lid_saved_wc_rgb).take()
        {
            if self.is_water_cooler_connected_now() {
                let mut cooler = self.lock_mutex(&self.cooler_controller);
                let _ = cooler.set_rgb(red, green, blue, state);
            }
        }
    }

    pub(super) fn set_keyboard_backlight_states_json_impl(
        &self,
        keyboard_backlight_states_json: &str,
    ) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        if keyboard_backlight_states_json.len() > 1024 * 1024 {
            return false;
        }
        let Some((states, kb_profile_id)) =
            Self::parse_keyboard_states_payload(keyboard_backlight_states_json)
        else {
            return false;
        };

        // Write to hardware BEFORE updating persistent settings (`ISSUE-13`).
        let blanked_under_lid = self.is_lid_closed()
            && self
                .current_lid_source_policy()
                .turn_off_keyboard_backlight_on_lid_close;
        {
            let mut ctrl = self.lock_mutex(&self.keyboard_ctrl);
            if !ctrl.is_available() {
                let _ = ctrl.init();
            }
            if blanked_under_lid {
                let dark = Self::blanked_keyboard_states(&states);
                if ctrl.apply_states(&dark).is_err() {
                    return false;
                }
            } else if ctrl.apply_states(&states).is_err() {
                return false;
            }
        }
        if blanked_under_lid {
            *self.lock_mutex(&self.lid_saved_keyboard_states) = Some(states.clone());
        } else {
            *self.lock_mutex(&self.lid_saved_keyboard_states) = None;
        }

        if self
            .update_settings(|s| {
                s.keyboard_backlight_states = Some(states.clone());
                Some(())
            })
            .is_none()
        {
            return false;
        }

        if !blanked_under_lid {
            self.lock_mutex(&self.lightbar_ctrl).keyboard_changed();
        }

        if let Some(ref pid) = kb_profile_id {
            let updated = {
                let mut active = self.write_rw(&self.active_profile);
                active.selected_keyboard_profile = Some(pid.clone());
                active.keyboard.keyboard_profile_id = pid.clone();
                active.clone()
            };
            self.emit_profile_changed(&updated);
        }
        true
    }

    pub(super) fn save_custom_keyboard_profile_impl(
        &self,
        id: &str,
        name: &str,
        json_str: &str,
    ) -> bool {
        if ucc_config::is_read_only_preview()
            || self.shutting_down.load(Ordering::SeqCst)
            || json_str.len() > 1024 * 1024
        {
            return false;
        }
        if id.trim().is_empty() {
            return false;
        }
        let Ok(parsed) = serde_json::from_str::<Value>(json_str) else {
            return false;
        };
        self.update_settings(|s| {
            s.custom_keyboard_profiles
                .get_or_insert_with(std::collections::HashMap::new)
                .insert(
                    id.to_string(),
                    ucc_config::CustomKeyboardProfile {
                        id: id.to_string(),
                        name: name.to_string(),
                        json: parsed,
                    },
                );
            Some(())
        })
        .is_some()
    }

    pub(super) fn set_lightbar_settings_json_impl(&self, json_str: &str) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let Ok(val) = serde_json::from_str::<Value>(json_str) else {
            return false;
        };
        let Ok(normalized_cfg) = ucc_lighting::LightbarSettings::normalize_json(&val) else {
            return false;
        };
        let normalized_json = normalized_cfg.to_json();
        if self
            .update_settings(|s| {
                s.lightbar = Some(normalized_json.clone());
                Some(())
            })
            .is_none()
        {
            return false;
        }
        let mut lb = self.lock_mutex(&self.lightbar_ctrl);
        let _ = lb.configure(&val);
        let actual_json = lb.settings_json().to_string();
        drop(lb);
        self.emit_signal(DbusSignalEvent::LightbarSettingsChanged { json: actual_json });
        true
    }

    /// Advances an active lightbar animation deterministically on periodic daemon ticks (`M8-REVIEW-18`).
    pub fn tick_lightbar_animation(&self, started_at: &mut Option<std::time::Instant>) {
        if self.shutting_down.load(Ordering::SeqCst) || ucc_config::is_read_only_preview() {
            return;
        }
        let mut lb = self.lock_mutex(&self.lightbar_ctrl);
        if lb.is_animating() {
            lb.tick_now();
            *started_at = lb.is_animating().then(std::time::Instant::now);
        } else {
            *started_at = None;
        }
    }

    /// Records a departed D-Bus connection and removes its in-memory power requests without
    /// running hardware reconciliation (for async callers that offload reconciliation).
    pub(crate) fn record_dbus_client_disconnected(&self, owner: &str) -> bool {
        if owner.is_empty() {
            return false;
        }
        if owner.starts_with(':') {
            let mut disconnected = self.lock_mutex(&self.disconnected_senders);
            if disconnected.0.insert(owner.to_string()) {
                disconnected.1.push_back(owner.to_string());
                if disconnected.1.len() > 1024 {
                    if let Some(oldest) = disconnected.1.pop_front() {
                        disconnected.0.remove(&oldest);
                    }
                }
            }
        }
        let mut policy = self.write_rw(&self.power_policy);
        let existed = policy.requests().keys().any(|(o, _)| o == owner);
        if existed {
            policy.remove_owner(owner);
        }
        existed
    }

    /// Evicts any power requests owned by a departed D-Bus connection (`M8-REVIEW-19`).
    pub fn on_dbus_client_disconnected(&self, owner: &str) {
        if self.record_dbus_client_disconnected(owner) {
            self.reconcile_power_state(false);
        }
    }

    pub(super) fn apply_wakeup_policy_from_settings(&self, pd_val: &Value) {
        if self.is_vendor_unsupported() || ucc_config::is_read_only_preview() {
            return;
        }
        if let Some(wake_only) = pd_val
            .get("lid")
            .and_then(|l| l.get("wakeOnlyLidAndKeyboard"))
            .and_then(|v| v.as_bool())
        {
            if wake_only {
                let policy = WakeupPolicy::with_sys_proc(
                    self.sys_root.join("sys"),
                    self.sys_root.join("proc"),
                );
                let _ = policy.apply(true, self.sysfs.as_ref());
            }
        }
    }

    pub(super) fn set_power_device_settings_json_impl(&self, json_str: &str) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let Ok(val) = serde_json::from_str::<Value>(json_str) else {
            return false;
        };
        if PowerDeviceController::validate(&val).is_err() {
            return false;
        }
        if self
            .update_settings(|s| {
                s.power_devices = Some(val.clone());
                Some(())
            })
            .is_none()
        {
            return false;
        }
        if self.power_devices.set_settings(val.clone()).is_err() {
            return false;
        }
        let effective_mode = self.current_decision().effective;
        if !self.is_vendor_unsupported() {
            self.power_devices.apply(effective_mode.mode_name());
            self.apply_wakeup_policy_from_settings(&val);
        }
        let pd_rev = self.power_devices.revision();
        *self.lock_mutex(&self.last_power_device_revision) = Some(pd_rev);
        let active = self.read_rw(&self.active_profile).clone();
        match self.apply_cpu_settings(&active) {
            Ok(res) if res.fully_applied => {
                *self.lock_mutex(&self.cpu_policy_applied_rev) = Some((pd_rev, effective_mode));
            }
            _ => {
                *self.lock_mutex(&self.cpu_policy_applied_rev) = None;
            }
        }
        self.emit_signal(DbusSignalEvent::PowerDeviceStatusChanged {
            json: self.power_devices.status().to_string(),
        });
        true
    }

    pub(super) fn scan_power_devices_impl(&self) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let scanned = self.power_devices.scan();
        if PowerDeviceController::validate(&scanned).is_err() {
            return false;
        }
        if self
            .update_settings(|s| {
                s.power_devices = Some(scanned.clone());
                Some(())
            })
            .is_none()
        {
            return false;
        }
        let _ = self.power_devices.set_settings(scanned);
        true
    }
}
