//! D-Bus service and client proxy interface for `com.uniwill.uccd`.
//!
//! Provides D-Bus object server endpoints, client proxies, and signal definitions
//! matching the baseline `de.uniwill.uccd.xml` contract.

use std::collections::HashMap;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::Value;

pub const DBUS_SERVICE: &str = "com.uniwill.uccd";
pub const DBUS_PATH: &str = "/com/uniwill/uccd";
pub const DBUS_INTERFACE: &str = "com.uniwill.uccd";

/// Shared state or service backend trait for UCC D-Bus methods.
pub trait UccdBackend: Send + Sync + 'static {
    fn device_name(&self) -> String {
        "Frixit Control Center Laptop".to_string()
    }
    fn system_info_json(&self) -> String {
        "{}".to_string()
    }
    fn is_device_supported(&self) -> bool {
        true
    }
    fn uccd_version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }
    fn power_state(&self) -> String {
        "power_ac".to_string()
    }
    fn active_profile_json(&self) -> String {
        "{}".to_string()
    }
    fn power_policy_json(&self) -> String {
        "{}".to_string()
    }
    fn settings_json(&self) -> String {
        "{}".to_string()
    }
    fn set_manual_power_mode(&self, _mode: &str) -> bool {
        true
    }
    fn register_power_request(&self, _name: &str, _reason: &str, _mode: &str) -> bool {
        true
    }
    fn unregister_power_request(&self, _name: &str) -> bool {
        true
    }
    fn set_state_map(&self, _state: &str, _profile_id: &str) -> bool {
        true
    }
    fn set_batch_state_map(&self, _state_map_json: &str) -> bool {
        true
    }
    fn set_active_profile(&self, _profile_id: &str) -> bool {
        true
    }
    fn apply_profile(&self, _profile_json: &str) -> bool {
        true
    }
    fn save_custom_profile(&self, _profile_json: &str) -> bool {
        true
    }
    fn delete_custom_profile(&self, _profile_id: &str) -> bool {
        true
    }
    fn fan_data_cpu(&self) -> HashMap<String, Value<'static>> {
        HashMap::new()
    }
    fn fan_data_gpu1(&self) -> HashMap<String, Value<'static>> {
        HashMap::new()
    }
    fn fan_data_gpu2(&self) -> HashMap<String, Value<'static>> {
        HashMap::new()
    }
    fn display_brightness(&self) -> i32 {
        100
    }
    fn set_display_brightness(&self, _brightness: i32) -> bool {
        true
    }
    fn water_cooler_connected(&self) -> bool {
        false
    }
    fn water_cooler_fan_speed(&self) -> i32 {
        0
    }
    fn water_cooler_pump_level(&self) -> i32 {
        0
    }
    fn set_water_cooler_fan_speed(&self, _duty_cycle: i32) -> bool {
        true
    }
    fn set_water_cooler_pump_voltage(&self, _voltage: i32) -> bool {
        true
    }
    fn turn_off_water_cooler_fan(&self) -> bool {
        true
    }
    fn turn_off_water_cooler_pump(&self) -> bool {
        true
    }
    fn turn_off_water_cooler_led(&self) -> bool {
        true
    }
    fn current_charging_profile(&self) -> String {
        "balanced".to_string()
    }
    fn set_charging_profile(&self, _profile: &str) -> bool {
        true
    }
    fn charge_start_threshold(&self) -> i32 {
        -1
    }
    fn charge_end_threshold(&self) -> i32 {
        -1
    }
    fn set_charge_start_threshold(&self, _value: i32) -> bool {
        true
    }
    fn set_charge_end_threshold(&self, _value: i32) -> bool {
        true
    }
    fn charge_type(&self) -> String {
        "Standard".to_string()
    }
    fn set_charge_type(&self, _charge_type: &str) -> bool {
        true
    }
}

/// Default mock/in-memory backend for UccdInterface.
#[derive(Debug, Default)]
pub struct DefaultUccdBackend;
impl UccdBackend for DefaultUccdBackend {}

/// Main D-Bus interface implementation for `com.uniwill.uccd`.
pub struct UccdInterface<B: UccdBackend = DefaultUccdBackend> {
    backend: B,
}

impl Default for UccdInterface<DefaultUccdBackend> {
    fn default() -> Self {
        Self::new(DefaultUccdBackend)
    }
}

impl<B: UccdBackend> UccdInterface<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }
}

#[zbus::interface(name = "com.uniwill.uccd")]
impl<B: UccdBackend> UccdInterface<B> {
    // Signals
    #[zbus(signal)]
    pub async fn profile_changed(
        emitter: &SignalEmitter<'_>,
        profile_id: &str,
        keyboard_profile_id: &str,
        fan_profile_id: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn power_state_changed(emitter: &SignalEmitter<'_>, state: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn power_policy_changed(emitter: &SignalEmitter<'_>, json: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn water_cooler_status_changed(
        emitter: &SignalEmitter<'_>,
        status: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn lightbar_settings_changed(
        emitter: &SignalEmitter<'_>,
        json: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn power_device_status_changed(
        emitter: &SignalEmitter<'_>,
        json: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn mode_reapply_pending_changed(
        emitter: &SignalEmitter<'_>,
        pending: bool,
    ) -> zbus::Result<()>;

    // Device / System Info
    #[zbus(name = "GetDeviceName")]
    pub fn get_device_name(&self) -> String {
        self.backend.device_name()
    }

    #[zbus(name = "GetSystemInfoJSON")]
    pub fn get_system_info_json(&self) -> String {
        self.backend.system_info_json()
    }

    #[zbus(name = "IsDeviceSupported")]
    pub fn is_device_supported(&self) -> bool {
        self.backend.is_device_supported()
    }

    #[zbus(name = "UccdVersion")]
    pub fn uccd_version(&self) -> String {
        self.backend.uccd_version()
    }

    // Power & Profiles
    #[zbus(name = "GetPowerState")]
    pub fn get_power_state(&self) -> String {
        self.backend.power_state()
    }

    #[zbus(name = "GetActiveProfileJSON")]
    pub fn get_active_profile_json(&self) -> String {
        self.backend.active_profile_json()
    }

    #[zbus(name = "GetPowerPolicyJSON")]
    pub fn get_power_policy_json(&self) -> String {
        self.backend.power_policy_json()
    }

    #[zbus(name = "SetManualPowerMode")]
    pub fn set_manual_power_mode(&self, mode: &str) -> bool {
        self.backend.set_manual_power_mode(mode)
    }

    #[zbus(name = "RegisterPowerRequest")]
    pub fn register_power_request(&self, name: &str, reason: &str, mode: &str) -> bool {
        self.backend.register_power_request(name, reason, mode)
    }

    #[zbus(name = "UnregisterPowerRequest")]
    pub fn unregister_power_request(&self, name: &str) -> bool {
        self.backend.unregister_power_request(name)
    }

    #[zbus(name = "GetSettingsJSON")]
    pub fn get_settings_json(&self) -> String {
        self.backend.settings_json()
    }

    #[zbus(name = "SetStateMap")]
    pub fn set_state_map(&self, state: &str, profile_id: &str) -> bool {
        self.backend.set_state_map(state, profile_id)
    }

    #[zbus(name = "SetBatchStateMap")]
    pub fn set_batch_state_map(&self, state_map_json: &str) -> bool {
        self.backend.set_batch_state_map(state_map_json)
    }

    #[zbus(name = "SetActiveProfile")]
    pub fn set_active_profile(&self, profile_id: &str) -> bool {
        self.backend.set_active_profile(profile_id)
    }

    #[zbus(name = "ApplyProfile")]
    pub fn apply_profile(&self, profile_json: &str) -> bool {
        self.backend.apply_profile(profile_json)
    }

    #[zbus(name = "SaveCustomProfile")]
    pub fn save_custom_profile(&self, profile_json: &str) -> bool {
        self.backend.save_custom_profile(profile_json)
    }

    #[zbus(name = "DeleteCustomProfile")]
    pub fn delete_custom_profile(&self, profile_id: &str) -> bool {
        self.backend.delete_custom_profile(profile_id)
    }

    // Fans & Sensors
    #[zbus(name = "GetFanDataCPU")]
    pub fn get_fan_data_cpu(&self) -> HashMap<String, Value<'static>> {
        self.backend.fan_data_cpu()
    }

    #[zbus(name = "GetFanDataGPU1")]
    pub fn get_fan_data_gpu1(&self) -> HashMap<String, Value<'static>> {
        self.backend.fan_data_gpu1()
    }

    #[zbus(name = "GetFanDataGPU2")]
    pub fn get_fan_data_gpu2(&self) -> HashMap<String, Value<'static>> {
        self.backend.fan_data_gpu2()
    }

    // Display
    #[zbus(name = "GetDisplayBrightness")]
    pub fn get_display_brightness(&self) -> i32 {
        self.backend.display_brightness()
    }

    #[zbus(name = "SetDisplayBrightness")]
    pub fn set_display_brightness(&self, brightness: i32) -> bool {
        self.backend.set_display_brightness(brightness)
    }

    // Water Cooler
    #[zbus(name = "GetWaterCoolerConnected")]
    pub fn get_water_cooler_connected(&self) -> bool {
        self.backend.water_cooler_connected()
    }

    #[zbus(name = "GetWaterCoolerFanSpeed")]
    pub fn get_water_cooler_fan_speed(&self) -> i32 {
        self.backend.water_cooler_fan_speed()
    }

    #[zbus(name = "GetWaterCoolerPumpLevel")]
    pub fn get_water_cooler_pump_level(&self) -> i32 {
        self.backend.water_cooler_pump_level()
    }

    #[zbus(name = "SetWaterCoolerFanSpeed")]
    pub fn set_water_cooler_fan_speed(&self, duty_cycle_percent: i32) -> bool {
        self.backend.set_water_cooler_fan_speed(duty_cycle_percent)
    }

    #[zbus(name = "SetWaterCoolerPumpVoltage")]
    pub fn set_water_cooler_pump_voltage(&self, voltage: i32) -> bool {
        self.backend.set_water_cooler_pump_voltage(voltage)
    }

    #[zbus(name = "TurnOffWaterCoolerFan")]
    pub fn turn_off_water_cooler_fan(&self) -> bool {
        self.backend.turn_off_water_cooler_fan()
    }

    #[zbus(name = "TurnOffWaterCoolerPump")]
    pub fn turn_off_water_cooler_pump(&self) -> bool {
        self.backend.turn_off_water_cooler_pump()
    }

    #[zbus(name = "TurnOffWaterCoolerLED")]
    pub fn turn_off_water_cooler_led(&self) -> bool {
        self.backend.turn_off_water_cooler_led()
    }

    // Charging
    #[zbus(name = "GetCurrentChargingProfile")]
    pub fn get_current_charging_profile(&self) -> String {
        self.backend.current_charging_profile()
    }

    #[zbus(name = "SetChargingProfile")]
    pub fn set_charging_profile(&self, profile_descriptor: &str) -> bool {
        self.backend.set_charging_profile(profile_descriptor)
    }

    #[zbus(name = "GetChargeStartThreshold")]
    pub fn get_charge_start_threshold(&self) -> i32 {
        self.backend.charge_start_threshold()
    }

    #[zbus(name = "GetChargeEndThreshold")]
    pub fn get_charge_end_threshold(&self) -> i32 {
        self.backend.charge_end_threshold()
    }

    #[zbus(name = "SetChargeStartThreshold")]
    pub fn set_charge_start_threshold(&self, value: i32) -> bool {
        self.backend.set_charge_start_threshold(value)
    }

    #[zbus(name = "SetChargeEndThreshold")]
    pub fn set_charge_end_threshold(&self, value: i32) -> bool {
        self.backend.set_charge_end_threshold(value)
    }

    #[zbus(name = "GetChargeType")]
    pub fn get_charge_type(&self) -> String {
        self.backend.charge_type()
    }

    #[zbus(name = "SetChargeType")]
    pub fn set_charge_type(&self, charge_type: &str) -> bool {
        self.backend.set_charge_type(charge_type)
    }
}

/// Client proxy for interacting with `com.uniwill.uccd` over D-Bus.
#[zbus::proxy(
    interface = "com.uniwill.uccd",
    default_service = "com.uniwill.uccd",
    default_path = "/com/uniwill/uccd"
)]
pub trait Uccd {
    #[zbus(name = "GetDeviceName")]
    fn get_device_name(&self) -> zbus::Result<String>;

    #[zbus(name = "GetSystemInfoJSON")]
    fn get_system_info_json(&self) -> zbus::Result<String>;

    #[zbus(name = "IsDeviceSupported")]
    fn is_device_supported(&self) -> zbus::Result<bool>;

    #[zbus(name = "UccdVersion")]
    fn uccd_version(&self) -> zbus::Result<String>;

    #[zbus(name = "GetPowerState")]
    fn get_power_state(&self) -> zbus::Result<String>;

    #[zbus(name = "GetActiveProfileJSON")]
    fn get_active_profile_json(&self) -> zbus::Result<String>;

    #[zbus(name = "GetPowerPolicyJSON")]
    fn get_power_policy_json(&self) -> zbus::Result<String>;

    #[zbus(name = "SetManualPowerMode")]
    fn set_manual_power_mode(&self, mode: &str) -> zbus::Result<bool>;

    #[zbus(name = "RegisterPowerRequest")]
    fn register_power_request(&self, name: &str, reason: &str, mode: &str) -> zbus::Result<bool>;

    #[zbus(name = "UnregisterPowerRequest")]
    fn unregister_power_request(&self, name: &str) -> zbus::Result<bool>;

    #[zbus(name = "GetSettingsJSON")]
    fn get_settings_json(&self) -> zbus::Result<String>;

    #[zbus(name = "SetStateMap")]
    fn set_state_map(&self, state: &str, profile_id: &str) -> zbus::Result<bool>;

    #[zbus(name = "SetBatchStateMap")]
    fn set_batch_state_map(&self, state_map_json: &str) -> zbus::Result<bool>;

    #[zbus(name = "SetActiveProfile")]
    fn set_active_profile(&self, profile_id: &str) -> zbus::Result<bool>;

    #[zbus(name = "ApplyProfile")]
    fn apply_profile(&self, profile_json: &str) -> zbus::Result<bool>;

    #[zbus(name = "SaveCustomProfile")]
    fn save_custom_profile(&self, profile_json: &str) -> zbus::Result<bool>;

    #[zbus(name = "DeleteCustomProfile")]
    fn delete_custom_profile(&self, profile_id: &str) -> zbus::Result<bool>;

    #[zbus(name = "GetDisplayBrightness")]
    fn get_display_brightness(&self) -> zbus::Result<i32>;

    #[zbus(name = "SetDisplayBrightness")]
    fn set_display_brightness(&self, brightness: i32) -> zbus::Result<bool>;

    #[zbus(name = "GetWaterCoolerConnected")]
    fn get_water_cooler_connected(&self) -> zbus::Result<bool>;

    #[zbus(name = "GetWaterCoolerFanSpeed")]
    fn get_water_cooler_fan_speed(&self) -> zbus::Result<i32>;

    #[zbus(name = "GetWaterCoolerPumpLevel")]
    fn get_water_cooler_pump_level(&self) -> zbus::Result<i32>;

    #[zbus(name = "SetWaterCoolerFanSpeed")]
    fn set_water_cooler_fan_speed(&self, duty_cycle_percent: i32) -> zbus::Result<bool>;

    #[zbus(name = "SetWaterCoolerPumpVoltage")]
    fn set_water_cooler_pump_voltage(&self, voltage: i32) -> zbus::Result<bool>;

    #[zbus(name = "TurnOffWaterCoolerFan")]
    fn turn_off_water_cooler_fan(&self) -> zbus::Result<bool>;

    #[zbus(name = "TurnOffWaterCoolerPump")]
    fn turn_off_water_cooler_pump(&self) -> zbus::Result<bool>;

    #[zbus(name = "TurnOffWaterCoolerLED")]
    fn turn_off_water_cooler_led(&self) -> zbus::Result<bool>;

    #[zbus(name = "GetCurrentChargingProfile")]
    fn get_current_charging_profile(&self) -> zbus::Result<String>;

    #[zbus(name = "SetChargingProfile")]
    fn set_charging_profile(&self, profile_descriptor: &str) -> zbus::Result<bool>;

    #[zbus(name = "GetChargeStartThreshold")]
    fn get_charge_start_threshold(&self) -> zbus::Result<i32>;

    #[zbus(name = "GetChargeEndThreshold")]
    fn get_charge_end_threshold(&self) -> zbus::Result<i32>;

    #[zbus(name = "SetChargeStartThreshold")]
    fn set_charge_start_threshold(&self, value: i32) -> zbus::Result<bool>;

    #[zbus(name = "SetChargeEndThreshold")]
    fn set_charge_end_threshold(&self, value: i32) -> zbus::Result<bool>;

    #[zbus(name = "GetChargeType")]
    fn get_charge_type(&self) -> zbus::Result<String>;

    #[zbus(name = "SetChargeType")]
    fn set_charge_type(&self, charge_type: &str) -> zbus::Result<bool>;
}
