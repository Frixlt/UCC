// SPDX-License-Identifier: GPL-3.0-or-later

//! Modular daemon backend orchestrating hardware controllers, telemetry, lighting, config,
//! power policy, and cooling into [`ucc_dbus::UccdBackend`].

mod cooling;
mod cpu;
mod dbus_impl;
mod display_gpu;
mod lighting_power;
mod profile_tx;

pub use cooling::{build_default_custom_fan_catalog, run_water_cooler_ble_loop};

use crate::lid::{LidController, LidPolicyConfig};
use crate::sleep_state::SleepStateStore;
use cooling::{default_custom_fan_profiles, normalize_and_parse_custom_fan_profile};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;
use tracing::error;
use ucc_config::{
    create_default_settings, default_high_performance_profile, ConfigManager, UccSettings,
};
use ucc_core::{FanProfile, PowerPolicy, ProfileState, TemporaryFanCurves, UccProfile};
use ucc_hardware::{
    build_laptop_model, classify_manufacturer, CoolerController, CpuTopology, DeviceVendor,
    DmiInfo, FanController, FanTickSnapshot, LaptopManufacturer, MetricsHistoryStore,
    MockHardwareIo, MockPlatformBackend, NvidiaPowerGuard, NvmlWrapper, PlatformBackend,
    PlatformCapabilities, PowerDeviceController, RealSysfs, Sysfs, TelemetryCollector,
    TelemetryConfig, TelemetrySnapshot, TuxedoIoBackend, TuxedoIoDevice,
};
use ucc_lighting::{
    KeyboardBacklightController, LightbarController, LightingTransport, SysfsLedTransport,
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
    amd_boost: Option<String>,
    hwp_dynamic_boost: Option<String>,
}

pub(crate) fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
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

#[derive(Debug, Clone, Default)]
pub(super) struct ChargingCommittedState {
    pub(super) start_threshold: Option<u8>,
    pub(super) end_threshold: Option<u8>,
    pub(super) charge_type: Option<String>,
    pub(super) charging_profile: Option<String>,
    pub(super) charging_priority: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct CommittedProfileRecord {
    pub(super) profile: UccProfile,
    pub(super) hw_tdps: Vec<Option<i32>>,
    pub(super) hw_odm: Option<i32>,
    pub(super) hw_ctgp: Option<i32>,
    pub(super) hw_kb: Option<lighting_power::KeyboardRollbackSnapshot>,
}

#[derive(Default)]
pub(super) struct SupersededUndo<'a> {
    pub(super) hw_odm_prev: Option<i32>,
    pub(super) wrote_odm: bool,
    pub(super) prev_tdps: &'a [Option<i32>],
    pub(super) tdps_written: usize,
    pub(super) cpu_rollback: Vec<(PathBuf, String)>,
    pub(super) charging_rb: Option<&'a display_gpu::ChargingRollback>,
    pub(super) prev_ctgp: Option<i32>,
    pub(super) wrote_ctgp: bool,
    pub(super) prev_kb: Option<&'a lighting_power::KeyboardRollbackSnapshot>,
    pub(super) wrote_kb: bool,
}

pub(super) struct InFlightTxGuard<'a> {
    pub(super) backend: &'a LiveUccdBackend,
    pub(super) tx_id: u64,
}

impl Drop for InFlightTxGuard<'_> {
    fn drop(&mut self) {
        let mut tdps = self
            .backend
            .lock_mutex(&self.backend.in_flight_tdp_baselines);
        tdps.retain(|(id, _, _)| *id != self.tx_id);
        drop(tdps);
        let mut odms = self
            .backend
            .lock_mutex(&self.backend.in_flight_odm_baselines);
        odms.retain(|(id, _)| *id != self.tx_id);
        drop(odms);
        let mut ctgps = self
            .backend
            .lock_mutex(&self.backend.in_flight_ctgp_baselines);
        ctgps.retain(|(id, _)| *id != self.tx_id);
        drop(ctgps);
        let mut kbs = self
            .backend
            .lock_mutex(&self.backend.in_flight_kb_baselines);
        kbs.retain(|(id, _)| *id != self.tx_id);
        drop(kbs);
        self.backend.tdp_write_seq.fetch_add(1, Ordering::SeqCst);
        self.backend.odm_write_seq.fetch_add(1, Ordering::SeqCst);
        self.backend.ctgp_write_seq.fetch_add(1, Ordering::SeqCst);
        self.backend.kb_write_seq.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ApplyOutcome {
    Committed,
    SupersededReconciled,
    StaleRevision,
    Failed,
}

pub(super) struct CpuApplyResult {
    pub(super) rollback: Vec<(PathBuf, String)>,
    pub(super) fully_applied: bool,
}

pub(super) type SystemInfoStaticCache = (
    String,
    String,
    String,
    String,
    LaptopManufacturer,
    String,
    String,
    String,
);

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
    pub(super) device_name: String,
    pub(super) platform: Arc<dyn PlatformBackend>,
    pub(super) sysfs: Arc<dyn Sysfs>,
    pub(super) sys_root: PathBuf,
    pub(super) capabilities: PlatformCapabilities,
    pub(super) config_manager: Mutex<ConfigManager>,
    pub(super) settings: RwLock<UccSettings>,
    pub(super) config_recovered: AtomicBool,
    pub(super) has_persisted_config: bool,
    pub(super) active_profile: RwLock<UccProfile>,
    pub(super) temp_profile_name: RwLock<Option<String>>,
    pub(super) custom_fan_profiles: RwLock<BTreeMap<String, FanProfile>>,

    pub(super) history: Arc<MetricsHistoryStore>,
    pub(super) telemetry: Arc<Mutex<TelemetryCollector>>,
    pub(super) latest_telemetry: RwLock<Option<TelemetrySnapshot>>,

    pub(super) fan_controller: Arc<Mutex<FanController>>,
    pub(super) temporary_fan_curves: Mutex<Option<TemporaryFanCurves>>,
    pub(super) latest_fans: RwLock<Option<FanTickSnapshot>>,
    pub(super) fan_control_enabled: AtomicBool,

    pub(super) cooler_controller: Arc<Mutex<CoolerController>>,
    pub(super) water_cooler_enabled: AtomicBool,
    pub(super) water_cooler_user_disabled: AtomicBool,
    pub(super) water_cooler_available: AtomicBool,
    pub(super) water_cooler_connected: AtomicBool,
    pub(super) cooler_suspending: AtomicBool,

    pub(super) keyboard_ctrl: Arc<Mutex<KeyboardBacklightController>>,
    pub(super) lightbar_ctrl: Arc<Mutex<LightbarController>>,
    pub(super) lid_closed: AtomicBool,
    pub(super) lid_saved_keyboard_states: Mutex<Option<Vec<ucc_core::KeyboardBacklightState>>>,
    pub(super) lid_saved_wc_rgb: Mutex<Option<ucc_hardware::RgbTarget>>,
    pub(super) power_devices: PowerDeviceController,
    pub(super) power_policy: RwLock<PowerPolicy>,
    pub(super) lid_controller: Mutex<LidController>,

    pub(super) sensor_data_collection: AtomicBool,
    pub(super) dgpu_d0_metrics: AtomicBool,
    pub(super) shutting_down: AtomicBool,
    pub(super) shutdown_wait_expired: AtomicBool,
    pub(super) prime_degraded: AtomicBool,
    pub(super) telemetry_poll_in_flight: AtomicBool,
    pub(super) hardware_activation_pending: AtomicBool,
    pub(super) hardware_activated: AtomicBool,
    pub(super) in_flight_ops_wait: Arc<(Mutex<usize>, Condvar)>,
    pub(super) nvml: Option<Arc<NvmlWrapper>>,
    pub(super) nvidia_default_power_limit_w: Mutex<Option<(String, f64)>>,
    pub(super) nvidia_power_offset: Mutex<Option<i32>>,
    pub(super) mode_reapply_pending: AtomicBool,
    pub(super) initial_cpu_defaults: Mutex<Option<CpuDefaultsSnapshot>>,
    pub(super) initial_tdp_defaults: Mutex<Option<Vec<Option<i32>>>>,
    pub(super) cached_real_topology: Mutex<Option<CpuTopology>>,
    pub(super) cached_mock_core_types: Mutex<HashMap<u32, String>>,
    pub(super) cached_system_info: Mutex<Option<SystemInfoStaticCache>>,
    pub(super) profile_commit_epoch: AtomicU64,
    pub(super) next_profile_tx_id: AtomicU64,
    pub(super) next_custom_profile_seq: AtomicU64,
    pub(super) tdp_write_seq: AtomicU64,
    pub(super) odm_write_seq: AtomicU64,
    pub(super) charging_write_seq: AtomicU64,
    pub(super) charging_op_seq: AtomicU64,
    pub(super) ctgp_write_seq: AtomicU64,
    pub(super) kb_write_seq: AtomicU64,
    pub(super) sleep_state_pending_clear: AtomicBool,
    pub(super) cpu_reapply_attempts: AtomicU32,
    pub(super) in_flight_tdp_baselines: Mutex<Vec<(u64, usize, i32)>>,
    pub(super) in_flight_odm_baselines: Mutex<Vec<(u64, i32)>>,
    pub(super) in_flight_ctgp_baselines: Mutex<Vec<(u64, i32)>>,
    pub(super) in_flight_kb_baselines: Mutex<Vec<(u64, lighting_power::KeyboardRollbackSnapshot)>>,
    pub(super) latest_committed_profile: Mutex<Option<CommittedProfileRecord>>,
    pub(super) hw_reconcile_lock: Mutex<()>,
    pub(super) charging_commit_state: Mutex<ChargingCommittedState>,
    pub(super) disconnected_senders: Mutex<(HashSet<String>, VecDeque<String>)>,
    pub(super) cpu_policy_applied_rev: Mutex<Option<(u64, ProfileState)>>,
    pub(super) last_power_state: Mutex<Option<String>>,
    pub(super) last_applied_profile_state: Mutex<Option<String>>,
    pub(super) last_effective_mode: Mutex<Option<ProfileState>>,
    pub(super) last_policy_json: Mutex<Option<String>>,
    pub(super) last_power_device_revision: Mutex<Option<u64>>,
    pub(super) signal_tx: broadcast::Sender<DbusSignalEvent>,
}

impl LiveUccdBackend {
    /// Builds a backend from explicit dependencies (`MockPlatformBackend`, `MockSysfs`, temp config, etc.).
    pub fn new(deps: UccdDependencies) -> Self {
        let capabilities = deps.platform.capabilities();
        let read_path = deps.config_manager.resolve_read_path();
        let has_read_path = read_path.is_some();
        let (mut settings, recovered) = match read_path {
            Some(path) => match ConfigManager::load_with_recovery(&path) {
                Ok((s, was_rec)) => (s, was_rec),
                Err(_) => (create_default_settings(), true),
            },
            None => (create_default_settings(), false),
        };
        let has_persisted_config = has_read_path && !recovered;

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
        let mut base_fan_profile = FanProfile::new(
            &configured_initial_profile.fan.fan_profile,
            &configured_initial_profile.fan.fan_profile,
        );
        base_fan_profile.table_cpu = configured_initial_profile.fan.table_cpu.clone();
        base_fan_profile.table_gpu = configured_initial_profile.fan.table_gpu.clone();
        base_fan_profile.table_pump = configured_initial_profile.fan.table_pump.clone();
        base_fan_profile.table_water_cooler_fan = configured_initial_profile
            .fan
            .table_water_cooler_fan
            .clone();

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

        let power_wc_enables_wc = settings
            .state_map
            .get("power_wc")
            .and_then(|id| settings.get_profile(id))
            .is_some_and(|p| p.fan.enable_water_cooler);
        let initial_wc_enabled = wc_supported
            && (configured_initial_profile.fan.enable_water_cooler || power_wc_enables_wc);

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
            has_persisted_config,
            active_profile: RwLock::new(active_profile),
            temp_profile_name: RwLock::new(None),
            custom_fan_profiles: RwLock::new(custom_fan_map),
            history,
            telemetry,
            latest_telemetry: RwLock::new(None),
            fan_controller,
            temporary_fan_curves: Mutex::new(None),
            latest_fans: RwLock::new(None),
            fan_control_enabled: AtomicBool::new(fan_enabled),
            cooler_controller: Arc::new(Mutex::new(cooler_ctrl)),
            water_cooler_enabled: AtomicBool::new(initial_wc_enabled),
            water_cooler_user_disabled: AtomicBool::new(false),
            water_cooler_available: AtomicBool::new(wc_supported),
            water_cooler_connected: AtomicBool::new(false),
            cooler_suspending: AtomicBool::new(false),
            keyboard_ctrl: Arc::new(Mutex::new(keyboard_ctrl)),
            lightbar_ctrl: Arc::new(Mutex::new(lightbar_ctrl)),
            lid_closed: AtomicBool::new(false),
            lid_saved_keyboard_states: Mutex::new(None),
            lid_saved_wc_rgb: Mutex::new(None),
            power_devices: deps.power_devices,
            power_policy: RwLock::new(power_policy),
            lid_controller: Mutex::new(LidController::new(deps.lid_policy)),
            sensor_data_collection: AtomicBool::new(true),
            dgpu_d0_metrics: AtomicBool::new(true),
            shutting_down: AtomicBool::new(false),
            shutdown_wait_expired: AtomicBool::new(false),
            prime_degraded: AtomicBool::new(false),
            telemetry_poll_in_flight: AtomicBool::new(false),
            hardware_activation_pending: AtomicBool::new(false),
            hardware_activated: AtomicBool::new(false),
            in_flight_ops_wait: Arc::new((Mutex::new(0), Condvar::new())),
            nvml,
            nvidia_default_power_limit_w: Mutex::new(None),
            nvidia_power_offset: Mutex::new(None),
            mode_reapply_pending: AtomicBool::new(false),
            initial_cpu_defaults: Mutex::new(None),
            initial_tdp_defaults: Mutex::new(None),
            cached_real_topology: Mutex::new(None),
            cached_mock_core_types: Mutex::new(HashMap::new()),
            cached_system_info: Mutex::new(None),
            profile_commit_epoch: AtomicU64::new(0),
            next_profile_tx_id: AtomicU64::new(0),
            next_custom_profile_seq: AtomicU64::new(0),
            tdp_write_seq: AtomicU64::new(0),
            odm_write_seq: AtomicU64::new(0),
            charging_write_seq: AtomicU64::new(0),
            charging_op_seq: AtomicU64::new(0),
            ctgp_write_seq: AtomicU64::new(0),
            kb_write_seq: AtomicU64::new(0),
            sleep_state_pending_clear: AtomicBool::new(false),
            cpu_reapply_attempts: AtomicU32::new(0),
            in_flight_tdp_baselines: Mutex::new(Vec::new()),
            in_flight_odm_baselines: Mutex::new(Vec::new()),
            in_flight_ctgp_baselines: Mutex::new(Vec::new()),
            in_flight_kb_baselines: Mutex::new(Vec::new()),
            latest_committed_profile: Mutex::new(None),
            hw_reconcile_lock: Mutex::new(()),
            charging_commit_state: Mutex::new(ChargingCommittedState::default()),
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

    /// Probes real Linux hardware and system paths, degrading gracefully when optional drivers are absent.
    pub fn new_production() -> Self {
        let sysfs: Arc<dyn Sysfs> = Arc::new(RealSysfs);
        let dmi = DmiInfo::read_from_sysfs(sysfs.as_ref());
        let platform: Arc<dyn PlatformBackend> =
            match TuxedoIoDevice::open(TuxedoIoDevice::DEFAULT_PATH) {
                Ok(dev) => Arc::new(TuxedoIoBackend::new(dev, Some(&dmi), Some(sysfs.as_ref()))),
                Err(_) => {
                    let hw = MockHardwareIo::new();
                    hw.set_available(false);
                    let mock = MockPlatformBackend::new(PlatformCapabilities {
                        backend: ucc_hardware::PlatformBackendKind::Unsupported,
                        device: ucc_hardware::detect_device_capabilities(
                            &hw,
                            Some(&dmi),
                            Some(sysfs.as_ref()),
                        ),
                        tdp_channels: 0,
                        perf_profiles_available: 0,
                    });
                    mock.set_available(false);
                    Arc::new(mock)
                }
            };

        let manufacturer = classify_manufacturer(&dmi.sys_vendor, &dmi.board_vendor);
        let model =
            build_laptop_model(platform.capabilities().device.device_id, manufacturer, &dmi);

        let nvml = NvmlWrapper::new(NvidiaPowerGuard::default())
            .ok()
            .map(Arc::new);
        let telemetry_config = TelemetryConfig {
            sys_root: PathBuf::from("/"),
            nvml,
            ..Default::default()
        };

        let backend = Self::new(UccdDependencies {
            device_name: model,
            platform,
            sysfs: Arc::clone(&sysfs),
            lighting_transport: Arc::new(SysfsLedTransport::default()),
            config_manager: ConfigManager::new(),
            sys_root: PathBuf::from("/"),
            telemetry_config,
            power_devices: PowerDeviceController::default(),
            lid_policy: LidPolicyConfig::default(),
        });
        backend.load_aquaris_saved_state_from(Path::new(cooling::AQUARIS_CONFIG_PATH));
        backend
    }

    pub(super) fn is_vendor_unsupported(&self) -> bool {
        self.capabilities.device.vendor == DeviceVendor::Unsupported
    }

    pub(super) fn get_device_name_impl(&self) -> String {
        let name = self.device_name.trim();
        if name.is_empty() {
            "ucc-rusted Laptop".to_string()
        } else {
            name.to_string()
        }
    }

    pub(super) fn webcam_sw_available_impl(&self) -> bool {
        self.capabilities.device.webcam_switch && self.platform.is_available()
    }

    pub(super) fn set_webcam_internal(&self, enabled: bool) -> bool {
        !self.shutdown_wait_expired.load(Ordering::SeqCst)
            && self.webcam_sw_available_impl()
            && self.platform.set_webcam(enabled).is_ok()
    }

    pub(super) fn nested_sample_value(ts_ms: i64, data: i32) -> OwnedValue {
        let mut inner = HashMap::<String, OwnedValue>::new();
        inner.insert("timestamp".to_string(), OwnedValue::from(ts_ms));
        inner.insert("data".to_string(), OwnedValue::from(data));
        OwnedValue::try_from(ZValue::from(inner)).unwrap_or_else(|_| OwnedValue::from(data))
    }

    pub(super) fn fan_map(ts_ms: i64, temp: i32, speed: i32) -> HashMap<String, OwnedValue> {
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

    /// Marks that `DaemonRuntime::shutdown` timed out waiting for in-flight hardware work.
    pub fn mark_shutdown_wait_expired(&self) {
        self.shutdown_wait_expired.store(true, Ordering::SeqCst);
    }

    /// Returns true once shutdown has started or its wait deadline has expired (`DEEP-01`).
    pub(super) fn write_aborted_by_shutdown(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
            || self.shutdown_wait_expired.load(Ordering::SeqCst)
    }

    /// Records whether startup priming timed out (`DEEP-03`).
    pub fn mark_prime_degraded(&self, degraded: bool) {
        self.prime_degraded.store(degraded, Ordering::SeqCst);
    }

    /// Returns whether startup priming timed out (`DEEP-03`).
    pub fn is_prime_degraded(&self) -> bool {
        self.prime_degraded.load(Ordering::SeqCst)
    }

    /// Returns whether a telemetry poll is currently in flight (`DEEP-03`).
    pub fn is_telemetry_poll_in_flight(&self) -> bool {
        self.telemetry_poll_in_flight.load(Ordering::SeqCst)
    }

    pub(super) fn emit_signal(&self, event: DbusSignalEvent) {
        let _ = self.signal_tx.send(event);
    }

    pub(super) fn emit_profile_changed(&self, profile: &UccProfile) {
        self.emit_signal(DbusSignalEvent::ProfileChanged {
            profile_id: profile.id.clone(),
            keyboard_profile_id: profile
                .selected_keyboard_profile
                .clone()
                .unwrap_or_else(|| profile.keyboard.keyboard_profile_id.clone()),
            fan_profile_id: profile.fan.fan_profile.clone(),
        });
    }

    pub fn was_hardware_activated(&self) -> bool {
        self.hardware_activated.load(Ordering::SeqCst)
    }

    pub fn set_activation_pending(&self, pending: bool) {
        self.hardware_activation_pending
            .store(pending, Ordering::SeqCst);
    }

    pub(super) fn enter_hardware_op(&self) -> Option<HardwareOpGuard> {
        if ucc_config::is_read_only_preview() {
            return None;
        }
        let mut count = self.lock_mutex(&self.in_flight_ops_wait.0);
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

    pub(crate) fn enter_worker_hw_op(&self) -> Option<HardwareOpGuard> {
        let mut count = self.lock_mutex(&self.in_flight_ops_wait.0);
        if self.shutting_down.load(Ordering::SeqCst) {
            return None;
        }
        *count += 1;
        Some(HardwareOpGuard {
            pair: Arc::clone(&self.in_flight_ops_wait),
        })
    }

    pub fn wait_in_flight_hw_ops(&self) {
        let (lock, cvar) = &*self.in_flight_ops_wait;
        let mut count = self.lock_mutex(lock);
        while *count > 0 {
            count = cvar.wait(count).unwrap_or_else(|e| e.into_inner());
        }
    }

    pub(super) fn capture_initial_tdp_defaults(&self) {
        if !self.platform.is_available() || self.capabilities.tdp_channels == 0 {
            return;
        }
        let mut slot = self.lock_mutex(&self.initial_tdp_defaults);
        if slot.is_none() {
            let defaults = (0..self.capabilities.tdp_channels)
                .map(|i| self.platform.read_tdp(i).ok())
                .collect();
            *slot = Some(defaults);
        }
    }

    fn effective_sleep_state_store(&self) -> Option<SleepStateStore> {
        if self.sys_root == Path::new("/") {
            return Some(SleepStateStore::default());
        }
        let test_path = self.sys_root.join("run/ucc/sleep-state.json");
        if std::fs::symlink_metadata(&test_path).is_ok() {
            let owner_uid = unsafe { nix::libc::geteuid() };
            return Some(SleepStateStore::new(test_path, owner_uid));
        }
        None
    }

    /// Applies an already-validated [`SleepStateStore`] snapshot into RAM without deleting it (`FULL-27-01`).
    pub fn load_sleep_state_from(&self, store: &SleepStateStore) -> bool {
        if let Ok(Some(state)) = store.load() {
            let _ = self
                .write_rw(&self.power_policy)
                .set_manual_mode(&state.manual_mode);
            if self.water_cooler_available.load(Ordering::SeqCst) {
                self.water_cooler_enabled
                    .store(state.cooler_enabled, Ordering::SeqCst);
            }
            self.lock_mutex(&self.lightbar_ctrl).skip_welcome();
            self.sleep_state_pending_clear.store(true, Ordering::SeqCst);
            return true;
        }
        false
    }

    /// Loads volatile `/run/ucc/sleep-state.json` BEFORE `request_name` on production hosts (`ISSUE-04`, `FULL-27-01`).
    pub fn load_sleep_state_read_only(&self) -> bool {
        let Some(store) = self.effective_sleep_state_store() else {
            return false;
        };
        self.load_sleep_state_from(&store)
    }

    /// Deletes a previously loaded sleep snapshot once daemon hardware activation completes (`FULL-27-01`).
    pub fn clear_consumed_sleep_state(&self) {
        if ucc_config::is_read_only_preview() {
            return;
        }
        if !self.sleep_state_pending_clear.swap(false, Ordering::SeqCst) {
            return;
        }
        if let Some(store) = self.effective_sleep_state_store() {
            let _ = store.clear();
        }
    }

    /// Primes telemetry, CPU topology cache, and fan snapshots strictly read-only (`M8-PROD-02`).
    pub fn prime_read_only(&self) {
        let _ = self.read_cpu_topology_cached_json();
        if self.sensor_data_collection.load(Ordering::SeqCst)
            && self.read_rw(&self.latest_telemetry).is_none()
        {
            let _ = self.poll_telemetry_once();
        }
        if self.lock_mutex(&self.latest_committed_profile).is_none() {
            *self.write_rw(&self.active_profile) = Self::build_unapplied_hardware_profile(
                self.platform.as_ref(),
                self.sysfs.as_ref(),
                &self.sys_root,
                &self.capabilities,
            );
        }

        if self.read_rw(&self.latest_fans).is_none() {
            let num_fans = self.capabilities.device.number_of_fans;
            let mut temps = Vec::with_capacity(num_fans);
            let mut speeds = Vec::with_capacity(num_fans);
            for i in 0..num_fans {
                temps.push(self.platform.read_fan_temp(i).ok());
                speeds.push(self.platform.read_fan_speed_percent(i).ok());
            }
            *self.write_rw(&self.latest_fans) = Some(FanTickSnapshot {
                timestamp_ms: unix_ms(),
                temps_c: temps,
                calculated_speeds: speeds.clone(),
                speeds_percent: speeds,
                fallback_auto: !self.fan_control_enabled.load(Ordering::SeqCst),
            });
        }
    }

    /// Applies saved lighting, initial profile, charging settings, wakeup policy, and fan control
    /// AFTER exclusive D-Bus name ownership. Runs strictly passive when `DeviceVendor::Unsupported`.
    pub fn activate_hardware(&self) {
        if ucc_config::is_read_only_preview() || self.is_vendor_unsupported() {
            self.hardware_activated.store(true, Ordering::SeqCst);
            if !self.is_prime_degraded() && !self.is_telemetry_poll_in_flight() {
                self.prime_read_only();
            }
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
        self.capture_initial_tdp_defaults();
        if !self.is_prime_degraded() && !self.is_telemetry_poll_in_flight() {
            self.prime_read_only();
        }

        let (kb_states, lb_val, pd_val) = {
            let s = self.read_rw(&self.settings);
            (
                s.keyboard_backlight_states.clone(),
                s.lightbar.clone(),
                s.power_devices.clone(),
            )
        };

        {
            let mut kb = self.lock_mutex(&self.keyboard_ctrl);
            let _ = kb.init();
            if let Some(ref states) = kb_states {
                if !states.is_empty() && kb.is_available() && !self.shutting_down.load(Ordering::SeqCst) {
                    let _ = kb.apply_states(states);
                }
            }
            if !self.shutting_down.load(Ordering::SeqCst) {
                kb.hydrate_from_hardware();
            }
        }
        if self.shutting_down.load(Ordering::SeqCst) {
            return;
        }

        if let Some(ref lb_cfg) = lb_val {
            let mut lb = self.lock_mutex(&self.lightbar_ctrl);
            let _ = lb.configure(lb_cfg);
        }
        if self.shutting_down.load(Ordering::SeqCst) {
            return;
        }

        if let Some(ref pd) = pd_val {
            self.apply_wakeup_policy_from_settings(pd);
        }
        if self.shutting_down.load(Ordering::SeqCst) {
            return;
        }

        self.reconcile_power_state(true);
        if self.shutting_down.load(Ordering::SeqCst) {
            return;
        }
        let active_now = self.read_rw(&self.active_profile).clone();
        self.apply_charging_settings_for_profile(&active_now);
        let _ = self.tick_fans_once();
        if !self.shutting_down.load(Ordering::SeqCst) {
            self.hardware_activation_pending
                .store(false, Ordering::SeqCst);
        }
    }

    pub fn activate_hardware_and_prime(&self) {
        self.prime_read_only();
        self.activate_hardware();
    }

    /// Runs one synchronous hardware telemetry poll and stores latest snapshot + history.
    pub fn poll_telemetry_once(&self) -> TelemetrySnapshot {
        let fallback_snap = || {
            if let Some(prev) = self.read_rw(&self.latest_telemetry).clone() {
                return prev;
            }
            TelemetrySnapshot {
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
            }
        };
        if self.shutting_down.load(Ordering::SeqCst)
            || !self.sensor_data_collection.load(Ordering::SeqCst)
        {
            return fallback_snap();
        }
        if self
            .telemetry_poll_in_flight
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return fallback_snap();
        }
        struct TelemetryInFlightGuard<'a>(&'a AtomicBool);
        impl Drop for TelemetryInFlightGuard<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
        let _in_flight_guard = TelemetryInFlightGuard(&self.telemetry_poll_in_flight);

        let now = Instant::now();
        let ts = unix_ms();
        let mut collector = self.lock_mutex(&self.telemetry);
        let snap = collector.poll(now, ts);
        drop(collector);
        if self.shutting_down.load(Ordering::SeqCst) {
            return snap;
        }
        self.record_nvidia_baseline_if_needed(&snap);
        *self.write_rw(&self.latest_telemetry) = Some(snap.clone());
        snap
    }

    pub(super) fn record_nvidia_baseline_if_needed(&self, snap: &TelemetrySnapshot) {
        if let Some(ref dgpu) = snap.dgpu {
            let immutable_base = dgpu
                .default_power_limit_w
                .or(dgpu.max_power_limit_w)
                .or(dgpu.enforced_power_limit_w);
            if let (Some(ref bdf), Some(base_w)) = (&dgpu.bdf, immutable_base) {
                let mut baseline = self.lock_mutex(&self.nvidia_default_power_limit_w);
                if baseline
                    .as_ref()
                    .is_none_or(|(stored_bdf, _)| stored_bdf != bdf)
                {
                    *baseline = Some((bdf.clone(), base_w));
                }
            }
        }
    }

    /// Tears down transient BLE cooler state before system suspend (`PrepareForSleep(true)`).
    pub fn on_system_suspend(&self) {
        self.cooler_suspending.store(true, Ordering::SeqCst);
        self.lock_mutex(&self.cooler_controller)
            .on_prepare_for_sleep(true);
    }

    /// Restores lightbar idle state on resume from sleep without repeating welcome animation (`ISSUE-04`).
    pub fn on_system_resume(&self) {
        self.cooler_suspending.store(false, Ordering::SeqCst);
        self.lock_mutex(&self.cooler_controller)
            .on_prepare_for_sleep(false);
        if self.water_cooler_enabled.load(Ordering::SeqCst) {
            self.replay_active_cooler_profile();
        }
        let _ = self.lock_mutex(&self.lightbar_ctrl).on_resume();
        if !self.is_lid_closed() {
            self.restore_lid_lighting();
        }
    }

    pub fn config_recovered_at_startup(&self) -> bool {
        self.config_recovered.load(Ordering::SeqCst)
    }

    pub fn lid_controller(&self) -> &Mutex<LidController> {
        &self.lid_controller
    }

    /// Bounded hardware cleanup on shutdown (`M8.3`).
    pub fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        self.fan_control_enabled.store(false, Ordering::SeqCst);
        if ucc_config::is_read_only_preview() || self.is_vendor_unsupported() {
            return;
        }
        if !self.shutdown_wait_expired.load(Ordering::SeqCst) {
            let _ = self.restore_cpu_defaults();
        }
        self.lock_mutex(&self.cooler_controller).on_disconnected();
        self.water_cooler_connected.store(false, Ordering::SeqCst);
        if !self.shutdown_wait_expired.load(Ordering::SeqCst) {
            let _ = self.lock_mutex(&self.lightbar_ctrl).shutdown();
        }
        *self.lock_mutex(&self.temporary_fan_curves) = None;
        let auto_res = {
            let mut ctrl = self.lock_mutex(&self.fan_controller);
            ctrl.clear_temporary_curves();
            ctrl.restore_auto(self.platform.as_ref())
                .map_err(|e| e.to_string())
        };
        if let Err(err) = auto_res {
            error!("Failed to restore fans to firmware auto control during shutdown: {err}");
        }
    }
}
