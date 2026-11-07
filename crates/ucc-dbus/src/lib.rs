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
    fn default_profiles_json(&self) -> String {
        "[]".to_string()
    }
    fn custom_profiles_json(&self) -> String {
        "[]".to_string()
    }
    fn cpu_frequency_limits_json(&self) -> String {
        "{}".to_string()
    }
    fn default_values_profile_json(&self) -> String {
        "{}".to_string()
    }
    fn fan_profile(&self, _id: &str) -> String {
        "{}".to_string()
    }
    fn fan_profile_names(&self) -> String {
        "[]".to_string()
    }
    fn set_fan_profile(&self, _id: &str, _json: &str) -> bool {
        true
    }
    fn apply_fan_profiles(&self, _json: &str) -> bool {
        true
    }
    fn revert_fan_profiles(&self) -> bool {
        true
    }
    fn custom_fan_profiles_json(&self) -> String {
        "[]".to_string()
    }
    fn save_custom_fan_profile(&self, _id: &str, _name: &str, _json: &str) -> bool {
        true
    }
    fn delete_custom_fan_profile(&self, _id: &str) -> bool {
        true
    }
    fn keyboard_backlight_control_supported(&self) -> bool {
        true
    }
    fn keyboard_backlight_capabilities_json(&self) -> String {
        "{}".to_string()
    }
    fn keyboard_backlight_states_json(&self) -> String {
        "{}".to_string()
    }
    fn set_keyboard_backlight_states_json(&self, _json: &str) -> bool {
        true
    }
    fn custom_keyboard_profiles_json(&self) -> String {
        "[]".to_string()
    }
    fn save_custom_keyboard_profile(&self, _id: &str, _name: &str, _json: &str) -> bool {
        true
    }
    fn delete_custom_keyboard_profile(&self, _id: &str) -> bool {
        true
    }
    fn webcam_sw_status(&self) -> bool {
        true
    }
    fn set_webcam(&self, _enabled: bool) -> bool {
        true
    }
    fn fn_lock_status(&self) -> bool {
        false
    }
    fn set_fn_lock_status(&self, _enabled: bool) -> bool {
        true
    }
    fn water_cooler_supported(&self) -> bool {
        false
    }
    fn enable_water_cooler(&self, _enable: bool) -> bool {
        true
    }
    fn is_water_cooler_enabled(&self) -> bool {
        false
    }
    fn set_water_cooler_auto_control(&self, _enabled: bool) -> bool {
        true
    }
    fn set_water_cooler_led_color(&self, _r: i32, _g: i32, _b: i32, _mode: i32) -> bool {
        true
    }
    fn charging_profiles_available(&self) -> String {
        "[]".to_string()
    }
    fn charging_priorities_available(&self) -> String {
        "[]".to_string()
    }
    fn current_charging_priority(&self) -> String {
        "".to_string()
    }
    fn set_charging_priority(&self, _priority: &str) -> bool {
        true
    }
    fn charge_start_available_thresholds(&self) -> String {
        "[]".to_string()
    }
    fn charge_end_available_thresholds(&self) -> String {
        "[]".to_string()
    }
    fn cpu_frequency_mhz(&self) -> i32 {
        0
    }
    fn cpu_power_values_json(&self) -> String {
        "{}".to_string()
    }
    fn cpu_core_count(&self) -> i32 {
        1
    }
    fn available_governors(&self) -> String {
        "[]".to_string()
    }
    fn available_epps(&self) -> String {
        "[]".to_string()
    }
    fn d_gpu_info_values_json(&self) -> String {
        "{}".to_string()
    }
    fn i_gpu_info_values_json(&self) -> String {
        "{}".to_string()
    }
    fn odm_power_limits_json(&self) -> String {
        "[]".to_string()
    }
    fn nvidia_power_ctrl_available(&self) -> bool {
        false
    }
    fn nvidia_power_ctrl_max_power_limit(&self) -> i32 {
        0
    }
    fn nvidia_power_ctrl_default_power_limit(&self) -> i32 {
        0
    }
    fn set_nvidia_power_offset(&self, _offset: i32) -> bool {
        true
    }
    fn lightbar_settings_json(&self) -> String {
        "{}".to_string()
    }
    fn lightbar_status_json(&self) -> String {
        "{}".to_string()
    }
    fn set_lightbar_settings_json(&self, _json: &str) -> bool {
        true
    }
    fn trigger_lightbar_animation(&self, _force: bool) -> bool {
        true
    }
    fn power_device_settings_json(&self) -> String {
        "{}".to_string()
    }
    fn power_device_status_json(&self) -> String {
        "{}".to_string()
    }
    fn set_power_device_settings_json(&self, _json: &str) -> bool {
        true
    }
    fn scan_power_devices(&self) -> bool {
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

    // Additional Profiles & Config
    #[zbus(name = "GetDefaultProfilesJSON")]
    pub fn get_default_profiles_json(&self) -> String {
        self.backend.default_profiles_json()
    }

    #[zbus(name = "GetCustomProfilesJSON")]
    pub fn get_custom_profiles_json(&self) -> String {
        self.backend.custom_profiles_json()
    }

    #[zbus(name = "GetCpuFrequencyLimitsJSON")]
    pub fn get_cpu_frequency_limits_json(&self) -> String {
        self.backend.cpu_frequency_limits_json()
    }

    #[zbus(name = "GetDefaultValuesProfileJSON")]
    pub fn get_default_values_profile_json(&self) -> String {
        self.backend.default_values_profile_json()
    }

    // Additional Fans
    #[zbus(name = "GetFanProfile")]
    pub fn get_fan_profile(&self, fan_profile_id: &str) -> String {
        self.backend.fan_profile(fan_profile_id)
    }

    #[zbus(name = "GetFanProfileNames")]
    pub fn get_fan_profile_names(&self) -> String {
        self.backend.fan_profile_names()
    }

    #[zbus(name = "SetFanProfile")]
    pub fn set_fan_profile(&self, fan_profile_id: &str, json: &str) -> bool {
        self.backend.set_fan_profile(fan_profile_id, json)
    }

    #[zbus(name = "ApplyFanProfiles")]
    pub fn apply_fan_profiles(&self, json: &str) -> bool {
        self.backend.apply_fan_profiles(json)
    }

    #[zbus(name = "RevertFanProfiles")]
    pub fn revert_fan_profiles(&self) -> bool {
        self.backend.revert_fan_profiles()
    }

    #[zbus(name = "GetCustomFanProfilesJSON")]
    pub fn get_custom_fan_profiles_json(&self) -> String {
        self.backend.custom_fan_profiles_json()
    }

    #[zbus(name = "SaveCustomFanProfile")]
    pub fn save_custom_fan_profile(&self, id: &str, name: &str, json: &str) -> bool {
        self.backend.save_custom_fan_profile(id, name, json)
    }

    #[zbus(name = "DeleteCustomFanProfile")]
    pub fn delete_custom_fan_profile(&self, id: &str) -> bool {
        self.backend.delete_custom_fan_profile(id)
    }

    // Keyboard
    #[zbus(name = "GetKeyboardBacklightControlSupported")]
    pub fn get_keyboard_backlight_control_supported(&self) -> bool {
        self.backend.keyboard_backlight_control_supported()
    }

    #[zbus(name = "GetKeyboardBacklightCapabilitiesJSON")]
    pub fn get_keyboard_backlight_capabilities_json(&self) -> String {
        self.backend.keyboard_backlight_capabilities_json()
    }

    #[zbus(name = "GetKeyboardBacklightStatesJSON")]
    pub fn get_keyboard_backlight_states_json(&self) -> String {
        self.backend.keyboard_backlight_states_json()
    }

    #[zbus(name = "SetKeyboardBacklightStatesJSON")]
    pub fn set_keyboard_backlight_states_json(&self, json: &str) -> bool {
        self.backend.set_keyboard_backlight_states_json(json)
    }

    #[zbus(name = "GetCustomKeyboardProfilesJSON")]
    pub fn get_custom_keyboard_profiles_json(&self) -> String {
        self.backend.custom_keyboard_profiles_json()
    }

    #[zbus(name = "SaveCustomKeyboardProfile")]
    pub fn save_custom_keyboard_profile(&self, id: &str, name: &str, json: &str) -> bool {
        self.backend.save_custom_keyboard_profile(id, name, json)
    }

    #[zbus(name = "DeleteCustomKeyboardProfile")]
    pub fn delete_custom_keyboard_profile(&self, id: &str) -> bool {
        self.backend.delete_custom_keyboard_profile(id)
    }

    // Hardware toggles
    #[zbus(name = "GetWebcamSWStatus")]
    pub fn get_webcam_sw_status(&self) -> bool {
        self.backend.webcam_sw_status()
    }

    #[zbus(name = "SetWebcam")]
    pub fn set_webcam(&self, enabled: bool) -> bool {
        self.backend.set_webcam(enabled)
    }

    #[zbus(name = "GetFnLockStatus")]
    pub fn get_fn_lock_status(&self) -> bool {
        self.backend.fn_lock_status()
    }

    #[zbus(name = "SetFnLockStatus")]
    pub fn set_fn_lock_status(&self, enabled: bool) -> bool {
        self.backend.set_fn_lock_status(enabled)
    }

    // Water cooler extensions
    #[zbus(name = "GetWaterCoolerSupported")]
    pub fn get_water_cooler_supported(&self) -> bool {
        self.backend.water_cooler_supported()
    }

    #[zbus(name = "EnableWaterCooler")]
    pub fn enable_water_cooler(&self, enable: bool) -> bool {
        self.backend.enable_water_cooler(enable)
    }

    #[zbus(name = "IsWaterCoolerEnabled")]
    pub fn is_water_cooler_enabled(&self) -> bool {
        self.backend.is_water_cooler_enabled()
    }

    #[zbus(name = "SetWaterCoolerAutoControl")]
    pub fn set_water_cooler_auto_control(&self, enabled: bool) -> bool {
        self.backend.set_water_cooler_auto_control(enabled)
    }

    #[zbus(name = "SetWaterCoolerLEDColor")]
    pub fn set_water_cooler_led_color(&self, r: i32, g: i32, b: i32, mode: i32) -> bool {
        self.backend.set_water_cooler_led_color(r, g, b, mode)
    }

    // Charging extensions
    #[zbus(name = "GetChargingProfilesAvailable")]
    pub fn get_charging_profiles_available(&self) -> String {
        self.backend.charging_profiles_available()
    }

    #[zbus(name = "GetChargingPrioritiesAvailable")]
    pub fn get_charging_priorities_available(&self) -> String {
        self.backend.charging_priorities_available()
    }

    #[zbus(name = "GetCurrentChargingPriority")]
    pub fn get_current_charging_priority(&self) -> String {
        self.backend.current_charging_priority()
    }

    #[zbus(name = "SetChargingPriority")]
    pub fn set_charging_priority(&self, priority: &str) -> bool {
        self.backend.set_charging_priority(priority)
    }

    #[zbus(name = "GetChargeStartAvailableThresholds")]
    pub fn get_charge_start_available_thresholds(&self) -> String {
        self.backend.charge_start_available_thresholds()
    }

    #[zbus(name = "GetChargeEndAvailableThresholds")]
    pub fn get_charge_end_available_thresholds(&self) -> String {
        self.backend.charge_end_available_thresholds()
    }

    // CPU / GPU telemetry & power
    #[zbus(name = "GetCpuFrequencyMHz")]
    pub fn get_cpu_frequency_mhz(&self) -> i32 {
        self.backend.cpu_frequency_mhz()
    }

    #[zbus(name = "GetCpuPowerValuesJSON")]
    pub fn get_cpu_power_values_json(&self) -> String {
        self.backend.cpu_power_values_json()
    }

    #[zbus(name = "GetCpuCoreCount")]
    pub fn get_cpu_core_count(&self) -> i32 {
        self.backend.cpu_core_count()
    }

    #[zbus(name = "GetAvailableGovernors")]
    pub fn get_available_governors(&self) -> String {
        self.backend.available_governors()
    }

    #[zbus(name = "GetAvailableEPPs")]
    pub fn get_available_epps(&self) -> String {
        self.backend.available_epps()
    }

    #[zbus(name = "GetDGpuInfoValuesJSON")]
    pub fn get_d_gpu_info_values_json(&self) -> String {
        self.backend.d_gpu_info_values_json()
    }

    #[zbus(name = "GetIGpuInfoValuesJSON")]
    pub fn get_i_gpu_info_values_json(&self) -> String {
        self.backend.i_gpu_info_values_json()
    }

    #[zbus(name = "ODMPowerLimitsJSON")]
    pub fn odm_power_limits_json(&self) -> String {
        self.backend.odm_power_limits_json()
    }

    #[zbus(name = "GetNVIDIAPowerCTRLAvailable")]
    pub fn get_nvidia_power_ctrl_available(&self) -> bool {
        self.backend.nvidia_power_ctrl_available()
    }

    #[zbus(name = "GetNVIDIAPowerCTRLMaxPowerLimit")]
    pub fn get_nvidia_power_ctrl_max_power_limit(&self) -> i32 {
        self.backend.nvidia_power_ctrl_max_power_limit()
    }

    #[zbus(name = "GetNVIDIAPowerCTRLDefaultPowerLimit")]
    pub fn get_nvidia_power_ctrl_default_power_limit(&self) -> i32 {
        self.backend.nvidia_power_ctrl_default_power_limit()
    }

    #[zbus(name = "SetNVIDIAPowerOffset")]
    pub fn set_nvidia_power_offset(&self, offset: i32) -> bool {
        self.backend.set_nvidia_power_offset(offset)
    }

    // Lightbar & Power devices
    #[zbus(name = "GetLightbarSettingsJSON")]
    pub fn get_lightbar_settings_json(&self) -> String {
        self.backend.lightbar_settings_json()
    }

    #[zbus(name = "GetLightbarStatusJSON")]
    pub fn get_lightbar_status_json(&self) -> String {
        self.backend.lightbar_status_json()
    }

    #[zbus(name = "SetLightbarSettingsJSON")]
    pub fn set_lightbar_settings_json(&self, json: &str) -> bool {
        self.backend.set_lightbar_settings_json(json)
    }

    #[zbus(name = "TriggerLightbarAnimation")]
    pub fn trigger_lightbar_animation(&self, force: bool) -> bool {
        self.backend.trigger_lightbar_animation(force)
    }

    #[zbus(name = "GetPowerDeviceSettingsJSON")]
    pub fn get_power_device_settings_json(&self) -> String {
        self.backend.power_device_settings_json()
    }

    #[zbus(name = "GetPowerDeviceStatusJSON")]
    pub fn get_power_device_status_json(&self) -> String {
        self.backend.power_device_status_json()
    }

    #[zbus(name = "SetPowerDeviceSettingsJSON")]
    pub fn set_power_device_settings_json(&self, json: &str) -> bool {
        self.backend.set_power_device_settings_json(json)
    }

    #[zbus(name = "ScanPowerDevices")]
    pub fn scan_power_devices(&self) -> bool {
        self.backend.scan_power_devices()
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

    // Additional Profiles & Config
    #[zbus(name = "GetDefaultProfilesJSON")]
    fn get_default_profiles_json(&self) -> zbus::Result<String>;

    #[zbus(name = "GetCustomProfilesJSON")]
    fn get_custom_profiles_json(&self) -> zbus::Result<String>;

    #[zbus(name = "GetCpuFrequencyLimitsJSON")]
    fn get_cpu_frequency_limits_json(&self) -> zbus::Result<String>;

    #[zbus(name = "GetDefaultValuesProfileJSON")]
    fn get_default_values_profile_json(&self) -> zbus::Result<String>;

    // Fans
    #[zbus(name = "GetFanDataCPU")]
    fn get_fan_data_cpu(&self) -> zbus::Result<std::collections::HashMap<String, zbus::zvariant::OwnedValue>>;

    #[zbus(name = "GetFanDataGPU1")]
    fn get_fan_data_gpu1(&self) -> zbus::Result<std::collections::HashMap<String, zbus::zvariant::OwnedValue>>;

    #[zbus(name = "GetFanDataGPU2")]
    fn get_fan_data_gpu2(&self) -> zbus::Result<std::collections::HashMap<String, zbus::zvariant::OwnedValue>>;

    #[zbus(name = "GetFanProfile")]
    fn get_fan_profile(&self, fan_profile_id: &str) -> zbus::Result<String>;

    #[zbus(name = "GetFanProfileNames")]
    fn get_fan_profile_names(&self) -> zbus::Result<String>;

    #[zbus(name = "SetFanProfile")]
    fn set_fan_profile(&self, fan_profile_id: &str, json: &str) -> zbus::Result<bool>;

    #[zbus(name = "ApplyFanProfiles")]
    fn apply_fan_profiles(&self, fan_profiles_json: &str) -> zbus::Result<bool>;

    #[zbus(name = "RevertFanProfiles")]
    fn revert_fan_profiles(&self) -> zbus::Result<bool>;

    #[zbus(name = "GetCustomFanProfilesJSON")]
    fn get_custom_fan_profiles_json(&self) -> zbus::Result<String>;

    #[zbus(name = "SaveCustomFanProfile")]
    fn save_custom_fan_profile(&self, id: &str, name: &str, json: &str) -> zbus::Result<bool>;

    #[zbus(name = "DeleteCustomFanProfile")]
    fn delete_custom_fan_profile(&self, id: &str) -> zbus::Result<bool>;

    // Keyboard
    #[zbus(name = "GetKeyboardBacklightControlSupported")]
    fn get_keyboard_backlight_control_supported(&self) -> zbus::Result<bool>;

    #[zbus(name = "GetKeyboardBacklightCapabilitiesJSON")]
    fn get_keyboard_backlight_capabilities_json(&self) -> zbus::Result<String>;

    #[zbus(name = "GetKeyboardBacklightStatesJSON")]
    fn get_keyboard_backlight_states_json(&self) -> zbus::Result<String>;

    #[zbus(name = "SetKeyboardBacklightStatesJSON")]
    fn set_keyboard_backlight_states_json(&self, config: &str) -> zbus::Result<bool>;

    #[zbus(name = "GetCustomKeyboardProfilesJSON")]
    fn get_custom_keyboard_profiles_json(&self) -> zbus::Result<String>;

    #[zbus(name = "SaveCustomKeyboardProfile")]
    fn save_custom_keyboard_profile(&self, id: &str, name: &str, json: &str) -> zbus::Result<bool>;

    #[zbus(name = "DeleteCustomKeyboardProfile")]
    fn delete_custom_keyboard_profile(&self, id: &str) -> zbus::Result<bool>;

    // Hardware toggles
    #[zbus(name = "GetWebcamSWStatus")]
    fn get_webcam_sw_status(&self) -> zbus::Result<bool>;

    #[zbus(name = "SetWebcam")]
    fn set_webcam(&self, enabled: bool) -> zbus::Result<bool>;

    #[zbus(name = "GetFnLockStatus")]
    fn get_fn_lock_status(&self) -> zbus::Result<bool>;

    #[zbus(name = "SetFnLockStatus")]
    fn set_fn_lock_status(&self, enabled: bool) -> zbus::Result<()>;

    // Water cooler extensions
    #[zbus(name = "GetWaterCoolerSupported")]
    fn get_water_cooler_supported(&self) -> zbus::Result<bool>;

    #[zbus(name = "EnableWaterCooler")]
    fn enable_water_cooler(&self, enable: bool) -> zbus::Result<bool>;

    #[zbus(name = "IsWaterCoolerEnabled")]
    fn is_water_cooler_enabled(&self) -> zbus::Result<bool>;

    #[zbus(name = "SetWaterCoolerAutoControl")]
    fn set_water_cooler_auto_control(&self, enabled: bool) -> zbus::Result<bool>;

    #[zbus(name = "SetWaterCoolerLEDColor")]
    fn set_water_cooler_led_color(&self, r: i32, g: i32, b: i32, mode: i32) -> zbus::Result<bool>;

    // Charging extensions
    #[zbus(name = "GetChargingProfilesAvailable")]
    fn get_charging_profiles_available(&self) -> zbus::Result<String>;

    #[zbus(name = "GetChargingPrioritiesAvailable")]
    fn get_charging_priorities_available(&self) -> zbus::Result<String>;

    #[zbus(name = "GetCurrentChargingPriority")]
    fn get_current_charging_priority(&self) -> zbus::Result<String>;

    #[zbus(name = "SetChargingPriority")]
    fn set_charging_priority(&self, priority: &str) -> zbus::Result<bool>;

    #[zbus(name = "GetChargeStartAvailableThresholds")]
    fn get_charge_start_available_thresholds(&self) -> zbus::Result<String>;

    #[zbus(name = "GetChargeEndAvailableThresholds")]
    fn get_charge_end_available_thresholds(&self) -> zbus::Result<String>;

    // CPU / GPU telemetry & power
    #[zbus(name = "GetCpuFrequencyMHz")]
    fn get_cpu_frequency_mhz(&self) -> zbus::Result<i32>;

    #[zbus(name = "GetCpuPowerValuesJSON")]
    fn get_cpu_power_values_json(&self) -> zbus::Result<String>;

    #[zbus(name = "GetCpuCoreCount")]
    fn get_cpu_core_count(&self) -> zbus::Result<i32>;

    #[zbus(name = "GetAvailableGovernors")]
    fn get_available_governors(&self) -> zbus::Result<String>;

    #[zbus(name = "GetAvailableEPPs")]
    fn get_available_epps(&self) -> zbus::Result<String>;

    #[zbus(name = "GetDGpuInfoValuesJSON")]
    fn get_d_gpu_info_values_json(&self) -> zbus::Result<String>;

    #[zbus(name = "GetIGpuInfoValuesJSON")]
    fn get_i_gpu_info_values_json(&self) -> zbus::Result<String>;

    #[zbus(name = "ODMPowerLimitsJSON")]
    fn odm_power_limits_json(&self) -> zbus::Result<String>;

    #[zbus(name = "GetNVIDIAPowerCTRLAvailable")]
    fn get_nvidia_power_ctrl_available(&self) -> zbus::Result<bool>;

    #[zbus(name = "GetNVIDIAPowerCTRLMaxPowerLimit")]
    fn get_nvidia_power_ctrl_max_power_limit(&self) -> zbus::Result<i32>;

    #[zbus(name = "GetNVIDIAPowerCTRLDefaultPowerLimit")]
    fn get_nvidia_power_ctrl_default_power_limit(&self) -> zbus::Result<i32>;

    #[zbus(name = "SetNVIDIAPowerOffset")]
    fn set_nvidia_power_offset(&self, offset: i32) -> zbus::Result<bool>;

    // Lightbar & Power devices
    #[zbus(name = "GetLightbarSettingsJSON")]
    fn get_lightbar_settings_json(&self) -> zbus::Result<String>;

    #[zbus(name = "GetLightbarStatusJSON")]
    fn get_lightbar_status_json(&self) -> zbus::Result<String>;

    #[zbus(name = "SetLightbarSettingsJSON")]
    fn set_lightbar_settings_json(&self, json: &str) -> zbus::Result<bool>;

    #[zbus(name = "TriggerLightbarAnimation")]
    fn trigger_lightbar_animation(&self, force: bool) -> zbus::Result<bool>;

    #[zbus(name = "GetPowerDeviceSettingsJSON")]
    fn get_power_device_settings_json(&self) -> zbus::Result<String>;

    #[zbus(name = "GetPowerDeviceStatusJSON")]
    fn get_power_device_status_json(&self) -> zbus::Result<String>;

    #[zbus(name = "SetPowerDeviceSettingsJSON")]
    fn set_power_device_settings_json(&self, json: &str) -> zbus::Result<bool>;

    #[zbus(name = "ScanPowerDevices")]
    fn scan_power_devices(&self) -> zbus::Result<bool>;
}
