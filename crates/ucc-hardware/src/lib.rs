pub mod bluez_cooler;
pub mod cooler;
pub mod cpu;
pub mod detection;
pub mod devices;
pub mod fan_controller;
pub mod ioctl;
pub mod nvml;
pub mod platform;
pub mod sysfs;
pub mod telemetry;
pub mod traits;
pub mod uwcc;
pub mod windows_wmi;

pub use cpu::{
    find_closest_value, parse_cpu_range_list, CpuController, CpuInfo, CpuTopology, CpuWorker,
    LogicalCpu, ScalingDriver, MAX_REAPPLY_ATTEMPTS,
};
pub use devices::{
    default_process_runner, is_valid_aspm, is_valid_epp, is_valid_governor, is_valid_mode,
    is_valid_path, is_valid_value, validate_settings_json, CpuModeSettings, DeviceEntry,
    PowerDeviceController, PowerDeviceSettings, ProcessRunner as DeviceProcessRunner, VALID_ASPM,
    VALID_EPP, VALID_GOVERNORS, VALID_MODES, WHITELISTED_SYS_PATHS,
};

pub use bluez_cooler::{
    detect_lct_model, select_cooler, BluezCoolerClient, BluezGattSession, DiscoveredCooler,
    LctDeviceModel, SavedCoolerIdentity,
};
pub use cooler::{
    BleTransport, BleWriteLimiter, CoolerCommand, CoolerCommandQueue, CoolerController,
    CoolerError, CoolerMode, CoolerPacketEncoder, CoolerState, CoolerStateMachine, FanTarget,
    MockBleTransport, PumpTarget, PumpVoltage, QueuedCoolerCommand, RgbState, RgbTarget,
    BLE_WRITE_GAP, CMD_FAN, CMD_PUMP, CMD_RESET, CMD_RGB, FAST_RECONNECT_MAX_FAILURES,
    KEEPALIVE_INTERVAL, KEEPALIVE_MISS_THRESHOLD, NORDIC_UART_CHAR_RX, NORDIC_UART_CHAR_TX,
    NORDIC_UART_SERVICE_UUID,
};
pub use detection::{
    build_laptop_model, classify_manufacturer, detect_device_capabilities, detect_vendor,
    identify_device, is_ctgp_supported, is_device_supported, is_hwp_dynamic_boost_supported,
    is_water_cooler_supported, DeviceCapabilities, DeviceVendor, DmiInfo, DmiQuirks, DmiTuple,
    LaptopManufacturer, UniwillDeviceID, CTGP_HIDDEN_DEVICES, SUPPORTED_DEVICE_SKUS,
    WATER_COOLER_DEVICES,
};
pub use fan_controller::{
    spawn_fan_worker, FanController, FanTickSnapshot, FanWorkerConfig, FanWorkerHandle,
};
pub use ioctl::{
    magic, read_uniwill_fan_speed, read_uniwill_fan_temp, read_uniwill_tdp_limits, requests,
    set_clevo_fan_speed, set_clevo_fans_auto, set_perf_profile, set_uniwill_fan_speed,
    set_uniwill_fans_auto, set_uniwill_mode, set_uniwill_mode_enable, set_uniwill_tdp, IoctlError,
    TuxedoIoDevice,
};
pub use nvml::{
    GpuRuntimeStatus, NvidiaPowerGuard, NvmlError, NvmlMemory, NvmlTelemetry,
    NvmlTelemetrySnapshot, NvmlUtilization, NvmlWrapper,
};
pub use platform::{
    MockPlatformBackend, PlatformBackend, PlatformBackendKind, PlatformCapabilities, PlatformError,
    PlatformWrite, TuxedoIoBackend,
};
pub use sysfs::{
    format_cpu_range, parse_cpu_range, SysfsError, SysfsJournalEntry, SysfsNode, SysfsRoot,
    SysfsValue, SysfsWriteJournal,
};
pub use telemetry::*;
pub use traits::{
    Clock, GpuBackend, GpuTelemetry, GpuWrite, HardwareIo, HardwareIoWrite, MockClock,
    MockGpuBackend, MockHardwareIo, MockProcessRunner, MockSysfs, ProcessExecution, ProcessOutput,
    ProcessRunner, RealClock, RealProcessRunner, RealSysfs, Sysfs, SysfsWrite,
};
#[cfg(unix)]
pub use uwcc::LinuxUinputEmitter;
pub use uwcc::{
    decode_tachometer_rpm, ec_duty_to_percent, hotkey_to_linux_keycode, parse_backend_selection,
    percent_to_ec_duty, translate_wmi_event, uwcc_ec_reg, MockUinputEmitter, MockUwccTransport,
    PlatformBackendSelection, RamFanController, RamFanCurve, RamFanPoint, SmrwPowerLimits,
    UinputEmitter, UsbWebcamController, UsbWebcamDevice, UwccBackend, UwccDevice, UwccEcReg,
    UwccHotkeyEvent, UwccInfo, UwccPwup, UwccSmrw, UwccTransport, UwccTransportWrite, UwccWmiEvent,
    CHASSIS_PROJECT_0X1A, MAX_CHARGE_THRESHOLD_PCT, MAX_SANE_FAN_RPM, MAX_TCC_OFFSET_C,
    MIN_CHARGE_THRESHOLD_PCT, RAMFAN_BASE, RAMFAN_END, RAMFAN_MAX_TEMP_C, RAMFAN_POINTS,
    SMRW_POWER_LIMITS_LEN, SMRW_POWER_LIMITS_OFFSET, UINPUT_DEVICE_PATH, USB_CLASS_VIDEO,
    USB_DEVICES_ROOT, UWCC_API_VERSION, UWCC_DEVICE_PATH, UWCC_FLAG_HAS_EC, UWCC_FLAG_HAS_HWMON,
    UWCC_FLAG_HAS_PWUP, UWCC_FLAG_HAS_SMRW, UWCC_FLAG_HAS_WMI_EVENTS, UWCC_IOC_EC_READ,
    UWCC_IOC_EC_WRITE, UWCC_IOC_GET_INFO, UWCC_IOC_PWUP, UWCC_IOC_SMRW,
    UWCC_NR_EC_READ, UWCC_NR_EC_WRITE, UWCC_NR_GET_INFO, UWCC_NR_PWUP, UWCC_NR_SMRW, UWCC_MAGIC,
    UWCC_SMRW_MAX_LEN, UWCC_WMI_RING_CAPACITY,
};
pub use windows_wmi::{
    ec_reg, MockWmiEcTransport, WindowsWmiEcBackend, WmiEcOp, WmiEcTransport, WmiPlatformLimits,
};
