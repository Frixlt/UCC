// SPDX-License-Identifier: GPL-3.0-or-later

//! Display backlight/DRM/PRIME/charging, GPU PCI discovery, cached SystemInfo, and NVIDIA cTGP (`ISSUE-08`, `ISSUE-19`, `ISSUE-21`, `ISSUE-23`, `ISSUE-24`).

use super::{LiveUccdBackend};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use ucc_core::UccProfile;
use ucc_hardware::{classify_manufacturer, dgpu_to_legacy_json, igpu_to_legacy_json};

const BACKLIGHT_RAMP_MAX_WAIT_MS: usize = 250;
const BACKLIGHT_RAMP_POLL_STEP: Duration = Duration::from_millis(5);
const PRIME_SELECT_TIMEOUT: Duration = Duration::from_secs(2);

impl LiveUccdBackend {
    pub(super) fn read_dmi_field(&self, field: &str) -> String {
        self.sysfs
            .read(&self.sys_root.join("sys/class/dmi/id").join(field))
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    }

    pub(super) fn read_cpu_model(&self) -> String {
        let raw = self
            .sysfs
            .read(&self.sys_root.join("proc/cpuinfo"))
            .or_else(|_| std::fs::read_to_string("/proc/cpuinfo"));
        let Ok(raw) = raw else {
            return String::new();
        };
        for line in raw.lines() {
            if let Some(rest) = line.strip_prefix("model name") {
                if let Some((_, val)) = rest.split_once(':') {
                    return val.trim().to_string();
                }
            }
        }
        String::new()
    }

    pub(super) fn parse_pci_hex(raw: &str) -> Option<u32> {
        let trimmed = raw.trim();
        let hex = trimmed
            .strip_prefix("0x")
            .or_else(|| trimmed.strip_prefix("0X"))
            .unwrap_or(trimmed);
        u32::from_str_radix(hex, 16).ok()
    }

    pub(super) fn decode_pci_name(raw: &str, vendor: u32) -> String {
        let base = if let Some(start) = raw.find('[') {
            if let Some(end_rel) = raw[start + 1..].find(']') {
                let bracketed = raw[start + 1..start + 1 + end_rel].trim();
                if !bracketed.is_empty() {
                    bracketed
                } else {
                    raw.trim()
                }
            } else {
                raw.trim()
            }
        } else {
            raw.trim()
        };
        let lower = base.to_ascii_lowercase();
        match vendor {
            0x10de if !lower.contains("nvidia") => format!("NVIDIA {base}"),
            0x1002 if !lower.contains("amd") => format!("AMD {base}"),
            0x8086 if !lower.contains("intel") => format!("Intel {base}"),
            _ => base.to_string(),
        }
        .trim()
        .to_string()
    }

    pub(super) fn lookup_pci_ids_in_text(text: &str, vendor: u32, device: u32) -> Option<String> {
        let target_vendor = format!("{vendor:04x}");
        let target_device = format!("{device:04x}");
        let mut in_vendor = false;
        for line in text.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if !line.starts_with('\t') {
                if in_vendor {
                    break;
                }
                let mut parts = line.splitn(2, char::is_whitespace);
                let vid = parts.next().unwrap_or_default().trim();
                if vid.eq_ignore_ascii_case(&target_vendor) {
                    in_vendor = true;
                }
            } else if in_vendor && !line.starts_with("\t\t") {
                let stripped = line.trim_start_matches('\t');
                let mut parts = stripped.splitn(2, char::is_whitespace);
                let did = parts.next().unwrap_or_default().trim();
                if did.eq_ignore_ascii_case(&target_device) {
                    if let Some(name) = parts.next() {
                        let trimmed = name.trim();
                        if !trimmed.is_empty() {
                            return Some(trimmed.to_string());
                        }
                    }
                }
            }
        }
        None
    }

    pub(super) fn lookup_pci_ids(&self, vendor: u32, device: u32) -> Option<String> {
        for rel in [
            "usr/share/hwdata/pci.ids",
            "usr/share/misc/pci.ids",
            "usr/share/pci.ids",
        ] {
            let candidate = self.sys_root.join(rel);
            if let Ok(content) = self.sysfs.read(&candidate) {
                if let Some(found) = Self::lookup_pci_ids_in_text(&content, vendor, device) {
                    return Some(found);
                }
            }
        }
        if self.sys_root != Path::new("/") {
            for host in ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids"] {
                if let Ok(content) = std::fs::read_to_string(host) {
                    if let Some(found) = Self::lookup_pci_ids_in_text(&content, vendor, device) {
                        return Some(found);
                    }
                }
            }
        }
        None
    }

    pub(super) fn read_pci_gpu_product_name(&self, dev_dir: &Path) -> Option<String> {
        if let Ok(raw) = self.sysfs.read(&dev_dir.join("product_name")) {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
        if let Ok(mut cards) = self.sysfs.read_dir(&dev_dir.join("drm")) {
            cards.sort();
            for card in cards {
                if card
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("card"))
                {
                    if let Ok(raw) = self.sysfs.read(&card.join("device/product_name")) {
                        let trimmed = raw.trim();
                        if !trimmed.is_empty() {
                            return Some(trimmed.to_string());
                        }
                    }
                }
            }
        }
        None
    }

    pub(super) fn read_nvidia_gpu_information_model(&self, bdf: &str) -> Option<String> {
        let info_path = self
            .sys_root
            .join("proc/driver/nvidia/gpus")
            .join(bdf)
            .join("information");
        let raw = self.sysfs.read(&info_path).ok()?;
        for line in raw.lines() {
            if let Some(rest) = line.trim_start().strip_prefix("Model:") {
                let model = rest.trim();
                if !model.is_empty() {
                    return Some(model.to_string());
                }
            }
        }
        None
    }

    pub(super) fn detect_pci_gpus(&self) -> (String, String) {
        let pci_dir = self.sys_root.join("sys/bus/pci/devices");
        let Ok(mut entries) = self.sysfs.read_dir(&pci_dir) else {
            return (String::new(), String::new());
        };
        entries.sort();
        let mut igpu_model = String::new();
        let mut dgpu_model = String::new();

        for entry in entries {
            let Some(class) = self
                .sysfs
                .read(&entry.join("class"))
                .ok()
                .and_then(|s| Self::parse_pci_hex(&s))
            else {
                continue;
            };
            let base_class = class & 0xFFFF00;
            if base_class != 0x030000 && base_class != 0x030200 {
                continue;
            }
            let Some(vendor) = self
                .sysfs
                .read(&entry.join("vendor"))
                .ok()
                .and_then(|s| Self::parse_pci_hex(&s))
            else {
                continue;
            };
            let device = self
                .sysfs
                .read(&entry.join("device"))
                .ok()
                .and_then(|s| Self::parse_pci_hex(&s))
                .unwrap_or(0);
            let bdf = entry
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            let bus = bdf
                .split(':')
                .nth(1)
                .and_then(|b| u32::from_str_radix(b, 16).ok())
                .unwrap_or(0);

            let is_suspended = self
                .sysfs
                .read(&entry.join("power/runtime_status"))
                .is_ok_and(|s| s.trim() == "suspended");

            let model = self
                .sysfs
                .read(&entry.join("label"))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .or_else(|| self.read_pci_gpu_product_name(&entry))
                .or_else(|| {
                    if !is_suspended {
                        self.read_nvidia_gpu_information_model(&bdf)
                    } else {
                        None
                    }
                })
                .or_else(|| {
                    self.lookup_pci_ids(vendor, device)
                        .map(|raw| Self::decode_pci_name(&raw, vendor))
                })
                .unwrap_or_else(|| match vendor {
                    0x10de => "NVIDIA GPU".to_string(),
                    0x1002 => "AMD GPU".to_string(),
                    0x8086 => "Intel GPU".to_string(),
                    _ => "Unknown GPU".to_string(),
                });

            let is_igpu = (bus == 0 && (vendor == 0x8086 || vendor == 0x1002))
                || (vendor == 0x8086 && base_class == 0x030000);
            if is_igpu {
                if igpu_model.is_empty() {
                    igpu_model = model;
                }
            } else if dgpu_model.is_empty() {
                dgpu_model = model;
            }
        }
        (igpu_model, dgpu_model)
    }

    pub(super) fn get_system_info_json_impl(&self) -> String {
        let (sys_vendor, board_vendor, board_name, product_sku, manufacturer, cpu_model, pci_igpu, pci_dgpu) = {
            let mut cache = self.lock_mutex(&self.cached_system_info);
            if let Some(ref c) = *cache {
                c.clone()
            } else {
                let sv = self.read_dmi_field("sys_vendor");
                let bv = self.read_dmi_field("board_vendor");
                let bn = self.read_dmi_field("board_name");
                let sku = self.read_dmi_field("product_sku");
                let mfg = classify_manufacturer(&sv, &bv);
                let cpu = self.read_cpu_model();
                let (igpu, dgpu) = self.detect_pci_gpus();
                let tuple = (sv, bv, bn, sku, mfg, cpu, igpu, dgpu);
                *cache = Some(tuple.clone());
                tuple
            }
        };

        let latest = self.read_rw(&self.latest_telemetry).clone();
        let dgpu_model = if !pci_dgpu.is_empty() {
            pci_dgpu
        } else {
            latest
                .as_ref()
                .and_then(|s| s.dgpu.as_ref())
                .map(|d| d.vendor.as_str().to_string())
                .filter(|v| v != "unknown")
                .unwrap_or_default()
        };
        let igpu_model = if !pci_igpu.is_empty() {
            pci_igpu
        } else {
            latest
                .as_ref()
                .and_then(|s| s.igpu.as_ref())
                .map(|g| g.vendor.as_str().to_string())
                .filter(|v| v != "unknown")
                .unwrap_or_default()
        };
        let laptop_model = self.get_device_name_impl();
        json!({
            "cpuModel": cpu_model,
            "iGpuModel": igpu_model,
            "dGpuModel": dgpu_model,
            "manufacturer": format!("{manufacturer:?}").to_lowercase(),
            "laptopModel": laptop_model,
            "productSKU": product_sku,
            "boardName": board_name,
            "boardVendor": board_vendor,
            "sysVendor": sys_vendor,
            "deviceName": laptop_model,
            "vendor": self.capabilities.device.vendor.as_str(),
            "waterCoolerSupported": self.capabilities.device.water_cooler_supported,
            "ctgpSupported": self.capabilities.device.ctgp_supported,
            "hwpDynamicBoostSupported": self.capabilities.device.hwp_dynamic_boost_supported,
            "fansMinSpeed": self.capabilities.device.fans_min_speed,
            "fansOffAvailable": self.capabilities.device.fans_off_available,
            "tdpChannels": self.capabilities.tdp_channels,
            "perfProfilesAvailable": self.capabilities.perf_profiles_available,
        })
        .to_string()
    }

    pub(super) fn is_backlight_internal_connector(name: &str) -> bool {
        name.contains("-eDP-") || name.contains("-LVDS-") || name.contains("-DSI-")
    }

    pub(super) fn backlight_connector_score(&self, dir: &Path) -> i32 {
        let status = self.sysfs.read(&dir.join("status")).unwrap_or_default();
        let enabled = self.sysfs.read(&dir.join("enabled")).unwrap_or_default();
        if status.trim() == "disconnected" || enabled.trim() == "disabled" {
            return -1;
        }
        if enabled.trim() == "enabled" {
            return 300;
        }
        if status.trim() == "connected" {
            200
        } else {
            100
        }
    }

    pub(super) fn backlight_topology_score(&self, entry: &Path) -> i32 {
        let canonical = std::fs::canonicalize(entry).unwrap_or_else(|_| entry.to_path_buf());
        let mut cursor = canonical.as_path();
        loop {
            if let Some(name) = cursor.file_name().and_then(|n| n.to_str()) {
                if Self::is_backlight_internal_connector(name) {
                    return self.backlight_connector_score(cursor);
                }
            }
            let drm_sub = cursor.join("drm");
            if drm_sub.is_dir() {
                let mut best = -1;
                let mut found = false;
                if let Ok(cards) = std::fs::read_dir(&drm_sub) {
                    for card in cards.flatten() {
                        if !card.path().is_dir() {
                            continue;
                        }
                        if let Ok(conns) = std::fs::read_dir(card.path()) {
                            for conn in conns.flatten() {
                                if let Some(cname) = conn.file_name().to_str() {
                                    if Self::is_backlight_internal_connector(cname) {
                                        found = true;
                                        best = best.max(self.backlight_connector_score(&conn.path()));
                                    }
                                }
                            }
                        }
                    }
                }
                if found {
                    return best;
                }
                break;
            }
            let Some(parent) = cursor.parent() else {
                break;
            };
            cursor = parent;
        }
        100
    }

    /// Selects best backlight device using `DisplayBacklight.hpp` priority (DRM topology + `raw`>`platform`>`firmware`)
    /// and reports whether `amdgpu_bl` inversion applies (`ISSUE-23`).
    pub(super) fn discover_backlight_device(&self) -> Option<(PathBuf, i32, bool)> {
        let bl_dir = self.sys_root.join("sys/class/backlight");
        let Ok(mut entries) = self.sysfs.read_dir(&bl_dir) else {
            return None;
        };
        entries.sort();
        let mut best: Option<(i32, PathBuf, i32, bool)> = None;
        for entry in entries {
            let Ok(max_raw) = self.sysfs.read(&entry.join("max_brightness")) else {
                continue;
            };
            let Ok(max_val) = max_raw.trim().parse::<i32>() else {
                continue;
            };
            if max_val <= 0 || self.sysfs.read(&entry.join("brightness")).is_err() {
                continue;
            }
            let topo = self.backlight_topology_score(&entry);
            if topo < 0 {
                continue;
            }
            let kind = self.sysfs.read(&entry.join("type")).unwrap_or_default();
            let type_bonus = match kind.trim() {
                "raw" => 2,
                "platform" => 1,
                _ => 0,
            };
            let score = topo * 10 + type_bonus;
            let is_amdgpu = entry
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("amdgpu_bl"));

            match best {
                Some((best_score, _, _, _)) if score <= best_score => {}
                _ => {
                    best = Some((score, entry, max_val, is_amdgpu));
                }
            }
        }
        best.map(|(_, path, max_val, is_amdgpu)| (path, max_val, is_amdgpu))
    }

    pub(super) fn get_display_brightness_impl(&self) -> i32 {
        let Some((dev, max_val, is_amdgpu)) = self.discover_backlight_device() else {
            return -1;
        };
        let raw = self
            .sysfs
            .read(&dev.join("actual_brightness"))
            .or_else(|_| self.sysfs.read(&dev.join("brightness")))
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok());
        match raw {
            Some(v) if v >= 0 => {
                let adjusted = if is_amdgpu {
                    (max_val - v).clamp(0, max_val)
                } else {
                    v.clamp(0, max_val)
                };
                ((f64::from(adjusted) / f64::from(max_val)) * 100.0)
                    .round()
                    .clamp(0.0, 100.0) as i32
            }
            _ => -1,
        }
    }

    pub(super) fn set_display_brightness_internal(&self, brightness: i32) -> bool {
        if !(0..=100).contains(&brightness) || self.shutdown_wait_expired.load(Ordering::SeqCst) {
            return false;
        }
        let Some((dev, max_val, is_amdgpu)) = self.discover_backlight_device() else {
            return false;
        };
        let brightness_path = dev.join("brightness");
        let prev_raw = self.sysfs.read(&brightness_path).ok();
        if self.shutdown_wait_expired.load(Ordering::SeqCst) {
            return false;
        }
        let mut raw = ((f64::from(brightness) / 100.0) * f64::from(max_val))
            .round()
            .clamp(0.0, f64::from(max_val)) as i32;
        if is_amdgpu {
            raw = (max_val - raw).clamp(0, max_val);
        }
        if self.sysfs.write(&brightness_path, &raw.to_string()).is_err() {
            return false;
        }
        let tol = ((100.0 / f64::from(max_val)).ceil() as i32).max(1);
        let max_attempts = (BACKLIGHT_RAMP_MAX_WAIT_MS / BACKLIGHT_RAMP_POLL_STEP.as_millis() as usize).max(1);
        for attempt in 0..=max_attempts {
            if self.shutting_down.load(Ordering::SeqCst)
                || self.shutdown_wait_expired.load(Ordering::SeqCst)
            {
                break;
            }
            if attempt > 0 {
                std::thread::sleep(BACKLIGHT_RAMP_POLL_STEP);
                if self.shutting_down.load(Ordering::SeqCst)
                    || self.shutdown_wait_expired.load(Ordering::SeqCst)
                {
                    break;
                }
            }
            let actual = self.get_display_brightness_impl();
            if self.shutting_down.load(Ordering::SeqCst)
                || self.shutdown_wait_expired.load(Ordering::SeqCst)
            {
                break;
            }
            if actual >= 0 && (actual - brightness).abs() <= tol {
                return true;
            }
        }
        if !self.shutting_down.load(Ordering::SeqCst)
            && !self.shutdown_wait_expired.load(Ordering::SeqCst)
        {
            if let Some(prev) = prev_raw {
                let _ = self.sysfs.write(&brightness_path, prev.trim());
            }
        }
        false
    }

    pub(super) fn discover_fn_lock_path(&self) -> Option<PathBuf> {
        let base = self.sys_root.join("sys/devices/platform");
        for rel in [
            "tuxedo_keyboard/fn_lock",
            "uniwill/fn_lock",
            "clevo_acpi/fn_lock",
            "clevo_xsm_wmi/fn_lock",
        ] {
            let p = base.join(rel);
            if self.sysfs.exists(&p) {
                return Some(p);
            }
        }
        None
    }

    pub(super) fn charging_profile_paths(&self) -> Option<(PathBuf, PathBuf)> {
        let base = self
            .sys_root
            .join("sys/devices/platform/tuxedo_keyboard/charging_profile");
        let avail = base.join("charging_profiles_available");
        let current = base.join("charging_profile");
        if self.sysfs.exists(&avail) && self.sysfs.exists(&current) {
            Some((avail, current))
        } else {
            None
        }
    }

    pub(super) fn charging_priority_paths(&self) -> Option<(PathBuf, PathBuf)> {
        let base = self
            .sys_root
            .join("sys/devices/platform/tuxedo_keyboard/charging_priority");
        for (avail_name, cur_name) in [
            ("charging_prios_available", "charging_prio"),
            ("charging_priorities_available", "charging_priority"),
        ] {
            let avail = base.join(avail_name);
            let current = base.join(cur_name);
            if self.sysfs.exists(&avail) && self.sysfs.exists(&current) {
                return Some((avail, current));
            }
        }
        None
    }

    pub(super) fn nvidia_ctgp_sysfs_path(&self) -> Option<PathBuf> {
        for rel in [
            "sys/devices/platform/tuxedo_nvidia_power_ctrl/ctgp_offset",
            "sys/devices/platform/tuxedo_nvidia_ctgp/ctgp_offset",
        ] {
            let p = self.sys_root.join(rel);
            if self.sysfs.exists(&p) {
                return Some(p);
            }
        }
        None
    }

    pub(super) fn discover_battery_dir(&self) -> Option<PathBuf> {
        let ps_dir = self.sys_root.join("sys/class/power_supply");
        let Ok(mut entries) = self.sysfs.read_dir(&ps_dir) else {
            return None;
        };
        entries.sort();
        for entry in entries {
            let is_bat = self
                .sysfs
                .read(&entry.join("type"))
                .is_ok_and(|t| t.trim() == "Battery")
                || entry
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("BAT"));
            if is_bat {
                return Some(entry);
            }
        }
        None
    }

    /// Applies saved charging thresholds/profile/type on startup or profile commit (`ISSUE-08`, `ISSUE-24`).
    pub(super) fn apply_charging_settings_for_profile(&self, profile: &UccProfile) {
        if self.is_vendor_unsupported() || ucc_config::is_read_only_preview() {
            return;
        }
        let (cfg_start, cfg_end, cfg_type, cfg_prof, cfg_prio) = {
            let s = self.read_rw(&self.settings);
            if self.has_persisted_config {
                (
                    s.charge_start_threshold,
                    s.charge_end_threshold,
                    s.charge_type.clone(),
                    s.charging_profile.clone(),
                    s.charging_priority.clone(),
                )
            } else {
                (None, None, None, None, None)
            }
        };
        let start = profile.charging.start_threshold.or(cfg_start);
        let end = profile.charging.end_threshold.or(cfg_end);

        if let Some(bat) = self.discover_battery_dir() {
            let start_path = bat.join("charge_control_start_threshold");
            let end_path = bat.join("charge_control_end_threshold");
            let current_end = self
                .sysfs
                .read(&end_path)
                .ok()
                .and_then(|s| s.trim().parse::<u8>().ok());
            let write_end_first = matches!((start, current_end), (Some(s), Some(ce)) if s >= ce);
            if write_end_first {
                if let Some(e) = end {
                    if self.sysfs.exists(&end_path) {
                        let _ = self.sysfs.write(&end_path, &e.to_string());
                    }
                }
                if let Some(s) = start {
                    if self.sysfs.exists(&start_path) && end.is_none_or(|e| s < e) {
                        let _ = self.sysfs.write(&start_path, &s.to_string());
                    }
                }
            } else {
                if let Some(s) = start {
                    if self.sysfs.exists(&start_path) && end.is_none_or(|e| s < e) {
                        let _ = self.sysfs.write(&start_path, &s.to_string());
                    }
                }
                if let Some(e) = end {
                    if self.sysfs.exists(&end_path) {
                        let _ = self.sysfs.write(&end_path, &e.to_string());
                    }
                }
            }

            if let Some(ct) = profile
                .charging
                .charge_type
                .as_ref()
                .map(|c| c.as_str().to_string())
                .or(cfg_type)
            {
                let ct_path = bat.join("charge_type");
                if self.sysfs.exists(&ct_path) {
                    let _ = self.sysfs.write(&ct_path, ct.trim());
                }
            }
        }

        if let Some(prof_desc) = profile.charging.charging_profile.as_deref().or(cfg_prof.as_deref()) {
            if let Some((avail_path, cur_path)) = self.charging_profile_paths() {
                let avail = self.read_whitespace_tokens(&avail_path);
                if avail.iter().any(|p| p == prof_desc.trim()) {
                    let _ = self.sysfs.write(&cur_path, prof_desc.trim());
                }
            }
        }
        if let Some(prio_desc) = profile.charging.charging_priority.as_deref().or(cfg_prio.as_deref()) {
            if let Some((avail_path, cur_path)) = self.charging_priority_paths() {
                let avail = self.read_whitespace_tokens(&avail_path);
                if avail.iter().any(|p| p == prio_desc.trim()) {
                    let _ = self.sysfs.write(&cur_path, prio_desc.trim());
                }
            }
        }
    }

    pub(super) fn parse_drm_mode_line(line: &str) -> Option<(i32, i32, Option<i32>)> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return None;
        }
        let (res_part, hz) = if let Some((left, right)) = trimmed.split_once('@') {
            let hz_num = right
                .trim_end_matches(|c: char| !c.is_ascii_digit() && c != '.')
                .parse::<f64>()
                .ok()
                .map(|v| v.round() as i32)
                .filter(|&v| v > 0);
            (left, hz_num)
        } else {
            (trimmed, None)
        };
        let cleaned = res_part.trim_end_matches(['p', 'i', 'r']);
        let (w_str, h_str) = cleaned.split_once('x')?;
        let w = w_str.parse::<i32>().ok().filter(|&v| v > 0)?;
        let h = h_str.parse::<i32>().ok().filter(|&v| v > 0)?;
        Some((w, h, hz))
    }

    pub(super) fn get_display_modes_json_impl(&self) -> String {
        let drm_dir = self.sys_root.join("sys/class/drm");
        let empty = || {
            json!({
                "displayName": "",
                "activeMode": {
                    "xResolution": 0,
                    "yResolution": 0,
                    "refreshRates": [],
                },
                "displayModes": [],
            })
            .to_string()
        };
        let Ok(mut entries) = self.sysfs.read_dir(&drm_dir) else {
            return empty();
        };
        entries.sort();

        let mut connected: Vec<(bool, String, PathBuf)> = Vec::new();
        for entry in entries {
            let Some(name) = entry.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some((_, conn_name)) = name.split_once('-') else {
                continue;
            };
            if !name.starts_with("card") {
                continue;
            }
            let Ok(status) = self.sysfs.read(&entry.join("status")) else {
                continue;
            };
            if status.trim() != "connected" {
                continue;
            }
            let is_internal = conn_name.starts_with("eDP")
                || conn_name.starts_with("LVDS")
                || conn_name.starts_with("DSI");
            connected.push((!is_internal, conn_name.to_string(), entry));
        }
        connected.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

        for (_, display_name, entry) in connected {
            let Ok(modes_raw) = self.sysfs.read(&entry.join("modes")) else {
                continue;
            };
            let mut order: Vec<(i32, i32)> = Vec::new();
            let mut grouped: BTreeMap<(i32, i32), Vec<i32>> = BTreeMap::new();
            for line in modes_raw.lines() {
                if let Some((w, h, hz_opt)) = Self::parse_drm_mode_line(line) {
                    let rates = grouped.entry((w, h)).or_insert_with(|| {
                        order.push((w, h));
                        Vec::new()
                    });
                    if let Some(hz) = hz_opt {
                        if !rates.contains(&hz) {
                            rates.push(hz);
                        }
                    }
                }
            }
            if order.is_empty() {
                continue;
            }
            let active = self
                .sysfs
                .read(&entry.join("mode"))
                .ok()
                .and_then(|m| Self::parse_drm_mode_line(&m));
            let active_json = match active {
                Some((aw, ah, Some(ahz))) => json!({
                    "xResolution": aw,
                    "yResolution": ah,
                    "refreshRates": [ahz],
                }),
                Some((aw, ah, None)) => json!({
                    "xResolution": aw,
                    "yResolution": ah,
                    "refreshRates": [],
                }),
                None => json!({
                    "xResolution": 0,
                    "yResolution": 0,
                    "refreshRates": [],
                }),
            };
            let display_modes: Vec<Value> = order
                .into_iter()
                .map(|(w, h)| {
                    let rates = grouped.remove(&(w, h)).unwrap_or_default();
                    json!({
                        "xResolution": w,
                        "yResolution": h,
                        "refreshRates": rates,
                    })
                })
                .collect();
            return json!({
                "displayName": display_name,
                "activeMode": active_json,
                "displayModes": display_modes,
            })
            .to_string();
        }
        empty()
    }

    pub(super) fn get_is_x11_impl(&self) -> bool {
        let sessions_dir = self.sys_root.join("run/systemd/sessions");
        if let Ok(mut entries) = self.sysfs.read_dir(&sessions_dir) {
            entries.sort();
            for entry in entries {
                if entry
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with(".ref"))
                {
                    continue;
                }
                let Ok(content) = self.sysfs.read(&entry) else {
                    continue;
                };
                let mut active = false;
                let mut session_type = None::<String>;
                for line in content.lines() {
                    let line = line.trim();
                    if let Some(val) = line.strip_prefix("ACTIVE=") {
                        active |= val.trim() == "1";
                    } else if let Some(val) = line.strip_prefix("STATE=") {
                        active |= val.trim() == "active";
                    } else if let Some(val) = line.strip_prefix("TYPE=") {
                        session_type = Some(val.trim().to_ascii_lowercase());
                    }
                }
                if active {
                    match session_type.as_deref() {
                        Some("x11") => return true,
                        Some("wayland") => return false,
                        _ => {}
                    }
                }
            }
        }

        let session = std::env::var("XDG_SESSION_TYPE")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if session == "wayland" {
            return false;
        }
        if session == "x11" {
            return true;
        }
        std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_some()
    }

    pub(super) fn is_safe_drm_port(port: &str) -> bool {
        !port.is_empty() && !port.contains("..") && !port.contains('/') && !port.contains('\\')
    }

    pub(super) fn configured_ycbcr_targets(&self) -> Vec<(i32, String)> {
        let ycbcr = self.read_rw(&self.settings).ycbcr420_workaround.clone();
        let Some(entries) = ycbcr else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (idx, entry) in entries.into_iter().enumerate() {
            let default_card = idx as i32;
            match entry {
                ucc_config::YCbCrWorkaroundEntry::Map(map) => {
                    for port in map.into_keys() {
                        out.push((default_card, port));
                    }
                }
                ucc_config::YCbCrWorkaroundEntry::Port { port, .. } => {
                    out.push((0, port));
                }
                ucc_config::YCbCrWorkaroundEntry::Card { card, ports } => {
                    for item in ports {
                        if let Some(p) = item.as_str() {
                            out.push((card, p.to_string()));
                        } else if let Some(obj) = item.as_object() {
                            if let Some(p) = obj.get("port").and_then(|v| v.as_str()) {
                                out.push((card, p.to_string()));
                            } else {
                                for k in obj.keys() {
                                    out.push((card, k.clone()));
                                }
                            }
                        }
                    }
                }
                ucc_config::YCbCrWorkaroundEntry::String(port) => {
                    out.push((0, port));
                }
            }
        }
        out
    }

    pub(super) fn get_force_yuv420_output_switch_available_impl(&self) -> bool {
        let targets = self.configured_ycbcr_targets();
        if !targets.is_empty() {
            return targets.into_iter().any(|(card, port)| {
                Self::is_safe_drm_port(&port)
                    && (self.sysfs.exists(&self.sys_root.join(format!(
                        "sys/kernel/debug/dri/{card}/{port}/force_yuv420_output"
                    ))) || self.sysfs.exists(&self.sys_root.join(format!(
                        "sys/class/drm/card{card}-{port}/force_yuv420_output"
                    ))))
            });
        }

        let dri_dir = self.sys_root.join("sys/kernel/debug/dri");
        if let Ok(cards) = self.sysfs.read_dir(&dri_dir) {
            for card_dir in cards {
                if let Ok(ports) = self.sysfs.read_dir(&card_dir) {
                    if ports
                        .into_iter()
                        .any(|port_dir| self.sysfs.exists(&port_dir.join("force_yuv420_output")))
                    {
                        return true;
                    }
                }
            }
        }

        let drm_dir = self.sys_root.join("sys/class/drm");
        let Ok(entries) = self.sysfs.read_dir(&drm_dir) else {
            return false;
        };
        entries
            .into_iter()
            .any(|entry| self.sysfs.exists(&entry.join("force_yuv420_output")))
    }

    pub(super) fn map_prime_token(token: &str) -> Option<String> {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(match trimmed.to_ascii_lowercase().as_str() {
            "nvidia" | "on" => "dGPU".to_string(),
            "intel" | "off" => "iGPU".to_string(),
            "on-demand" => "on-demand".to_string(),
            _ => "off".to_string(),
        })
    }

    pub(super) fn get_prime_state_impl(&self) -> String {
        let prime_cmd = if self.sys_root == Path::new("/") {
            Some(PathBuf::from("prime-select"))
        } else {
            [
                self.sys_root.join("usr/bin/prime-select"),
                self.sys_root.join("bin/prime-select"),
            ]
            .into_iter()
            .find(|p| p.is_file())
        };
        if let Some(cmd) = prime_cmd {
            if let Ok(mut child) = std::process::Command::new(cmd)
                .arg("query")
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                let deadline = Instant::now() + PRIME_SELECT_TIMEOUT;
                loop {
                    match child.try_wait() {
                        Ok(Some(_)) => {
                            if let Ok(output) = child.wait_with_output() {
                                if output.status.success() {
                                    let stdout = String::from_utf8_lossy(&output.stdout);
                                    if let Some(mapped) = Self::map_prime_token(&stdout) {
                                        return mapped;
                                    }
                                }
                            }
                            break;
                        }
                        Ok(None) => {
                            if Instant::now() >= deadline {
                                let _ = child.kill();
                                let _ = child.wait();
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(15));
                        }
                        Err(_) => break,
                    }
                }
            }
        }
        let prime_file = self.sys_root.join("etc/prime-discrete");
        if let Ok(raw) = self.sysfs.read(&prime_file) {
            return Self::map_prime_token(&raw).unwrap_or_else(|| "-1".to_string());
        }
        "-1".to_string()
    }

    pub(super) fn get_driver_health_json_impl(&self) -> String {
        let mut issues = Vec::<String>::new();
        let module_exists = self
            .sysfs
            .exists(&self.sys_root.join("sys/module/tuxedo_io"));
        let dev_exists = self.sys_root.join("dev/tuxedo_io").exists()
            || self.sysfs.exists(&self.sys_root.join("dev/tuxedo_io"));
        if self.sys_root == Path::new("/") {
            if !module_exists {
                issues.push(
                    "Не загружен аппаратный драйвер TUXEDO: датчики, вентиляторы и часть управления недоступны. Проверьте сборку DKMS для текущего ядра."
                        .to_string(),
                );
            } else if !dev_exists {
                issues.push(
                    "Драйвер TUXEDO загружен, но аппаратный интерфейс /dev/tuxedo_io отсутствует."
                        .to_string(),
                );
            } else if !self.platform.is_available() {
                issues.push(
                    "Служба запущена без рабочего аппаратного интерфейса. После загрузки драйвера перезапустите uccd.service."
                        .to_string(),
                );
            }
        } else if module_exists && !dev_exists {
            issues.push(
                "Драйвер TUXEDO загружен, но аппаратный интерфейс /dev/tuxedo_io отсутствует."
                    .to_string(),
            );
        } else if !self.platform.is_available() {
            issues.push(
                "Не загружен аппаратный драйвер TUXEDO: датчики, вентиляторы и часть управления недоступны. Проверьте сборку DKMS для текущего ядра."
                    .to_string(),
            );
        }

        let board_name = self.read_dmi_field("board_name");
        if matches!(
            board_name.as_str(),
            "YAOSHI Series-X6AR55xY" | "X6AR55xY" | "X6AR5xxY"
        ) {
            let leds_dir = self.sys_root.join("sys/class/leds");
            let has_kbd = self.sysfs.read_dir(&leds_dir).is_ok_and(|entries| {
                entries.iter().any(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("rgb:kbd_backlight"))
                })
            });
            if !has_kbd {
                issues.push("Не обнаружен драйвер RGB-клавиатуры ITE 8291.".to_string());
            }
            if !self
                .sysfs
                .exists(&leds_dir.join("rgb:lightbar/multi_intensity"))
                && !self
                    .sysfs
                    .exists(&leds_dir.join("lightbar/multi_intensity"))
            {
                issues.push("Не обнаружен аппаратный интерфейс Lightbar.".to_string());
            }
        }

        let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let healthy = issues.is_empty();
        let message = if healthy {
            String::new()
        } else {
            format!("Проблема с драйверами · ядро {kernel}\n{}", issues.join("\n"))
        };
        json!({
            "healthy": healthy,
            "kernel": kernel,
            "issues": issues,
            "message": message,
        })
        .to_string()
    }

    pub(super) fn get_dgpu_info_values_json_impl(&self) -> String {
        if let Some(ref snap) = *self.read_rw(&self.latest_telemetry) {
            if let Some(ref dgpu) = snap.dgpu {
                return dgpu_to_legacy_json(dgpu).to_string();
            }
        }
        // Return neutral unknown values rather than fabricating vendor = "nvidia" (`t_hw_07`, `ISSUE-21`).
        json!({
            "vendor": "unknown",
            "temp": -1.0,
            "coreFrequency": -1.0,
            "maxCoreFrequency": -1.0,
            "powerDraw": -1.0,
            "perfLimitReason": "",
            "grClockOffsetMHz": -999,
        })
        .to_string()
    }

    pub(super) fn get_igpu_info_values_json_impl(&self) -> String {
        if let Some(ref snap) = *self.read_rw(&self.latest_telemetry) {
            if let Some(ref igpu) = snap.igpu {
                return igpu_to_legacy_json(igpu).to_string();
            }
        }
        json!({
            "vendor": "unknown",
            "temp": -1.0,
            "coreFrequency": -1.0,
            "maxCoreFrequency": -1.0,
            "powerDraw": -1.0,
        })
        .to_string()
    }

    pub(super) fn set_nvidia_power_offset_impl(&self, offset: i32) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        if !self.capabilities.device.ctgp_supported {
            return false;
        }
        if let Some(ref nvml) = self.nvml {
            let dgpu = self
                .read_rw(&self.latest_telemetry)
                .as_ref()
                .and_then(|s| s.dgpu.clone());
            if let Some(dgpu) = dgpu {
                if let Some(ref bdf) = dgpu.bdf {
                    let immutable_base = dgpu
                        .default_power_limit_w
                        .or(dgpu.max_power_limit_w)
                        .or(dgpu.enforced_power_limit_w);
                    let baseline_w = {
                        let mut stored = self.lock_mutex(&self.nvidia_default_power_limit_w);
                        if let Some((ref stored_bdf, val)) = *stored {
                            if stored_bdf == bdf {
                                Some(val)
                            } else {
                                if let Some(v) = immutable_base {
                                    *stored = Some((bdf.clone(), v));
                                }
                                immutable_base
                            }
                        } else {
                            if let Some(v) = immutable_base {
                                *stored = Some((bdf.clone(), v));
                            }
                            immutable_base
                        }
                    };
                    if let Some(base_w) = baseline_w {
                        if !base_w.is_finite() || base_w <= 0.0 {
                            return false;
                        }
                        let base_i64 = base_w.round() as i64;
                        let Some(target_w) = base_i64.checked_add(i64::from(offset)) else {
                            return false;
                        };
                        if target_w <= 0 {
                            return false;
                        }
                        let Some(target_mw_i64) = target_w.checked_mul(1000) else {
                            return false;
                        };
                        let Ok(target_mw) = u32::try_from(target_mw_i64) else {
                            return false;
                        };
                        if nvml.set_power_limit_by_bdf(bdf, target_mw).is_ok() {
                            *self.lock_mutex(&self.nvidia_power_offset) = Some(offset);
                            return true;
                        }
                    }
                }
            }
        }

        if let Some(ctgp_path) = self.nvidia_ctgp_sysfs_path() {
            if !(-1000..=1000).contains(&offset) {
                return false;
            }
            if self.sysfs.write(&ctgp_path, &offset.to_string()).is_ok() {
                *self.lock_mutex(&self.nvidia_power_offset) = Some(offset);
                return true;
            }
        }

        false
    }

    pub(super) fn get_charge_start_available_thresholds_impl(&self) -> String {
        let Some(bat) = self.discover_battery_dir() else {
            return "[]".to_string();
        };
        if !self.sysfs.exists(&bat.join("charge_control_start_threshold")) {
            return "[]".to_string();
        }
        let avail_path = bat.join("charge_control_start_available_thresholds");
        if self.sysfs.exists(&avail_path) {
            let parsed: Vec<i32> = self
                .read_whitespace_tokens(&avail_path)
                .into_iter()
                .filter_map(|t| t.parse::<i32>().ok())
                .collect();
            if !parsed.is_empty() {
                return serde_json::to_string(&parsed).unwrap_or_else(|_| "[]".to_string());
            }
        }
        let vals: Vec<i32> = (0..=99).collect();
        serde_json::to_string(&vals).unwrap_or_else(|_| "[]".to_string())
    }

    pub(super) fn get_charge_end_available_thresholds_impl(&self) -> String {
        let Some(bat) = self.discover_battery_dir() else {
            return "[]".to_string();
        };
        if !self.sysfs.exists(&bat.join("charge_control_end_threshold")) {
            return "[]".to_string();
        }
        let avail_path = bat.join("charge_control_end_available_thresholds");
        if self.sysfs.exists(&avail_path) {
            let parsed: Vec<i32> = self
                .read_whitespace_tokens(&avail_path)
                .into_iter()
                .filter_map(|t| t.parse::<i32>().ok())
                .collect();
            if !parsed.is_empty() {
                return serde_json::to_string(&parsed).unwrap_or_else(|_| "[]".to_string());
            }
        }
        let vals: Vec<i32> = (1..=100).collect();
        serde_json::to_string(&vals).unwrap_or_else(|_| "[]".to_string())
    }
}
