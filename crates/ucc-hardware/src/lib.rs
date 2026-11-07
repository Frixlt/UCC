pub mod cooler;
pub mod detection;
pub mod ioctl;
pub mod nvml;
pub mod sysfs;
pub mod traits;

pub use cooler::{
    BleWriteLimiter, CoolerError, CoolerPacketEncoder, CoolerState, CoolerStateMachine,
    PumpVoltage, RgbState, BLE_WRITE_GAP, CMD_FAN, CMD_PUMP, CMD_RESET, CMD_RGB,
    FAST_RECONNECT_MAX_FAILURES, KEEPALIVE_INTERVAL, KEEPALIVE_MISS_THRESHOLD, NORDIC_UART_CHAR_RX,
    NORDIC_UART_CHAR_TX, NORDIC_UART_SERVICE_UUID,
};
pub use detection::{
    build_laptop_model, classify_manufacturer, detect_device_capabilities, detect_vendor,
    identify_device, is_ctgp_supported, is_device_supported, is_hwp_dynamic_boost_supported,
    is_water_cooler_supported, DeviceCapabilities, DeviceVendor, DmiInfo, DmiQuirks, DmiTuple,
    LaptopManufacturer, UniwillDeviceID, CTGP_HIDDEN_DEVICES, SUPPORTED_DEVICE_SKUS,
    WATER_COOLER_DEVICES,
};
pub use ioctl::{
    magic, read_uniwill_fan_speed, read_uniwill_fan_temp, read_uniwill_tdp_limits, requests,
    set_clevo_fan_speed, set_clevo_fans_auto, set_perf_profile, set_uniwill_fan_speed,
    set_uniwill_fans_auto, set_uniwill_mode, set_uniwill_mode_enable, set_uniwill_tdp, IoctlError,
    TuxedoIoDevice,
};
pub use nvml::{
    GpuRuntimeStatus, NvidiaPowerGuard, NvmlError, NvmlMemory, NvmlTelemetry, NvmlUtilization,
    NvmlWrapper,
};
pub use sysfs::{
    format_cpu_range, parse_cpu_range, SysfsError, SysfsJournalEntry, SysfsNode, SysfsRoot,
    SysfsValue, SysfsWriteJournal,
};
pub use traits::{
    Clock, GpuBackend, GpuTelemetry, GpuWrite, HardwareIo, HardwareIoWrite, MockClock,
    MockGpuBackend, MockHardwareIo, MockProcessRunner, MockSysfs, ProcessExecution, ProcessOutput,
    ProcessRunner, RealClock, RealProcessRunner, RealSysfs, Sysfs, SysfsWrite,
};
