// SPDX-License-Identifier: GPL-3.0-or-later

//! Real daemon backend wiring hardware controllers, telemetry, lighting, config, power policy,
//! and cooling into [`ucc_dbus::UccdBackend`].

use crate::lid::{LidController, LidPolicyConfig};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;
use tracing::{error, warn};
use ucc_config::{
    create_default_settings, default_high_performance_profile, default_max_energy_save_profile,
    default_office_profile, default_quiet_profile, ConfigManager, CustomFanProfile,
    CustomKeyboardProfile, UccSettings,
};
use ucc_core::{
    FanCurve, FanPoint, FanProfile, KeyboardBacklightState, PowerPolicy, ProfileState, PumpPoint,
    UccProfile,
};
use ucc_dbus::UccdBackend;
use ucc_hardware::{
    build_laptop_model, classify_manufacturer, dgpu_to_legacy_json, igpu_to_legacy_json,
    CoolerController, CoolerMode, CoolerState, DGpuSnapshot, DeviceVendor, DmiInfo, FanController,
    FanTickSnapshot, MetricsHistoryStore, MockHardwareIo, MockPlatformBackend, NvidiaPowerGuard,
    NvmlWrapper, PlatformBackend, PlatformCapabilities, PowerDeviceController, RealSysfs, Sysfs,
    TelemetryCollector, TelemetryConfig, TelemetrySnapshot, TuxedoIoBackend, TuxedoIoDevice,
    cooler::{FanTarget, PumpTarget, PumpVoltage, RgbState},
};
use ucc_lighting::{
    KeyboardBacklightController, LightbarController, LightbarPowerMode, LightingTransport,
    SysfsLedTransport,
};
use zbus::zvariant::{OwnedValue, Value as ZValue};

/// Asynchronous D-Bus signals emitted by [`LiveUccdBackend`] and dispatched by `DaemonRuntime`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DbusSignalEvent {
    ProfileChanged {
        profile_id: String,
        keyboard_profile_id: String,
        fan_profile_id: String,
    },
    ModeReapplyPendingChanged {
        pending: bool,
    },
    PowerStateChanged {
        state: String,
    },
    PowerPolicyChanged {
        json: String,
    },
    PowerDeviceStatusChanged {
        json: String,
    },
    LightbarSettingsChanged {
        json: String,
    },
    WaterCoolerStatusChanged {
        status: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PerCpuSnapshot {
    online: Option<String>,
    governor: Option<String>,
    epp: Option<String>,
    min_freq_khz: Option<String>,
    max_freq_khz: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CpuDefaultsSnapshot {
    per_cpu: BTreeMap<PathBuf, PerCpuSnapshot>,
    no_turbo: Option<String>,
    hwp_dynamic_boost: Option<String>,
}

fn normalize_and_parse_custom_fan_profile(
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

    if !obj.contains_key("tableCPU") {
        if let Some(v) = obj.get("cpu").cloned() {
            obj.insert("tableCPU".to_string(), v);
        }
    }
    if !obj.contains_key("tableGPU") {
        if let Some(v) = obj.get("gpu").cloned() {
            obj.insert("tableGPU".to_string(), v);
        }
    }
    if !obj.contains_key("tableWaterCoolerFan") {
        if let Some(v) = obj.get("waterCoolerFan").cloned() {
            obj.insert("tableWaterCoolerFan".to_string(), v);
        }
    }
    if !obj.contains_key("tablePump") {
        if let Some(v) = obj.get("pump").cloned() {
            obj.insert("tablePump".to_string(), v);
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
struct ApplyFanProfilesDto {
    #[serde(default, alias = "tableCPU")]
    cpu: Option<Vec<FanPoint>>,
    #[serde(default, alias = "tableGPU")]
    gpu: Option<Vec<FanPoint>>,
    #[serde(default, alias = "tableWaterCoolerFan")]
    water_cooler_fan: Option<Vec<FanPoint>>,
    #[serde(default, alias = "tablePump")]
    pump: Option<Vec<PumpPoint>>,
    #[serde(default, alias = "name")]
    fan_profile_id: Option<String>,
}

fn is_builtin_profile_id(id: &str) -> bool {
    [
        default_quiet_profile().id,
        default_office_profile().id,
        default_high_performance_profile().id,
        default_max_energy_save_profile().id,
    ]
    .iter()
    .any(|builtin| builtin == id)
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn builtin_fan_profiles() -> Vec<FanProfile> {
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
                (20, 0), (25, 0), (30, 0), (35, 0), (40, 0), (45, 0), (50, 0), (55, 0), (60, 0),
                (65, 20), (70, 28), (75, 40), (80, 53), (85, 65), (90, 83), (95, 96), (100, 100),
            ]),
            table_gpu: pts(&[
                (20, 0), (25, 0), (30, 0), (35, 0), (40, 0), (45, 0), (50, 0), (55, 0), (60, 10),
                (65, 24), (70, 34), (75, 46), (80, 58), (85, 70), (90, 91), (95, 95), (100, 100),
            ]),
            table_pump: default_pump(),
            table_water_cooler_fan: pts(&[
                (20, 0), (25, 0), (30, 0), (35, 0), (40, 0), (45, 0), (50, 0), (55, 0), (60, 5),
                (65, 22), (70, 31), (75, 43), (80, 56), (85, 68), (90, 87), (95, 96), (100, 100),
            ]),
        },
        FanProfile {
            id: "fan-quiet".to_string(),
            name: "Quiet [Built-in]".to_string(),
            table_cpu: pts(&[
                (20, 0), (25, 0), (30, 0), (35, 0), (40, 0), (45, 0), (50, 0), (55, 10), (60, 20),
                (65, 24), (70, 33), (75, 46), (80, 55), (85, 68), (90, 85), (95, 96), (100, 100),
            ]),
            table_gpu: pts(&[
                (20, 0), (25, 0), (30, 0), (35, 0), (40, 0), (45, 0), (50, 0), (55, 10), (60, 20),
                (65, 26), (70, 35), (75, 46), (80, 55), (85, 68), (90, 90), (95, 95), (100, 100),
            ]),
            table_pump: default_pump(),
            table_water_cooler_fan: pts(&[
                (20, 0), (25, 0), (30, 0), (35, 0), (40, 0), (45, 0), (50, 0), (55, 10), (60, 20),
                (65, 25), (70, 34), (75, 46), (80, 55), (85, 68), (90, 88), (95, 96), (100, 100),
            ]),
        },
        FanProfile {
            id: "fan-balanced".to_string(),
            name: "Balanced [Built-in]".to_string(),
            table_cpu: pts(&[
                (20, 0), (25, 0), (30, 0), (35, 0), (40, 0), (45, 0), (50, 17), (55, 25), (60, 31),
                (65, 38), (70, 50), (75, 55), (80, 65), (85, 78), (90, 88), (95, 96), (100, 100),
            ]),
            table_gpu: pts(&[
                (20, 0), (25, 0), (30, 0), (35, 0), (40, 0), (45, 0), (50, 17), (55, 25), (60, 31),
                (65, 38), (70, 50), (75, 55), (80, 65), (85, 78), (90, 90), (95, 95), (100, 100),
            ]),
            table_pump: default_pump(),
            table_water_cooler_fan: pts(&[
                (20, 0), (25, 0), (30, 0), (35, 0), (40, 0), (45, 0), (50, 17), (55, 25), (60, 31),
                (65, 38), (70, 50), (75, 55), (80, 65), (85, 78), (90, 89), (95, 96), (100, 100),
            ]),
        },
        FanProfile {
            id: "fan-cool".to_string(),
            name: "Cool [Built-in]".to_string(),
            table_cpu: pts(&[
                (20, 0), (25, 0), (30, 0), (35, 0), (40, 3), (45, 20), (50, 25), (55, 29), (60, 35),
                (65, 43), (70, 50), (75, 58), (80, 72), (85, 85), (90, 93), (95, 96), (100, 100),
            ]),
            table_gpu: pts(&[
                (20, 0), (25, 0), (30, 0), (35, 0), (40, 5), (45, 26), (50, 31), (55, 36), (60, 41),
                (65, 46), (70, 52), (75, 62), (80, 71), (85, 79), (90, 97), (95, 100), (100, 100),
            ]),
            table_pump: default_pump(),
            table_water_cooler_fan: pts(&[
                (20, 0), (25, 0), (30, 0), (35, 0), (40, 4), (45, 23), (50, 28), (55, 33), (60, 38),
                (65, 45), (70, 51), (75, 60), (80, 72), (85, 82), (90, 95), (95, 98), (100, 100),
            ]),
        },
        FanProfile {
            id: "fan-freezy".to_string(),
            name: "Freezy [Built-in]".to_string(),
            table_cpu: pts(&[
                (20, 20), (25, 20), (30, 21), (35, 23), (40, 26), (45, 30), (50, 40), (55, 40),
                (60, 45), (65, 50), (70, 55), (75, 60), (80, 73), (85, 85), (90, 91), (95, 96),
                (100, 100),
            ]),
            table_gpu: pts(&[
                (20, 25), (25, 25), (30, 25), (35, 25), (40, 30), (45, 35), (50, 40), (55, 45),
                (60, 50), (65, 60), (70, 65), (75, 70), (80, 75), (85, 85), (90, 95), (95, 98),
                (100, 100),
            ]),
            table_pump: default_pump(),
            table_water_cooler_fan: pts(&[
                (20, 23), (25, 23), (30, 23), (35, 24), (40, 28), (45, 33), (50, 40), (55, 43),
                (60, 48), (65, 55), (70, 60), (75, 65), (80, 74), (85, 85), (90, 93), (95, 97),
                (100, 100),
            ]),
        },
    ]
}

fn default_custom_fan_profiles() -> BTreeMap<String, FanProfile> {
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

fn fallback_odm_profile_code(vendor: DeviceVendor, name: &str) -> Option<i32> {
    let lower = name.trim().to_ascii_lowercase();
    match vendor {
        DeviceVendor::Clevo => match lower.as_str() {
            "quiet" | "silent" => Some(0),
            "power_saving" | "powersave" => Some(1),
            "performance" | "balanced" => Some(2),
            "entertainment" | "overboost" => Some(3),
            _ => lower.parse::<i32>().ok().filter(|v| (0..=3).contains(v)),
        },
        DeviceVendor::Uniwill => match lower.as_str() {
            "power_save" | "powersave" | "quiet" => Some(1),
            "balanced" | "enthusiast" | "office" => Some(2),
            "overboost" | "performance" | "high_performance" => Some(3),
            _ => lower.parse::<i32>().ok().filter(|v| (1..=3).contains(v)),
        },
        DeviceVendor::Unsupported => None,
    }
}

/// Explicit dependency bundle allowing `LiveUccdBackend` to run against either real Linux
/// interfaces (`new_production`) or injected isolated mocks (`new`) in integration tests.
pub struct UccdDependencies {
    pub device_name: String,
    pub platform: Arc<dyn PlatformBackend>,
    pub sysfs: Arc<dyn Sysfs>,
    pub lighting_transport: Arc<dyn LightingTransport>,
    pub config_manager: ConfigManager,
    pub sys_root: PathBuf,
    pub telemetry_config: TelemetryConfig,
    pub power_devices: PowerDeviceController,
    pub lid_policy: LidPolicyConfig,
}

/// Live backend orchestrating UCC hardware controllers, telemetry history, settings persistence,
/// cooling fail-safe control, lighting, and lid/wakeup policy (`M8.1`, `M8.3`, `M8.9`).
pub struct LiveUccdBackend {
    device_name: String,
    platform: Arc<dyn PlatformBackend>,
    sysfs: Arc<dyn Sysfs>,
    sys_root: PathBuf,
    capabilities: PlatformCapabilities,
    config_manager: Mutex<ConfigManager>,
    settings: RwLock<UccSettings>,
    config_recovered: AtomicBool,
    active_profile: RwLock<UccProfile>,
    temp_profile_name: RwLock<Option<String>>,
    custom_fan_profiles: RwLock<BTreeMap<String, FanProfile>>,

    history: Arc<MetricsHistoryStore>,
    telemetry: Arc<Mutex<TelemetryCollector>>,
    latest_telemetry: RwLock<Option<TelemetrySnapshot>>,

    fan_controller: Arc<Mutex<FanController>>,
    latest_fans: RwLock<Option<FanTickSnapshot>>,
    fan_control_enabled: AtomicBool,

    cooler_controller: Arc<Mutex<CoolerController>>,
    water_cooler_enabled: AtomicBool,
    water_cooler_available: AtomicBool,
    water_cooler_connected: AtomicBool,

    keyboard_ctrl: Arc<Mutex<KeyboardBacklightController>>,
    lightbar_ctrl: Arc<Mutex<LightbarController>>,
    power_devices: PowerDeviceController,
    power_policy: RwLock<PowerPolicy>,
    lid_controller: Mutex<LidController>,

    sensor_data_collection: AtomicBool,
    dgpu_d0_metrics: AtomicBool,
    shutting_down: AtomicBool,
    shutdown_wait_expired: AtomicBool,
    hardware_activation_pending: AtomicBool,
    hardware_activated: AtomicBool,
    in_flight_ops_wait: Arc<(Mutex<usize>, Condvar)>,
    nvml: Option<Arc<NvmlWrapper>>,
    nvidia_default_power_limit_w: Mutex<Option<(String, f64)>>,
    nvidia_power_offset: Mutex<Option<i32>>,
    mode_reapply_pending: AtomicBool,
    initial_cpu_defaults: Mutex<Option<CpuDefaultsSnapshot>>,
    profile_commit_epoch: AtomicU64,
    next_profile_tx_id: AtomicU64,
    tdp_write_seq: AtomicU64,
    odm_write_seq: AtomicU64,
    in_flight_tdp_baselines: Mutex<Vec<(u64, usize, i32)>>,
    in_flight_odm_baselines: Mutex<Vec<(u64, i32)>>,
    latest_committed_profile: Mutex<Option<CommittedProfileRecord>>,
    hw_reconcile_lock: Mutex<()>,
    disconnected_senders: Mutex<(HashSet<String>, VecDeque<String>)>,
    cpu_policy_applied_rev: Mutex<Option<(u64, ProfileState)>>,
    last_power_state: Mutex<Option<String>>,
    last_applied_profile_state: Mutex<Option<String>>,
    last_effective_mode: Mutex<Option<ProfileState>>,
    last_policy_json: Mutex<Option<String>>,
    last_power_device_revision: Mutex<Option<u64>>,
    signal_tx: broadcast::Sender<DbusSignalEvent>,
}

pub(crate) struct HardwareOpGuard {
    pair: Arc<(Mutex<usize>, Condvar)>,
}

impl Drop for HardwareOpGuard {
    fn drop(&mut self) {
        let mut count = self.pair.0.lock().unwrap_or_else(|e| e.into_inner());
        *count = count.saturating_sub(1);
        if *count == 0 {
            self.pair.1.notify_all();
        }
    }
}

#[derive(Debug, Clone)]
struct CommittedProfileRecord {
    profile: UccProfile,
    hw_tdps: Vec<Option<i32>>,
    hw_odm: Option<i32>,
}

struct SupersededUndo<'a> {
    hw_odm_prev: Option<i32>,
    wrote_odm: bool,
    prev_tdps: &'a [Option<i32>],
    tdps_written: usize,
    cpu_rollback: Vec<(PathBuf, String)>,
}

struct InFlightTxGuard<'a> {
    backend: &'a LiveUccdBackend,
    tx_id: u64,
}

impl Drop for InFlightTxGuard<'_> {
    fn drop(&mut self) {
        self.backend
            .in_flight_tdp_baselines
            .lock()
            .unwrap()
            .retain(|(id, _, _)| *id != self.tx_id);
        self.backend
            .in_flight_odm_baselines
            .lock()
            .unwrap()
            .retain(|(id, _)| *id != self.tx_id);
        self.backend.tdp_write_seq.fetch_add(1, Ordering::SeqCst);
        self.backend.odm_write_seq.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApplyOutcome {
    Committed,
    SupersededReconciled,
    StaleRevision,
    Failed,
}

struct CpuApplyResult {
    rollback: Vec<(PathBuf, String)>,
    fully_applied: bool,
}

impl LiveUccdBackend {
    /// Builds a backend from explicit dependencies (`MockPlatformBackend`, `MockSysfs`, temp config, etc.).
    /// Performs strictly read-only configuration/capability initialization with zero hardware writes
    /// until [`Self::activate_hardware_and_prime`] runs after D-Bus name acquisition (`M8-REVIEW-01`).
    pub fn new(deps: UccdDependencies) -> Self {
        let capabilities = deps.platform.capabilities();
        let (mut settings, recovered) = match deps.config_manager.resolve_read_path() {
            Some(path) => match ConfigManager::load_with_recovery(&path) {
                Ok((s, was_rec)) => (s, was_rec),
                Err(_) => (create_default_settings(), true),
            },
            None => (create_default_settings(), false),
        };

        let mut custom_fan_map = default_custom_fan_profiles();
        if let Some(ref saved_fans) = settings.custom_fan_profiles {
            for (id, entry) in saved_fans {
                if let Some((fp, _)) =
                    normalize_and_parse_custom_fan_profile(id, &entry.name, &entry.json)
                {
                    custom_fan_map.insert(id.clone(), fp);
                }
            }
        }

        let wc_supported = capabilities.device.water_cooler_supported;
        let power_policy = PowerPolicy::new();
        let initial_phys = Self::detect_physical_power_source_for(&deps.sysfs, &deps.sys_root);
        let initial_decision = power_policy.evaluate(initial_phys, false);
        let configured_initial_profile = settings
            .get_active_profile_for_mode(initial_decision.profile_state.as_str())
            .cloned()
            .unwrap_or_else(default_high_performance_profile);
        let active_profile = Self::build_unapplied_hardware_profile(
            deps.platform.as_ref(),
            deps.sysfs.as_ref(),
            &deps.sys_root,
            &capabilities,
        );

        let fan_enabled = settings
            .fan_control_enabled
            .unwrap_or(configured_initial_profile.fan.use_control);
        let base_fan_profile = Self::fan_profile_from_ucc_profile(&configured_initial_profile);
        let fan_controller = Arc::new(Mutex::new(FanController::new(
            deps.platform.as_ref(),
            base_fan_profile,
            configured_initial_profile.fan.same_speed,
        )));

        let history = Arc::new(MetricsHistoryStore::new());
        let nvml = deps.telemetry_config.nvml.clone();
        let telemetry = Arc::new(Mutex::new(TelemetryCollector::new(
            Arc::clone(&deps.sysfs),
            deps.telemetry_config,
            Arc::clone(&history),
        )));

        let keyboard_ctrl = KeyboardBacklightController::new(Arc::clone(&deps.lighting_transport));
        let lightbar_ctrl = LightbarController::new(Arc::clone(&deps.lighting_transport));

        if let Some(ref pd_val) = settings.power_devices {
            if PowerDeviceController::validate(pd_val).is_ok() {
                let _ = deps.power_devices.set_settings(pd_val.clone());
            } else {
                settings.power_devices = None;
            }
        }

        let cooler_ctrl = CoolerController::new();
        let (signal_tx, _) = broadcast::channel(256);

        Self {
            device_name: deps.device_name,
            platform: deps.platform,
            sysfs: deps.sysfs,
            sys_root: deps.sys_root,
            capabilities,
            config_manager: Mutex::new(deps.config_manager),
            settings: RwLock::new(settings),
            config_recovered: AtomicBool::new(recovered),
            active_profile: RwLock::new(active_profile),
            temp_profile_name: RwLock::new(None),
            custom_fan_profiles: RwLock::new(custom_fan_map),
            history,
            telemetry,
            latest_telemetry: RwLock::new(None),
            fan_controller,
            latest_fans: RwLock::new(None),
            fan_control_enabled: AtomicBool::new(fan_enabled),
            cooler_controller: Arc::new(Mutex::new(cooler_ctrl)),
            water_cooler_enabled: AtomicBool::new(false),
            water_cooler_available: AtomicBool::new(wc_supported),
            water_cooler_connected: AtomicBool::new(false),
            keyboard_ctrl: Arc::new(Mutex::new(keyboard_ctrl)),
            lightbar_ctrl: Arc::new(Mutex::new(lightbar_ctrl)),
            power_devices: deps.power_devices,
            power_policy: RwLock::new(power_policy),
            lid_controller: Mutex::new(LidController::new(deps.lid_policy)),
            sensor_data_collection: AtomicBool::new(true),
            dgpu_d0_metrics: AtomicBool::new(true),
            shutting_down: AtomicBool::new(false),
            shutdown_wait_expired: AtomicBool::new(false),
            hardware_activation_pending: AtomicBool::new(false),
            hardware_activated: AtomicBool::new(false),
            in_flight_ops_wait: Arc::new((Mutex::new(0), Condvar::new())),
            nvml,
            nvidia_default_power_limit_w: Mutex::new(None),
            nvidia_power_offset: Mutex::new(None),
            mode_reapply_pending: AtomicBool::new(false),
            initial_cpu_defaults: Mutex::new(None),
            profile_commit_epoch: AtomicU64::new(0),
            next_profile_tx_id: AtomicU64::new(0),
            tdp_write_seq: AtomicU64::new(0),
            odm_write_seq: AtomicU64::new(0),
            in_flight_tdp_baselines: Mutex::new(Vec::new()),
            in_flight_odm_baselines: Mutex::new(Vec::new()),
            latest_committed_profile: Mutex::new(None),
            hw_reconcile_lock: Mutex::new(()),
            disconnected_senders: Mutex::new((HashSet::new(), VecDeque::new())),
            cpu_policy_applied_rev: Mutex::new(None),
            last_power_state: Mutex::new(None),
            last_applied_profile_state: Mutex::new(None),
            last_effective_mode: Mutex::new(None),
            last_policy_json: Mutex::new(None),
            last_power_device_revision: Mutex::new(None),
            signal_tx,
        }
    }

    /// Probes real Linux hardware and system paths, degrading gracefully when optional drivers
    /// (`/dev/tuxedo_io`, NVML, LED sysfs) are absent.
    pub fn new_production() -> Self {
        let sysfs: Arc<dyn Sysfs> = Arc::new(RealSysfs);
        let dmi = DmiInfo::read_from_sysfs(sysfs.as_ref());
        let platform: Arc<dyn PlatformBackend> = match TuxedoIoDevice::open(TuxedoIoDevice::DEFAULT_PATH)
        {
            Ok(dev) => Arc::new(TuxedoIoBackend::new(dev, Some(&dmi), Some(sysfs.as_ref()))),
            Err(_) => {
                let hw = MockHardwareIo::new();
                hw.set_available(false);
                let mock = MockPlatformBackend::new(PlatformCapabilities {
                    backend: ucc_hardware::PlatformBackendKind::Unsupported,
                    device: ucc_hardware::detect_device_capabilities(&hw, Some(&dmi), Some(sysfs.as_ref())),
                    tdp_channels: 0,
                    perf_profiles_available: 0,
                });
                mock.set_available(false);
                Arc::new(mock)
            }
        };

        let manufacturer = classify_manufacturer(&dmi.sys_vendor, &dmi.board_vendor);
        let model = build_laptop_model(platform.capabilities().device.device_id, manufacturer, &dmi);

        let nvml = NvmlWrapper::new(NvidiaPowerGuard::default()).ok().map(Arc::new);
        let telemetry_config = TelemetryConfig {
            sys_root: PathBuf::from("/"),
            nvml,
            ..Default::default()
        };

        Self::new(UccdDependencies {
            device_name: model,
            platform,
            sysfs: Arc::clone(&sysfs),
            lighting_transport: Arc::new(SysfsLedTransport::default()),
            config_manager: ConfigManager::new(),
            sys_root: PathBuf::from("/"),
            telemetry_config,
            power_devices: PowerDeviceController::default(),
            lid_policy: LidPolicyConfig::default(),
        })
    }

    fn fan_profile_from_ucc_profile(p: &UccProfile) -> FanProfile {
        let mut fp = FanProfile::new(&p.fan.fan_profile, &p.fan.fan_profile);
        fp.table_cpu = p.fan.table_cpu.clone();
        fp.table_gpu = p.fan.table_gpu.clone();
        fp.table_pump = p.fan.table_pump.clone();
        fp.table_water_cooler_fan = p.fan.table_water_cooler_fan.clone();
        fp
    }

    fn parse_fan_points_json(points_json: &str) -> Option<Vec<FanPoint>> {
        serde_json::from_str::<Vec<FanPoint>>(points_json).ok()
    }

    fn detect_physical_power_source_for(sysfs: &Arc<dyn Sysfs>, sys_root: &Path) -> ProfileState {
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
        // Matches uccd/inc/StateUtils.hpp: Type-C takes priority over simultaneous Mains.
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

    fn discover_all_cpu_dirs_for(sysfs: &dyn Sysfs, sys_root: &Path) -> Vec<(u32, PathBuf)> {
        let cpu_dir = sys_root.join("sys/devices/system/cpu");
        let Ok(entries) = sysfs.read_dir(&cpu_dir) else {
            return Vec::new();
        };
        let mut cpus: Vec<(u32, PathBuf)> = entries
            .into_iter()
            .filter_map(|entry| {
                let name = entry.file_name()?.to_str()?;
                let idx_str = name.strip_prefix("cpu")?;
                if idx_str.is_empty() || !idx_str.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                let idx = idx_str.parse::<u32>().ok()?;
                Some((idx, entry))
            })
            .collect();
        cpus.sort_by_key(|(idx, _)| *idx);
        cpus
    }

    fn discover_all_cpu_dirs(&self) -> Vec<(u32, PathBuf)> {
        Self::discover_all_cpu_dirs_for(self.sysfs.as_ref(), &self.sys_root)
    }

    fn odm_profile_name_for_code(capabilities: &PlatformCapabilities, code: i32) -> Option<String> {
        let names = match capabilities.device.vendor {
            DeviceVendor::Clevo if capabilities.perf_profiles_available > 0 => vec![
                "quiet".to_string(),
                "power_saving".to_string(),
                "performance".to_string(),
                "entertainment".to_string(),
            ],
            DeviceVendor::Uniwill if capabilities.perf_profiles_available >= 2 => {
                let mut v = vec!["power_save".to_string(), "enthusiast".to_string()];
                if capabilities.perf_profiles_available >= 3 {
                    v.push("overboost".to_string());
                }
                v
            }
            _ => Vec::new(),
        };
        names
            .into_iter()
            .find(|n| fallback_odm_profile_code(capabilities.device.vendor, n) == Some(code))
    }

    fn populate_profile_from_hw_readback(
        profile: &mut UccProfile,
        platform: &dyn PlatformBackend,
        sysfs: &dyn Sysfs,
        sys_root: &Path,
        capabilities: &PlatformCapabilities,
        preserve_empty_tdps: bool,
    ) {
        if platform.is_available() && capabilities.tdp_channels > 0 {
            if !preserve_empty_tdps || !profile.odm_power_limits.is_empty() {
                let target_len = if preserve_empty_tdps {
                    profile.odm_power_limits.len().min(capabilities.tdp_channels)
                } else {
                    capabilities.tdp_channels
                };
                let mut read_tdps = Vec::with_capacity(target_len);
                let mut all_ok = true;
                for idx in 0..target_len {
                    match platform.read_tdp(idx) {
                        Ok(w) => match u32::try_from(w) {
                            Ok(u) => read_tdps.push(u),
                            Err(_) => {
                                all_ok = false;
                                break;
                            }
                        },
                        Err(_) => {
                            all_ok = false;
                            break;
                        }
                    }
                }
                if all_ok || !read_tdps.is_empty() {
                    profile.odm_power_limits = read_tdps;
                } else if preserve_empty_tdps {
                    profile.odm_power_limits.clear();
                }
            }
            if let Ok(code) = platform.read_perf_profile() {
                if let Some(name) = Self::odm_profile_name_for_code(capabilities, code) {
                    profile.odm_profile_name = Some(name);
                }
            }
        }
        let cpu_root = sys_root.join("sys/devices/system/cpu");
        if let Ok(nt) = sysfs.read(&cpu_root.join("intel_pstate/no_turbo")) {
            profile.cpu.no_turbo = nt.trim() == "1";
        }
        if let Ok(hwp) = sysfs.read(&cpu_root.join("intel_pstate/hwp_dynamic_boost")) {
            profile.cpu.hwp_dynamic_boost = hwp.trim() == "1";
        }
        if let Some((_, first_cpu)) = Self::discover_all_cpu_dirs_for(sysfs, sys_root)
            .into_iter()
            .next()
        {
            let cpufreq = first_cpu.join("cpufreq");
            if let Ok(gov) = sysfs.read(&cpufreq.join("scaling_governor")) {
                let g = gov.trim();
                if !g.is_empty() {
                    profile.cpu.governor = g.to_string();
                }
            }
            if let Ok(epp) = sysfs.read(&cpufreq.join("energy_performance_preference")) {
                let e = epp.trim();
                if !e.is_empty() {
                    profile.cpu.energy_performance_preference = e.to_string();
                }
            }
        }
    }

    fn build_unapplied_hardware_profile(
        platform: &dyn PlatformBackend,
        sysfs: &dyn Sysfs,
        sys_root: &Path,
        capabilities: &PlatformCapabilities,
    ) -> UccProfile {
        let mut profile = UccProfile::new("unapplied", "Hardware Baseline (Unapplied)");
        profile.description =
            "Confirmed hardware readback (configured profile not yet applied)".to_string();
        Self::populate_profile_from_hw_readback(
            &mut profile,
            platform,
            sysfs,
            sys_root,
            capabilities,
            false,
        );
        profile
    }

    fn discover_cpu_dirs(&self) -> Vec<PathBuf> {
        self.discover_all_cpu_dirs()
            .into_iter()
            .filter_map(|(_, entry)| {
                let is_online = match self.sysfs.read(&entry.join("online")) {
                    Ok(v) => v.trim() == "1",
                    Err(_) => true,
                };
                is_online.then_some(entry)
            })
            .collect()
    }

    fn read_online_cpu_count(&self) -> i32 {
        self.discover_cpu_dirs().len().max(1) as i32
    }

    fn detect_physical_power_source(&self) -> ProfileState {
        Self::detect_physical_power_source_for(&self.sysfs, &self.sys_root)
    }

    fn current_decision(&self) -> ucc_core::PolicyDecision {
        let src = self.detect_physical_power_source();
        let wc = self.water_cooler_connected.load(Ordering::SeqCst);
        self.power_policy.read().unwrap().evaluate(src, wc)
    }

    fn nested_sample_value(ts_ms: i64, data: i32) -> OwnedValue {
        let mut inner = HashMap::<String, OwnedValue>::new();
        inner.insert("timestamp".to_string(), OwnedValue::from(ts_ms));
        inner.insert("data".to_string(), OwnedValue::from(data));
        OwnedValue::try_from(ZValue::from(inner))
            .expect("nested sample dict converts to OwnedValue")
    }

    fn fan_map(ts_ms: i64, temp: i32, speed: i32) -> HashMap<String, OwnedValue> {
        let mut m = HashMap::new();
        m.insert("temp".to_string(), Self::nested_sample_value(ts_ms, temp));
        m.insert("speed".to_string(), Self::nested_sample_value(ts_ms, speed));
        m
    }

    /// Subscribes to asynchronous D-Bus signal events emitted by this backend.
    pub fn subscribe_signals(&self) -> broadcast::Receiver<DbusSignalEvent> {
        self.signal_tx.subscribe()
    }

    /// Marks the backend as shutting down so periodic/late RPC hardware writes abort cleanly.
    pub fn begin_shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
    }

    /// Marks that `DaemonRuntime::shutdown` timed out waiting for in-flight hardware work and
    /// returned control to its caller, suppressing late stalled RPC rollback writes.
    pub fn mark_shutdown_wait_expired(&self) {
        self.shutdown_wait_expired.store(true, Ordering::SeqCst);
    }

    fn emit_signal(&self, event: DbusSignalEvent) {
        let _ = self.signal_tx.send(event);
    }

    fn emit_profile_changed(&self, profile: &UccProfile) {
        self.emit_signal(DbusSignalEvent::ProfileChanged {
            profile_id: profile.id.clone(),
            keyboard_profile_id: profile
                .selected_keyboard_profile
                .clone()
                .unwrap_or_else(|| profile.keyboard.keyboard_profile_id.clone()),
            fan_profile_id: profile.fan.fan_profile.clone(),
        });
    }

    /// Returns whether [`Self::activate_hardware`] has ever been invoked on this backend.
    pub fn was_hardware_activated(&self) -> bool {
        self.hardware_activated.load(Ordering::SeqCst)
    }

    /// Sets whether startup hardware activation is still pending (blocking mutating RPCs until ready).
    pub fn set_activation_pending(&self, pending: bool) {
        self.hardware_activation_pending
            .store(pending, Ordering::SeqCst);
    }

    fn enter_hardware_op(&self) -> Option<HardwareOpGuard> {
        if ucc_config::is_read_only_preview() {
            return None;
        }
        let mut count = self
            .in_flight_ops_wait
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if self.shutting_down.load(Ordering::SeqCst)
            || self.hardware_activation_pending.load(Ordering::SeqCst)
        {
            return None;
        }
        *count += 1;
        Some(HardwareOpGuard {
            pair: Arc::clone(&self.in_flight_ops_wait),
        })
    }

    fn record_in_flight_tdp_baseline(&self, tx_id: u64, idx: usize, prev_w: i32) {
        let mut guard = self.in_flight_tdp_baselines.lock().unwrap();
        if !guard.iter().any(|(id, ch, _)| *id == tx_id && *ch == idx) {
            guard.push((tx_id, idx, prev_w));
        }
        self.tdp_write_seq.fetch_add(1, Ordering::SeqCst);
    }

    fn record_in_flight_odm_baseline(&self, tx_id: u64, prev_code: i32) {
        let mut guard = self.in_flight_odm_baselines.lock().unwrap();
        if !guard.iter().any(|(id, _)| *id == tx_id) {
            guard.push((tx_id, prev_code));
        }
        self.odm_write_seq.fetch_add(1, Ordering::SeqCst);
    }

    fn lookup_other_in_flight_tdp(&self, tx_id: u64, idx: usize) -> Option<i32> {
        let guard = self.in_flight_tdp_baselines.lock().unwrap();
        guard
            .iter()
            .filter(|(id, ch, _)| *id != tx_id && *ch == idx)
            .min_by_key(|(id, _, _)| *id)
            .map(|(_, _, w)| *w)
    }

    fn snapshot_unmanaged_tdp_baseline(&self, tx_id: u64, idx: usize) -> Option<i32> {
        for _ in 0..4 {
            let seq_before = self.tdp_write_seq.load(Ordering::SeqCst);
            if let Some(w) = self.lookup_other_in_flight_tdp(tx_id, idx) {
                return Some(w);
            }
            let live = self.platform.read_tdp(idx).ok();
            if let Some(w) = self.lookup_other_in_flight_tdp(tx_id, idx) {
                return Some(w);
            }
            if self.tdp_write_seq.load(Ordering::SeqCst) == seq_before {
                return live;
            }
        }
        self.lookup_other_in_flight_tdp(tx_id, idx)
            .or_else(|| self.platform.read_tdp(idx).ok())
    }

    fn lookup_other_in_flight_odm(&self, tx_id: u64) -> Option<i32> {
        let guard = self.in_flight_odm_baselines.lock().unwrap();
        guard
            .iter()
            .filter(|(id, _)| *id != tx_id)
            .min_by_key(|(id, _)| *id)
            .map(|(_, c)| *c)
    }

    fn snapshot_unmanaged_odm_baseline(&self, tx_id: u64) -> Option<i32> {
        for _ in 0..4 {
            let seq_before = self.odm_write_seq.load(Ordering::SeqCst);
            if let Some(code) = self.lookup_other_in_flight_odm(tx_id) {
                return Some(code);
            }
            let live = self.platform.read_perf_profile().ok();
            if let Some(code) = self.lookup_other_in_flight_odm(tx_id) {
                return Some(code);
            }
            if self.odm_write_seq.load(Ordering::SeqCst) == seq_before {
                return live;
            }
        }
        self.lookup_other_in_flight_odm(tx_id)
            .or_else(|| self.platform.read_perf_profile().ok())
    }

    /// Registers a background worker or startup activation operation with the shared in-flight
    /// hardware operation tracker so shutdown waits for active worker I/O before releasing
    /// device ownership (`M8-LIFE-01`).
    pub(crate) fn enter_worker_hw_op(&self) -> Option<HardwareOpGuard> {
        let mut count = self
            .in_flight_ops_wait
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if self.shutting_down.load(Ordering::SeqCst) {
            return None;
        }
        *count += 1;
        Some(HardwareOpGuard {
            pair: Arc::clone(&self.in_flight_ops_wait),
        })
    }

    /// Blocks until every active external RPC or background worker hardware op exits.
    pub fn wait_in_flight_hw_ops(&self) {
        let (lock, cvar) = &*self.in_flight_ops_wait;
        let mut count = lock.lock().unwrap_or_else(|e| e.into_inner());
        while *count > 0 {
            count = cvar.wait(count).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn capture_initial_cpu_defaults(&self) {
        let all_cpus = self.discover_all_cpu_dirs();
        if all_cpus.is_empty() {
            return;
        }
        let cpu_root = self.sys_root.join("sys/devices/system/cpu");
        let mut defaults = self.initial_cpu_defaults.lock().unwrap();
        if defaults.is_none() {
            let mut per_cpu = BTreeMap::new();
            for (_, dir) in &all_cpus {
                let cpufreq = dir.join("cpufreq");
                per_cpu.insert(
                    dir.clone(),
                    PerCpuSnapshot {
                        online: self
                            .sysfs
                            .read(&dir.join("online"))
                            .ok()
                            .map(|s| s.trim().to_string()),
                        governor: self
                            .sysfs
                            .read(&cpufreq.join("scaling_governor"))
                            .ok()
                            .map(|s| s.trim().to_string()),
                        epp: self
                            .sysfs
                            .read(&cpufreq.join("energy_performance_preference"))
                            .ok()
                            .map(|s| s.trim().to_string()),
                        min_freq_khz: self
                            .sysfs
                            .read(&cpufreq.join("scaling_min_freq"))
                            .ok()
                            .map(|s| s.trim().to_string()),
                        max_freq_khz: self
                            .sysfs
                            .read(&cpufreq.join("scaling_max_freq"))
                            .ok()
                            .map(|s| s.trim().to_string()),
                    },
                );
            }
            *defaults = Some(CpuDefaultsSnapshot {
                per_cpu,
                no_turbo: self
                    .sysfs
                    .read(&cpu_root.join("intel_pstate/no_turbo"))
                    .ok()
                    .map(|s| s.trim().to_string()),
                hwp_dynamic_boost: self
                    .sysfs
                    .read(&cpu_root.join("intel_pstate/hwp_dynamic_boost"))
                    .ok()
                    .map(|s| s.trim().to_string()),
            });
        }
    }

    /// Primes telemetry and fan snapshots strictly read-only with ZERO hardware/sysfs/config writes
    /// before bus name acquisition (`M8-PROD-02`), without capturing restore CPU defaults or
    /// claiming unapplied configured profiles as active (`M8-COMMIT-04`, `05`).
    pub fn prime_read_only(&self) {
        if self.sensor_data_collection.load(Ordering::SeqCst)
            && self.latest_telemetry.read().unwrap().is_none()
        {
            let _ = self.poll_telemetry_once();
        }
        if self.latest_committed_profile.lock().unwrap().is_none() {
            *self.active_profile.write().unwrap() = Self::build_unapplied_hardware_profile(
                self.platform.as_ref(),
                self.sysfs.as_ref(),
                &self.sys_root,
                &self.capabilities,
            );
        }

        if self.latest_fans.read().unwrap().is_none() {
            let num_fans = self.capabilities.device.number_of_fans;
            let mut temps = Vec::with_capacity(num_fans);
            let mut speeds = Vec::with_capacity(num_fans);
            for i in 0..num_fans {
                temps.push(self.platform.read_fan_temp(i).ok());
                speeds.push(self.platform.read_fan_speed_percent(i).ok());
            }
            *self.latest_fans.write().unwrap() = Some(FanTickSnapshot {
                timestamp_ms: unix_ms(),
                temps_c: temps,
                calculated_speeds: speeds.clone(),
                speeds_percent: speeds,
                fallback_auto: !self.fan_control_enabled.load(Ordering::SeqCst),
            });
        }
    }

    /// Captures startup CPU baseline strictly AFTER predecessor handoff completes, applies saved
    /// lighting, activates initial power-source profile + PowerDeviceController, and starts
    /// hardware fan control AFTER exclusive D-Bus name ownership (`M8-PROD-01`, `02`, `M8-COMMIT-05`).
    pub fn activate_hardware(&self) {
        if ucc_config::is_read_only_preview() {
            self.hardware_activated.store(true, Ordering::SeqCst);
            self.prime_read_only();
            if !self.shutting_down.load(Ordering::SeqCst) {
                self.hardware_activation_pending
                    .store(false, Ordering::SeqCst);
            }
            return;
        }
        let Some(_worker_guard) = self.enter_worker_hw_op() else {
            return;
        };
        self.hardware_activated.store(true, Ordering::SeqCst);
        self.capture_initial_cpu_defaults();
        self.prime_read_only();

        let (kb_states, lb_val) = {
            let s = self.settings.read().unwrap();
            (s.keyboard_backlight_states.clone(), s.lightbar.clone())
        };

        if let Ok(mut kb) = self.keyboard_ctrl.lock() {
            let _ = kb.init();
            if let Some(ref states) = kb_states {
                if !states.is_empty() && kb.is_available() {
                    let _ = kb.apply_states(states);
                }
            }
        }

        if let Some(ref lb_cfg) = lb_val {
            if let Ok(mut lb) = self.lightbar_ctrl.lock() {
                let _ = lb.configure(lb_cfg);
            }
        }

        self.reconcile_power_state(true);
        let _ = self.tick_fans_once();
        if !self.shutting_down.load(Ordering::SeqCst) {
            self.hardware_activation_pending
                .store(false, Ordering::SeqCst);
        }
    }

    /// Convenience helper combining [`Self::prime_read_only`] and [`Self::activate_hardware`].
    pub fn activate_hardware_and_prime(&self) {
        self.prime_read_only();
        self.activate_hardware();
    }

    /// Reconciles active profile, `PowerDeviceController`, `LightbarController`, and D-Bus signals
    /// with physical power supply and `PowerPolicy` state (`M8-RECHECK-04`, `09`, `M8-PROD-09`).
    pub fn reconcile_power_state(&self, force_apply: bool) {
        if self.shutting_down.load(Ordering::SeqCst) || ucc_config::is_read_only_preview() {
            return;
        }
        let decision = self.current_decision();
        let state_str = decision.profile_state.as_str().to_string();
        let effective_mode = decision.effective;
        let policy_json = decision.status.to_string();
        let pd_rev = self.power_devices.revision();

        let prev_state = self.last_power_state.lock().unwrap().clone();
        let state_changed = prev_state.as_deref() != Some(state_str.as_str());
        let prev_applied_state = self.last_applied_profile_state.lock().unwrap().clone();
        let profile_pending =
            force_apply || state_changed || prev_applied_state.as_deref() != Some(state_str.as_str());
        let prev_effective = *self.last_effective_mode.lock().unwrap();
        let effective_changed = prev_effective != Some(effective_mode);
        let prev_policy = self.last_policy_json.lock().unwrap().clone();
        let policy_changed = prev_policy.as_deref() != Some(policy_json.as_str());
        let prev_pd_rev = *self.last_power_device_revision.lock().unwrap();
        let pd_changed = prev_pd_rev != Some(pd_rev);
        let cpu_overlay_pending =
            *self.cpu_policy_applied_rev.lock().unwrap() != Some((pd_rev, effective_mode));

        if force_apply || effective_changed {
            let lb_mode = match effective_mode {
                ProfileState::Ac | ProfileState::Wc => LightbarPowerMode::Ac,
                ProfileState::TypeC => LightbarPowerMode::TypeC,
                ProfileState::Bat => LightbarPowerMode::Bat,
            };
            if let Ok(mut lb) = self.lightbar_ctrl.lock() {
                lb.set_power_mode(lb_mode);
            }
            *self.last_effective_mode.lock().unwrap() = Some(effective_mode);
        }

        if state_changed {
            *self.last_power_state.lock().unwrap() = Some(state_str.clone());
            if prev_state.is_some() {
                self.emit_signal(DbusSignalEvent::PowerStateChanged {
                    state: state_str.clone(),
                });
            }
        }

        let mut applied_profile_now = false;
        if profile_pending {
            if state_changed && !force_apply {
                *self.temp_profile_name.write().unwrap() = None;
            }
            for _ in 0..3 {
                if self.shutting_down.load(Ordering::SeqCst) {
                    break;
                }
                let apply_state = self.current_decision().profile_state.as_str().to_string();
                let (target_profile, expected_stored) = {
                    let s = self.settings.read().unwrap();
                    match s.get_active_profile_for_mode(&apply_state) {
                        Some(p) => (p.clone(), Some(p.clone())),
                        None => (default_high_performance_profile(), None),
                    }
                };
                match self.apply_profile_internal(
                    &target_profile,
                    prev_state.is_some() || state_changed,
                    None,
                    None,
                    None,
                    expected_stored.as_ref(),
                ) {
                    ApplyOutcome::Committed => {
                        let decision_matches =
                            self.current_decision().profile_state.as_str() == apply_state;
                        let stored_matches = expected_stored.as_ref().is_none_or(|exp| {
                            self.settings
                                .read()
                                .unwrap()
                                .get_active_profile_for_mode(&apply_state)
                                .is_some_and(|cur| cur == exp)
                        });
                        if decision_matches && stored_matches {
                            *self.last_applied_profile_state.lock().unwrap() = Some(apply_state);
                            applied_profile_now = true;
                            break;
                        }
                        *self.last_applied_profile_state.lock().unwrap() = None;
                        self.mode_reapply_pending.store(true, Ordering::SeqCst);
                    }
                    ApplyOutcome::SupersededReconciled => {
                        applied_profile_now = true;
                        break;
                    }
                    ApplyOutcome::StaleRevision => {
                        *self.last_applied_profile_state.lock().unwrap() = None;
                        self.mode_reapply_pending.store(true, Ordering::SeqCst);
                    }
                    ApplyOutcome::Failed => {
                        *self.last_applied_profile_state.lock().unwrap() = None;
                        self.mode_reapply_pending.store(true, Ordering::SeqCst);
                        break;
                    }
                }
            }
        }

        if self.shutting_down.load(Ordering::SeqCst) {
            return;
        }

        if force_apply || effective_changed || pd_changed || cpu_overlay_pending {
            if force_apply || effective_changed || pd_changed {
                self.power_devices.apply(effective_mode.mode_name());
                *self.last_power_device_revision.lock().unwrap() = Some(pd_rev);
                if prev_pd_rev.is_some() && pd_changed {
                    self.emit_signal(DbusSignalEvent::PowerDeviceStatusChanged {
                        json: self.power_devices.status().to_string(),
                    });
                }
            }
            if !applied_profile_now || cpu_overlay_pending {
                if self.shutting_down.load(Ordering::SeqCst) {
                    return;
                }
                let current_active = self.active_profile.read().unwrap().clone();
                match self.apply_cpu_settings(&current_active) {
                    Ok(res) if res.fully_applied => {
                        *self.cpu_policy_applied_rev.lock().unwrap() =
                            Some((pd_rev, effective_mode));
                    }
                    _ => {
                        *self.cpu_policy_applied_rev.lock().unwrap() = None;
                    }
                }
            }
        }

        if policy_changed {
            *self.last_policy_json.lock().unwrap() = Some(policy_json.clone());
            if prev_policy.is_some() {
                self.emit_signal(DbusSignalEvent::PowerPolicyChanged { json: policy_json });
            }
        }
    }

    /// Records a departed D-Bus connection and removes its in-memory power requests without
    /// running hardware reconciliation (for async callers that offload reconciliation).
    pub(crate) fn record_dbus_client_disconnected(&self, owner: &str) -> bool {
        if owner.is_empty() {
            return false;
        }
        if owner.starts_with(':') {
            let mut disconnected = self.disconnected_senders.lock().unwrap();
            if disconnected.0.insert(owner.to_string()) {
                disconnected.1.push_back(owner.to_string());
                if disconnected.1.len() > 1024 {
                    if let Some(oldest) = disconnected.1.pop_front() {
                        disconnected.0.remove(&oldest);
                    }
                }
            }
        }
        let mut policy = self.power_policy.write().unwrap();
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

    /// Advances an active lightbar animation deterministically on periodic daemon ticks (`M8-REVIEW-18`).
    pub fn tick_lightbar_animation(&self, started_at: &mut Option<Instant>) {
        if self.shutting_down.load(Ordering::SeqCst) || ucc_config::is_read_only_preview() {
            return;
        }
        if let Ok(mut lb) = self.lightbar_ctrl.lock() {
            if lb.is_animating() {
                let origin = started_at.get_or_insert_with(Instant::now);
                let elapsed_ms = origin.elapsed().as_millis() as u64;
                lb.advance(elapsed_ms);
                if !lb.is_animating() {
                    *started_at = None;
                }
            } else {
                *started_at = None;
            }
        }
    }

    fn read_whitespace_tokens(&self, path: &Path) -> Vec<String> {
        self.sysfs
            .read(path)
            .map(|raw| raw.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default()
    }

    fn discover_backlight_device(&self) -> Option<(PathBuf, i32)> {
        let bl_dir = self.sys_root.join("sys/class/backlight");
        let Ok(mut entries) = self.sysfs.read_dir(&bl_dir) else {
            return None;
        };
        entries.sort();
        for entry in entries {
            let Ok(max_raw) = self.sysfs.read(&entry.join("max_brightness")) else {
                continue;
            };
            let Ok(max_val) = max_raw.trim().parse::<i32>() else {
                continue;
            };
            if max_val > 0 && self.sysfs.read(&entry.join("brightness")).is_ok() {
                return Some((entry, max_val));
            }
        }
        None
    }

    fn discover_fn_lock_path(&self) -> Option<PathBuf> {
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

    fn charging_profile_paths(&self) -> Option<(PathBuf, PathBuf)> {
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

    fn charging_priority_paths(&self) -> Option<(PathBuf, PathBuf)> {
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

    fn nvidia_ctgp_sysfs_path(&self) -> Option<PathBuf> {
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

    fn discover_battery_dir(&self) -> Option<PathBuf> {
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

    /// Checks whether a specific CPU directory supports the requested governor and EPP values,
    /// matching `CpuWorker::apply_cpu_profile` tolerance for unsupported driver presets.
    fn check_cpu_support_for_dir(
        &self,
        cpu_dir: &Path,
        cpu: &ucc_core::ProfileCpu,
    ) -> (bool, bool, bool, bool) {
        let cpufreq = cpu_dir.join("cpufreq");
        let mut gov_ok =
            !cpu.governor.is_empty() && self.sysfs.exists(&cpufreq.join("scaling_governor"));
        let mut gov_skipped = false;
        if gov_ok {
            let avail_path = cpufreq.join("scaling_available_governors");
            if self.sysfs.exists(&avail_path) {
                let avail = self.read_whitespace_tokens(&avail_path);
                if !avail.iter().any(|g| g == &cpu.governor) {
                    warn!(
                        "Skipping unavailable CPU governor '{}' for {} (available: {:?})",
                        cpu.governor,
                        cpu_dir.display(),
                        avail
                    );
                    gov_ok = false;
                    gov_skipped = true;
                }
            }
        }
        let mut epp_ok = !cpu.energy_performance_preference.is_empty()
            && self
                .sysfs
                .exists(&cpufreq.join("energy_performance_preference"));
        let mut epp_skipped = false;
        if epp_ok {
            let avail_path = cpufreq.join("energy_performance_available_preferences");
            if self.sysfs.exists(&avail_path) {
                let avail = self.read_whitespace_tokens(&avail_path);
                if !avail
                    .iter()
                    .any(|e| e == &cpu.energy_performance_preference)
                {
                    warn!(
                        "Skipping unavailable CPU EPP '{}' for {} (available: {:?})",
                        cpu.energy_performance_preference,
                        cpu_dir.display(),
                        avail
                    );
                    epp_ok = false;
                    epp_skipped = true;
                }
            }
        }
        (gov_ok, gov_skipped, epp_ok, epp_skipped)
    }

    fn write_cpu_with_rollback(
        &self,
        path: &Path,
        val: &str,
        rollback: &mut Vec<(PathBuf, String)>,
    ) -> Result<(), String> {
        if self.shutdown_wait_expired.load(Ordering::SeqCst) {
            return Err("Shutdown deadline expired".to_string());
        }
        let prev = self
            .sysfs
            .read(path)
            .map_err(|e| format!("Failed reading baseline {}: {e}", path.display()))?;
        if self.shutdown_wait_expired.load(Ordering::SeqCst) {
            return Err("Shutdown deadline expired".to_string());
        }
        self.sysfs
            .write(path, val)
            .map_err(|e| format!("Failed writing {}: {e}", path.display()))?;
        rollback.push((path.to_path_buf(), prev.trim().to_string()));
        Ok(())
    }

    #[must_use]
    fn rollback_cpu_writes(&self, writes: Vec<(PathBuf, String)>) -> bool {
        let mut ok = true;
        for (path, prev) in writes.into_iter().rev() {
            if self.shutdown_wait_expired.load(Ordering::SeqCst) {
                return false;
            }
            if let Err(e) = self.sysfs.write(&path, &prev) {
                warn!(
                    "Failed rolling back CPU sysfs node {}: {e}",
                    path.display()
                );
                ok = false;
            } else if let Ok(actual) = self.sysfs.read(&path) {
                if actual.trim() != prev.trim() {
                    warn!(
                        "CPU sysfs readback mismatch on {}: expected '{}', got '{}'",
                        path.display(),
                        prev.trim(),
                        actual.trim()
                    );
                    ok = false;
                }
            }
        }
        ok
    }

    fn apply_cpu_settings(&self, profile: &UccProfile) -> Result<CpuApplyResult, String> {
        let enabled = self
            .settings
            .read()
            .unwrap()
            .cpu_settings_enabled
            .unwrap_or(true);
        if !enabled {
            return Ok(CpuApplyResult {
                rollback: Vec::new(),
                fully_applied: true,
            });
        }

        let mode_name = self.current_decision().effective.mode_name();
        let mut overlaid = profile.clone();
        self.power_devices.overlay_cpu(&mut overlaid, mode_name);
        let cpu = &overlaid.cpu;

        let all_cpus = self.discover_all_cpu_dirs();
        if all_cpus.is_empty() {
            return Ok(CpuApplyResult {
                rollback: Vec::new(),
                fully_applied: true,
            });
        }

        let all_ids: std::collections::HashSet<u32> = all_cpus.iter().map(|(id, _)| *id).collect();
        let desired_online: std::collections::HashSet<u32> = if let Some(ref sel) = cpu.enabled_cores
        {
            if sel.is_empty() || !sel.contains(&0) {
                return Err("enabled_cores must be non-empty and include core 0".to_string());
            }
            for id in sel {
                if !all_ids.contains(id) {
                    return Err(format!("Requested CPU core {id} does not exist"));
                }
            }
            sel.iter().copied().collect()
        } else if let Some(count) = cpu.online_cores {
            if count == 0 || (count as usize) > all_cpus.len() {
                return Err(format!(
                    "online_cores ({count}) outside detected core count ({})",
                    all_cpus.len()
                ));
            }
            all_cpus
                .iter()
                .take(count as usize)
                .map(|(id, _)| *id)
                .collect()
        } else {
            all_ids
        };

        self.capture_initial_cpu_defaults();
        let cpu_root = self.sys_root.join("sys/devices/system/cpu");
        let mut rollback: Vec<(PathBuf, String)> = Vec::new();
        let mut fully_applied = true;
        let res = (|| -> Result<(), String> {
            // 1. Bring desired cores online first so cpufreq nodes are accessible.
            for (id, dir) in &all_cpus {
                if *id > 0 && desired_online.contains(id) {
                    let online_path = dir.join("online");
                    if self
                        .sysfs
                        .read(&online_path)
                        .is_ok_and(|v| v.trim() != "1")
                    {
                        self.write_cpu_with_rollback(&online_path, "1", &mut rollback)?;
                    }
                }
            }

            // 2. Apply turbo / HWP flags.
            let no_turbo_path = cpu_root.join("intel_pstate/no_turbo");
            if self.sysfs.exists(&no_turbo_path) {
                self.write_cpu_with_rollback(
                    &no_turbo_path,
                    if cpu.no_turbo { "1" } else { "0" },
                    &mut rollback,
                )?;
            }
            let hwp_path = cpu_root.join("intel_pstate/hwp_dynamic_boost");
            if self.sysfs.exists(&hwp_path) {
                self.write_cpu_with_rollback(
                    &hwp_path,
                    if cpu.hwp_dynamic_boost { "1" } else { "0" },
                    &mut rollback,
                )?;
            }

            // 3. Apply per-CPU governor, EPP, and scaling frequencies on enabled cores.
            for (id, dir) in &all_cpus {
                if !desired_online.contains(id) {
                    continue;
                }
                let (apply_gov, skipped_gov, apply_epp, skipped_epp) =
                    self.check_cpu_support_for_dir(dir, cpu);
                if skipped_gov || skipped_epp {
                    fully_applied = false;
                }
                let cpufreq = dir.join("cpufreq");
                if apply_gov {
                    self.write_cpu_with_rollback(
                        &cpufreq.join("scaling_governor"),
                        &cpu.governor,
                        &mut rollback,
                    )?;
                }
                if apply_epp {
                    self.write_cpu_with_rollback(
                        &cpufreq.join("energy_performance_preference"),
                        &cpu.energy_performance_preference,
                        &mut rollback,
                    )?;
                }
                if let Some(max_khz) = cpu.max_freq_khz {
                    let p = cpufreq.join("scaling_max_freq");
                    if self.sysfs.exists(&p) {
                        self.write_cpu_with_rollback(&p, &max_khz.to_string(), &mut rollback)?;
                    }
                }
                if let Some(min_khz) = cpu.min_freq_khz {
                    let p = cpufreq.join("scaling_min_freq");
                    if self.sysfs.exists(&p) {
                        self.write_cpu_with_rollback(&p, &min_khz.to_string(), &mut rollback)?;
                    }
                }
            }

            // 4. Offline non-selected cores (`id > 0`).
            for (id, dir) in &all_cpus {
                if *id > 0 && !desired_online.contains(id) {
                    let online_path = dir.join("online");
                    if self.sysfs.exists(&online_path) {
                        self.write_cpu_with_rollback(&online_path, "0", &mut rollback)?;
                    }
                }
            }

            Ok(())
        })();

        match res {
            Ok(()) => Ok(CpuApplyResult {
                rollback,
                fully_applied,
            }),
            Err(err) => {
                if self.shutdown_wait_expired.load(Ordering::SeqCst) {
                    return Err(err);
                }
                if !self.rollback_cpu_writes(rollback) {
                    if self.shutdown_wait_expired.load(Ordering::SeqCst) {
                        return Err(err);
                    }
                    self.sync_active_profile_from_hw_readback();
                    *self.last_applied_profile_state.lock().unwrap() = None;
                    *self.cpu_policy_applied_rev.lock().unwrap() = None;
                    self.mode_reapply_pending.store(true, Ordering::SeqCst);
                }
                Err(err)
            }
        }
    }

    #[must_use]
    fn write_sysfs_if_changed(&self, path: &Path, target: &str) -> bool {
        if self.shutdown_wait_expired.load(Ordering::SeqCst) {
            return false;
        }
        if !self.sysfs.exists(path) {
            return true;
        }
        if let Ok(cur) = self.sysfs.read(path) {
            if self.shutdown_wait_expired.load(Ordering::SeqCst) {
                return false;
            }
            if cur.trim() == target.trim() {
                return true;
            }
        }
        if self.shutdown_wait_expired.load(Ordering::SeqCst) {
            return false;
        }
        let _ = self.sysfs.write(path, target);
        true
    }

    #[must_use]
    fn apply_cpu_defaults_snapshot(&self, defaults: &CpuDefaultsSnapshot) -> bool {
        let cpu_root = self.sys_root.join("sys/devices/system/cpu");
        if let Some(ref nt) = defaults.no_turbo {
            if !self.write_sysfs_if_changed(&cpu_root.join("intel_pstate/no_turbo"), nt) {
                return false;
            }
        }
        if let Some(ref hwp) = defaults.hwp_dynamic_boost {
            if !self.write_sysfs_if_changed(&cpu_root.join("intel_pstate/hwp_dynamic_boost"), hwp)
            {
                return false;
            }
        }
        for (dir, snap) in &defaults.per_cpu {
            if snap.online.as_deref() == Some("1")
                && !self.write_sysfs_if_changed(&dir.join("online"), "1")
            {
                return false;
            }
            let cpufreq = dir.join("cpufreq");
            if let Some(ref gov) = snap.governor {
                if !self.write_sysfs_if_changed(&cpufreq.join("scaling_governor"), gov) {
                    return false;
                }
            }
            if let Some(ref epp) = snap.epp {
                if !self.write_sysfs_if_changed(&cpufreq.join("energy_performance_preference"), epp)
                {
                    return false;
                }
            }
            if let Some(ref max_khz) = snap.max_freq_khz {
                if !self.write_sysfs_if_changed(&cpufreq.join("scaling_max_freq"), max_khz) {
                    return false;
                }
            }
            if let Some(ref min_khz) = snap.min_freq_khz {
                if !self.write_sysfs_if_changed(&cpufreq.join("scaling_min_freq"), min_khz) {
                    return false;
                }
            }
            if snap.online.as_deref() == Some("0")
                && !self.write_sysfs_if_changed(&dir.join("online"), "0")
            {
                return false;
            }
        }
        true
    }

    #[must_use]
    fn restore_cpu_defaults(&self) -> bool {
        if self.shutdown_wait_expired.load(Ordering::SeqCst) {
            return false;
        }
        let Some(defaults) = self.initial_cpu_defaults.lock().unwrap().clone() else {
            return true;
        };
        if !self.apply_cpu_defaults_snapshot(&defaults) {
            return false;
        }
        *self.initial_cpu_defaults.lock().unwrap() = None;
        true
    }

    pub(crate) fn take_unrestored_cpu_defaults(&self) -> Option<CpuDefaultsSnapshot> {
        self.initial_cpu_defaults.lock().unwrap().take()
    }

    pub(crate) fn seed_inherited_cpu_defaults(&self, defaults: Option<CpuDefaultsSnapshot>) {
        let Some(snapshot) = defaults else {
            return;
        };
        {
            let mut slot = self.initial_cpu_defaults.lock().unwrap();
            if slot.is_none() {
                *slot = Some(snapshot.clone());
            }
        }
        if !ucc_config::is_read_only_preview()
            && !self.shutting_down.load(Ordering::SeqCst)
            && !self.shutdown_wait_expired.load(Ordering::SeqCst)
        {
            let _ = self.apply_cpu_defaults_snapshot(&snapshot);
        }
    }

    fn sync_active_profile_from_hw_readback(&self) {
        let mut active = self.active_profile.read().unwrap().clone();
        Self::populate_profile_from_hw_readback(
            &mut active,
            self.platform.as_ref(),
            self.sysfs.as_ref(),
            &self.sys_root,
            &self.capabilities,
            true,
        );
        *self.active_profile.write().unwrap() = active;
    }

    #[allow(clippy::too_many_arguments)]
    #[must_use]
    fn rollback_hardware(
        &self,
        hw_odm_prev: Option<i32>,
        wrote_odm: bool,
        prev_tdps: &[Option<i32>],
        tdps_to_restore: usize,
        cpu_rollback: Vec<(PathBuf, String)>,
        start_epoch: Option<u64>,
    ) -> bool {
        let tdp_count = if wrote_odm {
            prev_tdps.len()
        } else {
            tdps_to_restore.min(prev_tdps.len())
        };
        let superseded =
            || start_epoch.is_some_and(|e| self.profile_commit_epoch.load(Ordering::SeqCst) != e);
        let mut cpu_rollback = Some(cpu_rollback);
        let reconcile_superseded = |cpu_rb: &mut Option<Vec<(PathBuf, String)>>| {
            let _ = self.reconcile_to_latest_committed(SupersededUndo {
                hw_odm_prev,
                wrote_odm,
                prev_tdps,
                tdps_written: tdp_count,
                cpu_rollback: cpu_rb.take().unwrap_or_default(),
            });
            false
        };
        if superseded() {
            return reconcile_superseded(&mut cpu_rollback);
        }

        let mut ok = true;
        if wrote_odm {
            if self.shutdown_wait_expired.load(Ordering::SeqCst) {
                return false;
            }
            if superseded() {
                return reconcile_superseded(&mut cpu_rollback);
            }
            if let Some(code) = hw_odm_prev {
                self.odm_write_seq.fetch_add(1, Ordering::SeqCst);
                let res = self.platform.set_perf_profile(code);
                self.odm_write_seq.fetch_add(1, Ordering::SeqCst);
                if superseded() {
                    return reconcile_superseded(&mut cpu_rollback);
                }
                if res.is_err() {
                    ok = false;
                } else if let Ok(actual) = self.platform.read_perf_profile() {
                    if actual != code {
                        ok = false;
                    }
                }
            } else {
                ok = false;
            }
        }
        for rb_idx in (0..tdp_count).rev() {
            if self.shutdown_wait_expired.load(Ordering::SeqCst) {
                return false;
            }
            if superseded() {
                return reconcile_superseded(&mut cpu_rollback);
            }
            let Some(prev_w) = prev_tdps.get(rb_idx).copied().flatten() else {
                ok = false;
                continue;
            };
            self.tdp_write_seq.fetch_add(1, Ordering::SeqCst);
            let set_res = self.platform.set_tdp(rb_idx, prev_w);
            self.tdp_write_seq.fetch_add(1, Ordering::SeqCst);
            if superseded() {
                return reconcile_superseded(&mut cpu_rollback);
            }
            if set_res.is_err() {
                ok = false;
            } else {
                match self.platform.read_tdp(rb_idx) {
                    Ok(actual) if actual == prev_w => {}
                    _ => ok = false,
                }
            }
        }
        if self.shutdown_wait_expired.load(Ordering::SeqCst) {
            return false;
        }
        if superseded() {
            return reconcile_superseded(&mut cpu_rollback);
        }
        if !self.rollback_cpu_writes(cpu_rollback.take().unwrap_or_default()) {
            ok = false;
        }
        if self.shutdown_wait_expired.load(Ordering::SeqCst) {
            return false;
        }
        if start_epoch.is_some() {
            let sync = self.hw_reconcile_lock.lock().unwrap();
            if superseded() {
                drop(sync);
                return reconcile_superseded(&mut cpu_rollback);
            }
        }
        if !ok {
            warn!("Hardware rollback incomplete; syncing active profile from confirmed hardware readback");
            self.sync_active_profile_from_hw_readback();
            *self.last_applied_profile_state.lock().unwrap() = None;
            *self.cpu_policy_applied_rev.lock().unwrap() = None;
            self.mode_reapply_pending.store(true, Ordering::SeqCst);
        }
        ok
    }

    #[allow(clippy::too_many_arguments)]
    fn restore_device_writes(
        &self,
        prev_brightness: Option<i32>,
        wrote_brightness: bool,
        prev_webcam: Option<bool>,
        wrote_webcam: bool,
        start_epoch: Option<u64>,
    ) {
        let superseded =
            || start_epoch.is_some_and(|e| self.profile_commit_epoch.load(Ordering::SeqCst) != e);
        if wrote_webcam {
            if self.shutdown_wait_expired.load(Ordering::SeqCst) || superseded() {
                return;
            }
            if let Some(prev) = prev_webcam {
                let _ = self.set_webcam_internal(prev);
            }
        }
        if wrote_brightness {
            if self.shutdown_wait_expired.load(Ordering::SeqCst) || superseded() {
                return;
            }
            if let Some(prev) = prev_brightness {
                let _ = self.set_display_brightness_internal(prev);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn abort_apply_on_shutdown(
        &self,
        prev_brightness: Option<i32>,
        wrote_brightness: bool,
        prev_webcam: Option<bool>,
        wrote_webcam: bool,
        hw_odm_prev: Option<i32>,
        wrote_odm: bool,
        prev_tdps: &[Option<i32>],
        tdps_written: usize,
        cpu_rollback: Vec<(PathBuf, String)>,
    ) -> ApplyOutcome {
        if self.shutdown_wait_expired.load(Ordering::SeqCst) {
            return ApplyOutcome::Failed;
        }
        self.restore_device_writes(
            prev_brightness,
            wrote_brightness,
            prev_webcam,
            wrote_webcam,
            None,
        );
        let _ = self.rollback_hardware(
            hw_odm_prev,
            wrote_odm,
            prev_tdps,
            tdps_written,
            cpu_rollback,
            None,
        );
        ApplyOutcome::Failed
    }

    fn apply_fan_and_device_state_locked(&self, profile: &UccProfile, apply_display_webcam: bool) {
        if apply_display_webcam {
            if profile.display.use_brightness && self.discover_backlight_device().is_some() {
                let _ = self.set_display_brightness_internal(profile.display.brightness);
            }
            if profile.webcam.use_status && self.webcam_sw_available() {
                let _ = self.set_webcam_internal(profile.webcam.status);
            }
        }

        let fan_enabled = self
            .settings
            .read()
            .unwrap()
            .fan_control_enabled
            .unwrap_or(profile.fan.use_control)
            && profile.fan.use_control;
        self.fan_control_enabled
            .store(fan_enabled, Ordering::SeqCst);
        {
            let mut fc = self.fan_controller.lock().unwrap();
            fc.set_same_speed(profile.fan.same_speed);
            fc.set_profile(Self::fan_profile_from_ucc_profile(profile));
            if !fan_enabled {
                if let Err(e) = fc.restore_auto(self.platform.as_ref()) {
                    warn!("Failed restoring auto fan control for non-controlled profile: {e}");
                }
            }
        }

        self.water_cooler_enabled
            .store(profile.fan.enable_water_cooler, Ordering::SeqCst);
        {
            let mut cooler = self.cooler_controller.lock().unwrap();
            cooler.set_mode(if profile.fan.auto_control_wc {
                CoolerMode::Auto
            } else {
                CoolerMode::Manual
            });
        }
    }

    fn write_tdp_verified(&self, idx: usize, target_w: i32) -> bool {
        self.tdp_write_seq.fetch_add(1, Ordering::SeqCst);
        let res = self.platform.set_tdp(idx, target_w);
        self.tdp_write_seq.fetch_add(1, Ordering::SeqCst);
        res.is_ok() && matches!(self.platform.read_tdp(idx), Ok(actual) if actual == target_w)
    }

    fn reconcile_to_latest_committed(&self, undo: SupersededUndo<'_>) -> ApplyOutcome {
        if self.shutting_down.load(Ordering::SeqCst)
            || self.shutdown_wait_expired.load(Ordering::SeqCst)
        {
            return ApplyOutcome::Failed;
        }
        let _guard = self.hw_reconcile_lock.lock().unwrap();
        if self.shutting_down.load(Ordering::SeqCst)
            || self.shutdown_wait_expired.load(Ordering::SeqCst)
        {
            return ApplyOutcome::Failed;
        }
        let latest = self.latest_committed_profile.lock().unwrap().clone();
        let Some(latest) = latest else {
            return ApplyOutcome::Failed;
        };

        let mut repair_ok = true;

        // 1. Undo mutations of the superseded call in channels NOT managed by the committed winner (`M8-COMMIT-01`).
        if !self.rollback_cpu_writes(undo.cpu_rollback) {
            repair_ok = false;
        }
        if self.shutting_down.load(Ordering::SeqCst)
            || self.shutdown_wait_expired.load(Ordering::SeqCst)
        {
            return ApplyOutcome::Failed;
        }
        if undo.wrote_odm && latest.profile.odm_profile_name.is_none() {
            if let Some(code) = latest.hw_odm.or(undo.hw_odm_prev) {
                self.odm_write_seq.fetch_add(1, Ordering::SeqCst);
                let res = self.platform.set_perf_profile(code);
                self.odm_write_seq.fetch_add(1, Ordering::SeqCst);
                if res.is_err() {
                    repair_ok = false;
                } else if let Ok(actual) = self.platform.read_perf_profile() {
                    if actual != code {
                        repair_ok = false;
                    }
                }
            } else {
                repair_ok = false;
            }
        }
        let undo_tdp_count = if undo.wrote_odm {
            undo.prev_tdps.len()
        } else {
            undo.tdps_written.min(undo.prev_tdps.len())
        };
        if self.platform.is_available() && self.capabilities.tdp_channels > 0 {
            for idx in 0..undo_tdp_count {
                if self.shutting_down.load(Ordering::SeqCst)
                    || self.shutdown_wait_expired.load(Ordering::SeqCst)
                {
                    return ApplyOutcome::Failed;
                }
                if idx >= latest.profile.odm_power_limits.len() {
                    let expected = latest
                        .hw_tdps
                        .get(idx)
                        .copied()
                        .flatten()
                        .or_else(|| undo.prev_tdps.get(idx).copied().flatten());
                    match expected {
                        Some(w) => {
                            if !self.write_tdp_verified(idx, w) {
                                repair_ok = false;
                            }
                        }
                        None => repair_ok = false,
                    }
                }
            }
        }

        // 2. Re-apply all fields managed by the committed winning profile and verify readback.
        if self.apply_cpu_settings(&latest.profile).is_err() {
            repair_ok = false;
        }
        if self.shutting_down.load(Ordering::SeqCst)
            || self.shutdown_wait_expired.load(Ordering::SeqCst)
        {
            return ApplyOutcome::Failed;
        }

        if let Some(ref odm_name) = latest.profile.odm_profile_name {
            if self.platform.is_available() && self.capabilities.perf_profiles_available > 0 {
                if let Some(code) =
                    fallback_odm_profile_code(self.capabilities.device.vendor, odm_name)
                {
                    self.odm_write_seq.fetch_add(1, Ordering::SeqCst);
                    let res = self.platform.set_perf_profile(code);
                    self.odm_write_seq.fetch_add(1, Ordering::SeqCst);
                    if res.is_err() {
                        repair_ok = false;
                    } else if let Ok(actual) = self.platform.read_perf_profile() {
                        if actual != code {
                            repair_ok = false;
                        }
                    }
                } else {
                    repair_ok = false;
                }
            }
        }
        if self.shutting_down.load(Ordering::SeqCst)
            || self.shutdown_wait_expired.load(Ordering::SeqCst)
        {
            return ApplyOutcome::Failed;
        }

        if !latest.profile.odm_power_limits.is_empty()
            && self.platform.is_available()
            && self.capabilities.tdp_channels > 0
        {
            for (idx, &watts) in latest.profile.odm_power_limits.iter().enumerate() {
                if self.shutting_down.load(Ordering::SeqCst)
                    || self.shutdown_wait_expired.load(Ordering::SeqCst)
                {
                    return ApplyOutcome::Failed;
                }
                let Ok(w_i32) = i32::try_from(watts) else {
                    repair_ok = false;
                    continue;
                };
                if !self.write_tdp_verified(idx, w_i32) {
                    repair_ok = false;
                }
            }
        }

        if self.shutting_down.load(Ordering::SeqCst)
            || self.shutdown_wait_expired.load(Ordering::SeqCst)
        {
            return ApplyOutcome::Failed;
        }
        self.apply_fan_and_device_state_locked(&latest.profile, true);

        if repair_ok {
            ApplyOutcome::SupersededReconciled
        } else {
            if !self.shutdown_wait_expired.load(Ordering::SeqCst) {
                self.sync_active_profile_from_hw_readback();
            }
            *self.last_applied_profile_state.lock().unwrap() = None;
            *self.cpu_policy_applied_rev.lock().unwrap() = None;
            self.mode_reapply_pending.store(true, Ordering::SeqCst);
            ApplyOutcome::Failed
        }
    }

    fn apply_profile_internal(
        &self,
        profile: &UccProfile,
        emit_signal: bool,
        persist_state_updates: Option<&[(String, String)]>,
        target_state_key: Option<&str>,
        persist_profile_update: Option<(&UccProfile, bool)>,
        expected_stored_profile: Option<&UccProfile>,
    ) -> ApplyOutcome {
        if self.shutting_down.load(Ordering::SeqCst)
            || self.shutdown_wait_expired.load(Ordering::SeqCst)
            || ucc_config::is_read_only_preview()
        {
            return ApplyOutcome::Failed;
        }
        if profile.validate().is_err() {
            return ApplyOutcome::Failed;
        }

        // Pre-validate ODM code + readback support AND all TDP channels with mandatory baseline
        // readback BEFORE touching hardware, disk, or supersession state (`M8-LIFE-03`, `RECHECK-02`, `FOLLOWUP-02`).
        let (target_odm_code, hw_odm_prev) = match profile.odm_profile_name {
            Some(ref odm_name)
                if self.platform.is_available() && self.capabilities.perf_profiles_available > 0 =>
            {
                let Some(code) =
                    fallback_odm_profile_code(self.capabilities.device.vendor, odm_name)
                else {
                    return ApplyOutcome::Failed;
                };
                let prev = match self.platform.read_perf_profile() {
                    Ok(cur) => Some(cur),
                    Err(ucc_hardware::PlatformError::Unavailable) => None,
                    Err(_) => return ApplyOutcome::Failed,
                };
                (Some(code), prev)
            }
            _ => (None, None),
        };

        let mut prev_tdps: Vec<Option<i32>> = Vec::new();
        let mut target_tdps: Vec<i32> = Vec::new();
        let has_tdps = !profile.odm_power_limits.is_empty()
            && self.platform.is_available()
            && self.capabilities.tdp_channels > 0;
        if has_tdps {
            if profile.odm_power_limits.len() > self.capabilities.tdp_channels {
                return ApplyOutcome::Failed;
            }
            for (idx, &watts) in profile.odm_power_limits.iter().enumerate() {
                let Ok(w_i32) = i32::try_from(watts) else {
                    return ApplyOutcome::Failed;
                };
                let Ok((min_w, max_w)) = self.platform.read_tdp_limits(idx) else {
                    return ApplyOutcome::Failed;
                };
                if !(min_w..=max_w).contains(&w_i32) {
                    return ApplyOutcome::Failed;
                }
                let Ok(cur_w) = self.platform.read_tdp(idx) else {
                    return ApplyOutcome::Failed;
                };
                prev_tdps.push(Some(cur_w));
                target_tdps.push(w_i32);
            }
            if target_odm_code.is_some() {
                for idx in profile.odm_power_limits.len()..self.capabilities.tdp_channels {
                    let Ok(cur_w) = self.platform.read_tdp(idx) else {
                        return ApplyOutcome::Failed;
                    };
                    prev_tdps.push(Some(cur_w));
                }
            }
        } else if target_odm_code.is_some()
            && self.platform.is_available()
            && self.capabilities.tdp_channels > 0
        {
            for idx in 0..self.capabilities.tdp_channels {
                let Ok(cur_w) = self.platform.read_tdp(idx) else {
                    return ApplyOutcome::Failed;
                };
                prev_tdps.push(Some(cur_w));
            }
        }

        let prev_brightness = (profile.display.use_brightness
            && self.discover_backlight_device().is_some())
        .then(|| self.get_display_brightness())
        .filter(|&b| b >= 0);
        let prev_webcam = (profile.webcam.use_status && self.webcam_sw_available())
            .then(|| self.platform.read_webcam().unwrap_or(false));
        let mut wrote_brightness = false;
        let mut wrote_webcam = false;

        let tx_id = self
            .next_profile_tx_id
            .fetch_add(1, Ordering::SeqCst)
            .saturating_add(1);
        let _tx_guard = InFlightTxGuard {
            backend: self,
            tx_id,
        };
        let start_epoch = self.profile_commit_epoch.load(Ordering::SeqCst);

        if self.shutting_down.load(Ordering::SeqCst) {
            return ApplyOutcome::Failed;
        }

        // Phase 1: Reversible CPU writes FIRST so CPU failures never leave ODM or TDP mutated (`M8-PROD-04`).
        let Ok(cpu_res) = self.apply_cpu_settings(profile) else {
            if !self.shutting_down.load(Ordering::SeqCst)
                && !self.shutdown_wait_expired.load(Ordering::SeqCst)
                && self.profile_commit_epoch.load(Ordering::SeqCst) != start_epoch
            {
                let _ = self.reconcile_to_latest_committed(SupersededUndo {
                    hw_odm_prev: None,
                    wrote_odm: false,
                    prev_tdps: &prev_tdps,
                    tdps_written: 0,
                    cpu_rollback: Vec::new(),
                });
            }
            return ApplyOutcome::Failed;
        };

        if self.shutting_down.load(Ordering::SeqCst) {
            return self.abort_apply_on_shutdown(
                prev_brightness,
                wrote_brightness,
                prev_webcam,
                wrote_webcam,
                None,
                false,
                &prev_tdps,
                0,
                cpu_res.rollback,
            );
        }
        if self.profile_commit_epoch.load(Ordering::SeqCst) != start_epoch {
            return self.reconcile_to_latest_committed(SupersededUndo {
                hw_odm_prev: None,
                wrote_odm: false,
                prev_tdps: &prev_tdps,
                tdps_written: 0,
                cpu_rollback: cpu_res.rollback,
            });
        }

        // Phase 2: ODM + TDP with mandatory readback verification (`M8-HANDOFF-05`, `M8-LIFE-05`, `M8-COMMIT-01`..`03`, `FOLLOWUP-02`).
        let mut wrote_odm = false;

        let odm_write_verified = |code: i32| -> bool {
            self.odm_write_seq.fetch_add(1, Ordering::SeqCst);
            let res = self.platform.set_perf_profile(code);
            self.odm_write_seq.fetch_add(1, Ordering::SeqCst);
            if res.is_err() {
                return false;
            }
            if hw_odm_prev.is_some() {
                return matches!(self.platform.read_perf_profile(), Ok(actual) if actual == code);
            }
            true
        };

        if target_odm_code.is_some() && has_tdps && hw_odm_prev.is_none() {
            // Branch A: Backend returned Unavailable (uninitialized mock) on read_perf_profile():
            // verify & write ALL TDPs BEFORE touching ODM so a Pass 1 failure leaves ODM unmodified.
            let mut wrote_tdps = 0usize;
            for (idx, &w) in target_tdps.iter().enumerate() {
                if self.shutting_down.load(Ordering::SeqCst) {
                    return self.abort_apply_on_shutdown(
                        prev_brightness,
                        wrote_brightness,
                        prev_webcam,
                        wrote_webcam,
                        None,
                        false,
                        &prev_tdps,
                        wrote_tdps,
                        cpu_res.rollback,
                    );
                }
                if let Some(prev_w) = prev_tdps[idx] {
                    self.record_in_flight_tdp_baseline(tx_id, idx, prev_w);
                }
                self.tdp_write_seq.fetch_add(1, Ordering::SeqCst);
                let set_ok = self.platform.set_tdp(idx, w).is_ok();
                self.tdp_write_seq.fetch_add(1, Ordering::SeqCst);
                if self.shutting_down.load(Ordering::SeqCst) {
                    return self.abort_apply_on_shutdown(
                        prev_brightness,
                        wrote_brightness,
                        prev_webcam,
                        wrote_webcam,
                        None,
                        false,
                        &prev_tdps,
                        idx + 1,
                        cpu_res.rollback,
                    );
                }
                if self.profile_commit_epoch.load(Ordering::SeqCst) != start_epoch {
                    let undo = SupersededUndo {
                        hw_odm_prev: None,
                        wrote_odm: false,
                        prev_tdps: &prev_tdps,
                        tdps_written: idx + 1,
                        cpu_rollback: cpu_res.rollback,
                    };
                    if set_ok {
                        return self.reconcile_to_latest_committed(undo);
                    }
                    let _ = self.reconcile_to_latest_committed(undo);
                    return ApplyOutcome::Failed;
                }
                if !set_ok || !matches!(self.platform.read_tdp(idx), Ok(actual) if actual == w) {
                    let _ = self.rollback_hardware(
                        None,
                        false,
                        &prev_tdps,
                        idx + 1,
                        cpu_res.rollback,
                        Some(start_epoch),
                    );
                    return ApplyOutcome::Failed;
                }
                wrote_tdps += 1;
            }

            if let Some(code) = target_odm_code {
                if self.shutting_down.load(Ordering::SeqCst) {
                    return self.abort_apply_on_shutdown(
                        prev_brightness,
                        wrote_brightness,
                        prev_webcam,
                        wrote_webcam,
                        None,
                        false,
                        &prev_tdps,
                        wrote_tdps,
                        cpu_res.rollback,
                    );
                }
                for (i, slot) in prev_tdps.iter().enumerate() {
                    if let Some(w) = *slot {
                        self.record_in_flight_tdp_baseline(tx_id, i, w);
                    }
                }
                let odm_ok = odm_write_verified(code);
                wrote_odm = odm_ok;
                if self.shutting_down.load(Ordering::SeqCst) {
                    return self.abort_apply_on_shutdown(
                        prev_brightness,
                        wrote_brightness,
                        prev_webcam,
                        wrote_webcam,
                        None,
                        true,
                        &prev_tdps,
                        if wrote_odm { prev_tdps.len() } else { wrote_tdps },
                        cpu_res.rollback,
                    );
                }
                if self.profile_commit_epoch.load(Ordering::SeqCst) != start_epoch {
                    let undo = SupersededUndo {
                        hw_odm_prev: None,
                        wrote_odm,
                        prev_tdps: &prev_tdps,
                        tdps_written: wrote_tdps,
                        cpu_rollback: cpu_res.rollback,
                    };
                    if odm_ok {
                        return self.reconcile_to_latest_committed(undo);
                    }
                    let _ = self.reconcile_to_latest_committed(undo);
                    return ApplyOutcome::Failed;
                }
                if !odm_ok {
                    let _ = self.rollback_hardware(
                        None,
                        true,
                        &prev_tdps,
                        wrote_tdps,
                        cpu_res.rollback,
                        Some(start_epoch),
                    );
                    return ApplyOutcome::Failed;
                }

                // Pass 2: Re-apply custom TDPs over any ODM default limit changes and verify readback.
                for (idx, &w) in target_tdps.iter().enumerate() {
                    if self.shutting_down.load(Ordering::SeqCst) {
                        return self.abort_apply_on_shutdown(
                            prev_brightness,
                            wrote_brightness,
                            prev_webcam,
                            wrote_webcam,
                            None,
                            true,
                            &prev_tdps,
                            prev_tdps.len(),
                            cpu_res.rollback,
                        );
                    }
                    if let Some(prev_w) = prev_tdps[idx] {
                        self.record_in_flight_tdp_baseline(tx_id, idx, prev_w);
                    }
                    self.tdp_write_seq.fetch_add(1, Ordering::SeqCst);
                    let set_ok = self.platform.set_tdp(idx, w).is_ok();
                    self.tdp_write_seq.fetch_add(1, Ordering::SeqCst);
                    if self.shutting_down.load(Ordering::SeqCst) {
                        return self.abort_apply_on_shutdown(
                            prev_brightness,
                            wrote_brightness,
                            prev_webcam,
                            wrote_webcam,
                            None,
                            true,
                            &prev_tdps,
                            prev_tdps.len(),
                            cpu_res.rollback,
                        );
                    }
                    if self.profile_commit_epoch.load(Ordering::SeqCst) != start_epoch {
                        let undo = SupersededUndo {
                            hw_odm_prev: None,
                            wrote_odm: true,
                            prev_tdps: &prev_tdps,
                            tdps_written: prev_tdps.len(),
                            cpu_rollback: cpu_res.rollback,
                        };
                        if set_ok {
                            return self.reconcile_to_latest_committed(undo);
                        }
                        let _ = self.reconcile_to_latest_committed(undo);
                        return ApplyOutcome::Failed;
                    }
                    if !set_ok || !matches!(self.platform.read_tdp(idx), Ok(actual) if actual == w)
                    {
                        let _ = self.rollback_hardware(
                            None,
                            true,
                            &prev_tdps,
                            prev_tdps.len(),
                            cpu_res.rollback,
                            Some(start_epoch),
                        );
                        return ApplyOutcome::Failed;
                    }
                }
            }
        } else {
            if let Some(code) = target_odm_code {
                if self.shutting_down.load(Ordering::SeqCst) {
                    return self.abort_apply_on_shutdown(
                        prev_brightness,
                        wrote_brightness,
                        prev_webcam,
                        wrote_webcam,
                        hw_odm_prev,
                        false,
                        &prev_tdps,
                        0,
                        cpu_res.rollback,
                    );
                }
                if let Some(prev_code) = hw_odm_prev {
                    self.record_in_flight_odm_baseline(tx_id, prev_code);
                }
                for (i, slot) in prev_tdps.iter().enumerate() {
                    if let Some(w) = *slot {
                        self.record_in_flight_tdp_baseline(tx_id, i, w);
                    }
                }
                let odm_ok = odm_write_verified(code);
                wrote_odm = true;
                if self.shutting_down.load(Ordering::SeqCst) {
                    return self.abort_apply_on_shutdown(
                        prev_brightness,
                        wrote_brightness,
                        prev_webcam,
                        wrote_webcam,
                        hw_odm_prev,
                        true,
                        &prev_tdps,
                        prev_tdps.len(),
                        cpu_res.rollback,
                    );
                }
                if self.profile_commit_epoch.load(Ordering::SeqCst) != start_epoch {
                    let undo = SupersededUndo {
                        hw_odm_prev,
                        wrote_odm,
                        prev_tdps: &prev_tdps,
                        tdps_written: prev_tdps.len(),
                        cpu_rollback: cpu_res.rollback,
                    };
                    if odm_ok {
                        return self.reconcile_to_latest_committed(undo);
                    }
                    let _ = self.reconcile_to_latest_committed(undo);
                    return ApplyOutcome::Failed;
                }
                if !odm_ok {
                    let _ = self.rollback_hardware(
                        hw_odm_prev,
                        true,
                        &prev_tdps,
                        prev_tdps.len(),
                        cpu_res.rollback,
                        Some(start_epoch),
                    );
                    return ApplyOutcome::Failed;
                }
            }

            if has_tdps {
                for (idx, &w) in target_tdps.iter().enumerate() {
                    if self.shutting_down.load(Ordering::SeqCst) {
                        return self.abort_apply_on_shutdown(
                            prev_brightness,
                            wrote_brightness,
                            prev_webcam,
                            wrote_webcam,
                            hw_odm_prev,
                            wrote_odm,
                            &prev_tdps,
                            if wrote_odm { prev_tdps.len() } else { idx },
                            cpu_res.rollback,
                        );
                    }
                    if let Some(prev_w) = prev_tdps[idx] {
                        self.record_in_flight_tdp_baseline(tx_id, idx, prev_w);
                    }
                    self.tdp_write_seq.fetch_add(1, Ordering::SeqCst);
                    let set_ok = self.platform.set_tdp(idx, w).is_ok();
                    self.tdp_write_seq.fetch_add(1, Ordering::SeqCst);
                    if self.shutting_down.load(Ordering::SeqCst) {
                        return self.abort_apply_on_shutdown(
                            prev_brightness,
                            wrote_brightness,
                            prev_webcam,
                            wrote_webcam,
                            hw_odm_prev,
                            wrote_odm,
                            &prev_tdps,
                            if wrote_odm { prev_tdps.len() } else { idx + 1 },
                            cpu_res.rollback,
                        );
                    }
                    if self.profile_commit_epoch.load(Ordering::SeqCst) != start_epoch {
                        let undo = SupersededUndo {
                            hw_odm_prev,
                            wrote_odm,
                            prev_tdps: &prev_tdps,
                            tdps_written: if wrote_odm { prev_tdps.len() } else { idx + 1 },
                            cpu_rollback: cpu_res.rollback,
                        };
                        if set_ok {
                            return self.reconcile_to_latest_committed(undo);
                        }
                        let _ = self.reconcile_to_latest_committed(undo);
                        return ApplyOutcome::Failed;
                    }
                    if !set_ok || !matches!(self.platform.read_tdp(idx), Ok(actual) if actual == w)
                    {
                        let _ = self.rollback_hardware(
                            hw_odm_prev,
                            wrote_odm,
                            &prev_tdps,
                            idx + 1,
                            cpu_res.rollback,
                            Some(start_epoch),
                        );
                        return ApplyOutcome::Failed;
                    }
                }
            }
        }

        let post_phase2_tdp_seq = self.tdp_write_seq.load(Ordering::SeqCst);
        let post_phase2_odm_seq = self.odm_write_seq.load(Ordering::SeqCst);
        let tdps_written = if has_tdps || wrote_odm {
            if wrote_odm {
                prev_tdps.len()
            } else {
                target_tdps.len()
            }
        } else {
            0
        };

        if self.shutting_down.load(Ordering::SeqCst) {
            return self.abort_apply_on_shutdown(
                prev_brightness,
                wrote_brightness,
                prev_webcam,
                wrote_webcam,
                hw_odm_prev,
                wrote_odm,
                &prev_tdps,
                tdps_written,
                cpu_res.rollback,
            );
        }
        if self.profile_commit_epoch.load(Ordering::SeqCst) != start_epoch {
            return self.reconcile_to_latest_committed(SupersededUndo {
                hw_odm_prev,
                wrote_odm,
                prev_tdps: &prev_tdps,
                tdps_written,
                cpu_rollback: cpu_res.rollback,
            });
        }

        // Apply display brightness & webcam switches BEFORE Phase 3 commit and verify success (`UCC-06`, `FOLLOWUP-01`).
        if profile.display.use_brightness && self.discover_backlight_device().is_some() {
            if !self.set_display_brightness_internal(profile.display.brightness) {
                if self.shutting_down.load(Ordering::SeqCst) {
                    return self.abort_apply_on_shutdown(
                        prev_brightness,
                        false,
                        prev_webcam,
                        false,
                        hw_odm_prev,
                        wrote_odm,
                        &prev_tdps,
                        tdps_written,
                        cpu_res.rollback,
                    );
                }
                let _ = self.rollback_hardware(
                    hw_odm_prev,
                    wrote_odm,
                    &prev_tdps,
                    tdps_written,
                    cpu_res.rollback,
                    Some(start_epoch),
                );
                return ApplyOutcome::Failed;
            }
            wrote_brightness = true;
        }
        if self.shutting_down.load(Ordering::SeqCst) {
            return self.abort_apply_on_shutdown(
                prev_brightness,
                wrote_brightness,
                prev_webcam,
                wrote_webcam,
                hw_odm_prev,
                wrote_odm,
                &prev_tdps,
                tdps_written,
                cpu_res.rollback,
            );
        }
        if profile.webcam.use_status && self.webcam_sw_available() {
            if !self.set_webcam_internal(profile.webcam.status) {
                if self.shutting_down.load(Ordering::SeqCst) {
                    return self.abort_apply_on_shutdown(
                        prev_brightness,
                        wrote_brightness,
                        prev_webcam,
                        false,
                        hw_odm_prev,
                        wrote_odm,
                        &prev_tdps,
                        tdps_written,
                        cpu_res.rollback,
                    );
                }
                self.restore_device_writes(
                    prev_brightness,
                    wrote_brightness,
                    prev_webcam,
                    wrote_webcam,
                    Some(start_epoch),
                );
                let _ = self.rollback_hardware(
                    hw_odm_prev,
                    wrote_odm,
                    &prev_tdps,
                    tdps_written,
                    cpu_res.rollback,
                    Some(start_epoch),
                );
                return ApplyOutcome::Failed;
            }
            wrote_webcam = true;
        }

        if self.shutting_down.load(Ordering::SeqCst) {
            return self.abort_apply_on_shutdown(
                prev_brightness,
                wrote_brightness,
                prev_webcam,
                wrote_webcam,
                hw_odm_prev,
                wrote_odm,
                &prev_tdps,
                tdps_written,
                cpu_res.rollback,
            );
        }
        if self.profile_commit_epoch.load(Ordering::SeqCst) != start_epoch {
            self.restore_device_writes(
                prev_brightness,
                wrote_brightness,
                prev_webcam,
                wrote_webcam,
                Some(start_epoch),
            );
            return self.reconcile_to_latest_committed(SupersededUndo {
                hw_odm_prev,
                wrote_odm,
                prev_tdps: &prev_tdps,
                tdps_written,
                cpu_rollback: cpu_res.rollback,
            });
        }

        // Phase 3: Atomic commit under `hw_reconcile_lock` (`M8-LIFE-03`..`05`, `M8-COMMIT-01`..`06`, `RECHECK-01`, `RECHECK-05`, `FOLLOWUP-04`).
        {
            let guard = self.hw_reconcile_lock.lock().unwrap();
            if self.shutting_down.load(Ordering::SeqCst) {
                drop(guard);
                return self.abort_apply_on_shutdown(
                    prev_brightness,
                    wrote_brightness,
                    prev_webcam,
                    wrote_webcam,
                    hw_odm_prev,
                    wrote_odm,
                    &prev_tdps,
                    tdps_written,
                    cpu_res.rollback,
                );
            }
            if self.profile_commit_epoch.load(Ordering::SeqCst) != start_epoch {
                drop(guard);
                self.restore_device_writes(
                    prev_brightness,
                    wrote_brightness,
                    prev_webcam,
                    wrote_webcam,
                    Some(start_epoch),
                );
                return self.reconcile_to_latest_committed(SupersededUndo {
                    hw_odm_prev,
                    wrote_odm,
                    prev_tdps: &prev_tdps,
                    tdps_written,
                    cpu_rollback: cpu_res.rollback,
                });
            }

            if let Some(code) = target_odm_code {
                if self.odm_write_seq.load(Ordering::SeqCst) != post_phase2_odm_seq {
                    let odm_ok = if hw_odm_prev.is_some()
                        && matches!(self.platform.read_perf_profile(), Ok(actual) if actual == code)
                    {
                        true
                    } else {
                        odm_write_verified(code)
                    };
                    if !odm_ok {
                        drop(guard);
                        self.restore_device_writes(
                            prev_brightness,
                            wrote_brightness,
                            prev_webcam,
                            wrote_webcam,
                            Some(start_epoch),
                        );
                        let _ = self.rollback_hardware(
                            hw_odm_prev,
                            wrote_odm,
                            &prev_tdps,
                            tdps_written,
                            cpu_res.rollback,
                            Some(start_epoch),
                        );
                        return ApplyOutcome::Failed;
                    }
                }
            }
            if has_tdps && self.tdp_write_seq.load(Ordering::SeqCst) != post_phase2_tdp_seq {
                for (idx, &target_w) in target_tdps.iter().enumerate() {
                    if !matches!(self.platform.read_tdp(idx), Ok(actual) if actual == target_w)
                        && !self.write_tdp_verified(idx, target_w)
                    {
                        drop(guard);
                        self.restore_device_writes(
                            prev_brightness,
                            wrote_brightness,
                            prev_webcam,
                            wrote_webcam,
                            Some(start_epoch),
                        );
                        let _ = self.rollback_hardware(
                            hw_odm_prev,
                            wrote_odm,
                            &prev_tdps,
                            tdps_written,
                            cpu_res.rollback,
                            Some(start_epoch),
                        );
                        return ApplyOutcome::Failed;
                    }
                }
            }

            if persist_state_updates.is_some()
                || persist_profile_update.is_some()
                || expected_stored_profile.is_some()
            {
                let mut stale_err = false;
                let txn_res = if persist_state_updates.is_some() || persist_profile_update.is_some() {
                    self.update_settings(|s| {
                        if let Some(expected) = expected_stored_profile {
                            match s.profiles.get(&expected.id) {
                                None => return None,
                                Some(stored) if stored != expected => {
                                    stale_err = true;
                                    return None;
                                }
                                Some(_) => {}
                            }
                        }
                        if let Some((updated_prof, require_existing)) = persist_profile_update {
                            if is_builtin_profile_id(&updated_prof.id)
                                || (require_existing && !s.profiles.contains_key(&updated_prof.id))
                            {
                                return None;
                            }
                            s.profiles
                                .insert(updated_prof.id.clone(), updated_prof.clone());
                        }
                        if let Some(updates) = persist_state_updates {
                            if updates
                                .iter()
                                .any(|(_, prof_id)| !s.profiles.contains_key(prof_id))
                            {
                                return None;
                            }
                            for (state_key, prof_id) in updates {
                                s.state_map.insert(state_key.clone(), prof_id.clone());
                            }
                        }
                        Some(())
                    })
                } else if let Some(expected) = expected_stored_profile {
                    if let Ok(_mgr) = self.config_manager.lock() {
                        let s = self.settings.read().unwrap();
                        match s.profiles.get(&expected.id) {
                            None => None,
                            Some(stored) if stored != expected => {
                                stale_err = true;
                                None
                            }
                            Some(_) => Some(()),
                        }
                    } else {
                        None
                    }
                } else {
                    Some(())
                };

                if txn_res.is_none() {
                    drop(guard);
                    self.restore_device_writes(
                        prev_brightness,
                        wrote_brightness,
                        prev_webcam,
                        wrote_webcam,
                        Some(start_epoch),
                    );
                    let _ = self.rollback_hardware(
                        hw_odm_prev,
                        wrote_odm,
                        &prev_tdps,
                        tdps_written,
                        cpu_res.rollback,
                        Some(start_epoch),
                    );
                    return if stale_err {
                        ApplyOutcome::StaleRevision
                    } else {
                        ApplyOutcome::Failed
                    };
                }
            }

            self.apply_fan_and_device_state_locked(profile, false);
            *self.active_profile.write().unwrap() = profile.clone();
            let hw_tdps = (0..self.capabilities.tdp_channels)
                .map(|idx| {
                    if idx < profile.odm_power_limits.len() {
                        let mut inflight = self.in_flight_tdp_baselines.lock().unwrap();
                        inflight.retain(|(id, ch, _)| *id == tx_id || *ch != idx);
                        drop(inflight);
                        self.platform.read_tdp(idx).ok()
                    } else {
                        self.snapshot_unmanaged_tdp_baseline(tx_id, idx)
                    }
                })
                .collect();
            let hw_odm = if profile.odm_profile_name.is_some() {
                let mut inflight = self.in_flight_odm_baselines.lock().unwrap();
                inflight.retain(|(id, _)| *id == tx_id);
                drop(inflight);
                self.platform.read_perf_profile().ok()
            } else {
                self.snapshot_unmanaged_odm_baseline(tx_id)
            };
            let _new_epoch = self
                .profile_commit_epoch
                .fetch_add(1, Ordering::SeqCst)
                .saturating_add(1);
            *self.latest_committed_profile.lock().unwrap() = Some(CommittedProfileRecord {
                profile: profile.clone(),
                hw_tdps,
                hw_odm,
            });
        }

        let decision = self.current_decision();
        let pd_rev = self.power_devices.revision();
        *self.cpu_policy_applied_rev.lock().unwrap() = if cpu_res.fully_applied {
            Some((pd_rev, decision.effective))
        } else {
            None
        };
        if persist_state_updates.is_some() {
            *self.temp_profile_name.write().unwrap() = None;
            if let Some(applied_state) = target_state_key {
                if decision.profile_state.as_str() == applied_state {
                    *self.last_applied_profile_state.lock().unwrap() =
                        Some(applied_state.to_string());
                } else {
                    *self.last_applied_profile_state.lock().unwrap() = None;
                }
            } else {
                *self.last_applied_profile_state.lock().unwrap() =
                    Some(decision.profile_state.as_str().to_string());
            }
        }
        self.mode_reapply_pending.store(false, Ordering::SeqCst);
        self.tick_fans_once();
        if emit_signal {
            self.emit_profile_changed(profile);
        }
        ApplyOutcome::Committed
    }

    fn set_webcam_internal(&self, enabled: bool) -> bool {
        !self.shutdown_wait_expired.load(Ordering::SeqCst)
            && self.webcam_sw_available()
            && self.platform.set_webcam(enabled).is_ok()
    }

    fn set_display_brightness_internal(&self, brightness: i32) -> bool {
        if !(0..=100).contains(&brightness) || self.shutdown_wait_expired.load(Ordering::SeqCst) {
            return false;
        }
        let Some((dev, max_val)) = self.discover_backlight_device() else {
            return false;
        };
        let brightness_path = dev.join("brightness");
        let prev_raw = self.sysfs.read(&brightness_path).ok();
        if self.shutdown_wait_expired.load(Ordering::SeqCst) {
            return false;
        }
        let raw = ((f64::from(brightness) / 100.0) * f64::from(max_val))
            .round()
            .clamp(0.0, f64::from(max_val)) as i32;
        if self.sysfs.write(&brightness_path, &raw.to_string()).is_err() {
            return false;
        }
        let tol = ((100.0 / f64::from(max_val)).ceil() as i32).max(1);
        for attempt in 0..3 {
            if self.shutting_down.load(Ordering::SeqCst)
                || self.shutdown_wait_expired.load(Ordering::SeqCst)
            {
                break;
            }
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(2));
                if self.shutting_down.load(Ordering::SeqCst)
                    || self.shutdown_wait_expired.load(Ordering::SeqCst)
                {
                    break;
                }
            }
            let actual = self.get_display_brightness();
            if self.shutting_down.load(Ordering::SeqCst)
                || self.shutdown_wait_expired.load(Ordering::SeqCst)
            {
                break;
            }
            if actual >= 0 && (actual - brightness).abs() <= tol {
                return true;
            }
        }
        if !self.shutdown_wait_expired.load(Ordering::SeqCst) {
            if let Some(prev) = prev_raw {
                let _ = self.sysfs.write(&brightness_path, prev.trim());
            }
        }
        false
    }

    /// Runs one synchronous hardware telemetry poll and stores latest snapshot + history.
    /// When sensor collection is disabled (`M8-REVIEW-16`), returns cached or synthetic idle snapshot
    /// without polling hardware or pushing new history points.
    pub fn poll_telemetry_once(&self) -> TelemetrySnapshot {
        if self.shutting_down.load(Ordering::SeqCst)
            || !self.sensor_data_collection.load(Ordering::SeqCst)
        {
            if let Some(prev) = self.latest_telemetry.read().unwrap().clone() {
                return prev;
            }
            return TelemetrySnapshot {
                timestamp_ms: unix_ms(),
                collected_at: Instant::now(),
                cpu: ucc_hardware::CpuTelemetrySnapshot {
                    temp_c: None,
                    freq_mhz: None,
                    power_w: None,
                    max_power_limit_w: None,
                    collected_at: Instant::now(),
                },
                igpu: None,
                dgpu: None,
            };
        }
        let now = Instant::now();
        let ts = unix_ms();
        let mut collector = self.telemetry.lock().unwrap();
        let snap = collector.poll(now, ts);
        drop(collector);
        if self.shutting_down.load(Ordering::SeqCst) {
            return snap;
        }
        self.record_nvidia_baseline_if_needed(&snap);
        *self.latest_telemetry.write().unwrap() = Some(snap.clone());
        snap
    }

    fn record_nvidia_baseline_if_needed(&self, snap: &TelemetrySnapshot) {
        if let Some(ref dgpu) = snap.dgpu {
            let immutable_base = dgpu
                .default_power_limit_w
                .or(dgpu.max_power_limit_w)
                .or(dgpu.enforced_power_limit_w);
            if let (Some(ref bdf), Some(base_w)) = (&dgpu.bdf, immutable_base) {
                let mut baseline = self.nvidia_default_power_limit_w.lock().unwrap();
                if baseline
                    .as_ref()
                    .is_none_or(|(stored_bdf, _)| stored_bdf != bdf)
                {
                    *baseline = Some((bdf.clone(), base_w));
                }
            }
        }
    }

    /// Runs one synchronous fan control tick and records fan snapshots.
    pub fn tick_fans_once(&self) -> FanTickSnapshot {
        if self.shutting_down.load(Ordering::SeqCst) {
            return self
                .latest_fans
                .read()
                .unwrap()
                .clone()
                .unwrap_or_else(|| FanTickSnapshot {
                    timestamp_ms: unix_ms(),
                    temps_c: Vec::new(),
                    speeds_percent: Vec::new(),
                    calculated_speeds: Vec::new(),
                    fallback_auto: true,
                });
        }
        let ts = unix_ms();
        if ucc_config::is_read_only_preview() {
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
            *self.latest_fans.write().unwrap() = Some(tick.clone());
            return tick;
        }
        let enabled = self.fan_control_enabled.load(Ordering::SeqCst);
        let mut ctrl = self.fan_controller.lock().unwrap();
        let tick = ctrl.tick_cancellable(self.platform.as_ref(), enabled, ts, || {
            self.shutting_down.load(Ordering::SeqCst)
        });
        drop(ctrl);

        if self.shutting_down.load(Ordering::SeqCst) {
            return tick;
        }
        *self.latest_fans.write().unwrap() = Some(tick.clone());
        if self.sensor_data_collection.load(Ordering::SeqCst) {
            let cpu_duty = tick.speeds_percent.first().copied().flatten();
            let gpu_duty = tick.speeds_percent.get(1).copied().flatten();
            if let Ok(col) = self.telemetry.lock() {
                col.record_fan_duties(ts, cpu_duty, gpu_duty);
            }
        }
        tick
    }

    /// Returns whether configuration fell back/recovered from backup or defaults at startup.
    pub fn config_recovered_at_startup(&self) -> bool {
        self.config_recovered.load(Ordering::SeqCst)
    }

    /// Borrows the lid controller mutex for event evaluation and tests (`M8.9`).
    pub fn lid_controller(&self) -> &Mutex<LidController> {
        &self.lid_controller
    }

    /// Performs bounded hardware cleanup during daemon shutdown (`M8.3`): restores firmware
    /// auto fan control, clears temporary fan curves, restores per-CPU defaults,
    /// and marks liquid cooler disconnected.
    pub fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        self.fan_control_enabled.store(false, Ordering::SeqCst);
        if ucc_config::is_read_only_preview() {
            return;
        }
        if !self.shutdown_wait_expired.load(Ordering::SeqCst) {
            let _ = self.restore_cpu_defaults();
        }
        if let Ok(mut cooler) = self.cooler_controller.lock() {
            cooler.on_disconnected();
        }
        self.water_cooler_connected.store(false, Ordering::SeqCst);
        if !self.shutdown_wait_expired.load(Ordering::SeqCst) {
            if let Ok(mut lb) = self.lightbar_ctrl.lock() {
                let _ = lb.shutdown();
            }
        }
        let auto_res = if let Ok(mut ctrl) = self.fan_controller.lock() {
            ctrl.clear_temporary_curves();
            ctrl.restore_auto(self.platform.as_ref())
                .map_err(|e| e.to_string())
        } else {
            self.platform.set_fans_auto().map_err(|e| e.to_string())
        };
        if let Err(err) = auto_res {
            error!("Failed to restore fans to firmware auto control during shutdown: {err}");
        }
    }

    /// Serializes read-modify-save-commit under `config_manager` lock (`M8-RECHECK-06`),
    /// ensuring concurrent callers never overwrite one another's changes and RAM never diverges from disk.
    fn update_settings<R>(
        &self,
        mutate: impl FnOnce(&mut UccSettings) -> Option<R>,
    ) -> Option<R> {
        let Ok(mgr) = self.config_manager.lock() else {
            return None;
        };
        let mut candidate = self.settings.read().unwrap().clone();
        let outcome = mutate(&mut candidate)?;
        if mgr.save(&candidate).is_err() {
            return None;
        }
        *self.settings.write().unwrap() = candidate;
        Some(outcome)
    }
    fn read_dmi_field(&self, field: &str) -> String {
        self.sysfs
            .read(&self.sys_root.join("sys/class/dmi/id").join(field))
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    }

    fn read_cpu_model(&self) -> String {
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

    fn parse_pci_hex(raw: &str) -> Option<u32> {
        let trimmed = raw.trim();
        let hex = trimmed
            .strip_prefix("0x")
            .or_else(|| trimmed.strip_prefix("0X"))
            .unwrap_or(trimmed);
        u32::from_str_radix(hex, 16).ok()
    }

    fn decode_pci_name(raw: &str, vendor: u32) -> String {
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

    fn lookup_pci_ids_in_text(text: &str, vendor: u32, device: u32) -> Option<String> {
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

    fn lookup_pci_ids(&self, vendor: u32, device: u32) -> Option<String> {
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

    fn read_pci_gpu_product_name(&self, dev_dir: &Path) -> Option<String> {
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

    fn read_nvidia_gpu_information_model(&self, bdf: &str) -> Option<String> {
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

    fn detect_pci_gpus(&self) -> (String, String) {
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

            let model = self
                .sysfs
                .read(&entry.join("label"))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .or_else(|| self.read_pci_gpu_product_name(&entry))
                .or_else(|| self.read_nvidia_gpu_information_model(&bdf))
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

    fn parse_drm_mode_line(line: &str) -> Option<(i32, i32, Option<i32>)> {
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

    fn is_safe_drm_port(port: &str) -> bool {
        !port.is_empty() && !port.contains("..") && !port.contains('/') && !port.contains('\\')
    }

    fn configured_ycbcr_targets(&self) -> Vec<(i32, String)> {
        let ycbcr = self.settings.read().unwrap().ycbcr420_workaround.clone();
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

    fn map_prime_token(token: &str) -> Option<String> {
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

    fn persist_or_reapply_custom_profile(
        &self,
        profile_json: &str,
        require_existing: bool,
    ) -> bool {
        if ucc_config::is_read_only_preview() || self.shutting_down.load(Ordering::SeqCst) {
            return false;
        }
        let Ok(prof) = serde_json::from_str::<UccProfile>(profile_json) else {
            return false;
        };
        if prof.id.trim().is_empty() || is_builtin_profile_id(&prof.id) || prof.validate().is_err()
        {
            return false;
        }
        if require_existing && !self.settings.read().unwrap().profiles.contains_key(&prof.id) {
            return false;
        }

        for _attempt in 0..4 {
            if self.shutting_down.load(Ordering::SeqCst) {
                return false;
            }
            if self.active_profile.read().unwrap().id == prof.id {
                let Some(_op_guard) = self.enter_hardware_op() else {
                    return false;
                };
                if self.active_profile.read().unwrap().id == prof.id {
                    match self.apply_profile_internal(
                        &prof,
                        true,
                        None,
                        None,
                        Some((&prof, require_existing)),
                        None,
                    ) {
                        ApplyOutcome::Committed => return true,
                        ApplyOutcome::SupersededReconciled => continue,
                        ApplyOutcome::StaleRevision | ApplyOutcome::Failed => return false,
                    }
                }
            }

            let _guard = self.hw_reconcile_lock.lock().unwrap();
            if self.shutting_down.load(Ordering::SeqCst) {
                return false;
            }
            if self.active_profile.read().unwrap().id == prof.id {
                continue;
            }
            return self
                .update_settings(|s| {
                    if require_existing && !s.profiles.contains_key(&prof.id) {
                        return None;
                    }
                    s.profiles.insert(prof.id.clone(), prof.clone());
                    Some(())
                })
                .is_some();
        }
        false
    }
}

impl UccdBackend for LiveUccdBackend {
    fn get_device_name(&self) -> String {
        let name = self.device_name.trim();
        if name.is_empty() {
            "Frixit Control Center Laptop".to_string()
        } else {
            name.to_string()
        }
    }

    fn get_system_info_json(&self) -> String {
        let sys_vendor = self.read_dmi_field("sys_vendor");
        let board_vendor = self.read_dmi_field("board_vendor");
        let board_name = self.read_dmi_field("board_name");
        let product_sku = self.read_dmi_field("product_sku");
        let manufacturer = classify_manufacturer(&sys_vendor, &board_vendor);
        let (mut igpu_model, mut dgpu_model) = self.detect_pci_gpus();
        let latest = self.latest_telemetry.read().unwrap().clone();
        if dgpu_model.is_empty() {
            dgpu_model = latest
                .as_ref()
                .and_then(|s| s.dgpu.as_ref())
                .map(|d| d.vendor.as_str().to_string())
                .filter(|v| v != "unknown")
                .unwrap_or_default();
        }
        if igpu_model.is_empty() {
            igpu_model = latest
                .as_ref()
                .and_then(|s| s.igpu.as_ref())
                .map(|g| g.vendor.as_str().to_string())
                .filter(|v| v != "unknown")
                .unwrap_or_default();
        }
        let laptop_model = self.get_device_name();
        json!({
            "cpuModel": self.read_cpu_model(),
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

    fn is_device_supported(&self) -> bool {
        self.capabilities.device.vendor != DeviceVendor::Unsupported
    }

    fn get_display_modes_json(&self) -> String {
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

    fn get_is_x11(&self) -> bool {
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

    fn get_force_yuv420_output_switch_available(&self) -> bool {
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

    fn set_display_refresh_rate(&self, _display: &str, _refresh_rate: i32) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        false
    }

    fn get_prime_state(&self) -> String {
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
            if let Ok(output) = std::process::Command::new(cmd).arg("query").output() {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    if let Some(mapped) = Self::map_prime_token(&stdout) {
                        return mapped;
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

    fn get_driver_health_json(&self) -> String {
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

    fn tuxedo_wmi_available(&self) -> bool {
        self.platform.is_available()
    }

    fn fan_hwmon_available(&self) -> bool {
        self.capabilities.device.number_of_fans > 0
    }

    fn get_fan_data_cpu(&self) -> HashMap<String, OwnedValue> {
        let cached = self.latest_fans.read().unwrap().clone();
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
        let cached = self.latest_fans.read().unwrap().clone();
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
        let cached = self.latest_fans.read().unwrap().clone();
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
        self.capabilities.device.webcam_switch && self.platform.is_available()
    }

    fn get_webcam_sw_status(&self) -> bool {
        self.webcam_sw_available() && self.platform.read_webcam().unwrap_or(false)
    }

    fn set_webcam(&self, enabled: bool) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        self.set_webcam_internal(enabled)
    }

    fn get_display_brightness(&self) -> i32 {
        let Some((dev, max_val)) = self.discover_backlight_device() else {
            return -1;
        };
        let raw = self
            .sysfs
            .read(&dev.join("actual_brightness"))
            .or_else(|_| self.sysfs.read(&dev.join("brightness")))
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok());
        match raw {
            Some(v) if v >= 0 => ((f64::from(v) / f64::from(max_val)) * 100.0)
                .round()
                .clamp(0.0, 100.0) as i32,
            _ => -1,
        }
    }

    fn get_keyboard_backlight_control_supported(&self) -> bool {
        let mut ctrl = self.keyboard_ctrl.lock().unwrap();
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
        if let Some(ref snap) = *self.latest_telemetry.read().unwrap() {
            if let Some(ref dgpu) = snap.dgpu {
                return dgpu_to_legacy_json(dgpu).to_string();
            }
        }
        let mut absent = DGpuSnapshot::sleeping_nvidia(None, Instant::now());
        absent.is_sleeping = false;
        dgpu_to_legacy_json(&absent).to_string()
    }

    fn get_igpu_info_values_json(&self) -> String {
        if let Some(ref snap) = *self.latest_telemetry.read().unwrap() {
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

    fn get_cpu_power_values_json(&self) -> String {
        if !self.sensor_data_collection.load(Ordering::SeqCst) {
            return json!({ "powerDraw": -1.0 }).to_string();
        }
        if let Some(ref snap) = *self.latest_telemetry.read().unwrap() {
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
        self.mode_reapply_pending.swap(false, Ordering::SeqCst)
    }

    fn get_active_profile_json(&self) -> String {
        let p = self.active_profile.read().unwrap().clone();
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
        if self.power_devices.set_settings(val).is_err() {
            return false;
        }
        let effective_mode = self.current_decision().effective;
        self.power_devices.apply(effective_mode.mode_name());
        let pd_rev = self.power_devices.revision();
        *self.last_power_device_revision.lock().unwrap() = Some(pd_rev);
        let active = self.active_profile.read().unwrap().clone();
        match self.apply_cpu_settings(&active) {
            Ok(res) if res.fully_applied => {
                *self.cpu_policy_applied_rev.lock().unwrap() = Some((pd_rev, effective_mode));
            }
            _ => {
                *self.cpu_policy_applied_rev.lock().unwrap() = None;
            }
        }
        self.emit_signal(DbusSignalEvent::PowerDeviceStatusChanged {
            json: self.power_devices.status().to_string(),
        });
        true
    }

    fn scan_power_devices(&self) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let scanned = self.power_devices.scan();
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

    fn get_lightbar_settings_json(&self) -> String {
        self.lightbar_ctrl
            .lock()
            .unwrap()
            .settings_json()
            .to_string()
    }

    fn get_lightbar_status_json(&self) -> String {
        self.lightbar_ctrl
            .lock()
            .unwrap()
            .status_json()
            .to_string()
    }

    fn set_lightbar_settings_json(&self, json_str: &str) -> bool {
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
        let mut lb = self.lightbar_ctrl.lock().unwrap();
        let _ = lb.configure(&val);
        let actual_json = lb.settings_json().to_string();
        drop(lb);
        self.emit_signal(DbusSignalEvent::LightbarSettingsChanged { json: actual_json });
        true
    }

    fn trigger_lightbar_animation(&self, force: bool) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        self.lightbar_ctrl.lock().unwrap().trigger(force)
    }

    fn set_manual_power_mode(&self, mode_val: &str) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let ok = self.power_policy.write().unwrap().set_manual_mode(mode_val);
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
            && self.disconnected_senders.lock().unwrap().0.contains(owner)
        {
            return false;
        }
        let ok = self
            .power_policy
            .write()
            .unwrap()
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
            .power_policy
            .write()
            .unwrap()
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
            *self.temp_profile_name.write().unwrap() = None;
            return true;
        }
        for _ in 0..3 {
            let found = {
                let settings = self.settings.read().unwrap();
                settings.get_profile(profile_name).cloned()
            };
            let Some(prof) = found else {
                return false;
            };
            match self.apply_profile_internal(&prof, true, None, None, None, Some(&prof)) {
                ApplyOutcome::Committed => {
                    *self.temp_profile_name.write().unwrap() = Some(prof.id);
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
                let settings = self.settings.read().unwrap();
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
        let Ok(profile) = serde_json::from_str::<UccProfile>(profile_json) else {
            return false;
        };
        let initial_state = self.get_power_state();
        match self.apply_profile_internal(&profile, true, None, Some(&initial_state), None, None) {
            ApplyOutcome::Committed => {
                if self.get_power_state() == initial_state {
                    *self.last_applied_profile_state.lock().unwrap() = Some(initial_state);
                } else {
                    *self.last_applied_profile_state.lock().unwrap() = None;
                }
                true
            }
            ApplyOutcome::SupersededReconciled => true,
            ApplyOutcome::StaleRevision | ApplyOutcome::Failed => false,
        }
    }

    fn get_profiles_json(&self) -> String {
        let mut list: Vec<UccProfile> = self
            .settings
            .read()
            .unwrap()
            .profiles
            .values()
            .cloned()
            .collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_string())
    }

    fn get_custom_profiles_json(&self) -> String {
        let mut list: Vec<UccProfile> = self
            .settings
            .read()
            .unwrap()
            .profiles
            .values()
            .filter(|p| !is_builtin_profile_id(&p.id))
            .cloned()
            .collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_string())
    }

    fn set_fan_profile_cpu(&self, points_json: &str) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let Some(points) = Self::parse_fan_points_json(points_json) else {
            return false;
        };
        if FanCurve::new(points.clone()).and_then(|c| c.validate()).is_err() {
            return false;
        }
        let gpu_points = self.active_profile.read().unwrap().fan.table_gpu.clone();
        let curves = ucc_core::TemporaryFanCurves {
            cpu: points,
            gpu: gpu_points,
            ..Default::default()
        };
        self.fan_controller
            .lock()
            .unwrap()
            .apply_temporary_curves(curves);
        true
    }

    fn set_fan_profile_dgpu(&self, points_json: &str) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let Some(points) = Self::parse_fan_points_json(points_json) else {
            return false;
        };
        if FanCurve::new(points.clone()).and_then(|c| c.validate()).is_err() {
            return false;
        }
        let cpu_points = self.active_profile.read().unwrap().fan.table_cpu.clone();
        let curves = ucc_core::TemporaryFanCurves {
            cpu: cpu_points,
            gpu: points,
            ..Default::default()
        };
        self.fan_controller
            .lock()
            .unwrap()
            .apply_temporary_curves(curves);
        true
    }

    fn apply_fan_profiles(&self, fan_profiles_json: &str) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let Ok(dto) = serde_json::from_str::<ApplyFanProfilesDto>(fan_profiles_json) else {
            return false;
        };

        let mut any_curve = false;
        if let Some(ref pts) = dto.cpu {
            if !pts.is_empty() {
                if FanCurve::new(pts.clone()).and_then(|c| c.validate()).is_err() {
                    return false;
                }
                any_curve = true;
            }
        }
        if let Some(ref pts) = dto.gpu {
            if !pts.is_empty() {
                if FanCurve::new(pts.clone()).and_then(|c| c.validate()).is_err() {
                    return false;
                }
                any_curve = true;
            }
        }
        if let Some(ref pts) = dto.water_cooler_fan {
            if !pts.is_empty() {
                if FanCurve::new(pts.clone()).and_then(|c| c.validate()).is_err() {
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
            let active_fan = self.active_profile.read().unwrap().fan.clone();
            ucc_core::TemporaryFanCurves {
                cpu: dto.cpu.unwrap_or(active_fan.table_cpu),
                gpu: dto.gpu.unwrap_or(active_fan.table_gpu),
                water_cooler_fan: dto
                    .water_cooler_fan
                    .unwrap_or(active_fan.table_water_cooler_fan),
                pump: dto.pump.unwrap_or(active_fan.table_pump),
            }
        } else if let Some(ref fp_id) = fan_profile_id {
            let resolved = builtin_fan_profiles()
                .into_iter()
                .find(|fp| fp.id == *fp_id || fp.name == *fp_id)
                .or_else(|| {
                    self.custom_fan_profiles
                        .read()
                        .unwrap()
                        .get(fp_id)
                        .cloned()
                })
                .or_else(|| {
                    let s = self.settings.read().unwrap();
                    let entry = s.custom_fan_profiles.as_ref()?.get(fp_id)?;
                    normalize_and_parse_custom_fan_profile(fp_id, &entry.name, &entry.json)
                        .map(|(fp, _)| fp)
                });
            let Some(fp) = resolved else {
                return false;
            };
            ucc_core::TemporaryFanCurves {
                cpu: fp.table_cpu,
                gpu: fp.table_gpu,
                water_cooler_fan: fp.table_water_cooler_fan,
                pump: fp.table_pump,
            }
        } else {
            return false;
        };

        self.fan_controller
            .lock()
            .unwrap()
            .apply_temporary_curves(curves);

        if let Some(fp_id) = fan_profile_id {
            let updated = {
                let mut active = self.active_profile.write().unwrap();
                active.fan.fan_profile = fp_id;
                active.clone()
            };
            self.emit_profile_changed(&updated);
        }

        self.tick_fans_once();
        true
    }

    fn revert_fan_profiles(&self) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        self.fan_controller.lock().unwrap().clear_temporary_curves();
        true
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
        if ucc_config::is_read_only_preview() || self.shutting_down.load(Ordering::SeqCst) {
            return false;
        }
        if is_builtin_profile_id(profile_id) {
            return false;
        }
        let fallback_id = default_office_profile().id;
        self.update_settings(|s| {
            s.profiles.remove(profile_id)?;
            for target in s.state_map.values_mut() {
                if target == profile_id {
                    *target = fallback_id.clone();
                }
            }
            Some(())
        })
        .is_some()
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
        let custom = self.custom_fan_profiles.read().unwrap();
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
        if ucc_config::is_read_only_preview() || self.shutting_down.load(Ordering::SeqCst) {
            return false;
        }
        if name.trim().is_empty() {
            return false;
        }
        let Ok(parsed_val) = serde_json::from_str::<Value>(json_str) else {
            return false;
        };
        let Some((fp, normalized_val)) =
            normalize_and_parse_custom_fan_profile(name, name, &parsed_val)
        else {
            return false;
        };
        if self
            .update_settings(|s| {
                s.custom_fan_profiles
                    .get_or_insert_with(HashMap::new)
                    .insert(
                        name.to_string(),
                        CustomFanProfile {
                            id: name.to_string(),
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
        self.custom_fan_profiles
            .write()
            .unwrap()
            .insert(name.to_string(), fp);
        true
    }

    fn get_settings_json(&self) -> String {
        self.settings
            .read()
            .unwrap()
            .to_json_pretty()
            .unwrap_or_else(|_| "{}".to_string())
    }

    fn set_state_map(&self, state: &str, profile_id: &str) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let Ok(parsed) = state.parse::<ProfileState>() else {
            return false;
        };
        let key = parsed.as_str().to_string();
        for _ in 0..3 {
            let resolved = {
                let s = self.settings.read().unwrap();
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
                let s = self.settings.read().unwrap();
                for (k, v) in &map {
                    let Ok(st) = k.parse::<ProfileState>() else {
                        return false;
                    };
                    let Some(prof) = s.get_profile(v).cloned() else {
                        return false;
                    };
                    if prof.validate().is_err() {
                        return false;
                    }
                    let key = st.as_str().to_string();
                    updates.push((key.clone(), prof.id.clone()));
                    resolved_profiles.push((key, prof));
                }
            }
            let cur_state = self.get_power_state();
            if let Some((_, active_prof)) = resolved_profiles
                .into_iter()
                .find(|(k, _)| k == &cur_state)
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
                return self
                    .update_settings(|s| {
                        if updates
                            .iter()
                            .any(|(_, prof_id)| !s.profiles.contains_key(prof_id))
                        {
                            return None;
                        }
                        for (k, prof_id) in &updates {
                            s.state_map.insert(k.clone(), prof_id.clone());
                        }
                        Some(())
                    })
                    .is_some();
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
        self.keyboard_ctrl.lock().unwrap().capabilities_json()
    }

    fn get_keyboard_backlight_states_json(&self) -> String {
        self.keyboard_ctrl.lock().unwrap().current_states_json()
    }

    fn set_keyboard_backlight_states_json(&self, keyboard_backlight_states_json: &str) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let Ok(states) =
            serde_json::from_str::<Vec<KeyboardBacklightState>>(keyboard_backlight_states_json)
        else {
            return false;
        };
        if self
            .update_settings(|s| {
                s.keyboard_backlight_states = Some(states.clone());
                Some(())
            })
            .is_none()
        {
            return false;
        }
        let mut ctrl = self.keyboard_ctrl.lock().unwrap();
        if ctrl.apply_states(&states).is_err() {
            return false;
        }
        drop(ctrl);
        self.lightbar_ctrl.lock().unwrap().keyboard_changed();
        true
    }

    fn get_custom_keyboard_profiles_json(&self) -> String {
        let settings = self.settings.read().unwrap();
        if let Some(ref map) = settings.custom_keyboard_profiles {
            let mut entries: Vec<&CustomKeyboardProfile> = map.values().collect();
            entries.sort_by(|a, b| a.id.cmp(&b.id));
            return serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string());
        }
        "[]".to_string()
    }

    fn save_custom_keyboard_profile(&self, id: &str, name: &str, json_str: &str) -> bool {
        if ucc_config::is_read_only_preview() || self.shutting_down.load(Ordering::SeqCst) {
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
                .get_or_insert_with(HashMap::new)
                .insert(
                    id.to_string(),
                    CustomKeyboardProfile {
                        id: id.to_string(),
                        name: name.to_string(),
                        json: parsed,
                    },
                );
            Some(())
        })
        .is_some()
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
        let settings = self.settings.read().unwrap();
        if let Some(ref map) = settings.custom_fan_profiles {
            let mut entries: Vec<&CustomFanProfile> = map.values().collect();
            entries.sort_by(|a, b| a.id.cmp(&b.id));
            return serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string());
        }
        "[]".to_string()
    }

    fn save_custom_fan_profile(&self, id: &str, name: &str, json_str: &str) -> bool {
        if ucc_config::is_read_only_preview() || self.shutting_down.load(Ordering::SeqCst) {
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
                        CustomFanProfile {
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
        self.custom_fan_profiles
            .write()
            .unwrap()
            .insert(id.to_string(), fp);
        true
    }

    fn delete_custom_fan_profile(&self, id: &str) -> bool {
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
        self.custom_fan_profiles.write().unwrap().remove(id);
        true
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
        let Some(prev) = self.update_settings(|s| {
            let p = s.charging_profile.clone();
            s.charging_profile = Some(desc.to_string());
            Some(p)
        }) else {
            return false;
        };
        if self.sysfs.write(&current_path, desc).is_err() {
            let _ = self.update_settings(|s| {
                if s.charging_profile.as_deref() == Some(desc) {
                    s.charging_profile = prev;
                    Some(())
                } else {
                    None
                }
            });
            return false;
        }
        true
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
        let Some(prev) = self.update_settings(|s| {
            let p = s.charging_priority.clone();
            s.charging_priority = Some(desc.to_string());
            Some(p)
        }) else {
            return false;
        };
        if self.sysfs.write(&current_path, desc).is_err() {
            let _ = self.update_settings(|s| {
                if s.charging_priority.as_deref() == Some(desc) {
                    s.charging_priority = prev;
                    Some(())
                } else {
                    None
                }
            });
            return false;
        }
        true
    }

    fn get_charge_start_available_thresholds(&self) -> String {
        let Some(bat) = self.discover_battery_dir() else {
            return "[]".to_string();
        };
        if !self.sysfs.exists(&bat.join("charge_control_start_threshold")) {
            return "[]".to_string();
        }
        let vals: Vec<i32> = (0..=99).collect();
        serde_json::to_string(&vals).unwrap_or_else(|_| "[]".to_string())
    }

    fn get_charge_end_available_thresholds(&self) -> String {
        let Some(bat) = self.discover_battery_dir() else {
            return "[]".to_string();
        };
        if !self.sysfs.exists(&bat.join("charge_control_end_threshold")) {
            return "[]".to_string();
        }
        let vals: Vec<i32> = (1..=100).collect();
        serde_json::to_string(&vals).unwrap_or_else(|_| "[]".to_string())
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
        let Some(prev) = self.update_settings(|s| {
            let p = s.charge_start_threshold;
            s.charge_start_threshold = Some(value as u8);
            Some(p)
        }) else {
            return false;
        };
        if self.sysfs.write(&path, &value.to_string()).is_err() {
            let _ = self.update_settings(|s| {
                if s.charge_start_threshold == Some(value as u8) {
                    s.charge_start_threshold = prev;
                    Some(())
                } else {
                    None
                }
            });
            return false;
        }
        true
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
        let Some(prev) = self.update_settings(|s| {
            let p = s.charge_end_threshold;
            s.charge_end_threshold = Some(value as u8);
            Some(p)
        }) else {
            return false;
        };
        if self.sysfs.write(&path, &value.to_string()).is_err() {
            let _ = self.update_settings(|s| {
                if s.charge_end_threshold == Some(value as u8) {
                    s.charge_end_threshold = prev;
                    Some(())
                } else {
                    None
                }
            });
            return false;
        }
        true
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
        let Some(prev) = self.update_settings(|s| {
            let p = s.charge_type.clone();
            s.charge_type = Some(trimmed.to_string());
            Some(p)
        }) else {
            return false;
        };
        if self.sysfs.write(&path, trimmed).is_err() {
            let _ = self.update_settings(|s| {
                if s.charge_type.as_deref() == Some(trimmed) {
                    s.charge_type = prev;
                    Some(())
                } else {
                    None
                }
            });
            return false;
        }
        true
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
        if let Ok(mut col) = self.telemetry.lock() {
            col.set_dgpu_d0_metrics(status);
        }
    }

    fn get_nvidia_power_ctrl_default_power_limit(&self) -> i32 {
        if let Some((_, base_w)) = *self.nvidia_default_power_limit_w.lock().unwrap() {
            return base_w.round() as i32;
        }
        self.latest_telemetry
            .read()
            .unwrap()
            .as_ref()
            .and_then(|s| s.dgpu.as_ref())
            .and_then(|d| {
                d.default_power_limit_w
                    .or(d.max_power_limit_w)
                    .or(d.enforced_power_limit_w)
            })
            .map(|w| w.round() as i32)
            .unwrap_or(0)
    }

    fn get_nvidia_power_ctrl_max_power_limit(&self) -> i32 {
        self.latest_telemetry
            .read()
            .unwrap()
            .as_ref()
            .and_then(|s| s.dgpu.as_ref())
            .and_then(|d| d.max_power_limit_w)
            .map(|w| w.round() as i32)
            .unwrap_or(0)
    }

    fn get_nvidia_power_ctrl_available(&self) -> bool {
        if !self.capabilities.device.ctgp_supported {
            return false;
        }
        let has_nvml_limit = self
            .latest_telemetry
            .read()
            .unwrap()
            .as_ref()
            .and_then(|s| s.dgpu.as_ref())
            .is_some_and(|d| d.max_power_limit_w.is_some());
        has_nvml_limit || self.nvidia_ctgp_sysfs_path().is_some()
    }

    fn set_nvidia_power_offset(&self, offset: i32) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        if !self.get_nvidia_power_ctrl_available() {
            return false;
        }
        if let Some(ref nvml) = self.nvml {
            let dgpu = self
                .latest_telemetry
                .read()
                .unwrap()
                .as_ref()
                .and_then(|s| s.dgpu.clone());
            if let Some(dgpu) = dgpu {
                if let Some(ref bdf) = dgpu.bdf {
                    let immutable_base = dgpu
                        .default_power_limit_w
                        .or(dgpu.max_power_limit_w)
                        .or(dgpu.enforced_power_limit_w);
                    let baseline_w = {
                        let mut stored = self.nvidia_default_power_limit_w.lock().unwrap();
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
                            *self.nvidia_power_offset.lock().unwrap() = Some(offset);
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
                *self.nvidia_power_offset.lock().unwrap() = Some(offset);
                return true;
            }
        }

        false
    }

    fn get_available_governors(&self) -> String {
        let Some(first) = self.discover_cpu_dirs().into_iter().next() else {
            return "[]".to_string();
        };
        let tokens = self.read_whitespace_tokens(&first.join("cpufreq/scaling_available_governors"));
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
        self.read_online_cpu_count()
    }

    fn get_cpu_topology_json(&self) -> String {
        if self.sys_root == Path::new("/") {
            let mut topo = ucc_hardware::CpuTopology::default();
            let cores = topo.read();
            if !cores.is_empty() {
                return serde_json::to_string(&cores).unwrap_or_else(|_| "[]".to_string());
            }
        }
        let all_cpus = self.discover_all_cpu_dirs();
        let rows: Vec<Value> = all_cpus
            .into_iter()
            .map(|(id, dir)| {
                let online_path = dir.join("online");
                let hotpluggable = self.sysfs.exists(&online_path);
                let online = if hotpluggable {
                    self.sysfs
                        .read(&online_path)
                        .ok()
                        .is_some_and(|v| v.trim() == "1")
                } else {
                    true
                };
                let physical_core = if online {
                    self.sysfs
                        .read(&dir.join("topology/core_id"))
                        .ok()
                        .and_then(|s| s.trim().parse::<u32>().ok())
                        .unwrap_or(id)
                } else {
                    id
                };
                let siblings = if online {
                    self.sysfs
                        .read(&dir.join("topology/thread_siblings_list"))
                        .ok()
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| id.to_string())
                } else {
                    id.to_string()
                };
                json!({
                    "id": id,
                    "online": online,
                    "hotpluggable": hotpluggable,
                    "type": "unknown",
                    "physicalCore": physical_core,
                    "siblings": siblings,
                })
            })
            .collect();
        serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string())
    }

    fn get_water_cooler_available(&self) -> bool {
        self.water_cooler_available.load(Ordering::SeqCst)
    }

    fn get_water_cooler_connected(&self) -> bool {
        self.water_cooler_connected.load(Ordering::SeqCst)
            || self.cooler_controller.lock().unwrap().state() == CoolerState::Connected
    }

    fn get_water_cooler_fan_speed(&self) -> i32 {
        match self.cooler_controller.lock().unwrap().desired_fan_target() {
            FanTarget::Speed(pct) => i32::from(pct),
            FanTarget::Off | FanTarget::Unset => 0,
        }
    }

    fn get_water_cooler_pump_level(&self) -> i32 {
        match self.cooler_controller.lock().unwrap().desired_pump_target() {
            PumpTarget::Voltage(PumpVoltage::Off) | PumpTarget::Unset => 0,
            PumpTarget::Voltage(PumpVoltage::V7) => 1,
            PumpTarget::Voltage(PumpVoltage::V8) => 2,
            PumpTarget::Voltage(PumpVoltage::V11) => 3,
            PumpTarget::Voltage(PumpVoltage::V12) => 4,
        }
    }

    fn enable_water_cooler(&self, enable: bool) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        if !self.water_cooler_available.load(Ordering::SeqCst) {
            return false;
        }
        let prev = self.water_cooler_enabled.swap(enable, Ordering::SeqCst);
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
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let Ok(pct) = u8::try_from(duty_cycle_percent) else {
            return false;
        };
        self.cooler_controller
            .lock()
            .unwrap()
            .set_fan_speed(pct)
            .is_ok()
    }

    fn set_water_cooler_pump_voltage(&self, voltage: i32) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let pump_v = match voltage {
            0 => PumpVoltage::Off,
            1 | 7 => PumpVoltage::V7,
            2 | 8 => PumpVoltage::V8,
            3 | 11 => PumpVoltage::V11,
            _ => return false,
        };
        self.cooler_controller
            .lock()
            .unwrap()
            .set_pump_voltage(pump_v)
            .is_ok()
    }

    fn set_water_cooler_led_color(&self, red: i32, green: i32, blue: i32, mode_val: i32) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let (Ok(r), Ok(g), Ok(b)) = (u8::try_from(red), u8::try_from(green), u8::try_from(blue))
        else {
            return false;
        };
        let state = match mode_val {
            0 => RgbState::Static,
            1 => RgbState::Breathe,
            2 => RgbState::Colorful,
            3 => RgbState::BreatheColor,
            _ => RgbState::Static,
        };
        self.cooler_controller
            .lock()
            .unwrap()
            .set_rgb(r, g, b, state)
            .is_ok()
    }

    fn turn_off_water_cooler_led(&self) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        self.cooler_controller.lock().unwrap().turn_off_led().is_ok()
    }

    fn turn_off_water_cooler_fan(&self) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        self.cooler_controller.lock().unwrap().turn_off_fan().is_ok()
    }

    fn turn_off_water_cooler_pump(&self) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        self.cooler_controller.lock().unwrap().turn_off_pump().is_ok()
    }

    fn is_water_cooler_auto_control_enabled(&self) -> bool {
        self.cooler_controller.lock().unwrap().mode() == CoolerMode::Auto
    }

    fn set_water_cooler_auto_control(&self, enabled: bool) -> bool {
        let Some(_op_guard) = self.enter_hardware_op() else {
            return false;
        };
        let mut ctrl = self.cooler_controller.lock().unwrap();
        let next_mode = if enabled {
            CoolerMode::Auto
        } else {
            CoolerMode::Manual
        };
        let changed = ctrl.mode() != next_mode;
        ctrl.set_mode(next_mode);
        drop(ctrl);
        if changed {
            self.emit_signal(DbusSignalEvent::WaterCoolerStatusChanged {
                status: if enabled { "auto" } else { "manual" }.to_string(),
            });
        }
        true
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
        self.latest_telemetry
            .read()
            .unwrap()
            .as_ref()
            .and_then(|s| s.cpu.freq_mhz)
            .map(|f| f as i32)
            .unwrap_or(-1)
    }
}
