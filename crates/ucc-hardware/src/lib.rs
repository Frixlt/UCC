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
pub mod windows_display;
pub mod windows_lid;
pub mod windows_power;
pub mod windows_wmi;
pub mod winrt_cooler;

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
    windows_nvapi_candidate_path, windows_nvml_candidate_paths, GpuRuntimeStatus, NvidiaPowerGuard,
    NvmlError, NvmlMemory, NvmlTelemetry, NvmlTelemetrySnapshot, NvmlUtilization, NvmlWrapper,
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
    MuxActiveGpu, MuxBootMode, MuxState, PlatformBackendSelection, RamFanController, RamFanCurve,
    RamFanPoint, SmrwPowerLimits, UinputEmitter, UsbWebcamController, UsbWebcamDevice, UwccBackend,
    UwccDevice, UwccEcReg, UwccGpuPower, UwccHotkeyEvent, UwccInfo, UwccMuxState, UwccPwup,
    UwccSmrw, UwccTransport, UwccTransportWrite, UwccWmiEvent, CHASSIS_PROJECT_0X1A,
    MAX_CHARGE_THRESHOLD_PCT, MAX_SANE_FAN_RPM, MAX_TCC_OFFSET_C, MIN_CHARGE_THRESHOLD_PCT,
    RAMFAN_BASE, RAMFAN_END, RAMFAN_MAX_TEMP_C, RAMFAN_POINTS, SMRW_POWER_LIMITS_LEN,
    SMRW_POWER_LIMITS_OFFSET, UINPUT_DEVICE_PATH, USB_CLASS_VIDEO, USB_DEVICES_ROOT,
    UWCC_API_VERSION, UWCC_DEVICE_PATH, UWCC_FLAG_HAS_EC, UWCC_FLAG_HAS_HWMON, UWCC_FLAG_HAS_MUX,
    UWCC_FLAG_HAS_PWBT, UWCC_FLAG_HAS_PWDB, UWCC_FLAG_HAS_PWUP, UWCC_FLAG_HAS_SMRW,
    UWCC_FLAG_HAS_WMI_EVENTS, UWCC_GPU_POWER_OP_BASE_TGP, UWCC_GPU_POWER_OP_DYNAMIC_BOOST,
    UWCC_IOC_EC_READ, UWCC_IOC_EC_WRITE, UWCC_IOC_GET_INFO, UWCC_IOC_GPU_POWER, UWCC_IOC_MUX_GET,
    UWCC_IOC_MUX_SET, UWCC_IOC_PWUP, UWCC_IOC_SMRW, UWCC_MAGIC, UWCC_MUX_ACTIVE_DGPU,
    UWCC_MUX_ACTIVE_IGPU, UWCC_MUX_ACTIVE_UNKNOWN, UWCC_MUX_MODE_DGPU_ONLY, UWCC_MUX_MODE_DYNAMIC,
    UWCC_MUX_MODE_IGPU_ONLY, UWCC_MUX_MODE_MSHYBRID, UWCC_MUX_MODE_UNKNOWN, UWCC_NR_EC_READ,
    UWCC_NR_EC_WRITE, UWCC_NR_GET_INFO, UWCC_NR_GPU_POWER, UWCC_NR_MUX_GET, UWCC_NR_MUX_SET,
    UWCC_NR_PWUP, UWCC_NR_SMRW, UWCC_SMRW_MAX_LEN, UWCC_WMI_RING_CAPACITY,
};
pub use windows_display::{
    DisplayModeSpec, MockWindowsDisplayTransport, WindowsDisplayController, WindowsDisplayError,
    WindowsDisplayTransport, WindowsDisplayWrite,
};
#[cfg(windows)]
pub use windows_display::{Win32DisplayTransport, WmiBrightnessBridge};
pub use windows_lid::{
    parse_lid_power_broadcast, LidGuid, LidRegistrationHandle, LidSwitchState,
    MockWindowsLidTransport, PowerBroadcastSetting, WindowsLidError, WindowsLidMonitor,
    WindowsLidTransport, DEVICE_NOTIFY_SERVICE_HANDLE, DEVICE_NOTIFY_WINDOW_HANDLE,
    GUID_LIDSWITCH_STATE_CHANGE, GUID_LIDSWITCH_STATE_CHANGE_BYTES,
};
#[cfg(windows)]
pub use windows_lid::{Win32LidNotificationGuard, Win32LidTransport};
#[cfg(windows)]
pub use windows_power::Win32PowerTransport;
pub use windows_power::{
    epp_percent_to_str, epp_str_to_percent, BoostMode, MockWindowsPowerTransport, PowerGuid,
    WindowsPowerController, WindowsPowerError, WindowsPowerSource, WindowsPowerTransport,
    WindowsPowerWrite, GUID_PROCESSOR_FREQUENCY_MAXIMUM, GUID_PROCESSOR_PERF_BOOST_MODE,
    GUID_PROCESSOR_PERF_ENERGY_PREFERENCE, GUID_PROCESSOR_SETTINGS_SUBGROUP,
    GUID_PROCESSOR_THROTTLE_MAXIMUM, GUID_PROCESSOR_THROTTLE_MINIMUM,
};
#[cfg(windows)]
pub use windows_wmi::Win32WmiEcTransport;
pub use windows_wmi::{
    decode_ec_read_byte, ec_reg, encode_ec_read_arg, encode_ec_write_arg, feature_requirement,
    feature_support, AcpiWmiCommand, DriverRequirement, MockWmiEcTransport, WindowsWmiEcBackend,
    WmiEcOp, WmiEcTransport, WmiFeature, WmiFeatureSupport, WmiPlatformLimits,
    WINDOWS_WMI_CAPABILITY_MATRIX, WMI_CLASS_ACPI_TEST, WMI_EC_READ_FLAG, WMI_METHOD_GET_SET_ULONG,
    WMI_METHOD_ID_EC_RW, WMI_NAMESPACE,
};
#[cfg(windows)]
pub use winrt_cooler::LiveWinRtGattBridge;
pub use winrt_cooler::{
    format_ble_address_u64, nordic_uart_guids, parse_ble_address_u64, parse_winrt_guid,
    validate_cooler_tx_packet, MockWinRtGattBridge, WinRtBleDeviceInfo, WinRtBleError,
    WinRtCoolerTransport, WinRtGattBridge, WinRtGattHandles, WinRtGuid, NORDIC_UART_RX_CHAR_GUID,
    NORDIC_UART_SERVICE_GUID, NORDIC_UART_TX_CHAR_GUID,
};
