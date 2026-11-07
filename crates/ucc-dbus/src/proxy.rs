//! Client proxies for `com.uniwill.uccd` and `de.uniwill.uccd`.

pub mod primary {
    use std::collections::HashMap;
    use zbus::zvariant::OwnedValue;

    #[zbus::proxy(
        interface = "com.uniwill.uccd",
        default_service = "com.uniwill.uccd",
        default_path = "/com/uniwill/uccd"
    )]
    pub trait Uccd {
        #[zbus(signal)]
        fn profile_changed(
            &self,
            profile_id: String,
            keyboard_profile_id: String,
            fan_profile_id: String,
        ) -> zbus::Result<()>;

        #[zbus(signal)]
        fn mode_reapply_pending_changed(&self, pending: bool) -> zbus::Result<()>;

        #[zbus(signal)]
        fn power_state_changed(&self, state: String) -> zbus::Result<()>;

        #[zbus(signal)]
        fn power_policy_changed(&self, json: String) -> zbus::Result<()>;

        #[zbus(signal)]
        fn power_device_status_changed(&self, json: String) -> zbus::Result<()>;

        #[zbus(signal)]
        fn lightbar_settings_changed(&self, json: String) -> zbus::Result<()>;

        #[zbus(signal)]
        fn water_cooler_status_changed(&self, status: String) -> zbus::Result<()>;

        // Methods (122)
        #[zbus(name = "GetDeviceName")]
        fn get_device_name(&self) -> zbus::Result<String>;

        #[zbus(name = "GetSystemInfoJSON")]
        fn get_system_info_json(&self) -> zbus::Result<String>;

        #[zbus(name = "IsDeviceSupported")]
        fn is_device_supported(&self) -> zbus::Result<bool>;

        #[zbus(name = "GetDisplayModesJSON")]
        fn get_display_modes_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetIsX11")]
        fn get_is_x11(&self) -> zbus::Result<bool>;

        #[zbus(name = "TuxedoWmiAvailable")]
        fn tuxedo_wmi_available(&self) -> zbus::Result<bool>;

        #[zbus(name = "FanHwmonAvailable")]
        fn fan_hwmon_available(&self) -> zbus::Result<bool>;

        #[zbus(name = "UccdVersion")]
        fn uccd_version(&self) -> zbus::Result<String>;

        #[zbus(name = "GetFanDataCPU")]
        fn get_fan_data_cpu(&self) -> zbus::Result<HashMap<String, OwnedValue>>;

        #[zbus(name = "GetFanDataGPU1")]
        fn get_fan_data_gpu1(&self) -> zbus::Result<HashMap<String, OwnedValue>>;

        #[zbus(name = "GetFanDataGPU2")]
        fn get_fan_data_gpu2(&self) -> zbus::Result<HashMap<String, OwnedValue>>;

        #[zbus(name = "WebcamSWAvailable")]
        fn webcam_sw_available(&self) -> zbus::Result<bool>;

        #[zbus(name = "GetWebcamSWStatus")]
        fn get_webcam_sw_status(&self) -> zbus::Result<bool>;

        #[zbus(name = "GetForceYUV420OutputSwitchAvailable")]
        fn get_force_yuv420_output_switch_available(&self) -> zbus::Result<bool>;

        #[zbus(name = "GetDisplayBrightness")]
        fn get_display_brightness(&self) -> zbus::Result<i32>;

        #[zbus(name = "GetKeyboardBacklightControlSupported")]
        fn get_keyboard_backlight_control_supported(&self) -> zbus::Result<bool>;

        #[zbus(name = "SetDisplayBrightness")]
        fn set_display_brightness(&self, brightness: i32) -> zbus::Result<bool>;

        #[zbus(name = "SetDisplayRefreshRate")]
        fn set_display_refresh_rate(&self, display: &str, refresh_rate: i32) -> zbus::Result<bool>;

        #[zbus(name = "GetDGpuInfoValuesJSON")]
        fn get_dgpu_info_values_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetIGpuInfoValuesJSON")]
        fn get_igpu_info_values_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetCpuPowerValuesJSON")]
        fn get_cpu_power_values_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetPrimeState")]
        fn get_prime_state(&self) -> zbus::Result<String>;

        #[zbus(name = "ConsumeModeReapplyPending")]
        fn consume_mode_reapply_pending(&self) -> zbus::Result<bool>;

        #[zbus(name = "GetActiveProfileJSON")]
        fn get_active_profile_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetPowerState")]
        fn get_power_state(&self) -> zbus::Result<String>;

        #[zbus(name = "GetPowerPolicyJSON")]
        fn get_power_policy_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetPowerDeviceSettingsJSON")]
        fn get_power_device_settings_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetPowerDeviceStatusJSON")]
        fn get_power_device_status_json(&self) -> zbus::Result<String>;

        #[zbus(name = "SetPowerDeviceSettingsJSON")]
        fn set_power_device_settings_json(&self, json: &str) -> zbus::Result<bool>;

        #[zbus(name = "ScanPowerDevices")]
        fn scan_power_devices(&self) -> zbus::Result<bool>;

        #[zbus(name = "GetLightbarSettingsJSON")]
        fn get_lightbar_settings_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetLightbarStatusJSON")]
        fn get_lightbar_status_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetDriverHealthJSON")]
        fn get_driver_health_json(&self) -> zbus::Result<String>;

        #[zbus(name = "SetLightbarSettingsJSON")]
        fn set_lightbar_settings_json(&self, json: &str) -> zbus::Result<bool>;

        #[zbus(name = "TriggerLightbarAnimation")]
        fn trigger_lightbar_animation(&self, force: bool) -> zbus::Result<bool>;

        #[zbus(name = "SetManualPowerMode")]
        fn set_manual_power_mode(&self, mode_val: &str) -> zbus::Result<bool>;

        #[zbus(name = "RegisterPowerRequest")]
        fn register_power_request(&self, name: &str, reason: &str, mode_val: &str) -> zbus::Result<bool>;

        #[zbus(name = "UnregisterPowerRequest")]
        fn unregister_power_request(&self, name: &str) -> zbus::Result<bool>;

        #[zbus(name = "SetTempProfile")]
        fn set_temp_profile(&self, profile_name: &str) -> zbus::Result<bool>;

        #[zbus(name = "SetActiveProfile")]
        fn set_active_profile(&self, id: &str) -> zbus::Result<bool>;

        #[zbus(name = "ApplyProfile")]
        fn apply_profile(&self, profile_json: &str) -> zbus::Result<bool>;

        #[zbus(name = "GetProfilesJSON")]
        fn get_profiles_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetCustomProfilesJSON")]
        fn get_custom_profiles_json(&self) -> zbus::Result<String>;

        #[zbus(name = "SetFanProfileCPU")]
        fn set_fan_profile_cpu(&self, points_json: &str) -> zbus::Result<bool>;

        #[zbus(name = "SetFanProfileDGPU")]
        fn set_fan_profile_dgpu(&self, points_json: &str) -> zbus::Result<bool>;

        #[zbus(name = "ApplyFanProfiles")]
        fn apply_fan_profiles(&self, fan_profiles_json: &str) -> zbus::Result<bool>;

        #[zbus(name = "RevertFanProfiles")]
        fn revert_fan_profiles(&self) -> zbus::Result<bool>;

        #[zbus(name = "GetDefaultProfilesJSON")]
        fn get_default_profiles_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetCpuFrequencyLimitsJSON")]
        fn get_cpu_frequency_limits_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetDefaultValuesProfileJSON")]
        fn get_default_values_profile_json(&self) -> zbus::Result<String>;

        #[zbus(name = "AddCustomProfile")]
        fn add_custom_profile(&self, profile_json: &str) -> zbus::Result<bool>;

        #[zbus(name = "SaveCustomProfile")]
        fn save_custom_profile(&self, profile_json: &str) -> zbus::Result<bool>;

        #[zbus(name = "DeleteCustomProfile")]
        fn delete_custom_profile(&self, profile_id: &str) -> zbus::Result<bool>;

        #[zbus(name = "UpdateCustomProfile")]
        fn update_custom_profile(&self, profile_json: &str) -> zbus::Result<bool>;

        #[zbus(name = "GetFanProfile")]
        fn get_fan_profile(&self, name: &str) -> zbus::Result<String>;

        #[zbus(name = "GetFanProfileNames")]
        fn get_fan_profile_names(&self) -> zbus::Result<String>;

        #[zbus(name = "SetFanProfile")]
        fn set_fan_profile(&self, name: &str, json: &str) -> zbus::Result<bool>;

        #[zbus(name = "GetSettingsJSON")]
        fn get_settings_json(&self) -> zbus::Result<String>;

        #[zbus(name = "SetStateMap")]
        fn set_state_map(&self, state: &str, profile_id: &str) -> zbus::Result<bool>;

        #[zbus(name = "SetBatchStateMap")]
        fn set_batch_state_map(&self, state_map_json: &str) -> zbus::Result<bool>;

        #[zbus(name = "ODMProfilesAvailable")]
        fn odm_profiles_available(&self) -> zbus::Result<Vec<String>>;

        #[zbus(name = "ODMPowerLimitsJSON")]
        fn odm_power_limits_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetKeyboardBacklightCapabilitiesJSON")]
        fn get_keyboard_backlight_capabilities_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetKeyboardBacklightStatesJSON")]
        fn get_keyboard_backlight_states_json(&self) -> zbus::Result<String>;

        #[zbus(name = "SetKeyboardBacklightStatesJSON")]
        fn set_keyboard_backlight_states_json(&self, keyboard_backlight_states_json: &str) -> zbus::Result<bool>;

        #[zbus(name = "GetCustomKeyboardProfilesJSON")]
        fn get_custom_keyboard_profiles_json(&self) -> zbus::Result<String>;

        #[zbus(name = "SaveCustomKeyboardProfile")]
        fn save_custom_keyboard_profile(&self, id: &str, name: &str, json: &str) -> zbus::Result<bool>;

        #[zbus(name = "DeleteCustomKeyboardProfile")]
        fn delete_custom_keyboard_profile(&self, id: &str) -> zbus::Result<bool>;

        #[zbus(name = "GetCustomFanProfilesJSON")]
        fn get_custom_fan_profiles_json(&self) -> zbus::Result<String>;

        #[zbus(name = "SaveCustomFanProfile")]
        fn save_custom_fan_profile(&self, id: &str, name: &str, json: &str) -> zbus::Result<bool>;

        #[zbus(name = "DeleteCustomFanProfile")]
        fn delete_custom_fan_profile(&self, id: &str) -> zbus::Result<bool>;

        #[zbus(name = "GetFansMinSpeed")]
        fn get_fans_min_speed(&self) -> zbus::Result<i32>;

        #[zbus(name = "GetFansOffAvailable")]
        fn get_fans_off_available(&self) -> zbus::Result<bool>;

        #[zbus(name = "GetChargingProfilesAvailable")]
        fn get_charging_profiles_available(&self) -> zbus::Result<String>;

        #[zbus(name = "GetCurrentChargingProfile")]
        fn get_current_charging_profile(&self) -> zbus::Result<String>;

        #[zbus(name = "SetChargingProfile")]
        fn set_charging_profile(&self, profile_descriptor: &str) -> zbus::Result<bool>;

        #[zbus(name = "GetChargingPrioritiesAvailable")]
        fn get_charging_priorities_available(&self) -> zbus::Result<String>;

        #[zbus(name = "GetCurrentChargingPriority")]
        fn get_current_charging_priority(&self) -> zbus::Result<String>;

        #[zbus(name = "SetChargingPriority")]
        fn set_charging_priority(&self, priority_descriptor: &str) -> zbus::Result<bool>;

        #[zbus(name = "GetChargeStartAvailableThresholds")]
        fn get_charge_start_available_thresholds(&self) -> zbus::Result<String>;

        #[zbus(name = "GetChargeEndAvailableThresholds")]
        fn get_charge_end_available_thresholds(&self) -> zbus::Result<String>;

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
        fn set_charge_type(&self, type_val: &str) -> zbus::Result<bool>;

        #[zbus(name = "GetFnLockSupported")]
        fn get_fn_lock_supported(&self) -> zbus::Result<bool>;

        #[zbus(name = "GetFnLockStatus")]
        fn get_fn_lock_status(&self) -> zbus::Result<bool>;

        #[zbus(name = "SetFnLockStatus")]
        fn set_fn_lock_status(&self, status: bool) -> zbus::Result<()>;

        #[zbus(name = "SetSensorDataCollectionStatus")]
        fn set_sensor_data_collection_status(&self, status: bool) -> zbus::Result<()>;

        #[zbus(name = "GetSensorDataCollectionStatus")]
        fn get_sensor_data_collection_status(&self) -> zbus::Result<bool>;

        #[zbus(name = "SetDGpuD0Metrics")]
        fn set_dgpu_d0_metrics(&self, status: bool) -> zbus::Result<()>;

        #[zbus(name = "GetNVIDIAPowerCTRLDefaultPowerLimit")]
        fn get_nvidia_power_ctrl_default_power_limit(&self) -> zbus::Result<i32>;

        #[zbus(name = "GetNVIDIAPowerCTRLMaxPowerLimit")]
        fn get_nvidia_power_ctrl_max_power_limit(&self) -> zbus::Result<i32>;

        #[zbus(name = "GetNVIDIAPowerCTRLAvailable")]
        fn get_nvidia_power_ctrl_available(&self) -> zbus::Result<bool>;

        #[zbus(name = "SetNVIDIAPowerOffset")]
        fn set_nvidia_power_offset(&self, offset: i32) -> zbus::Result<bool>;

        #[zbus(name = "GetAvailableGovernors")]
        fn get_available_governors(&self) -> zbus::Result<String>;

        #[zbus(name = "GetAvailableEPPs")]
        fn get_available_epps(&self) -> zbus::Result<String>;

        #[zbus(name = "GetCpuCoreCount")]
        fn get_cpu_core_count(&self) -> zbus::Result<i32>;

        #[zbus(name = "GetCpuTopologyJSON")]
        fn get_cpu_topology_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetWaterCoolerAvailable")]
        fn get_water_cooler_available(&self) -> zbus::Result<bool>;

        #[zbus(name = "GetWaterCoolerConnected")]
        fn get_water_cooler_connected(&self) -> zbus::Result<bool>;

        #[zbus(name = "GetWaterCoolerFanSpeed")]
        fn get_water_cooler_fan_speed(&self) -> zbus::Result<i32>;

        #[zbus(name = "GetWaterCoolerPumpLevel")]
        fn get_water_cooler_pump_level(&self) -> zbus::Result<i32>;

        #[zbus(name = "EnableWaterCooler")]
        fn enable_water_cooler(&self, enable: bool) -> zbus::Result<bool>;

        #[zbus(name = "IsWaterCoolerEnabled")]
        fn is_water_cooler_enabled(&self) -> zbus::Result<bool>;

        #[zbus(name = "SetWaterCoolerFanSpeed")]
        fn set_water_cooler_fan_speed(&self, duty_cycle_percent: i32) -> zbus::Result<bool>;

        #[zbus(name = "SetWaterCoolerPumpVoltage")]
        fn set_water_cooler_pump_voltage(&self, voltage: i32) -> zbus::Result<bool>;

        #[zbus(name = "SetWaterCoolerLEDColor")]
        fn set_water_cooler_led_color(&self, red: i32, green: i32, blue: i32, mode_val: i32) -> zbus::Result<bool>;

        #[zbus(name = "TurnOffWaterCoolerLED")]
        fn turn_off_water_cooler_led(&self) -> zbus::Result<bool>;

        #[zbus(name = "TurnOffWaterCoolerFan")]
        fn turn_off_water_cooler_fan(&self) -> zbus::Result<bool>;

        #[zbus(name = "TurnOffWaterCoolerPump")]
        fn turn_off_water_cooler_pump(&self) -> zbus::Result<bool>;

        #[zbus(name = "IsWaterCoolerAutoControlEnabled")]
        fn is_water_cooler_auto_control_enabled(&self) -> zbus::Result<bool>;

        #[zbus(name = "SetWaterCoolerAutoControl")]
        fn set_water_cooler_auto_control(&self, enabled: bool) -> zbus::Result<bool>;

        #[zbus(name = "GetWaterCoolerSupported")]
        fn get_water_cooler_supported(&self) -> zbus::Result<bool>;

        #[zbus(name = "GetCTGPAdjustmentSupported")]
        fn get_ctgp_adjustment_supported(&self) -> zbus::Result<bool>;

        #[zbus(name = "GetHwpDynamicBoostSupported")]
        fn get_hwp_dynamic_boost_supported(&self) -> zbus::Result<bool>;

        #[zbus(name = "GetMonitorDataSince")]
        fn get_monitor_data_since(&self, since_timestamp_ms: i64) -> zbus::Result<Vec<u8>>;

        #[zbus(name = "SetMonitorHistoryHorizon")]
        fn set_monitor_history_horizon(&self, seconds: i32) -> zbus::Result<()>;

        #[zbus(name = "GetMonitorHistoryHorizon")]
        fn get_monitor_history_horizon(&self) -> zbus::Result<i32>;

        #[zbus(name = "GetCpuFrequencyMHz")]
        fn get_cpu_frequency_mhz(&self) -> zbus::Result<i32>;

    }
}

pub mod legacy {
    use std::collections::HashMap;
    use zbus::zvariant::OwnedValue;

    #[zbus::proxy(
        interface = "de.uniwill.uccd",
        default_service = "com.uniwill.uccd",
        default_path = "/com/uniwill/uccd"
    )]
    pub trait UccdLegacy {
        #[zbus(signal)]
        fn profile_changed(
            &self,
            profile_id: String,
            keyboard_profile_id: String,
            fan_profile_id: String,
        ) -> zbus::Result<()>;

        #[zbus(signal)]
        fn mode_reapply_pending_changed(&self, pending: bool) -> zbus::Result<()>;

        #[zbus(signal)]
        fn power_state_changed(&self, state: String) -> zbus::Result<()>;

        #[zbus(signal)]
        fn power_policy_changed(&self, json: String) -> zbus::Result<()>;

        #[zbus(signal)]
        fn power_device_status_changed(&self, json: String) -> zbus::Result<()>;

        #[zbus(signal)]
        fn lightbar_settings_changed(&self, json: String) -> zbus::Result<()>;

        #[zbus(signal)]
        fn water_cooler_status_changed(&self, status: String) -> zbus::Result<()>;

        // Methods (122)
        #[zbus(name = "GetDeviceName")]
        fn get_device_name(&self) -> zbus::Result<String>;

        #[zbus(name = "GetSystemInfoJSON")]
        fn get_system_info_json(&self) -> zbus::Result<String>;

        #[zbus(name = "IsDeviceSupported")]
        fn is_device_supported(&self) -> zbus::Result<bool>;

        #[zbus(name = "GetDisplayModesJSON")]
        fn get_display_modes_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetIsX11")]
        fn get_is_x11(&self) -> zbus::Result<bool>;

        #[zbus(name = "TuxedoWmiAvailable")]
        fn tuxedo_wmi_available(&self) -> zbus::Result<bool>;

        #[zbus(name = "FanHwmonAvailable")]
        fn fan_hwmon_available(&self) -> zbus::Result<bool>;

        #[zbus(name = "UccdVersion")]
        fn uccd_version(&self) -> zbus::Result<String>;

        #[zbus(name = "GetFanDataCPU")]
        fn get_fan_data_cpu(&self) -> zbus::Result<HashMap<String, OwnedValue>>;

        #[zbus(name = "GetFanDataGPU1")]
        fn get_fan_data_gpu1(&self) -> zbus::Result<HashMap<String, OwnedValue>>;

        #[zbus(name = "GetFanDataGPU2")]
        fn get_fan_data_gpu2(&self) -> zbus::Result<HashMap<String, OwnedValue>>;

        #[zbus(name = "WebcamSWAvailable")]
        fn webcam_sw_available(&self) -> zbus::Result<bool>;

        #[zbus(name = "GetWebcamSWStatus")]
        fn get_webcam_sw_status(&self) -> zbus::Result<bool>;

        #[zbus(name = "GetForceYUV420OutputSwitchAvailable")]
        fn get_force_yuv420_output_switch_available(&self) -> zbus::Result<bool>;

        #[zbus(name = "GetDisplayBrightness")]
        fn get_display_brightness(&self) -> zbus::Result<i32>;

        #[zbus(name = "GetKeyboardBacklightControlSupported")]
        fn get_keyboard_backlight_control_supported(&self) -> zbus::Result<bool>;

        #[zbus(name = "SetDisplayBrightness")]
        fn set_display_brightness(&self, brightness: i32) -> zbus::Result<bool>;

        #[zbus(name = "SetDisplayRefreshRate")]
        fn set_display_refresh_rate(&self, display: &str, refresh_rate: i32) -> zbus::Result<bool>;

        #[zbus(name = "GetDGpuInfoValuesJSON")]
        fn get_dgpu_info_values_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetIGpuInfoValuesJSON")]
        fn get_igpu_info_values_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetCpuPowerValuesJSON")]
        fn get_cpu_power_values_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetPrimeState")]
        fn get_prime_state(&self) -> zbus::Result<String>;

        #[zbus(name = "ConsumeModeReapplyPending")]
        fn consume_mode_reapply_pending(&self) -> zbus::Result<bool>;

        #[zbus(name = "GetActiveProfileJSON")]
        fn get_active_profile_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetPowerState")]
        fn get_power_state(&self) -> zbus::Result<String>;

        #[zbus(name = "GetPowerPolicyJSON")]
        fn get_power_policy_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetPowerDeviceSettingsJSON")]
        fn get_power_device_settings_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetPowerDeviceStatusJSON")]
        fn get_power_device_status_json(&self) -> zbus::Result<String>;

        #[zbus(name = "SetPowerDeviceSettingsJSON")]
        fn set_power_device_settings_json(&self, json: &str) -> zbus::Result<bool>;

        #[zbus(name = "ScanPowerDevices")]
        fn scan_power_devices(&self) -> zbus::Result<bool>;

        #[zbus(name = "GetLightbarSettingsJSON")]
        fn get_lightbar_settings_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetLightbarStatusJSON")]
        fn get_lightbar_status_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetDriverHealthJSON")]
        fn get_driver_health_json(&self) -> zbus::Result<String>;

        #[zbus(name = "SetLightbarSettingsJSON")]
        fn set_lightbar_settings_json(&self, json: &str) -> zbus::Result<bool>;

        #[zbus(name = "TriggerLightbarAnimation")]
        fn trigger_lightbar_animation(&self, force: bool) -> zbus::Result<bool>;

        #[zbus(name = "SetManualPowerMode")]
        fn set_manual_power_mode(&self, mode_val: &str) -> zbus::Result<bool>;

        #[zbus(name = "RegisterPowerRequest")]
        fn register_power_request(&self, name: &str, reason: &str, mode_val: &str) -> zbus::Result<bool>;

        #[zbus(name = "UnregisterPowerRequest")]
        fn unregister_power_request(&self, name: &str) -> zbus::Result<bool>;

        #[zbus(name = "SetTempProfile")]
        fn set_temp_profile(&self, profile_name: &str) -> zbus::Result<bool>;

        #[zbus(name = "SetActiveProfile")]
        fn set_active_profile(&self, id: &str) -> zbus::Result<bool>;

        #[zbus(name = "ApplyProfile")]
        fn apply_profile(&self, profile_json: &str) -> zbus::Result<bool>;

        #[zbus(name = "GetProfilesJSON")]
        fn get_profiles_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetCustomProfilesJSON")]
        fn get_custom_profiles_json(&self) -> zbus::Result<String>;

        #[zbus(name = "SetFanProfileCPU")]
        fn set_fan_profile_cpu(&self, points_json: &str) -> zbus::Result<bool>;

        #[zbus(name = "SetFanProfileDGPU")]
        fn set_fan_profile_dgpu(&self, points_json: &str) -> zbus::Result<bool>;

        #[zbus(name = "ApplyFanProfiles")]
        fn apply_fan_profiles(&self, fan_profiles_json: &str) -> zbus::Result<bool>;

        #[zbus(name = "RevertFanProfiles")]
        fn revert_fan_profiles(&self) -> zbus::Result<bool>;

        #[zbus(name = "GetDefaultProfilesJSON")]
        fn get_default_profiles_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetCpuFrequencyLimitsJSON")]
        fn get_cpu_frequency_limits_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetDefaultValuesProfileJSON")]
        fn get_default_values_profile_json(&self) -> zbus::Result<String>;

        #[zbus(name = "AddCustomProfile")]
        fn add_custom_profile(&self, profile_json: &str) -> zbus::Result<bool>;

        #[zbus(name = "SaveCustomProfile")]
        fn save_custom_profile(&self, profile_json: &str) -> zbus::Result<bool>;

        #[zbus(name = "DeleteCustomProfile")]
        fn delete_custom_profile(&self, profile_id: &str) -> zbus::Result<bool>;

        #[zbus(name = "UpdateCustomProfile")]
        fn update_custom_profile(&self, profile_json: &str) -> zbus::Result<bool>;

        #[zbus(name = "GetFanProfile")]
        fn get_fan_profile(&self, name: &str) -> zbus::Result<String>;

        #[zbus(name = "GetFanProfileNames")]
        fn get_fan_profile_names(&self) -> zbus::Result<String>;

        #[zbus(name = "SetFanProfile")]
        fn set_fan_profile(&self, name: &str, json: &str) -> zbus::Result<bool>;

        #[zbus(name = "GetSettingsJSON")]
        fn get_settings_json(&self) -> zbus::Result<String>;

        #[zbus(name = "SetStateMap")]
        fn set_state_map(&self, state: &str, profile_id: &str) -> zbus::Result<bool>;

        #[zbus(name = "SetBatchStateMap")]
        fn set_batch_state_map(&self, state_map_json: &str) -> zbus::Result<bool>;

        #[zbus(name = "ODMProfilesAvailable")]
        fn odm_profiles_available(&self) -> zbus::Result<Vec<String>>;

        #[zbus(name = "ODMPowerLimitsJSON")]
        fn odm_power_limits_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetKeyboardBacklightCapabilitiesJSON")]
        fn get_keyboard_backlight_capabilities_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetKeyboardBacklightStatesJSON")]
        fn get_keyboard_backlight_states_json(&self) -> zbus::Result<String>;

        #[zbus(name = "SetKeyboardBacklightStatesJSON")]
        fn set_keyboard_backlight_states_json(&self, keyboard_backlight_states_json: &str) -> zbus::Result<bool>;

        #[zbus(name = "GetCustomKeyboardProfilesJSON")]
        fn get_custom_keyboard_profiles_json(&self) -> zbus::Result<String>;

        #[zbus(name = "SaveCustomKeyboardProfile")]
        fn save_custom_keyboard_profile(&self, id: &str, name: &str, json: &str) -> zbus::Result<bool>;

        #[zbus(name = "DeleteCustomKeyboardProfile")]
        fn delete_custom_keyboard_profile(&self, id: &str) -> zbus::Result<bool>;

        #[zbus(name = "GetCustomFanProfilesJSON")]
        fn get_custom_fan_profiles_json(&self) -> zbus::Result<String>;

        #[zbus(name = "SaveCustomFanProfile")]
        fn save_custom_fan_profile(&self, id: &str, name: &str, json: &str) -> zbus::Result<bool>;

        #[zbus(name = "DeleteCustomFanProfile")]
        fn delete_custom_fan_profile(&self, id: &str) -> zbus::Result<bool>;

        #[zbus(name = "GetFansMinSpeed")]
        fn get_fans_min_speed(&self) -> zbus::Result<i32>;

        #[zbus(name = "GetFansOffAvailable")]
        fn get_fans_off_available(&self) -> zbus::Result<bool>;

        #[zbus(name = "GetChargingProfilesAvailable")]
        fn get_charging_profiles_available(&self) -> zbus::Result<String>;

        #[zbus(name = "GetCurrentChargingProfile")]
        fn get_current_charging_profile(&self) -> zbus::Result<String>;

        #[zbus(name = "SetChargingProfile")]
        fn set_charging_profile(&self, profile_descriptor: &str) -> zbus::Result<bool>;

        #[zbus(name = "GetChargingPrioritiesAvailable")]
        fn get_charging_priorities_available(&self) -> zbus::Result<String>;

        #[zbus(name = "GetCurrentChargingPriority")]
        fn get_current_charging_priority(&self) -> zbus::Result<String>;

        #[zbus(name = "SetChargingPriority")]
        fn set_charging_priority(&self, priority_descriptor: &str) -> zbus::Result<bool>;

        #[zbus(name = "GetChargeStartAvailableThresholds")]
        fn get_charge_start_available_thresholds(&self) -> zbus::Result<String>;

        #[zbus(name = "GetChargeEndAvailableThresholds")]
        fn get_charge_end_available_thresholds(&self) -> zbus::Result<String>;

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
        fn set_charge_type(&self, type_val: &str) -> zbus::Result<bool>;

        #[zbus(name = "GetFnLockSupported")]
        fn get_fn_lock_supported(&self) -> zbus::Result<bool>;

        #[zbus(name = "GetFnLockStatus")]
        fn get_fn_lock_status(&self) -> zbus::Result<bool>;

        #[zbus(name = "SetFnLockStatus")]
        fn set_fn_lock_status(&self, status: bool) -> zbus::Result<()>;

        #[zbus(name = "SetSensorDataCollectionStatus")]
        fn set_sensor_data_collection_status(&self, status: bool) -> zbus::Result<()>;

        #[zbus(name = "GetSensorDataCollectionStatus")]
        fn get_sensor_data_collection_status(&self) -> zbus::Result<bool>;

        #[zbus(name = "SetDGpuD0Metrics")]
        fn set_dgpu_d0_metrics(&self, status: bool) -> zbus::Result<()>;

        #[zbus(name = "GetNVIDIAPowerCTRLDefaultPowerLimit")]
        fn get_nvidia_power_ctrl_default_power_limit(&self) -> zbus::Result<i32>;

        #[zbus(name = "GetNVIDIAPowerCTRLMaxPowerLimit")]
        fn get_nvidia_power_ctrl_max_power_limit(&self) -> zbus::Result<i32>;

        #[zbus(name = "GetNVIDIAPowerCTRLAvailable")]
        fn get_nvidia_power_ctrl_available(&self) -> zbus::Result<bool>;

        #[zbus(name = "SetNVIDIAPowerOffset")]
        fn set_nvidia_power_offset(&self, offset: i32) -> zbus::Result<bool>;

        #[zbus(name = "GetAvailableGovernors")]
        fn get_available_governors(&self) -> zbus::Result<String>;

        #[zbus(name = "GetAvailableEPPs")]
        fn get_available_epps(&self) -> zbus::Result<String>;

        #[zbus(name = "GetCpuCoreCount")]
        fn get_cpu_core_count(&self) -> zbus::Result<i32>;

        #[zbus(name = "GetCpuTopologyJSON")]
        fn get_cpu_topology_json(&self) -> zbus::Result<String>;

        #[zbus(name = "GetWaterCoolerAvailable")]
        fn get_water_cooler_available(&self) -> zbus::Result<bool>;

        #[zbus(name = "GetWaterCoolerConnected")]
        fn get_water_cooler_connected(&self) -> zbus::Result<bool>;

        #[zbus(name = "GetWaterCoolerFanSpeed")]
        fn get_water_cooler_fan_speed(&self) -> zbus::Result<i32>;

        #[zbus(name = "GetWaterCoolerPumpLevel")]
        fn get_water_cooler_pump_level(&self) -> zbus::Result<i32>;

        #[zbus(name = "EnableWaterCooler")]
        fn enable_water_cooler(&self, enable: bool) -> zbus::Result<bool>;

        #[zbus(name = "IsWaterCoolerEnabled")]
        fn is_water_cooler_enabled(&self) -> zbus::Result<bool>;

        #[zbus(name = "SetWaterCoolerFanSpeed")]
        fn set_water_cooler_fan_speed(&self, duty_cycle_percent: i32) -> zbus::Result<bool>;

        #[zbus(name = "SetWaterCoolerPumpVoltage")]
        fn set_water_cooler_pump_voltage(&self, voltage: i32) -> zbus::Result<bool>;

        #[zbus(name = "SetWaterCoolerLEDColor")]
        fn set_water_cooler_led_color(&self, red: i32, green: i32, blue: i32, mode_val: i32) -> zbus::Result<bool>;

        #[zbus(name = "TurnOffWaterCoolerLED")]
        fn turn_off_water_cooler_led(&self) -> zbus::Result<bool>;

        #[zbus(name = "TurnOffWaterCoolerFan")]
        fn turn_off_water_cooler_fan(&self) -> zbus::Result<bool>;

        #[zbus(name = "TurnOffWaterCoolerPump")]
        fn turn_off_water_cooler_pump(&self) -> zbus::Result<bool>;

        #[zbus(name = "IsWaterCoolerAutoControlEnabled")]
        fn is_water_cooler_auto_control_enabled(&self) -> zbus::Result<bool>;

        #[zbus(name = "SetWaterCoolerAutoControl")]
        fn set_water_cooler_auto_control(&self, enabled: bool) -> zbus::Result<bool>;

        #[zbus(name = "GetWaterCoolerSupported")]
        fn get_water_cooler_supported(&self) -> zbus::Result<bool>;

        #[zbus(name = "GetCTGPAdjustmentSupported")]
        fn get_ctgp_adjustment_supported(&self) -> zbus::Result<bool>;

        #[zbus(name = "GetHwpDynamicBoostSupported")]
        fn get_hwp_dynamic_boost_supported(&self) -> zbus::Result<bool>;

        #[zbus(name = "GetMonitorDataSince")]
        fn get_monitor_data_since(&self, since_timestamp_ms: i64) -> zbus::Result<Vec<u8>>;

        #[zbus(name = "SetMonitorHistoryHorizon")]
        fn set_monitor_history_horizon(&self, seconds: i32) -> zbus::Result<()>;

        #[zbus(name = "GetMonitorHistoryHorizon")]
        fn get_monitor_history_horizon(&self) -> zbus::Result<i32>;

        #[zbus(name = "GetCpuFrequencyMHz")]
        fn get_cpu_frequency_mhz(&self) -> zbus::Result<i32>;

    }
}

pub use legacy::UccdLegacyProxy;
pub use primary::UccdProxy;
