//! Unified hardware telemetry (`M7.1`–`M7.7`): CPU/RAPL, iGPU/dGPU sysfs, guarded NVML,
//! bounded 10-series history store, binary codec, and legacy IPC sentinel conversion.

pub mod collector;
pub mod cpu;
pub mod gpu;
pub mod history;

pub use collector::{
    dgpu_to_legacy_json, igpu_to_legacy_json, TelemetryCollector, TelemetryConfig,
    TelemetrySnapshot,
};
pub use cpu::{CpuTelemetryReader, CpuTelemetrySnapshot, RaplEnergyCounter};
pub use gpu::{
    find_intel_gpu_rapl_path, find_intel_gpu_rapl_path_for_card, nvidia_device_telemetry_due,
    nvidia_telemetry_due, read_amd_gpu_hwmon, read_amd_igpu_sysfs, read_intel_igpu_sysfs,
    read_pci_slot_name, DGpuSnapshot, GpuVendor, IGpuSnapshot,
};
pub use history::{
    decode_history_binary, HistoryCodecError, MetricDataPoint, MetricId, MetricsHistoryStore,
    MAX_POINTS_PER_SERIES,
};
