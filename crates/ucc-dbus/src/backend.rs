//! Backend trait defining the business logic for UCC D-Bus methods.

use std::collections::HashMap;
use zbus::zvariant::OwnedValue;

/// Backend trait covering all 122 baseline D-Bus methods.
pub trait UccdBackend: Send + Sync + 'static {
    fn get_device_name(&self) -> String {
        "Frixit Control Center Laptop".to_string()
    }
    fn get_system_info_json(&self) -> String {
        "{}".to_string()
    }
    fn is_device_supported(&self) -> bool {
        true
    }
    fn get_display_modes_json(&self) -> String {
        "{}".to_string()
    }
    fn get_is_x11(&self) -> bool {
        false
    }
    fn tuxedo_wmi_available(&self) -> bool {
        true
    }
    fn fan_hwmon_available(&self) -> bool {
        true
    }
    fn uccd_version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }
    fn get_fan_data_cpu(&self) -> HashMap<String, OwnedValue> {
        HashMap::new()
    }
    fn get_fan_data_gpu1(&self) -> HashMap<String, OwnedValue> {
        HashMap::new()
    }
    fn get_fan_data_gpu2(&self) -> HashMap<String, OwnedValue> {
        HashMap::new()
    }
    fn webcam_sw_available(&self) -> bool {
        false
    }
    fn get_webcam_sw_status(&self) -> bool {
        false
    }
    fn get_force_yuv420_output_switch_available(&self) -> bool {
        false
    }
    fn get_display_brightness(&self) -> i32 {
        100
    }
    fn get_keyboard_backlight_control_supported(&self) -> bool {
        true
    }
    fn set_display_brightness(&self, _brightness: i32) -> bool {
        true
    }
    fn set_display_refresh_rate(&self, _display: &str, _refresh_rate: i32) -> bool {
        true
    }
    fn get_dgpu_info_values_json(&self) -> String {
        "{}".to_string()
    }
    fn get_igpu_info_values_json(&self) -> String {
        "{}".to_string()
    }
    fn get_cpu_power_values_json(&self) -> String {
        "{}".to_string()
    }
    fn get_prime_state(&self) -> String {
        "integrated".to_string()
    }
    fn consume_mode_reapply_pending(&self) -> bool {
        false
    }
    fn get_active_profile_json(&self) -> String {
        "{}".to_string()
    }
    fn get_power_state(&self) -> String {
        "power_ac".to_string()
    }
    fn get_power_policy_json(&self) -> String {
        "{}".to_string()
    }
    fn get_power_device_settings_json(&self) -> String {
        "{}".to_string()
    }
    fn get_power_device_status_json(&self) -> String {
        "{}".to_string()
    }
    fn set_power_device_settings_json(&self, _json: &str) -> bool {
        true
    }
    fn scan_power_devices(&self) -> bool {
        true
    }
    fn get_lightbar_settings_json(&self) -> String {
        "{}".to_string()
    }
    fn get_lightbar_status_json(&self) -> String {
        "{}".to_string()
    }
    fn get_driver_health_json(&self) -> String {
        "{}".to_string()
    }
    fn set_lightbar_settings_json(&self, _json: &str) -> bool {
        true
    }
    fn trigger_lightbar_animation(&self, _force: bool) -> bool {
        true
    }
    fn set_manual_power_mode(&self, _mode_val: &str) -> bool {
        true
    }
    fn register_power_request(&self, _name: &str, _reason: &str, _mode_val: &str) -> bool {
        true
    }
    fn unregister_power_request(&self, _name: &str) -> bool {
        true
    }
    fn set_temp_profile(&self, _profile_name: &str) -> bool {
        true
    }
    fn set_active_profile(&self, _id: &str) -> bool {
        true
    }
    fn apply_profile(&self, _profile_json: &str) -> bool {
        true
    }
    fn get_profiles_json(&self) -> String {
        "{}".to_string()
    }
    fn get_custom_profiles_json(&self) -> String {
        "{}".to_string()
    }
    fn set_fan_profile_cpu(&self, _points_json: &str) -> bool {
        true
    }
    fn set_fan_profile_dgpu(&self, _points_json: &str) -> bool {
        true
    }
    fn apply_fan_profiles(&self, _fan_profiles_json: &str) -> bool {
        true
    }
    fn revert_fan_profiles(&self) -> bool {
        true
    }
    fn get_default_profiles_json(&self) -> String {
        "{}".to_string()
    }
    fn get_cpu_frequency_limits_json(&self) -> String {
        "{}".to_string()
    }
    fn get_default_values_profile_json(&self) -> String {
        "{}".to_string()
    }
    fn add_custom_profile(&self, _profile_json: &str) -> bool {
        true
    }
    fn save_custom_profile(&self, _profile_json: &str) -> bool {
        true
    }
    fn delete_custom_profile(&self, _profile_id: &str) -> bool {
        true
    }
    fn update_custom_profile(&self, _profile_json: &str) -> bool {
        true
    }
    fn get_fan_profile(&self, _name: &str) -> String {
        "{}".to_string()
    }
    fn get_fan_profile_names(&self) -> String {
        "{}".to_string()
    }
    fn set_fan_profile(&self, _name: &str, _json: &str) -> bool {
        true
    }
    fn get_settings_json(&self) -> String {
        "{}".to_string()
    }
    fn set_state_map(&self, _state: &str, _profile_id: &str) -> bool {
        true
    }
    fn set_batch_state_map(&self, _state_map_json: &str) -> bool {
        true
    }
    fn odm_profiles_available(&self) -> Vec<String> {
        Vec::new()
    }
    fn odm_power_limits_json(&self) -> String {
        "{}".to_string()
    }
    fn get_keyboard_backlight_capabilities_json(&self) -> String {
        "{}".to_string()
    }
    fn get_keyboard_backlight_states_json(&self) -> String {
        "{}".to_string()
    }
    fn set_keyboard_backlight_states_json(&self, _keyboard_backlight_states_json: &str) -> bool {
        true
    }
    fn get_custom_keyboard_profiles_json(&self) -> String {
        "{}".to_string()
    }
    fn save_custom_keyboard_profile(&self, _id: &str, _name: &str, _json: &str) -> bool {
        true
    }
    fn delete_custom_keyboard_profile(&self, _id: &str) -> bool {
        true
    }
    fn get_custom_fan_profiles_json(&self) -> String {
        "{}".to_string()
    }
    fn save_custom_fan_profile(&self, _id: &str, _name: &str, _json: &str) -> bool {
        true
    }
    fn delete_custom_fan_profile(&self, _id: &str) -> bool {
        true
    }
    fn get_fans_min_speed(&self) -> i32 {
        0
    }
    fn get_fans_off_available(&self) -> bool {
        false
    }
    fn get_charging_profiles_available(&self) -> String {
        "{}".to_string()
    }
    fn get_current_charging_profile(&self) -> String {
        "balanced".to_string()
    }
    fn set_charging_profile(&self, _profile_descriptor: &str) -> bool {
        true
    }
    fn get_charging_priorities_available(&self) -> String {
        String::new()
    }
    fn get_current_charging_priority(&self) -> String {
        "balanced".to_string()
    }
    fn set_charging_priority(&self, _priority_descriptor: &str) -> bool {
        true
    }
    fn get_charge_start_available_thresholds(&self) -> String {
        String::new()
    }
    fn get_charge_end_available_thresholds(&self) -> String {
        String::new()
    }
    fn get_charge_start_threshold(&self) -> i32 {
        -1
    }
    fn get_charge_end_threshold(&self) -> i32 {
        -1
    }
    fn set_charge_start_threshold(&self, _value: i32) -> bool {
        true
    }
    fn set_charge_end_threshold(&self, _value: i32) -> bool {
        true
    }
    fn get_charge_type(&self) -> String {
        "Standard".to_string()
    }
    fn set_charge_type(&self, _type_val: &str) -> bool {
        true
    }
    fn get_fn_lock_supported(&self) -> bool {
        false
    }
    fn get_fn_lock_status(&self) -> bool {
        false
    }
    fn set_fn_lock_status(&self, _status: bool) {
        
    }
    fn set_sensor_data_collection_status(&self, _status: bool) {
        
    }
    fn get_sensor_data_collection_status(&self) -> bool {
        false
    }
    fn set_dgpu_d0_metrics(&self, _status: bool) {
        
    }
    fn get_nvidia_power_ctrl_default_power_limit(&self) -> i32 {
        0
    }
    fn get_nvidia_power_ctrl_max_power_limit(&self) -> i32 {
        0
    }
    fn get_nvidia_power_ctrl_available(&self) -> bool {
        false
    }
    fn set_nvidia_power_offset(&self, _offset: i32) -> bool {
        true
    }
    fn get_available_governors(&self) -> String {
        "{}".to_string()
    }
    fn get_available_epps(&self) -> String {
        "{}".to_string()
    }
    fn get_cpu_core_count(&self) -> i32 {
        1
    }
    fn get_cpu_topology_json(&self) -> String {
        "{}".to_string()
    }
    fn get_water_cooler_available(&self) -> bool {
        false
    }
    fn get_water_cooler_connected(&self) -> bool {
        false
    }
    fn get_water_cooler_fan_speed(&self) -> i32 {
        0
    }
    fn get_water_cooler_pump_level(&self) -> i32 {
        0
    }
    fn enable_water_cooler(&self, _enable: bool) -> bool {
        true
    }
    fn is_water_cooler_enabled(&self) -> bool {
        false
    }
    fn set_water_cooler_fan_speed(&self, _duty_cycle_percent: i32) -> bool {
        true
    }
    fn set_water_cooler_pump_voltage(&self, _voltage: i32) -> bool {
        true
    }
    fn set_water_cooler_led_color(&self, _red: i32, _green: i32, _blue: i32, _mode_val: i32) -> bool {
        true
    }
    fn turn_off_water_cooler_led(&self) -> bool {
        true
    }
    fn turn_off_water_cooler_fan(&self) -> bool {
        true
    }
    fn turn_off_water_cooler_pump(&self) -> bool {
        true
    }
    fn is_water_cooler_auto_control_enabled(&self) -> bool {
        false
    }
    fn set_water_cooler_auto_control(&self, _enabled: bool) -> bool {
        true
    }
    fn get_water_cooler_supported(&self) -> bool {
        false
    }
    fn get_ctgp_adjustment_supported(&self) -> bool {
        false
    }
    fn get_hwp_dynamic_boost_supported(&self) -> bool {
        false
    }
    fn get_monitor_data_since(&self, _since_timestamp_ms: i64) -> Vec<u8> {
        Vec::new()
    }
    fn set_monitor_history_horizon(&self, _seconds: i32) {
        
    }
    fn get_monitor_history_horizon(&self) -> i32 {
        60
    }
    fn get_cpu_frequency_mhz(&self) -> i32 {
        0
    }
}

/// Default mock/in-memory backend for UccdInterface.
#[derive(Debug, Default)]
pub struct DefaultUccdBackend;
impl UccdBackend for DefaultUccdBackend {}
