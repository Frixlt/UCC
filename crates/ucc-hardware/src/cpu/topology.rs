// SPDX-License-Identifier: GPL-3.0-or-later
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Information about a single CPU core discovered from sysfs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CpuInfo {
    /// Logical core index (e.g. 0, 1, 8, 9)
    pub id: u32,
    /// Whether this logical core is currently online
    pub online: bool,
    /// Whether this core has an /online file (core 0 is non-hotpluggable mandatory core)
    pub hotpluggable: bool,
    /// Core type: "P" (Performance), "E" (Efficient), or "unknown"
    #[serde(rename = "type")]
    pub core_type: String,
    /// Physical core ID from topology/core_id
    pub physical_core: u32,
    /// Sibling threads from topology/thread_siblings_list
    pub siblings: String,
}

/// Discovers and tracks CPU topology from sysfs.
///
/// Discovers present cores without bringing offline cores online.
/// Discovered core types are cached in memory so offlined cores retain
/// their "P" or "E" labels in the UI and telemetry.
#[derive(Debug, Clone)]
pub struct CpuTopology {
    root_path: PathBuf,
    cache: HashMap<u32, CachedCoreInfo>,
}

#[derive(Debug, Clone, Default)]
struct CachedCoreInfo {
    core_type: String,
    physical_core: u32,
    siblings: String,
}

impl Default for CpuTopology {
    fn default() -> Self {
        Self::new("/sys/devices/system/cpu")
    }
}

impl CpuTopology {
    /// Create a new CPU topology scanner rooted at the given sysfs directory.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root_path: root.into(),
            cache: HashMap::new(),
        }
    }

    /// Root path of the sysfs cpu directory.
    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    /// Read the current CPU topology.
    pub fn read(&mut self) -> Vec<CpuInfo> {
        let mut result = Vec::new();

        // Read present cores (fallback to possible if present is unavailable)
        let present_ids = self.read_id_list("present")
            .or_else(|| self.read_id_list("possible"))
            .unwrap_or_default();

        for id in present_ids {
            let cpu_dir = self.root_path.join(format!("cpu{}", id));
            let online_file = cpu_dir.join("online");
            let hotplug = online_file.exists();
            let online = if !hotplug {
                true // Core 0 without online file is always online
            } else {
                read_file_bool(&online_file).unwrap_or(false)
            };

            let (physical, siblings) = if online {
                let physical = read_file_u32(&cpu_dir.join("topology/core_id")).unwrap_or(id);
                let siblings = fs::read_to_string(cpu_dir.join("topology/thread_siblings_list"))
                    .map(|s| s.trim().to_string())
                    .unwrap_or_else(|_| id.to_string());
                (physical, siblings)
            } else {
                (id, id.to_string())
            };

            let cached_type = self.cache.get(&id).map(|c| c.core_type.clone()).unwrap_or_default();
            let probed_type = if online && (cached_type.is_empty() || cached_type == "unknown") {
                Some(self.probe_core_type(id))
            } else {
                None
            };

            let cached = self.cache.entry(id).or_default();

            if online {
                cached.physical_core = physical;
                cached.siblings = siblings;
                if let Some(t) = probed_type {
                    cached.core_type = t;
                }
            }

            let core_type = if cached.core_type.is_empty() {
                "unknown".to_string()
            } else {
                cached.core_type.clone()
            };

            let physical_core = if cached.physical_core != 0 || !cached.core_type.is_empty() {
                cached.physical_core
            } else {
                id
            };

            let siblings = if cached.siblings.is_empty() {
                id.to_string()
            } else {
                cached.siblings.clone()
            };

            result.push(CpuInfo {
                id,
                online,
                hotpluggable: hotplug,
                core_type,
                physical_core,
                siblings,
            });
        }

        result
    }

    /// Parse a comma-separated range list file like "present" or "possible".
    fn read_id_list(&self, filename: &str) -> Option<Vec<u32>> {
        let path = self.root_path.join(filename);
        let content = fs::read_to_string(path).ok()?;
        Some(parse_cpu_range_list(&content))
    }

    /// Probe core type (P or E) for an online core using CPUID leaf 0x1A with an affinity guard.
    ///
    /// Never touches thread affinity or CPUID if running against a fake root path (fixtures).
    fn probe_core_type(&self, id: u32) -> String {
        // Safe guard: only probe real sysfs
        if self.root_path != Path::new("/sys/devices/system/cpu") {
            return "unknown".to_string();
        }

        probe_core_type_cpuid(id)
    }
}

/// Parse CPU range list strings like "0-1,8-9" or "0,2,4-7" into a sorted list of IDs.
pub fn parse_cpu_range_list(s: &str) -> Vec<u32> {
    let mut ids = Vec::new();
    for token in s.trim().split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        if let Some((start_s, end_s)) = token.split_once('-') {
            if let (Ok(start), Ok(end)) = (start_s.trim().parse::<u32>(), end_s.trim().parse::<u32>()) {
                for i in start..=end {
                    ids.push(i);
                }
            }
        } else if let Ok(val) = token.parse::<u32>() {
            ids.push(val);
        }
    }
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn read_file_bool(path: &Path) -> Option<bool> {
    let content = fs::read_to_string(path).ok()?;
    match content.trim() {
        "1" | "true" | "y" | "yes" => Some(true),
        "0" | "false" | "n" | "no" => Some(false),
        _ => None,
    }
}

fn read_file_u32(path: &Path) -> Option<u32> {
    let content = fs::read_to_string(path).ok()?;
    content.trim().parse::<u32>().ok()
}

/// Probes core type (P or E) using CPUID leaf 0x1A while preserving thread affinity.
#[cfg(all(target_os = "linux", any(target_arch = "x86", target_arch = "x86_64")))]
fn probe_core_type_cpuid(id: u32) -> String {
    if id >= libc::CPU_SETSIZE as u32 {
        return "unknown".to_string();
    }

    unsafe {
        let mut original: libc::cpu_set_t = std::mem::zeroed();
        if libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut original) != 0 {
            return "unknown".to_string();
        }
        if !libc::CPU_ISSET(id as usize, &original) {
            return "unknown".to_string();
        }

        let mut selected: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_ZERO(&mut selected);
        libc::CPU_SET(id as usize, &mut selected);
        if libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &selected) != 0 {
            return "unknown".to_string();
        }

        // RAII affinity guard ensuring original mask is restored even on early exit
        struct AffinityGuard {
            original: libc::cpu_set_t,
        }
        impl Drop for AffinityGuard {
            fn drop(&mut self) {
                unsafe {
                    let _ = libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &self.original);
                }
            }
        }
        let guard = AffinityGuard { original };

        let max_leaf = core::arch::x86_64::__get_cpuid_max(0).0;
        let mut core_type = "unknown";
        if max_leaf >= 0x1a {
            let res = core::arch::x86_64::__cpuid_count(0x1a, 0);
            let val = res.eax >> 24;
            if val == 0x40 {
                core_type = "P";
            } else if val == 0x20 {
                core_type = "E";
            }
        }

        drop(guard);

        // Verify affinity was restored successfully
        let mut check: libc::cpu_set_t = std::mem::zeroed();
        if libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut check) != 0 {
            return "unknown".to_string();
        }

        core_type.to_string()
    }
}

#[cfg(not(all(target_os = "linux", any(target_arch = "x86", target_arch = "x86_64"))))]
fn probe_core_type_cpuid(_id: u32) -> String {
    "unknown".to_string()
}
