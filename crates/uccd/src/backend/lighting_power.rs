// SPDX-License-Identifier: GPL-3.0-or-later

//! Keyboard RGB, Lightbar animation ticks, PowerPolicy/PowerDevices, and WakeupPolicy (`ISSUE-03`, `ISSUE-13`, `ISSUE-34`).

use super::{DbusSignalEvent, LiveUccdBackend};
use crate::lid::{parse_lid_policy_from_json, LidSourcePolicy, WakeupPolicy};
use serde_json::Value;

pub(super) type KeyboardRollbackSnapshot = (
    ucc_lighting::KeyboardHardwareSnapshot,
    Option<Vec<ucc_core::KeyboardBacklightState>>,
);
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use ucc_config::UccSettings;
use ucc_core::{KeyboardBacklightState, PolicyDecision, ProfileState, UccProfile};
use ucc_dbus::UccdBackend;
use ucc_hardware::{
    CoolerState, PlatformBackend, PlatformBackendKind, PlatformError, PowerDeviceController,
    RgbTarget, Sysfs,
};
use ucc_lighting::{
    EcLidLogoHook, EcLightbarPowerHook, HidrawDeviceTransport, LightingError, LightingTransport,
    SysfsLedTransport, UniwillHidTransport,
};

/// Checks whether `tuxedo-drivers` / `ite_8291` sysfs LED class nodes are present under `sys_root`.
pub fn has_ite_sysfs_leds(sysfs: &dyn Sysfs, sys_root: &Path) -> bool {
    let leds_dir = sys_root.join("sys/class/leds");
    let has_kbd = sysfs.read_dir(&leds_dir).is_ok_and(|entries| {
        entries.iter().any(|entry| {
            entry.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.starts_with("rgb:kbd_backlight")
                    || n.starts_with("tuxedo::kbd_backlight")
                    || n.starts_with("ite_8291")
            })
        })
    });
    let has_lb = sysfs.exists(&leds_dir.join("rgb:lightbar/multi_intensity"))
        || sysfs.exists(&leds_dir.join("lightbar/multi_intensity"));
    has_kbd || has_lb
}

/// Selects the lighting transport for keyboard backlight and front lightbar (`M14` / `ADR-05`).
///
/// Prefers direct USB `hidraw` ([`UniwillHidTransport`] + [`HidrawDeviceTransport`]) when
/// [`PlatformBackendKind::Uwcc`] is active or when `ite_8291` sysfs LED nodes are absent,
/// while preserving [`SysfsLedTransport`] when running on `tuxedo-drivers`.
pub fn select_lighting_transport(
    backend_kind: PlatformBackendKind,
    sysfs: &dyn Sysfs,
    sys_root: &Path,
) -> Arc<dyn LightingTransport> {
    if backend_kind == PlatformBackendKind::Uwcc || !has_ite_sysfs_leds(sysfs, sys_root) {
        Arc::new(UniwillHidTransport::new(Arc::new(
            HidrawDeviceTransport::new(),
        )))
    } else {
        Arc::new(SysfsLedTransport::default())
    }
}

/// Selects the lighting transport and wires the EC Lightbar power rail hook (`0x0748`)
/// to `platform` when using direct USB `hidraw` (`M14.6` / `ADR-05`).
pub fn select_lighting_transport_with_platform(
    platform: &Arc<dyn PlatformBackend>,
    sysfs: &dyn Sysfs,
    sys_root: &Path,
) -> Arc<dyn LightingTransport> {
    if platform.kind() == PlatformBackendKind::Uwcc || !has_ite_sysfs_leds(sysfs, sys_root) {
        let transport = UniwillHidTransport::new(Arc::new(HidrawDeviceTransport::new()));
        if platform.kind() == PlatformBackendKind::Uwcc {
            let platform_ref = Arc::clone(platform);
            let ec_power_hook: EcLightbarPowerHook =
                Arc::new(
                    move |enabled: bool| match platform_ref.set_lightbar_power(enabled) {
                        Ok(()) | Err(PlatformError::Unsupported(_)) => Ok(()),
                        Err(err) => Err(LightingError::WriteFailed(err.to_string())),
                    },
                );
            let mut transport = transport.with_ec_lightbar_power_hook(ec_power_hook);
            if platform.has_lid_logo_ec() {
                let logo_platform = Arc::clone(platform);
                let logo_hook: EcLidLogoHook = Arc::new(move |enabled, rgb, brightness| {
                    match logo_platform.set_lid_logo_ec(enabled, rgb.r, rgb.g, rgb.b, brightness) {
                        Ok(()) | Err(PlatformError::Unsupported(_)) => Ok(()),
                        Err(err) => Err(LightingError::WriteFailed(err.to_string())),
                    }
                });
                transport = transport.with_ec_lid_logo_hook(logo_hook);
            }
            Arc::new(transport)
        } else {
            Arc::new(transport)
        }
    } else {
        Arc::new(SysfsLedTransport::default())
    }
}

impl LiveUccdBackend {
    pub(super) fn lock_mutex<'a, T>(
        &self,
        m: &'a std::sync::Mutex<T>,
    ) -> std::sync::MutexGuard<'a, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(super) fn read_rw<'a, T>(
        &self,
        rw: &'a std::sync::RwLock<T>,
    ) -> std::sync::RwLockReadGuard<'a, T> {
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
        if self.write_aborted_by_shutdown() {
            tracing::warn!("update_settings aborted by shutdown");
            return None;
        }
        let mut candidate = self.read_rw(&self.settings).clone();
        let outcome = mutate(&mut candidate)?;
        if let Err(err) = mgr.save(&candidate) {
            tracing::warn!("update_settings: ConfigManager::save failed: {err}");
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

    pub(super) fn profile_has_keyboard(profile: &UccProfile) -> bool {
        profile
            .selected_keyboard_profile
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .is_some_and(|_| true)
            || !profile.keyboard.keyboard_profile_id.trim().is_empty()
    }

    pub(super) fn snapshot_keyboard_rollback(&self) -> Option<KeyboardRollbackSnapshot> {
        if self.is_vendor_unsupported() || ucc_config::is_read_only_preview() {
            return None;
        }
        let hw = {
            let mut ctrl = self.lock_mutex(&self.keyboard_ctrl);
            ctrl.snapshot_hardware_state()
        }?;
        let lid_saved = self.lock_mutex(&self.lid_saved_keyboard_states).clone();
        Some((hw, lid_saved))
    }

    pub(super) fn restore_keyboard_rollback(
        &self,
        snap: &KeyboardRollbackSnapshot,
        start_epoch: Option<u64>,
    ) -> bool {
        let superseded =
            start_epoch.is_some_and(|e| self.profile_commit_epoch.load(Ordering::SeqCst) != e);
        if self.shutdown_wait_expired.load(Ordering::SeqCst) || superseded {
            return false;
        }
        self.kb_write_seq.fetch_add(1, Ordering::SeqCst);
        let ok = {
            let mut ctrl = self.lock_mutex(&self.keyboard_ctrl);
            ctrl.restore_hardware_state(&snap.0).is_ok()
        };
        *self.lock_mutex(&self.lid_saved_keyboard_states) = snap.1.clone();
        self.kb_write_seq.fetch_add(1, Ordering::SeqCst);
        if ok {
            self.lock_mutex(&self.lightbar_ctrl).keyboard_changed();
        }
        ok
    }

    pub(super) fn apply_profile_keyboard(&self, profile: &UccProfile) -> Result<bool, ()> {
        if self.is_vendor_unsupported() || ucc_config::is_read_only_preview() {
            return Ok(false);
        }
        let kb_id = profile
            .selected_keyboard_profile
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(profile.keyboard.keyboard_profile_id.trim());
        if kb_id.is_empty() {
            return Ok(false);
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
            return Ok(false);
        };
        let Some((states, _)) = Self::parse_keyboard_states_value(&val) else {
            return Ok(false);
        };
        let (blank_kbd_on_lid_close, _) = self.effective_lid_lighting_policy_for(profile);
        let blanked_under_lid = self.is_lid_closed() && blank_kbd_on_lid_close;
        let prev_lid_saved = self.lock_mutex(&self.lid_saved_keyboard_states).clone();
        let target_states = if blanked_under_lid {
            Self::blanked_keyboard_states(&states)
        } else {
            states.clone()
        };
        self.kb_write_seq.fetch_add(1, Ordering::SeqCst);
        let hw_res = {
            let mut ctrl = self.lock_mutex(&self.keyboard_ctrl);
            if !ctrl.is_available() {
                let _ = ctrl.init();
            }
            if !ctrl.is_available() {
                return Ok(false);
            }
            ctrl.apply_states(&target_states)
        };
        self.kb_write_seq.fetch_add(1, Ordering::SeqCst);
        if hw_res.is_err() {
            *self.lock_mutex(&self.lid_saved_keyboard_states) = prev_lid_saved;
            return Err(());
        }
        if blanked_under_lid {
            *self.lock_mutex(&self.lid_saved_keyboard_states) = Some(states);
        } else {
            *self.lock_mutex(&self.lid_saved_keyboard_states) = None;
            self.lock_mutex(&self.lightbar_ctrl).keyboard_changed();
        }
        Ok(true)
    }

    pub(super) fn current_lid_source_policy(&self) -> LidSourcePolicy {
        let pd_val = serde_json::from_str::<Value>(&self.get_power_device_settings_json())
            .unwrap_or(Value::Null);
        parse_lid_policy_from_json(&pd_val)
    }

    /// Returns `(turn_off_keyboard_backlight_on_lid_close, turn_off_water_cooler_led_on_lid_close)`
    /// checking per-profile `lid` overrides first and falling back to global `powerDevices.lid`.
    pub(super) fn effective_lid_lighting_policy_for(&self, profile: &UccProfile) -> (bool, bool) {
        let fallback = self.current_lid_source_policy();
        let lid = profile.lid.as_ref();
        (
            lid.and_then(|l| l.turn_off_keyboard_backlight_on_lid_close)
                .unwrap_or(fallback.turn_off_keyboard_backlight_on_lid_close),
            lid.and_then(|l| l.turn_off_water_cooler_led_on_lid_close)
                .unwrap_or(fallback.turn_off_water_cooler_led_on_lid_close),
        )
    }

    pub(super) fn effective_lid_lighting_policy(&self) -> (bool, bool) {
        let active = self.read_rw(&self.active_profile).clone();
        self.effective_lid_lighting_policy_for(&active)
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
    /// Returns `true` when the lid state actually transitioned.
    pub fn on_lid_state_changed(&self, closed: bool) -> bool {
        if self.shutting_down.load(Ordering::SeqCst)
            || ucc_config::is_read_only_preview()
            || self.is_vendor_unsupported()
        {
            self.lid_closed.store(closed, Ordering::SeqCst);
            return false;
        }
        let prev = self.lid_closed.swap(closed, Ordering::SeqCst);
        if prev == closed {
            return false;
        }
        tracing::info!(closed, "Lid state changed");
        let src = self.detect_physical_power_source();
        let ac_online = matches!(src, ProfileState::Ac | ProfileState::TypeC);
        if !closed {
            self.lock_mutex(&self.lid_controller).evaluate_lid_event(
                std::time::Instant::now(),
                ac_online,
                false,
            );
            self.restore_lid_lighting();
            return true;
        }

        let (blank_kbd_on_lid_close, blank_wc_led_on_lid_close) =
            self.effective_lid_lighting_policy();
        if blank_kbd_on_lid_close {
            let mut ctrl = self.lock_mutex(&self.keyboard_ctrl);
            if !ctrl.is_available() {
                let _ = ctrl.init();
            }
            if ctrl.is_available() && ctrl.current_states().is_empty() {
                ctrl.hydrate_from_hardware();
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
                    if let Err(err) = ctrl.apply_states(&dark) {
                        tracing::warn!("Failed to blank keyboard backlight on lid close: {err}");
                    } else {
                        tracing::info!(
                            zones = dark.len(),
                            "Blanked keyboard backlight in RAM on lid close"
                        );
                    }
                }
            }
        }

        if blank_wc_led_on_lid_close && self.is_water_cooler_connected_now() {
            let mut cooler = self.lock_mutex(&self.cooler_controller);
            let current_rgb = cooler.desired_rgb_target();
            if current_rgb != RgbTarget::Off {
                {
                    let mut saved = self.lock_mutex(&self.lid_saved_wc_rgb);
                    if saved.is_none() {
                        *saved = Some(current_rgb);
                    }
                }
                let _ = cooler.turn_off_led();
            }
        }
        true
    }

    /// Returns `true` when `uccd` has a configured lid-close policy (either on the active profile
    /// or in `powerDevices.lid`) and should hold the systemd-logind `handle-lid-switch` inhibitor (`V2-P1-02`).
    pub fn should_inhibit_lid_switch(&self) -> bool {
        let profile_override = {
            let active = self.read_rw(&self.active_profile);
            active
                .display
                .lid_close_action
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .and_then(crate::lid::LidSourceAction::parse)
                .is_some()
        };
        if profile_override {
            return true;
        }
        serde_json::from_str::<Value>(&self.get_power_device_settings_json())
            .ok()
            .and_then(|v| v.get("lid").cloned())
            .is_some_and(|v| !v.is_null())
    }

    /// Evaluates the configured lid-close power action for the current power source (`N01`, `V2-P1-02`).
    /// Prefers `active_profile.display.lid_close_action` when present and non-empty, falling back
    /// to `powerDevices.lid.<source>`.
    pub fn evaluate_lid_close_action(&self) -> crate::lid::LidSourceAction {
        let now = std::time::Instant::now();
        let src = self.detect_physical_power_source();
        let ac_online = matches!(src, ProfileState::Ac | ProfileState::TypeC);
        let mut policy = self.current_lid_source_policy();
        let profile_override = {
            let active = self.read_rw(&self.active_profile);
            active
                .display
                .lid_close_action
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .and_then(crate::lid::LidSourceAction::parse)
        };
        if let Some(action) = profile_override {
            policy.ac = action;
            policy.type_c = action;
            policy.bat = action;
        }
        let mut ctrl = self.lock_mutex(&self.lid_controller);
        let _ = ctrl.evaluate_lid_event(now, ac_online, true);
        ctrl.action_for_source(&policy, src.mode_name(), now, ac_online, true)
    }

    /// Restores keyboard backlight and water cooler LED previously blanked by lid close (`M8.10`).
    pub fn restore_lid_lighting(&self) {
        if self.shutting_down.load(Ordering::SeqCst)
            || ucc_config::is_read_only_preview()
            || self.is_vendor_unsupported()
        {
            return;
        }
        let saved_kbd = self.lock_mutex(&self.lid_saved_keyboard_states).take();
        if let Some(states) = saved_kbd {
            let mut ctrl = self.lock_mutex(&self.keyboard_ctrl);
            if !ctrl.is_available() {
                let _ = ctrl.init();
            }
            if ctrl.is_available() {
                if let Err(err) = ctrl.apply_states(&states) {
                    tracing::warn!("Failed to restore keyboard backlight after lid open: {err}");
                } else {
                    tracing::info!(
                        zones = states.len(),
                        "Restored keyboard backlight from RAM after lid open"
                    );
                }
            }
        }
        let saved_wc = self.lock_mutex(&self.lid_saved_wc_rgb).take();
        if let Some(saved_wc_rgb) = saved_wc {
            let mut cooler = self.lock_mutex(&self.cooler_controller);
            match saved_wc_rgb {
                RgbTarget::Color {
                    red,
                    green,
                    blue,
                    state,
                } => {
                    // Always restore `desired_rgb` even if BLE is currently disconnected so
                    // `CoolerController::on_connected()` replays the saved color on reconnect (`TA-06`).
                    let _ = cooler.set_rgb(red, green, blue, state);
                }
                RgbTarget::Unset => {
                    // `TurnOffLed` was already sent over BLE on lid close, so merely clearing
                    // `desired_rgb` to `Unset` would leave the hardware LED dark. Enqueue an
                    // explicit default static white RGB command so the BLE loop wakes and
                    // restores illumination immediately.
                    let _ = cooler.set_rgb(255, 255, 255, ucc_hardware::RgbState::Static);
                }
                RgbTarget::Off => {}
            }
        }
    }

    pub(super) fn set_keyboard_backlight_states_json_impl(
        &self,
        keyboard_backlight_states_json: &str,
    ) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            tracing::warn!(
                shutting_down = self.shutting_down.load(Ordering::SeqCst),
                hardware_activation_pending =
                    self.hardware_activation_pending.load(Ordering::SeqCst),
                "set_keyboard_backlight_states_json rejected: enter_hardware_op returned None"
            );
            return false;
        };
        if keyboard_backlight_states_json.len() > 1024 * 1024 {
            tracing::warn!(
                len = keyboard_backlight_states_json.len(),
                "set_keyboard_backlight_states_json rejected: payload exceeds 1 MiB"
            );
            return false;
        }
        let Some((states, kb_profile_id)) =
            Self::parse_keyboard_states_payload(keyboard_backlight_states_json)
        else {
            tracing::warn!(
                "set_keyboard_backlight_states_json rejected: parse_keyboard_states_payload failed"
            );
            return false;
        };

        // Write to hardware BEFORE updating persistent settings (`ISSUE-13`).
        if self.write_aborted_by_shutdown() {
            tracing::warn!(
                "set_keyboard_backlight_states_json rejected: write_aborted_by_shutdown"
            );
            return false;
        }
        let prev_kb = self.snapshot_keyboard_rollback();
        let blanked_under_lid = self.is_lid_closed() && self.effective_lid_lighting_policy().0;
        let my_seq = self.kb_write_seq.fetch_add(1, Ordering::SeqCst) + 1;
        {
            let mut ctrl = self.lock_mutex(&self.keyboard_ctrl);
            if !ctrl.is_available() {
                if let Err(err) = ctrl.init() {
                    tracing::warn!(
                        "set_keyboard_backlight_states_json: keyboard init failed: {err}"
                    );
                }
            }
            let write_res = if blanked_under_lid {
                let dark = Self::blanked_keyboard_states(&states);
                ctrl.apply_states(&dark)
            } else {
                ctrl.apply_states(&states)
            };
            drop(ctrl);
            if let Err(err) = write_res {
                tracing::warn!(
                    "set_keyboard_backlight_states_json rejected: ctrl.apply_states failed: {err}"
                );
                if let Some(ref snap) = prev_kb {
                    if self.kb_write_seq.load(Ordering::SeqCst) == my_seq {
                        let _ = self.restore_keyboard_rollback(snap, None);
                    }
                }
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
            tracing::warn!(
                "set_keyboard_backlight_states_json rejected: update_settings returned None"
            );
            if let Some(ref snap) = prev_kb {
                if self.kb_write_seq.load(Ordering::SeqCst) == my_seq {
                    let _ = self.restore_keyboard_rollback(snap, None);
                }
            }
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
        if self.write_aborted_by_shutdown() {
            return false;
        }
        let normalized_json = normalized_cfg.to_json();
        let (cfg_res, actual_json, prev_cfg) = {
            let mut lb = self.lock_mutex(&self.lightbar_ctrl);
            let prev_cfg = lb.settings().clone();
            let res = lb.configure(&val);
            if let Err(ref err) = res {
                if !matches!(err, ucc_lighting::LightingError::Unavailable) {
                    lb.restore_config_preserving_error(prev_cfg.clone());
                }
            }
            self.set_lightbar_animating(lb.is_animating());
            let json = lb.settings_json().to_string();
            (res, json, prev_cfg)
        };
        if let Err(err) = cfg_res {
            if !matches!(err, ucc_lighting::LightingError::Unavailable) {
                return false;
            }
        }
        if self
            .update_settings(|s| {
                s.lightbar = Some(normalized_json.clone());
                Some(())
            })
            .is_none()
        {
            if !self.shutdown_wait_expired.load(Ordering::SeqCst) {
                let persisted_json = self.read_rw(&self.settings).lightbar.clone();
                let mut lb = self.lock_mutex(&self.lightbar_ctrl);
                let rollback_json = persisted_json.unwrap_or_else(|| prev_cfg.to_json());
                let _ = lb.configure(&rollback_json);
                self.set_lightbar_animating(lb.is_animating());
            }
            return false;
        }
        self.emit_signal(DbusSignalEvent::LightbarSettingsChanged { json: actual_json });
        true
    }

    pub fn is_lightbar_animating(&self) -> bool {
        !self.shutting_down.load(Ordering::SeqCst)
            && !ucc_config::is_read_only_preview()
            && self.lightbar_animating.load(Ordering::SeqCst)
    }

    /// Advances an active lightbar animation deterministically on periodic daemon ticks (`M8-REVIEW-18`).
    pub fn tick_lightbar_animation(&self, started_at: &mut Option<std::time::Instant>) {
        if self.shutting_down.load(Ordering::SeqCst) || ucc_config::is_read_only_preview() {
            self.lightbar_animating.store(false, Ordering::SeqCst);
            return;
        }
        let mut lb = self.lock_mutex(&self.lightbar_ctrl);
        if lb.is_animating() {
            lb.tick_now();
            let still = lb.is_animating();
            self.lightbar_animating.store(still, Ordering::SeqCst);
            *started_at = still.then(std::time::Instant::now);
        } else {
            self.lightbar_animating.store(false, Ordering::SeqCst);
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
