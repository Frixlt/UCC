// SPDX-License-Identifier: GPL-3.0-or-later

//! Fan profiles, failsafe fan ticks, water cooler protocol & BlueZ GATT loop (`ISSUE-01`, `ISSUE-02`, `ISSUE-24`).

use super::{DbusSignalEvent, LiveUccdBackend};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tracing::{debug, warn};
use ucc_core::{FanCurve, FanPoint, FanProfile, PumpPoint, TemporaryFanCurves, UccProfile};
use ucc_hardware::cooler::{FanTarget, PumpTarget, PumpVoltage, RgbState};
use ucc_hardware::{
    select_cooler, BluezCoolerClient, BluezGattSession, CoolerMode, FanTickSnapshot,
    MockBleTransport, SavedCoolerIdentity,
};

pub(super) const AQUARIS_CONFIG_PATH: &str = "/etc/ucc/aquaris.json";
const WC_CONNECT_DEBOUNCE: Duration = Duration::from_secs(3);
const WC_DISCONNECT_DEBOUNCE: Duration = Duration::from_secs(10);

pub(super) fn normalize_and_parse_custom_fan_profile(
    id: &str,
    outer_name: &str,
    raw: &Value,
) -> Option<(FanProfile, Value)> {
    let mut obj = raw.as_object()?.clone();
    if id.trim().is_empty() {
        return None;
    }
    if obj
        .get("id")
        .and_then(|v| v.as_str())
        .is_none_or(|s| s.trim().is_empty())
    {
        obj.insert("id".to_string(), Value::String(id.to_string()));
    }
    let resolved_name = obj
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            let trimmed = outer_name.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        })
        .unwrap_or_else(|| id.to_string());
    obj.insert("name".to_string(), Value::String(resolved_name.clone()));

    for (target, alias) in [
        ("tableCPU", "cpu"),
        ("tableGPU", "gpu"),
        ("tableWaterCoolerFan", "waterCoolerFan"),
        ("tablePump", "pump"),
    ] {
        if !obj.contains_key(target) {
            if let Some(v) = obj.get(alias).cloned() {
                obj.insert(target.to_string(), v);
            }
        }
        if let Some(Value::Array(points)) = obj.get_mut(target) {
            for pt in points.iter_mut() {
                if let Value::Object(pt_obj) = pt {
                    for coord in ["temp", "speed"] {
                        if let Some(val) = pt_obj.get(coord) {
                            if val.as_i64().is_none() {
                                if let Some(f) = val.as_f64().filter(|f| f.is_finite()) {
                                    pt_obj.insert(coord.to_string(), Value::from(f.round() as i64));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let normalized_val = Value::Object(obj);
    let mut fp = serde_json::from_value::<FanProfile>(normalized_val.clone()).ok()?;
    if fp.validate().is_err() {
        return None;
    }
    fp.name = resolved_name;
    Some((fp, normalized_val))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ApplyFanProfilesDto {
    #[serde(default, alias = "tableCPU")]
    pub(super) cpu: Option<Vec<FanPoint>>,
    #[serde(default, alias = "tableGPU")]
    pub(super) gpu: Option<Vec<FanPoint>>,
    #[serde(default, alias = "tableWaterCoolerFan")]
    pub(super) water_cooler_fan: Option<Vec<FanPoint>>,
    #[serde(default, alias = "tablePump")]
    pub(super) pump: Option<Vec<PumpPoint>>,
    #[serde(default, alias = "name")]
    pub(super) fan_profile_id: Option<String>,
}

pub(super) fn builtin_fan_profiles() -> Vec<FanProfile> {
    let pts = |pairs: &[(i32, u8)]| -> Vec<FanPoint> {
        pairs.iter().map(|&(t, s)| FanPoint::new(t, s)).collect()
    };
    let pump_pts = |pairs: &[(i32, u8)]| -> Vec<PumpPoint> {
        pairs
            .iter()
            .map(|&(t, s)| PumpPoint { temp: t, speed: s })
            .collect()
    };
    let default_pump = || pump_pts(&[(35, 1), (50, 2), (65, 3), (75, 4)]);

    vec![
        FanProfile {
            id: "fan-silent".to_string(),
            name: "Silent [Built-in]".to_string(),
            table_cpu: pts(&[
                (20, 0),
                (25, 0),
                (30, 0),
                (35, 0),
                (40, 0),
                (45, 0),
                (50, 0),
                (55, 0),
                (60, 0),
                (65, 20),
                (70, 28),
                (75, 40),
                (80, 53),
                (85, 65),
                (90, 83),
                (95, 96),
                (100, 100),
            ]),
            table_gpu: pts(&[
                (20, 0),
                (25, 0),
                (30, 0),
                (35, 0),
                (40, 0),
                (45, 0),
                (50, 0),
                (55, 0),
                (60, 10),
                (65, 24),
                (70, 34),
                (75, 46),
                (80, 58),
                (85, 70),
                (90, 91),
                (95, 95),
                (100, 100),
            ]),
            table_pump: default_pump(),
            table_water_cooler_fan: pts(&[
                (20, 0),
                (25, 0),
                (30, 0),
                (35, 0),
                (40, 0),
                (45, 0),
                (50, 0),
                (55, 0),
                (60, 5),
                (65, 22),
                (70, 31),
                (75, 43),
                (80, 56),
                (85, 68),
                (90, 87),
                (95, 96),
                (100, 100),
            ]),
        },
        FanProfile {
            id: "fan-quiet".to_string(),
            name: "Quiet [Built-in]".to_string(),
            table_cpu: pts(&[
                (20, 0),
                (25, 0),
                (30, 0),
                (35, 0),
                (40, 0),
                (45, 0),
                (50, 0),
                (55, 10),
                (60, 20),
                (65, 24),
                (70, 33),
                (75, 46),
                (80, 55),
                (85, 68),
                (90, 85),
                (95, 96),
                (100, 100),
            ]),
            table_gpu: pts(&[
                (20, 0),
                (25, 0),
                (30, 0),
                (35, 0),
                (40, 0),
                (45, 0),
                (50, 0),
                (55, 10),
                (60, 20),
                (65, 26),
                (70, 35),
                (75, 46),
                (80, 55),
                (85, 68),
                (90, 90),
                (95, 95),
                (100, 100),
            ]),
            table_pump: default_pump(),
            table_water_cooler_fan: pts(&[
                (20, 0),
                (25, 0),
                (30, 0),
                (35, 0),
                (40, 0),
                (45, 0),
                (50, 0),
                (55, 10),
                (60, 20),
                (65, 25),
                (70, 34),
                (75, 46),
                (80, 55),
                (85, 68),
                (90, 88),
                (95, 96),
                (100, 100),
            ]),
        },
        FanProfile {
            id: "fan-balanced".to_string(),
            name: "Balanced [Built-in]".to_string(),
            table_cpu: pts(&[
                (20, 0),
                (25, 0),
                (30, 0),
                (35, 0),
                (40, 0),
                (45, 0),
                (50, 17),
                (55, 25),
                (60, 31),
                (65, 38),
                (70, 50),
                (75, 55),
                (80, 65),
                (85, 78),
                (90, 88),
                (95, 96),
                (100, 100),
            ]),
            table_gpu: pts(&[
                (20, 0),
                (25, 0),
                (30, 0),
                (35, 0),
                (40, 0),
                (45, 0),
                (50, 17),
                (55, 25),
                (60, 31),
                (65, 38),
                (70, 50),
                (75, 55),
                (80, 65),
                (85, 78),
                (90, 90),
                (95, 95),
                (100, 100),
            ]),
            table_pump: default_pump(),
            table_water_cooler_fan: pts(&[
                (20, 0),
                (25, 0),
                (30, 0),
                (35, 0),
                (40, 0),
                (45, 0),
                (50, 17),
                (55, 25),
                (60, 31),
                (65, 38),
                (70, 50),
                (75, 55),
                (80, 65),
                (85, 78),
                (90, 89),
                (95, 96),
                (100, 100),
            ]),
        },
        FanProfile {
            id: "fan-cool".to_string(),
            name: "Cool [Built-in]".to_string(),
            table_cpu: pts(&[
                (20, 0),
                (25, 0),
                (30, 0),
                (35, 0),
                (40, 3),
                (45, 20),
                (50, 25),
                (55, 29),
                (60, 35),
                (65, 43),
                (70, 50),
                (75, 58),
                (80, 72),
                (85, 85),
                (90, 93),
                (95, 96),
                (100, 100),
            ]),
            table_gpu: pts(&[
                (20, 0),
                (25, 0),
                (30, 0),
                (35, 0),
                (40, 5),
                (45, 26),
                (50, 31),
                (55, 36),
                (60, 41),
                (65, 46),
                (70, 52),
                (75, 62),
                (80, 71),
                (85, 79),
                (90, 97),
                (95, 100),
                (100, 100),
            ]),
            table_pump: default_pump(),
            table_water_cooler_fan: pts(&[
                (20, 0),
                (25, 0),
                (30, 0),
                (35, 0),
                (40, 4),
                (45, 23),
                (50, 28),
                (55, 33),
                (60, 38),
                (65, 45),
                (70, 51),
                (75, 60),
                (80, 72),
                (85, 82),
                (90, 95),
                (95, 98),
                (100, 100),
            ]),
        },
        FanProfile {
            id: "fan-freezy".to_string(),
            name: "Freezy [Built-in]".to_string(),
            table_cpu: pts(&[
                (20, 20),
                (25, 20),
                (30, 21),
                (35, 23),
                (40, 26),
                (45, 30),
                (50, 40),
                (55, 40),
                (60, 45),
                (65, 50),
                (70, 55),
                (75, 60),
                (80, 73),
                (85, 85),
                (90, 91),
                (95, 96),
                (100, 100),
            ]),
            table_gpu: pts(&[
                (20, 25),
                (25, 25),
                (30, 25),
                (35, 25),
                (40, 30),
                (45, 35),
                (50, 40),
                (55, 45),
                (60, 50),
                (65, 60),
                (70, 65),
                (75, 70),
                (80, 75),
                (85, 85),
                (90, 95),
                (95, 98),
                (100, 100),
            ]),
            table_pump: default_pump(),
            table_water_cooler_fan: pts(&[
                (20, 23),
                (25, 23),
                (30, 23),
                (35, 24),
                (40, 28),
                (45, 33),
                (50, 40),
                (55, 43),
                (60, 48),
                (65, 55),
                (70, 60),
                (75, 65),
                (80, 74),
                (85, 85),
                (90, 93),
                (95, 97),
                (100, 100),
            ]),
        },
    ]
}

pub(super) fn default_custom_fan_profiles() -> BTreeMap<String, FanProfile> {
    let mut map = BTreeMap::new();
    for fp in builtin_fan_profiles() {
        map.insert(fp.id.clone(), fp);
    }
    map
}

/// Builds a serializable catalog of built-in fan profiles matching UCC defaults.
pub fn build_default_custom_fan_catalog() -> BTreeMap<String, FanProfile> {
    default_custom_fan_profiles()
}

/// Persisted aquaris.json device identity and last known RGB LED state (`M5.9`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedCoolerConfig {
    pub identity: Option<SavedCoolerIdentity>,
    pub led_rgb: Option<(u8, u8, u8, RgbState)>,
}

fn rgb_state_from_mode(mode_val: i32) -> RgbState {
    match mode_val {
        0 => RgbState::Static,
        1 => RgbState::Breathe,
        2 => RgbState::Colorful,
        3 => RgbState::BreatheColor,
        _ => RgbState::Static,
    }
}

/// Parses device identity and optional saved LED RGB settings from `aquaris.json` (`M5.9`).
pub fn load_saved_cooler_config(path: &Path) -> Option<SavedCoolerConfig> {
    let raw = std::fs::read_to_string(path).ok()?;
    let val: Value = serde_json::from_str(&raw).ok()?;
    let obj = val.as_object()?;
    let address = obj
        .get("address")
        .or_else(|| obj.get("macAddress"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let name = obj
        .get("name")
        .or_else(|| obj.get("deviceName"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let identity = if address.is_none() && name.is_none() {
        None
    } else {
        Some(SavedCoolerIdentity { address, name })
    };

    let parse_channel = |key: &str| -> Option<u8> {
        let v = obj.get(key)?.as_i64()?;
        u8::try_from(v).ok()
    };
    let led_rgb = match (
        parse_channel("red"),
        parse_channel("green"),
        parse_channel("blue"),
        obj.get("ledMode").and_then(|v| v.as_i64()),
    ) {
        (Some(r), Some(g), Some(b), Some(mode)) if mode >= 0 => {
            Some((r, g, b, rgb_state_from_mode(mode as i32)))
        }
        _ => None,
    };

    if identity.is_none() && led_rgb.is_none() {
        None
    } else {
        Some(SavedCoolerConfig { identity, led_rgb })
    }
}

pub fn load_saved_cooler_identity(path: &Path) -> Option<SavedCoolerIdentity> {
    load_saved_cooler_config(path).and_then(|c| c.identity)
}

impl LiveUccdBackend {
    pub(super) fn resolve_named_fan_profile(&self, key: &str) -> Option<FanProfile> {
        let trimmed = key.trim();
        if trimmed.is_empty() {
            return None;
        }
        let lower = trimmed.to_ascii_lowercase();
        let normalized_id = match lower.as_str() {
            "balanced" | "balanced [built-in]" | "fan-balanced" => "fan-balanced",
            "silent" | "silent [built-in]" | "fan-silent" => "fan-silent",
            "quiet" | "quiet [built-in]" | "fan-quiet" => "fan-quiet",
            "cool" | "cool [built-in]" | "fan-cool" => "fan-cool",
            "freezy" | "freezy [built-in]" | "fan-freezy" => "fan-freezy",
            _ => trimmed,
        };
        let builtins = builtin_fan_profiles();
        if let Some(fp) = builtins.iter().find(|fp| {
            fp.id.eq_ignore_ascii_case(normalized_id)
                || fp.name.eq_ignore_ascii_case(trimmed)
                || fp.id.eq_ignore_ascii_case(trimmed)
        }) {
            return Some(fp.clone());
        }
        let custom = self.read_rw(&self.custom_fan_profiles);
        if let Some(fp) = custom.get(trimmed).or_else(|| {
            custom
                .values()
                .find(|fp| fp.name.eq_ignore_ascii_case(trimmed))
        }) {
            return Some(fp.clone());
        }
        drop(custom);
        let settings = self.read_rw(&self.settings);
        let entry = settings.custom_fan_profiles.as_ref().and_then(|m| {
            m.get(trimmed)
                .or_else(|| m.values().find(|e| e.name.eq_ignore_ascii_case(trimmed)))
        })?;
        normalize_and_parse_custom_fan_profile(&entry.id, &entry.name, &entry.json)
            .map(|(fp, _)| fp)
    }

    pub(super) fn resolve_profile_with_fan_preset(&self, p: &UccProfile) -> UccProfile {
        let mut out = p.clone();
        if (out.fan.table_cpu.is_empty()
            || out.fan.table_gpu.is_empty()
            || out.fan.table_water_cooler_fan.is_empty()
            || out.fan.table_pump.is_empty())
            && !out.fan.fan_profile.trim().is_empty()
        {
            if let Some(preset) = self.resolve_named_fan_profile(&out.fan.fan_profile) {
                if out.fan.table_cpu.is_empty() {
                    out.fan.table_cpu = preset.table_cpu;
                }
                if out.fan.table_gpu.is_empty() {
                    out.fan.table_gpu = preset.table_gpu;
                }
                if out.fan.table_water_cooler_fan.is_empty() {
                    out.fan.table_water_cooler_fan = preset.table_water_cooler_fan;
                }
                if out.fan.table_pump.is_empty() {
                    out.fan.table_pump = preset.table_pump;
                }
            }
        }
        out
    }

    pub(super) fn fan_profile_from_ucc_profile(&self, p: &UccProfile) -> FanProfile {
        let resolved = self.resolve_profile_with_fan_preset(p);
        let mut fp = FanProfile::new(&resolved.fan.fan_profile, &resolved.fan.fan_profile);
        fp.table_cpu = resolved.fan.table_cpu;
        fp.table_gpu = resolved.fan.table_gpu;
        fp.table_pump = resolved.fan.table_pump;
        fp.table_water_cooler_fan = resolved.fan.table_water_cooler_fan;
        fp
    }

    pub(super) fn parse_fan_points_json(points_json: &str) -> Option<Vec<FanPoint>> {
        serde_json::from_str::<Vec<FanPoint>>(points_json).ok()
    }

    /// Runs one synchronous fan control tick and records fan snapshots + auto WC target updates.
    pub fn tick_fans_once(&self) -> FanTickSnapshot {
        if self.shutting_down.load(Ordering::SeqCst) {
            return self
                .read_rw(&self.latest_fans)
                .clone()
                .unwrap_or_else(|| FanTickSnapshot {
                    timestamp_ms: super::unix_ms(),
                    temps_c: Vec::new(),
                    speeds_percent: Vec::new(),
                    calculated_speeds: Vec::new(),
                    fallback_auto: true,
                });
        }
        let ts = super::unix_ms();
        if self.is_vendor_unsupported() || ucc_config::is_read_only_preview() {
            let num_fans = self.capabilities.device.number_of_fans;
            let mut temps = Vec::with_capacity(num_fans);
            let mut speeds = Vec::with_capacity(num_fans);
            for i in 0..num_fans {
                temps.push(self.platform.read_fan_temp(i).ok());
                speeds.push(self.platform.read_fan_speed_percent(i).ok());
            }
            let tick = FanTickSnapshot {
                timestamp_ms: ts,
                temps_c: temps,
                calculated_speeds: speeds.clone(),
                speeds_percent: speeds,
                fallback_auto: !self.fan_control_enabled.load(Ordering::SeqCst),
            };
            *self.write_rw(&self.latest_fans) = Some(tick.clone());
            return tick;
        }
        let enabled = self.fan_control_enabled.load(Ordering::SeqCst);
        let mut ctrl = self.lock_mutex(&self.fan_controller);
        let tick = ctrl.tick_cancellable(self.platform.as_ref(), enabled, ts, || {
            self.shutting_down.load(Ordering::SeqCst)
        });
        drop(ctrl);

        if self.shutting_down.load(Ordering::SeqCst) {
            return tick;
        }
        *self.write_rw(&self.latest_fans) = Some(tick.clone());

        if self.water_cooler_enabled.load(Ordering::SeqCst) {
            if let Some(max_temp) = tick.temps_c.iter().flatten().copied().max() {
                let active_prof = self.read_rw(&self.active_profile).clone();
                let fan_profile = self.fan_profile_from_ucc_profile(&active_prof);
                let mut cooler = self.lock_mutex(&self.cooler_controller);
                if cooler.mode() == CoolerMode::Auto {
                    let _ = cooler.apply_profile_temperature(&fan_profile, max_temp);
                }
            }
        }

        if self.sensor_data_collection.load(Ordering::SeqCst) {
            let cpu_duty = tick.speeds_percent.first().copied().flatten();
            let gpu_duty = tick.speeds_percent.get(1).copied().flatten();
            if let Ok(col) = self.telemetry.try_lock() {
                col.record_fan_duties(ts, cpu_duty, gpu_duty);
            }
        }
        tick
    }

    fn mark_wc_manual_targets_and_emit(&self, fan_speed: Option<u8>, pump_voltage: Option<i32>) {
        let updated = {
            let mut active = self.write_rw(&self.active_profile);
            active.fan.auto_control_wc = false;
            if let Some(spd) = fan_speed {
                active.fan.manual_fan_speed = Some(spd);
            }
            if let Some(code) = pump_voltage {
                active.fan.manual_pump_voltage = Some(code);
            }
            active.clone()
        };
        self.emit_profile_changed(&updated);
    }

    fn current_wc_control_temp_c(&self) -> i32 {
        self.read_rw(&self.latest_fans)
            .as_ref()
            .and_then(|tick| tick.temps_c.iter().flatten().copied().max())
            .or_else(|| {
                (0..self.capabilities.device.number_of_fans)
                    .filter_map(|i| self.platform.read_fan_temp(i).ok())
                    .max()
            })
            .unwrap_or(50)
    }

    /// Evaluates active WC curve/manual targets and queues hardware packets deterministically (`M5.9`).
    pub(super) fn apply_active_cooler_targets_locked(
        &self,
        cooler: &mut ucc_hardware::CoolerController,
        profile: &UccProfile,
    ) {
        let temp_c = self.current_wc_control_temp_c();
        let fan_profile = self.fan_profile_from_ucc_profile(profile);
        let _ = cooler.apply_profile_temperature(&fan_profile, temp_c);
        cooler.force_reapply_active_targets();
    }

    /// Re-applies active WC fan, pump, and RGB targets onto the controller queue (`M5.9`).
    pub fn replay_active_cooler_profile(&self) {
        let active = self.read_rw(&self.active_profile).clone();
        let mut cooler = self.lock_mutex(&self.cooler_controller);
        self.apply_active_cooler_targets_locked(&mut cooler, &active);
    }

    /// Loads persisted RGB preferences from `aquaris.json` if no explicit LED command was issued yet (`M5.9`).
    pub fn load_aquaris_saved_state_from(&self, path: &Path) {
        if let Some(cfg) = load_saved_cooler_config(path) {
            if let Some((r, g, b, state)) = cfg.led_rgb {
                self.lock_mutex(&self.cooler_controller)
                    .seed_desired_rgb_if_unset(r, g, b, state);
            }
        }
    }

    pub(super) fn get_water_cooler_fan_speed_impl(&self) -> i32 {
        match self
            .lock_mutex(&self.cooler_controller)
            .desired_fan_target()
        {
            FanTarget::Speed(pct) => i32::from(pct),
            FanTarget::Off | FanTarget::Unset => -1,
        }
    }

    /// Returns exact UCC D-Bus pump discrete codes (`0=11V, 2=7V, 3=8V, 4=Off, -1=Unset`) (`ISSUE-01`).
    pub(super) fn get_water_cooler_pump_level_impl(&self) -> i32 {
        match self
            .lock_mutex(&self.cooler_controller)
            .desired_pump_target()
        {
            PumpTarget::Unset => -1,
            PumpTarget::Voltage(PumpVoltage::V11) => 0,
            PumpTarget::Voltage(PumpVoltage::V12) => 1,
            PumpTarget::Voltage(PumpVoltage::V7) => 2,
            PumpTarget::Voltage(PumpVoltage::V8) => 3,
            PumpTarget::Voltage(PumpVoltage::Off) => 4,
        }
    }

    pub(super) fn set_water_cooler_fan_speed_impl(&self, duty_cycle_percent: i32) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let Ok(pct) = u8::try_from(duty_cycle_percent) else {
            return false;
        };
        let ok = {
            let mut ctrl = self.lock_mutex(&self.cooler_controller);
            ctrl.set_mode(CoolerMode::Manual);
            ctrl.set_fan_speed(pct).is_ok()
        };
        if ok {
            self.mark_wc_manual_targets_and_emit(Some(pct), None);
        }
        ok
    }

    pub(super) fn set_water_cooler_pump_voltage_impl(&self, voltage: i32) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let Ok(core_v) = ucc_core::PumpVoltage::from_discrete_code(voltage) else {
            return false;
        };
        let hw_v = match core_v {
            ucc_core::PumpVoltage::V11 => PumpVoltage::V11,
            ucc_core::PumpVoltage::V12 => PumpVoltage::V12,
            ucc_core::PumpVoltage::V7 => PumpVoltage::V7,
            ucc_core::PumpVoltage::V8 => PumpVoltage::V8,
            ucc_core::PumpVoltage::Off => PumpVoltage::Off,
        };
        let ok = {
            let mut ctrl = self.lock_mutex(&self.cooler_controller);
            ctrl.set_mode(CoolerMode::Manual);
            ctrl.set_pump_voltage(hw_v).is_ok()
        };
        if ok {
            self.mark_wc_manual_targets_and_emit(None, Some(voltage));
        }
        ok
    }

    pub(super) fn set_water_cooler_led_color_impl(
        &self,
        red: i32,
        green: i32,
        blue: i32,
        mode_val: i32,
    ) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let (Ok(r), Ok(g), Ok(b)) = (u8::try_from(red), u8::try_from(green), u8::try_from(blue))
        else {
            return false;
        };
        let state = rgb_state_from_mode(mode_val);
        if self.is_lid_closed()
            && self
                .current_lid_source_policy()
                .turn_off_water_cooler_led_on_lid_close
            && self.is_water_cooler_connected_now()
        {
            *self.lock_mutex(&self.lid_saved_wc_rgb) = Some(ucc_hardware::RgbTarget::Color {
                red: r,
                green: g,
                blue: b,
                state,
            });
            return self
                .lock_mutex(&self.cooler_controller)
                .turn_off_led()
                .is_ok();
        }
        *self.lock_mutex(&self.lid_saved_wc_rgb) = None;
        self.lock_mutex(&self.cooler_controller)
            .set_rgb(r, g, b, state)
            .is_ok()
    }

    pub(super) fn turn_off_water_cooler_led_impl(&self) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        *self.lock_mutex(&self.lid_saved_wc_rgb) = None;
        self.lock_mutex(&self.cooler_controller)
            .turn_off_led()
            .is_ok()
    }

    pub(super) fn turn_off_water_cooler_fan_impl(&self) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let ok = {
            let mut ctrl = self.lock_mutex(&self.cooler_controller);
            ctrl.set_mode(CoolerMode::Manual);
            ctrl.turn_off_fan().is_ok()
        };
        if ok {
            self.mark_wc_manual_targets_and_emit(Some(0), None);
        }
        ok
    }

    pub(super) fn turn_off_water_cooler_pump_impl(&self) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let ok = {
            let mut ctrl = self.lock_mutex(&self.cooler_controller);
            ctrl.set_mode(CoolerMode::Manual);
            ctrl.turn_off_pump().is_ok()
        };
        if ok {
            self.mark_wc_manual_targets_and_emit(None, Some(4));
        }
        ok
    }

    pub(super) fn set_water_cooler_auto_control_impl(&self, enabled: bool) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let updated = {
            let mut active = self.write_rw(&self.active_profile);
            active.fan.auto_control_wc = enabled;
            active.clone()
        };
        let changed = {
            let mut ctrl = self.lock_mutex(&self.cooler_controller);
            let next_mode = if enabled {
                CoolerMode::Auto
            } else {
                CoolerMode::Manual
            };
            let ch = ctrl.mode() != next_mode;
            ctrl.set_mode(next_mode);
            self.apply_active_cooler_targets_locked(&mut ctrl, &updated);
            ch
        };
        self.emit_profile_changed(&updated);
        if changed {
            self.emit_signal(DbusSignalEvent::WaterCoolerStatusChanged {
                status: if enabled { "auto" } else { "manual" }.to_string(),
            });
        }
        true
    }

    pub(super) fn set_fan_profile_cpu_impl(&self, points_json: &str) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let Some(points) = Self::parse_fan_points_json(points_json) else {
            return false;
        };
        if FanCurve::new(points.clone())
            .and_then(|c| c.validate())
            .is_err()
        {
            return false;
        }
        let mut temp_curves = self.lock_mutex(&self.temporary_fan_curves);
        let fallback_gpu = self.read_rw(&self.active_profile).fan.table_gpu.clone();
        let existing = temp_curves.take().unwrap_or_else(|| TemporaryFanCurves {
            gpu: fallback_gpu,
            ..Default::default()
        });
        let curves = TemporaryFanCurves {
            cpu: points,
            gpu: existing.gpu,
            water_cooler_fan: existing.water_cooler_fan,
            pump: existing.pump,
        };
        *temp_curves = Some(curves.clone());
        drop(temp_curves);
        self.lock_mutex(&self.fan_controller)
            .apply_temporary_curves(curves);
        true
    }

    pub(super) fn set_fan_profile_dgpu_impl(&self, points_json: &str) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let Some(points) = Self::parse_fan_points_json(points_json) else {
            return false;
        };
        if FanCurve::new(points.clone())
            .and_then(|c| c.validate())
            .is_err()
        {
            return false;
        }
        let mut temp_curves = self.lock_mutex(&self.temporary_fan_curves);
        let fallback_cpu = self.read_rw(&self.active_profile).fan.table_cpu.clone();
        let existing = temp_curves.take().unwrap_or_else(|| TemporaryFanCurves {
            cpu: fallback_cpu,
            ..Default::default()
        });
        let curves = TemporaryFanCurves {
            cpu: existing.cpu,
            gpu: points,
            water_cooler_fan: existing.water_cooler_fan,
            pump: existing.pump,
        };
        *temp_curves = Some(curves.clone());
        drop(temp_curves);
        self.lock_mutex(&self.fan_controller)
            .apply_temporary_curves(curves);
        true
    }

    pub(super) fn apply_fan_profiles_impl(&self, fan_profiles_json: &str) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let Ok(dto) = serde_json::from_str::<ApplyFanProfilesDto>(fan_profiles_json) else {
            return false;
        };

        let mut any_curve = false;
        for pts in [&dto.cpu, &dto.gpu, &dto.water_cooler_fan]
            .into_iter()
            .flatten()
        {
            if !pts.is_empty() {
                if FanCurve::new(pts.clone())
                    .and_then(|c| c.validate())
                    .is_err()
                {
                    return false;
                }
                any_curve = true;
            }
        }
        if let Some(ref pts) = dto.pump {
            if !pts.is_empty() {
                for pt in pts {
                    if pt.validate().is_err() {
                        return false;
                    }
                }
                any_curve = true;
            }
        }

        let fan_profile_id = dto
            .fan_profile_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        if !any_curve && fan_profile_id.is_none() {
            return false;
        }

        let curves = if any_curve {
            let active_fan = self.read_rw(&self.active_profile).fan.clone();
            TemporaryFanCurves {
                cpu: dto.cpu.unwrap_or(active_fan.table_cpu),
                gpu: dto.gpu.unwrap_or(active_fan.table_gpu),
                water_cooler_fan: dto
                    .water_cooler_fan
                    .unwrap_or(active_fan.table_water_cooler_fan),
                pump: dto.pump.unwrap_or(active_fan.table_pump),
            }
        } else if let Some(ref fp_id) = fan_profile_id {
            let Some(fp) = self.resolve_named_fan_profile(fp_id) else {
                return false;
            };
            TemporaryFanCurves {
                cpu: fp.table_cpu,
                gpu: fp.table_gpu,
                water_cooler_fan: fp.table_water_cooler_fan,
                pump: fp.table_pump,
            }
        } else {
            return false;
        };

        *self.lock_mutex(&self.temporary_fan_curves) = Some(curves.clone());
        self.lock_mutex(&self.fan_controller)
            .apply_temporary_curves(curves);

        if let Some(fp_id) = fan_profile_id {
            let updated = {
                let mut active = self.write_rw(&self.active_profile);
                active.fan.fan_profile = fp_id;
                active.clone()
            };
            self.emit_profile_changed(&updated);
        }

        let _ = self.tick_fans_once();
        true
    }

    pub(super) fn revert_fan_profiles_impl(&self) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        *self.lock_mutex(&self.temporary_fan_curves) = None;
        self.lock_mutex(&self.fan_controller)
            .clear_temporary_curves();
        true
    }

    pub(super) fn save_custom_fan_profile_impl(
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
        if id.trim().is_empty() || json_str.trim().is_empty() {
            return false;
        }
        let Ok(parsed_val) = serde_json::from_str::<Value>(json_str) else {
            return false;
        };
        let Some((fp, normalized_val)) =
            normalize_and_parse_custom_fan_profile(id, name, &parsed_val)
        else {
            return false;
        };

        if self
            .update_settings(|s| {
                s.custom_fan_profiles
                    .get_or_insert_with(HashMap::new)
                    .insert(
                        id.to_string(),
                        ucc_config::CustomFanProfile {
                            id: id.to_string(),
                            name: fp.name.clone(),
                            json: normalized_val,
                        },
                    );
                Some(())
            })
            .is_none()
        {
            return false;
        }
        self.write_rw(&self.custom_fan_profiles)
            .insert(id.to_string(), fp);
        true
    }

    pub(super) fn delete_custom_fan_profile_impl(&self, id: &str) -> bool {
        if ucc_config::is_read_only_preview() || self.shutting_down.load(Ordering::SeqCst) {
            return false;
        }
        if self
            .update_settings(|s| {
                s.custom_fan_profiles
                    .as_mut()
                    .and_then(|m| m.remove(id))
                    .map(|_| ())
            })
            .is_none()
        {
            return false;
        }
        self.write_rw(&self.custom_fan_profiles).remove(id);
        true
    }

    pub fn set_cooler_connected_state(&self, connected: bool) {
        self.water_cooler_connected
            .store(connected, Ordering::SeqCst);
        let mut cooler = self.lock_mutex(&self.cooler_controller);
        if connected {
            cooler.on_connected();
        } else {
            cooler.on_disconnected();
        }
    }

    pub fn cooler_desired_rgb(&self) -> ucc_hardware::RgbTarget {
        self.lock_mutex(&self.cooler_controller)
            .desired_rgb_target()
    }

    pub fn drain_cooler_commands(&self) -> Vec<[u8; 8]> {
        let sink = MockBleTransport::new(true);
        let mut cooler = self.lock_mutex(&self.cooler_controller);
        let _ = cooler.flush_ready(&sink, Instant::now());
        sink.packets()
    }
}

async fn sync_wc_power_and_targets(backend: &Arc<LiveUccdBackend>, replay_targets: bool) {
    let b = Arc::clone(backend);
    let _ = tokio::task::spawn_blocking(move || {
        if let Some(_g) = b.enter_worker_hw_op() {
            b.reconcile_power_state(false);
            if replay_targets {
                b.replay_active_cooler_profile();
            }
        }
    })
    .await;
}

/// Asynchronous BlueZ BLE GATT worker loop for external liquid coolers (`ISSUE-02`).
pub async fn run_water_cooler_ble_loop(
    connection: zbus::Connection,
    backend: Arc<LiveUccdBackend>,
    mut stop_rx: watch::Receiver<bool>,
) {
    let client = BluezCoolerClient::new_system(connection);
    let mut active_session: Option<BluezGattSession> = None;
    let mut connect_since: Option<Instant> = None;
    let mut disconnect_since: Option<Instant> = None;

    loop {
        if *stop_rx.borrow() || backend.shutting_down.load(Ordering::SeqCst) {
            break;
        }
        if backend.cooler_suspending.load(Ordering::SeqCst) {
            if let Some(sess) = active_session.take() {
                let _ = sess.disconnect().await;
            }
            connect_since = None;
            disconnect_since = None;
            tokio::select! {
                biased;
                _ = stop_rx.changed() => break,
                _ = tokio::time::sleep(Duration::from_millis(500)) => {}
            }
            continue;
        }
        let wc_enabled = backend.water_cooler_enabled.load(Ordering::SeqCst);
        if !wc_enabled {
            if let Some(sess) = active_session.take() {
                let _ = tokio::time::timeout(Duration::from_secs(3), sess.disconnect()).await;
            }
            connect_since = None;
            disconnect_since = None;
            backend
                .lock_mutex(&backend.cooler_controller)
                .on_disconnected();
            if backend.water_cooler_connected.swap(false, Ordering::SeqCst) {
                backend.emit_signal(DbusSignalEvent::WaterCoolerStatusChanged {
                    status: "disconnected".to_string(),
                });
                sync_wc_power_and_targets(&backend, false).await;
            }
        } else if active_session.is_none() {
            let saved = load_saved_cooler_identity(Path::new(AQUARIS_CONFIG_PATH));
            match client.discover_coolers().await {
                Ok(devices) => {
                    if let Some(chosen) = select_cooler(&devices, saved.as_ref()) {
                        match client.connect_cooler(chosen).await {
                            Ok(sess) => {
                                active_session = Some(sess);
                                disconnect_since = None;
                                connect_since.get_or_insert_with(Instant::now);
                                backend
                                    .lock_mutex(&backend.cooler_controller)
                                    .on_connected();
                                debug!("Connected to BLE water cooler {}", chosen.name);
                            }
                            Err(e) => warn!("BLE water cooler connect failed: {e}"),
                        }
                    } else {
                        connect_since = None;
                    }
                }
                Err(_) => {
                    connect_since = None;
                }
            }
        } else if let Some(ref sess) = active_session {
            if let Some(since) = connect_since {
                if since.elapsed() >= WC_CONNECT_DEBOUNCE
                    && !backend.water_cooler_connected.swap(true, Ordering::SeqCst)
                {
                    backend.emit_signal(DbusSignalEvent::WaterCoolerStatusChanged {
                        status: "connected".to_string(),
                    });
                    sync_wc_power_and_targets(&backend, true).await;
                }
            }

            let packets = backend.drain_cooler_commands();
            let mut failed = false;
            for pkt in packets {
                if sess.write_packet(pkt).await.is_err() {
                    failed = true;
                    break;
                }
            }
            if failed {
                active_session = None;
                connect_since = None;
                disconnect_since.get_or_insert_with(Instant::now);
                backend
                    .lock_mutex(&backend.cooler_controller)
                    .on_disconnected();
            } else {
                disconnect_since = None;
            }
        }

        if let Some(since) = disconnect_since {
            if since.elapsed() >= WC_DISCONNECT_DEBOUNCE
                && backend.water_cooler_connected.swap(false, Ordering::SeqCst)
            {
                backend.emit_signal(DbusSignalEvent::WaterCoolerStatusChanged {
                    status: "disconnected".to_string(),
                });
                sync_wc_power_and_targets(&backend, false).await;
            }
        }

        tokio::select! {
            biased;
            _ = stop_rx.changed() => break,
            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
        }
    }

    if let Some(sess) = active_session {
        let _ = sess.disconnect().await;
    }
}
