// SPDX-License-Identifier: GPL-3.0-or-later

//! Lid-switch policy (`M8.9`) and wakeup sysfs/procfs enforcement (`LidController` / `WakeupPolicy`).

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};
use ucc_hardware::Sysfs;

/// Summary of wakeup sysfs/procfs policy updates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WakeupApplyResult {
    pub enabled: usize,
    pub disabled: usize,
    pub failed: usize,
}

/// Default grace period after AC unplug before closing the lid triggers suspend.
pub const DEFAULT_AC_UNPLUG_DEBOUNCE: Duration = Duration::from_secs(3);

/// Decision produced by [`LidController`] on a lid/AC transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LidAction {
    Ignore,
    Suspend,
}

/// Configuration for [`LidController`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LidPolicyConfig {
    pub ignore_lid_on_ac: bool,
    pub ac_unplug_debounce: Duration,
}

impl Default for LidPolicyConfig {
    fn default() -> Self {
        Self {
            ignore_lid_on_ac: false,
            ac_unplug_debounce: DEFAULT_AC_UNPLUG_DEBOUNCE,
        }
    }
}

/// Per-source lid action matching C++ `LidController` (`screen_off`, `ignore`, `suspend`,
/// `hibernate`, `poweroff`, `lock`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LidSourceAction {
    ScreenOff,
    Ignore,
    #[default]
    Suspend,
    Hibernate,
    PowerOff,
    Lock,
}

impl LidSourceAction {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "screen_off" => Some(Self::ScreenOff),
            "ignore" => Some(Self::Ignore),
            "suspend" => Some(Self::Suspend),
            "hibernate" => Some(Self::Hibernate),
            "poweroff" => Some(Self::PowerOff),
            "lock" => Some(Self::Lock),
            _ => None,
        }
    }

    pub fn requires_inhibitor(self) -> bool {
        self != Self::Suspend
    }
}

/// Multi-source lid switch policy parsed from `powerDevices["lid"]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LidSourcePolicy {
    pub ac: LidSourceAction,
    pub type_c: LidSourceAction,
    pub bat: LidSourceAction,
    pub wake_only_lid_and_keyboard: bool,
    pub turn_off_keyboard_backlight_on_lid_close: bool,
    pub turn_off_water_cooler_led_on_lid_close: bool,
}

impl Default for LidSourcePolicy {
    fn default() -> Self {
        Self {
            ac: LidSourceAction::Ignore,
            type_c: LidSourceAction::Ignore,
            bat: LidSourceAction::Suspend,
            wake_only_lid_and_keyboard: true,
            turn_off_keyboard_backlight_on_lid_close: true,
            turn_off_water_cooler_led_on_lid_close: false,
        }
    }
}

/// Parses `powerDevices["lid"]` into [`LidSourcePolicy`].
pub fn parse_lid_policy_from_json(power_devices: &serde_json::Value) -> LidSourcePolicy {
    let mut policy = LidSourcePolicy::default();
    let Some(lid) = power_devices.get("lid").and_then(|v| v.as_object()) else {
        return policy;
    };
    if let Some(act) = lid
        .get("ac")
        .and_then(|v| v.as_str())
        .and_then(LidSourceAction::parse)
    {
        policy.ac = act;
    }
    if let Some(act) = lid
        .get("type_c")
        .and_then(|v| v.as_str())
        .and_then(LidSourceAction::parse)
    {
        policy.type_c = act;
    }
    if let Some(act) = lid
        .get("bat")
        .and_then(|v| v.as_str())
        .and_then(LidSourceAction::parse)
    {
        policy.bat = act;
    }
    if let Some(w) = lid.get("wakeOnlyLidAndKeyboard").and_then(|v| v.as_bool()) {
        policy.wake_only_lid_and_keyboard = w;
    }
    if let Some(v) = lid
        .get("turnOffKeyboardBacklightOnLidClose")
        .and_then(|v| v.as_bool())
    {
        policy.turn_off_keyboard_backlight_on_lid_close = v;
    }
    if let Some(v) = lid
        .get("turnOffWaterCoolerLedOnLidClose")
        .and_then(|v| v.as_bool())
    {
        policy.turn_off_water_cooler_led_on_lid_close = v;
    }
    policy
}

/// Coordinates systemd-logind `handle-lid-switch` inhibitor state and AC-unplug debounce (`M8.9`).
#[derive(Debug, Clone)]
pub struct LidController {
    config: LidPolicyConfig,
    ac_online: bool,
    lid_closed: bool,
    last_ac_unplug: Option<Instant>,
}

impl LidController {
    pub fn new(config: LidPolicyConfig) -> Self {
        Self {
            config,
            ac_online: false,
            lid_closed: false,
            last_ac_unplug: None,
        }
    }

    pub fn config(&self) -> LidPolicyConfig {
        self.config
    }

    pub fn set_config(&mut self, config: LidPolicyConfig) {
        self.config = config;
    }

    /// Evaluates per-source action honoring the 3s AC-unplug debounce window.
    pub fn action_for_source(
        &self,
        policy: &LidSourcePolicy,
        mode: &str,
        now: Instant,
        ac_online: bool,
        lid_closed: bool,
    ) -> LidSourceAction {
        if !lid_closed {
            return LidSourceAction::Ignore;
        }
        let configured = match mode {
            "ac" => policy.ac,
            "type_c" => policy.type_c,
            _ => policy.bat,
        };
        if !ac_online
            && configured == LidSourceAction::Suspend
            && self.last_ac_unplug.is_some_and(|at| {
                now.saturating_duration_since(at) < self.config.ac_unplug_debounce
            })
        {
            LidSourceAction::Ignore
        } else {
            configured
        }
    }

    /// Evaluates an updated `(ac_online, lid_closed)` state and returns whether suspend should occur.
    pub fn evaluate_lid_event(
        &mut self,
        now: Instant,
        ac_online: bool,
        lid_closed: bool,
    ) -> LidAction {
        if self.ac_online && !ac_online {
            self.last_ac_unplug = Some(now);
        } else if ac_online {
            self.last_ac_unplug = None;
        }
        self.ac_online = ac_online;
        self.lid_closed = lid_closed;

        if !lid_closed {
            return LidAction::Ignore;
        }
        if self.should_hold_inhibitor(now) {
            LidAction::Ignore
        } else {
            LidAction::Suspend
        }
    }

    /// Returns `true` when `handle-lid-switch` inhibitor should be held by `uccd`.
    pub fn should_hold_inhibitor(&self, now: Instant) -> bool {
        if !self.config.ignore_lid_on_ac {
            return false;
        }
        if self.ac_online {
            return true;
        }
        self.last_ac_unplug.is_some_and(|unplugged_at| {
            now.saturating_duration_since(unplugged_at) < self.config.ac_unplug_debounce
        })
    }
}

/// Strict allowlisted ACPI/PCI/USB wakeup sysfs/procfs policy ported from `uccd/inc/WakeupPolicy.hpp` (`M8.9`).
#[derive(Debug, Clone)]
pub struct WakeupPolicy {
    sys_dir: PathBuf,
    proc_dir: PathBuf,
}

impl Default for WakeupPolicy {
    fn default() -> Self {
        Self::new("/")
    }
}

impl WakeupPolicy {
    /// Creates a policy rooted at filesystem root (e.g. `"/"` or a test tempdir root containing `sys/` and `proc/`).
    pub fn new(root: impl AsRef<Path>) -> Self {
        let r = root.as_ref();
        Self {
            sys_dir: r.join("sys"),
            proc_dir: r.join("proc"),
        }
    }

    /// Creates a policy with explicit `/sys` and `/proc` directories.
    pub fn with_sys_proc(sys_dir: impl AsRef<Path>, proc_dir: impl AsRef<Path>) -> Self {
        Self {
            sys_dir: sys_dir.as_ref().to_path_buf(),
            proc_dir: proc_dir.as_ref().to_path_buf(),
        }
    }

    /// Validates and resolves a wakeup sysfs node strictly inside `<sys_dir>/devices/.../power/wakeup`
    /// or `<proc_dir>/acpi/wakeup`, rejecting any `..` traversal or unexpected node names.
    pub fn resolve_wakeup_path(&self, raw_path: &str) -> Result<PathBuf, &'static str> {
        let p = Path::new(raw_path);
        if p.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err("Path traversal rejected in wakeup path");
        }
        let rel = p.strip_prefix("/").unwrap_or(p);
        let rel_str = rel.to_str().ok_or("Invalid UTF-8 in wakeup path")?;
        if rel_str.starts_with("sys/devices/") && rel_str.ends_with("/power/wakeup") {
            let after_sys = rel_str.strip_prefix("sys/").unwrap_or(rel_str);
            return Ok(self.sys_dir.join(after_sys));
        }
        if rel_str == "proc/acpi/wakeup" {
            return Ok(self.proc_dir.join("acpi/wakeup"));
        }
        Err("Wakeup path outside allowlisted sysfs/procfs nodes")
    }

    /// Applies `"enabled"` or `"disabled"` to an allowlisted wakeup control node via [`Sysfs`].
    pub fn set_device_wakeup(
        &self,
        sysfs: &dyn Sysfs,
        raw_path: &str,
        enabled: bool,
    ) -> Result<(), String> {
        let target = self
            .resolve_wakeup_path(raw_path)
            .map_err(|e| e.to_string())?;
        let val = if enabled { "enabled" } else { "disabled" };
        sysfs
            .write(&target, val)
            .map_err(|e| format!("Failed writing wakeup node {}: {e}", target.display()))
    }

    /// Applies the lid+keyboard-only wakeup policy across `<sys_dir>/devices` and `<proc_dir>/acpi/wakeup`.
    pub fn apply(&self, wake_only_lid_and_keyboard: bool, sysfs: &dyn Sysfs) -> WakeupApplyResult {
        apply_wakeup_policy(
            wake_only_lid_and_keyboard,
            &self.sys_dir,
            &self.proc_dir,
            sysfs,
        )
    }
}

/// Checks whether `device_dir` (relative to `devices_root`, e.g. `/sys/devices`) is allowed to wake
/// the system under the lid+keyboard policy (matches `WakeupPolicy::isAllowedWakeupDevice`).
pub fn is_allowed_wakeup_device(device_dir: &Path, devices_root: &Path, sysfs: &dyn Sysfs) -> bool {
    if device_dir
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return false;
    }
    let Ok(rel) = device_dir.strip_prefix(devices_root) else {
        return false;
    };
    let rel_str = rel.to_string_lossy();
    let rel_str = rel_str.trim_start_matches('/');
    if rel_str.is_empty() {
        return false;
    }

    // 1. ACPI Lid Switch: platform/PNP0C0D:<id>
    if let Some(lid_suffix) = rel_str.strip_prefix("platform/PNP0C0D:") {
        if !lid_suffix.is_empty() && !lid_suffix.contains('/') {
            return true;
        }
    }

    // 2. Internal PS/2 keyboard on i8042: platform/i8042/serio<N> with "KBD" in description
    if let Some(serio_suffix) = rel_str.strip_prefix("platform/i8042/serio") {
        if !serio_suffix.is_empty() && !serio_suffix.contains('/') {
            if let Ok(desc) = sysfs.read(&device_dir.join("description")) {
                if desc.contains("KBD") {
                    return true;
                }
            }
        }
    }

    false
}

/// Applies the wakeup policy across `sys_dir/devices` (`.../power/wakeup`) and `proc_dir/acpi/wakeup`.
pub fn apply_wakeup_policy(
    wake_only_lid_and_keyboard: bool,
    sys_dir: &Path,
    proc_dir: &Path,
    sysfs: &dyn Sysfs,
) -> WakeupApplyResult {
    let mut result = WakeupApplyResult::default();
    if !wake_only_lid_and_keyboard {
        return result;
    }

    let devices_root = sys_dir.join("devices");
    let mut stack: Vec<(PathBuf, usize)> = vec![(devices_root.clone(), 0)];
    let mut visited: HashSet<PathBuf> = HashSet::new();

    while let Some((dir, depth)) = stack.pop() {
        if !visited.insert(dir.clone()) {
            continue;
        }

        let pw_path = dir.join("power/wakeup");
        if let Ok(cur_raw) = sysfs.read(&pw_path) {
            let cur = cur_raw.trim();
            if is_allowed_wakeup_device(&dir, &devices_root, sysfs) {
                if cur == "disabled" {
                    if sysfs.write(&pw_path, "enabled\n").is_ok() {
                        result.enabled += 1;
                    } else {
                        result.failed += 1;
                    }
                }
            } else if cur == "enabled" {
                if sysfs.write(&pw_path, "disabled\n").is_ok() {
                    result.disabled += 1;
                } else {
                    result.failed += 1;
                }
            }
        }

        if depth >= 16 {
            continue;
        }

        if let Ok(mut children) = sysfs.read_dir(&dir) {
            children.sort();
            for child in children {
                let Some(fname) = child.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if matches!(fname, "power" | "subsystem" | "driver" | "firmware_node") {
                    continue;
                }
                stack.push((child, depth + 1));
            }
        }
    }

    let acpi_wakeup = proc_dir.join("acpi/wakeup");
    if let Ok(content) = sysfs.read(&acpi_wakeup) {
        let mut toggled = HashSet::new();
        for (idx, line) in content.lines().enumerate() {
            if idx == 0 && line.trim_start().starts_with("Device") {
                continue;
            }
            let mut parts = line.split_whitespace();
            let Some(dev) = parts.next() else {
                continue;
            };
            let Some(_sstate) = parts.next() else {
                continue;
            };
            let Some(status) = parts.next() else {
                continue;
            };
            if dev == "Device" || !status.starts_with("*enabled") {
                continue;
            }
            if !toggled.insert(dev.to_string()) {
                continue;
            }
            if sysfs.write(&acpi_wakeup, &format!("{dev}\n")).is_ok() {
                result.disabled += 1;
            } else {
                result.failed += 1;
            }
        }
    }

    result
}
