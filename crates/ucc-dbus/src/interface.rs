//! D-Bus Interface definitions for `com.uniwill.uccd` and `de.uniwill.uccd`.

use crate::auth::{AuthAction, Authorizer, PolkitAuthorizer};
use crate::backend::{DefaultUccdBackend, UccdBackend};
use std::collections::HashMap;
use std::sync::Arc;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedValue;

/// D-Bus interface handler for primary service `com.uniwill.uccd`.
pub struct UccdInterface<B: UccdBackend = DefaultUccdBackend> {
    backend: Arc<B>,
    authorizer: Arc<dyn Authorizer>,
}

impl<B: UccdBackend> UccdInterface<B> {
    pub fn new(backend: B) -> Self {
        Self::with_authorizer(backend, Arc::new(PolkitAuthorizer::new()))
    }

    pub fn with_authorizer(backend: B, authorizer: Arc<dyn Authorizer>) -> Self {
        Self {
            backend: Arc::new(backend),
            authorizer,
        }
    }

    pub fn new_with_backend_authorizer(backend: Arc<B>, authorizer: Arc<dyn Authorizer>) -> Self {
        Self { backend, authorizer }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn authorizer(&self) -> &dyn Authorizer {
        &*self.authorizer
    }
}

impl Default for UccdInterface<DefaultUccdBackend> {
    fn default() -> Self {
        Self::new(DefaultUccdBackend)
    }
}

/// D-Bus interface handler for legacy alias `de.uniwill.uccd`.
pub struct UccdLegacyInterface<B: UccdBackend = DefaultUccdBackend> {
    backend: Arc<B>,
    authorizer: Arc<dyn Authorizer>,
}

impl<B: UccdBackend> UccdLegacyInterface<B> {
    pub fn new(backend: B) -> Self {
        Self::with_authorizer(backend, Arc::new(PolkitAuthorizer::new()))
    }

    pub fn with_authorizer(backend: B, authorizer: Arc<dyn Authorizer>) -> Self {
        Self {
            backend: Arc::new(backend),
            authorizer,
        }
    }

    pub fn new_with_backend_authorizer(backend: Arc<B>, authorizer: Arc<dyn Authorizer>) -> Self {
        Self { backend, authorizer }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn authorizer(&self) -> &dyn Authorizer {
        &*self.authorizer
    }
}

impl Default for UccdLegacyInterface<DefaultUccdBackend> {
    fn default() -> Self {
        Self::new(DefaultUccdBackend)
    }
}

macro_rules! impl_uccd_methods {
    ($struct_name:ident, $iface_name:literal) => {
        #[zbus::interface(name = $iface_name)]
        impl<B: UccdBackend> $struct_name<B> {
            // Signals
            #[zbus(signal)]
            pub async fn profile_changed(
                emitter: &SignalEmitter<'_>,
                profile_id: &str,
                keyboard_profile_id: &str,
                fan_profile_id: &str,
            ) -> zbus::Result<()>;

            #[zbus(signal)]
            pub async fn mode_reapply_pending_changed(
                emitter: &SignalEmitter<'_>,
                pending: bool,
            ) -> zbus::Result<()>;

            #[zbus(signal)]
            pub async fn power_state_changed(emitter: &SignalEmitter<'_>, state: &str) -> zbus::Result<()>;

            #[zbus(signal)]
            pub async fn power_policy_changed(emitter: &SignalEmitter<'_>, json: &str) -> zbus::Result<()>;

            #[zbus(signal)]
            pub async fn power_device_status_changed(
                emitter: &SignalEmitter<'_>,
                json: &str,
            ) -> zbus::Result<()>;

            #[zbus(signal)]
            pub async fn lightbar_settings_changed(
                emitter: &SignalEmitter<'_>,
                json: &str,
            ) -> zbus::Result<()>;

            #[zbus(signal)]
            pub async fn water_cooler_status_changed(
                emitter: &SignalEmitter<'_>,
                status: &str,
            ) -> zbus::Result<()>;

            // Methods (122)
            #[zbus(name = "GetDeviceName")]
            pub async fn get_device_name(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return String::new();
                }
                self.backend.get_device_name()
            }

            #[zbus(name = "GetSystemInfoJSON")]
            pub async fn get_system_info_json(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return "{}".to_string();
                }
                self.backend.get_system_info_json()
            }

            #[zbus(name = "IsDeviceSupported")]
            pub async fn is_device_supported(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return false;
                }
                self.backend.is_device_supported()
            }

            #[zbus(name = "GetDisplayModesJSON")]
            pub async fn get_display_modes_json(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return "{}".to_string();
                }
                self.backend.get_display_modes_json()
            }

            #[zbus(name = "GetIsX11")]
            pub async fn get_is_x11(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return false;
                }
                self.backend.get_is_x11()
            }

            #[zbus(name = "TuxedoWmiAvailable")]
            pub async fn tuxedo_wmi_available(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return false;
                }
                self.backend.tuxedo_wmi_available()
            }

            #[zbus(name = "FanHwmonAvailable")]
            pub async fn fan_hwmon_available(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return false;
                }
                self.backend.fan_hwmon_available()
            }

            #[zbus(name = "UccdVersion")]
            pub async fn uccd_version(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return String::new();
                }
                self.backend.uccd_version()
            }

            #[zbus(name = "GetFanDataCPU")]
            pub async fn get_fan_data_cpu(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> HashMap<String, OwnedValue> {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return HashMap::new();
                }
                self.backend.get_fan_data_cpu()
            }

            #[zbus(name = "GetFanDataGPU1")]
            pub async fn get_fan_data_gpu1(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> HashMap<String, OwnedValue> {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return HashMap::new();
                }
                self.backend.get_fan_data_gpu1()
            }

            #[zbus(name = "GetFanDataGPU2")]
            pub async fn get_fan_data_gpu2(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> HashMap<String, OwnedValue> {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return HashMap::new();
                }
                self.backend.get_fan_data_gpu2()
            }

            #[zbus(name = "WebcamSWAvailable")]
            pub async fn webcam_sw_available(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return false;
                }
                self.backend.webcam_sw_available()
            }

            #[zbus(name = "GetWebcamSWStatus")]
            pub async fn get_webcam_sw_status(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return false;
                }
                self.backend.get_webcam_sw_status()
            }

            #[zbus(name = "SetWebcam")]
            pub async fn set_webcam(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, enabled: bool) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.set_webcam(enabled)
            }

            #[zbus(name = "GetForceYUV420OutputSwitchAvailable")]
            pub async fn get_force_yuv420_output_switch_available(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return false;
                }
                self.backend.get_force_yuv420_output_switch_available()
            }

            #[zbus(name = "GetDisplayBrightness")]
            pub async fn get_display_brightness(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> i32 {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return 0;
                }
                self.backend.get_display_brightness()
            }

            #[zbus(name = "GetKeyboardBacklightControlSupported")]
            pub async fn get_keyboard_backlight_control_supported(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return false;
                }
                self.backend.get_keyboard_backlight_control_supported()
            }

            #[zbus(name = "SetDisplayBrightness")]
            pub async fn set_display_brightness(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, brightness: i32) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.set_display_brightness(brightness)
            }

            #[zbus(name = "SetDisplayRefreshRate")]
            pub async fn set_display_refresh_rate(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, display: &str, refresh_rate: i32) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.set_display_refresh_rate(display, refresh_rate)
            }

            #[zbus(name = "GetDGpuInfoValuesJSON")]
            pub async fn get_dgpu_info_values_json(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return "{}".to_string();
                }
                self.backend.get_dgpu_info_values_json()
            }

            #[zbus(name = "GetIGpuInfoValuesJSON")]
            pub async fn get_igpu_info_values_json(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return "{}".to_string();
                }
                self.backend.get_igpu_info_values_json()
            }

            #[zbus(name = "GetCpuPowerValuesJSON")]
            pub async fn get_cpu_power_values_json(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return "{}".to_string();
                }
                self.backend.get_cpu_power_values_json()
            }

            #[zbus(name = "GetPrimeState")]
            pub async fn get_prime_state(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return String::new();
                }
                self.backend.get_prime_state()
            }

            #[zbus(name = "ConsumeModeReapplyPending")]
            pub async fn consume_mode_reapply_pending(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return false;
                }
                self.backend.consume_mode_reapply_pending()
            }

            #[zbus(name = "GetActiveProfileJSON")]
            pub async fn get_active_profile_json(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return "{}".to_string();
                }
                self.backend.get_active_profile_json()
            }

            #[zbus(name = "GetPowerState")]
            pub async fn get_power_state(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return String::new();
                }
                self.backend.get_power_state()
            }

            #[zbus(name = "GetPowerPolicyJSON")]
            pub async fn get_power_policy_json(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return "{}".to_string();
                }
                self.backend.get_power_policy_json()
            }

            #[zbus(name = "GetPowerDeviceSettingsJSON")]
            pub async fn get_power_device_settings_json(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return "{}".to_string();
                }
                self.backend.get_power_device_settings_json()
            }

            #[zbus(name = "GetPowerDeviceStatusJSON")]
            pub async fn get_power_device_status_json(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return "{}".to_string();
                }
                self.backend.get_power_device_status_json()
            }

            #[zbus(name = "SetPowerDeviceSettingsJSON")]
            pub async fn set_power_device_settings_json(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, json: &str) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.set_power_device_settings_json(json)
            }

            #[zbus(name = "ScanPowerDevices")]
            pub async fn scan_power_devices(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.scan_power_devices()
            }

            #[zbus(name = "GetLightbarSettingsJSON")]
            pub async fn get_lightbar_settings_json(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return "{}".to_string();
                }
                self.backend.get_lightbar_settings_json()
            }

            #[zbus(name = "GetLightbarStatusJSON")]
            pub async fn get_lightbar_status_json(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return "{}".to_string();
                }
                self.backend.get_lightbar_status_json()
            }

            #[zbus(name = "GetDriverHealthJSON")]
            pub async fn get_driver_health_json(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return "{}".to_string();
                }
                self.backend.get_driver_health_json()
            }

            #[zbus(name = "SetLightbarSettingsJSON")]
            pub async fn set_lightbar_settings_json(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, json: &str) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.set_lightbar_settings_json(json)
            }

            #[zbus(name = "TriggerLightbarAnimation")]
            pub async fn trigger_lightbar_animation(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, force: bool) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.trigger_lightbar_animation(force)
            }

            #[zbus(name = "SetManualPowerMode")]
            pub async fn set_manual_power_mode(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, mode_val: &str) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.set_manual_power_mode(mode_val)
            }

            #[zbus(name = "RegisterPowerRequest")]
            pub async fn register_power_request(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, name: &str, reason: &str, mode_val: &str) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.register_power_request(name, reason, mode_val)
            }

            #[zbus(name = "UnregisterPowerRequest")]
            pub async fn unregister_power_request(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, name: &str) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.unregister_power_request(name)
            }

            #[zbus(name = "SetTempProfile")]
            pub async fn set_temp_profile(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, profile_name: &str) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.set_temp_profile(profile_name)
            }

            #[zbus(name = "SetActiveProfile")]
            pub async fn set_active_profile(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, id: &str) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.set_active_profile(id)
            }

            #[zbus(name = "ApplyProfile")]
            pub async fn apply_profile(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, profile_json: &str) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.apply_profile(profile_json)
            }

            #[zbus(name = "GetProfilesJSON")]
            pub async fn get_profiles_json(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return "{}".to_string();
                }
                self.backend.get_profiles_json()
            }

            #[zbus(name = "GetCustomProfilesJSON")]
            pub async fn get_custom_profiles_json(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return "{}".to_string();
                }
                self.backend.get_custom_profiles_json()
            }

            #[zbus(name = "SetFanProfileCPU")]
            pub async fn set_fan_profile_cpu(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, points_json: &str) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.set_fan_profile_cpu(points_json)
            }

            #[zbus(name = "SetFanProfileDGPU")]
            pub async fn set_fan_profile_dgpu(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, points_json: &str) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.set_fan_profile_dgpu(points_json)
            }

            #[zbus(name = "ApplyFanProfiles")]
            pub async fn apply_fan_profiles(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, fan_profiles_json: &str) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.apply_fan_profiles(fan_profiles_json)
            }

            #[zbus(name = "RevertFanProfiles")]
            pub async fn revert_fan_profiles(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.revert_fan_profiles()
            }

            #[zbus(name = "GetDefaultProfilesJSON")]
            pub async fn get_default_profiles_json(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return "{}".to_string();
                }
                self.backend.get_default_profiles_json()
            }

            #[zbus(name = "GetCpuFrequencyLimitsJSON")]
            pub async fn get_cpu_frequency_limits_json(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return "{}".to_string();
                }
                self.backend.get_cpu_frequency_limits_json()
            }

            #[zbus(name = "GetDefaultValuesProfileJSON")]
            pub async fn get_default_values_profile_json(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return "{}".to_string();
                }
                self.backend.get_default_values_profile_json()
            }

            #[zbus(name = "AddCustomProfile")]
            pub async fn add_custom_profile(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, profile_json: &str) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.add_custom_profile(profile_json)
            }

            #[zbus(name = "SaveCustomProfile")]
            pub async fn save_custom_profile(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, profile_json: &str) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.save_custom_profile(profile_json)
            }

            #[zbus(name = "DeleteCustomProfile")]
            pub async fn delete_custom_profile(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, profile_id: &str) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.delete_custom_profile(profile_id)
            }

            #[zbus(name = "UpdateCustomProfile")]
            pub async fn update_custom_profile(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, profile_json: &str) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.update_custom_profile(profile_json)
            }

            #[zbus(name = "GetFanProfile")]
            pub async fn get_fan_profile(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, name: &str) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return String::new();
                }
                self.backend.get_fan_profile(name)
            }

            #[zbus(name = "GetFanProfileNames")]
            pub async fn get_fan_profile_names(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return String::new();
                }
                self.backend.get_fan_profile_names()
            }

            #[zbus(name = "SetFanProfile")]
            pub async fn set_fan_profile(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, name: &str, json: &str) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.set_fan_profile(name, json)
            }

            #[zbus(name = "GetSettingsJSON")]
            pub async fn get_settings_json(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return "{}".to_string();
                }
                self.backend.get_settings_json()
            }

            #[zbus(name = "SetStateMap")]
            pub async fn set_state_map(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, state: &str, profile_id: &str) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.set_state_map(state, profile_id)
            }

            #[zbus(name = "SetBatchStateMap")]
            pub async fn set_batch_state_map(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, state_map_json: &str) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.set_batch_state_map(state_map_json)
            }

            #[zbus(name = "ODMProfilesAvailable")]
            pub async fn odm_profiles_available(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> Vec<String> {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return Vec::new();
                }
                self.backend.odm_profiles_available()
            }

            #[zbus(name = "ODMPowerLimitsJSON")]
            pub async fn odm_power_limits_json(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return "{}".to_string();
                }
                self.backend.odm_power_limits_json()
            }

            #[zbus(name = "GetKeyboardBacklightCapabilitiesJSON")]
            pub async fn get_keyboard_backlight_capabilities_json(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return "{}".to_string();
                }
                self.backend.get_keyboard_backlight_capabilities_json()
            }

            #[zbus(name = "GetKeyboardBacklightStatesJSON")]
            pub async fn get_keyboard_backlight_states_json(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return "{}".to_string();
                }
                self.backend.get_keyboard_backlight_states_json()
            }

            #[zbus(name = "SetKeyboardBacklightStatesJSON")]
            pub async fn set_keyboard_backlight_states_json(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, keyboard_backlight_states_json: &str) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.set_keyboard_backlight_states_json(keyboard_backlight_states_json)
            }

            #[zbus(name = "GetCustomKeyboardProfilesJSON")]
            pub async fn get_custom_keyboard_profiles_json(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return "{}".to_string();
                }
                self.backend.get_custom_keyboard_profiles_json()
            }

            #[zbus(name = "SaveCustomKeyboardProfile")]
            pub async fn save_custom_keyboard_profile(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, id: &str, name: &str, json: &str) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.save_custom_keyboard_profile(id, name, json)
            }

            #[zbus(name = "DeleteCustomKeyboardProfile")]
            pub async fn delete_custom_keyboard_profile(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, id: &str) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.delete_custom_keyboard_profile(id)
            }

            #[zbus(name = "GetCustomFanProfilesJSON")]
            pub async fn get_custom_fan_profiles_json(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return "{}".to_string();
                }
                self.backend.get_custom_fan_profiles_json()
            }

            #[zbus(name = "SaveCustomFanProfile")]
            pub async fn save_custom_fan_profile(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, id: &str, name: &str, json: &str) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.save_custom_fan_profile(id, name, json)
            }

            #[zbus(name = "DeleteCustomFanProfile")]
            pub async fn delete_custom_fan_profile(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, id: &str) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.delete_custom_fan_profile(id)
            }

            #[zbus(name = "GetFansMinSpeed")]
            pub async fn get_fans_min_speed(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> i32 {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return 0;
                }
                self.backend.get_fans_min_speed()
            }

            #[zbus(name = "GetFansOffAvailable")]
            pub async fn get_fans_off_available(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return false;
                }
                self.backend.get_fans_off_available()
            }

            #[zbus(name = "GetChargingProfilesAvailable")]
            pub async fn get_charging_profiles_available(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return String::new();
                }
                self.backend.get_charging_profiles_available()
            }

            #[zbus(name = "GetCurrentChargingProfile")]
            pub async fn get_current_charging_profile(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return String::new();
                }
                self.backend.get_current_charging_profile()
            }

            #[zbus(name = "SetChargingProfile")]
            pub async fn set_charging_profile(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, profile_descriptor: &str) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.set_charging_profile(profile_descriptor)
            }

            #[zbus(name = "GetChargingPrioritiesAvailable")]
            pub async fn get_charging_priorities_available(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return String::new();
                }
                self.backend.get_charging_priorities_available()
            }

            #[zbus(name = "GetCurrentChargingPriority")]
            pub async fn get_current_charging_priority(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return String::new();
                }
                self.backend.get_current_charging_priority()
            }

            #[zbus(name = "SetChargingPriority")]
            pub async fn set_charging_priority(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, priority_descriptor: &str) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.set_charging_priority(priority_descriptor)
            }

            #[zbus(name = "GetChargeStartAvailableThresholds")]
            pub async fn get_charge_start_available_thresholds(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return String::new();
                }
                self.backend.get_charge_start_available_thresholds()
            }

            #[zbus(name = "GetChargeEndAvailableThresholds")]
            pub async fn get_charge_end_available_thresholds(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return String::new();
                }
                self.backend.get_charge_end_available_thresholds()
            }

            #[zbus(name = "GetChargeStartThreshold")]
            pub async fn get_charge_start_threshold(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> i32 {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return -1;
                }
                self.backend.get_charge_start_threshold()
            }

            #[zbus(name = "GetChargeEndThreshold")]
            pub async fn get_charge_end_threshold(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> i32 {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return -1;
                }
                self.backend.get_charge_end_threshold()
            }

            #[zbus(name = "SetChargeStartThreshold")]
            pub async fn set_charge_start_threshold(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, value: i32) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::ManageHardware).await {
                    return false;
                }
                self.backend.set_charge_start_threshold(value)
            }

            #[zbus(name = "SetChargeEndThreshold")]
            pub async fn set_charge_end_threshold(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, value: i32) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::ManageHardware).await {
                    return false;
                }
                self.backend.set_charge_end_threshold(value)
            }

            #[zbus(name = "GetChargeType")]
            pub async fn get_charge_type(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return String::new();
                }
                self.backend.get_charge_type()
            }

            #[zbus(name = "SetChargeType")]
            pub async fn set_charge_type(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, type_val: &str) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::ManageHardware).await {
                    return false;
                }
                self.backend.set_charge_type(type_val)
            }

            #[zbus(name = "GetFnLockSupported")]
            pub async fn get_fn_lock_supported(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return false;
                }
                self.backend.get_fn_lock_supported()
            }

            #[zbus(name = "GetFnLockStatus")]
            pub async fn get_fn_lock_status(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return false;
                }
                self.backend.get_fn_lock_status()
            }

            #[zbus(name = "SetFnLockStatus")]
            pub async fn set_fn_lock_status(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, status: bool) {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return;
                }
                self.backend.set_fn_lock_status(status);
            }

            #[zbus(name = "SetSensorDataCollectionStatus")]
            pub async fn set_sensor_data_collection_status(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, status: bool) {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return;
                }
                self.backend.set_sensor_data_collection_status(status);
            }

            #[zbus(name = "GetSensorDataCollectionStatus")]
            pub async fn get_sensor_data_collection_status(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return false;
                }
                self.backend.get_sensor_data_collection_status()
            }

            #[zbus(name = "SetDGpuD0Metrics")]
            pub async fn set_dgpu_d0_metrics(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, status: bool) {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return;
                }
                self.backend.set_dgpu_d0_metrics(status);
            }

            #[zbus(name = "GetNVIDIAPowerCTRLDefaultPowerLimit")]
            pub async fn get_nvidia_power_ctrl_default_power_limit(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> i32 {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return 0;
                }
                self.backend.get_nvidia_power_ctrl_default_power_limit()
            }

            #[zbus(name = "GetNVIDIAPowerCTRLMaxPowerLimit")]
            pub async fn get_nvidia_power_ctrl_max_power_limit(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> i32 {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return 0;
                }
                self.backend.get_nvidia_power_ctrl_max_power_limit()
            }

            #[zbus(name = "GetNVIDIAPowerCTRLAvailable")]
            pub async fn get_nvidia_power_ctrl_available(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return false;
                }
                self.backend.get_nvidia_power_ctrl_available()
            }

            #[zbus(name = "SetNVIDIAPowerOffset")]
            pub async fn set_nvidia_power_offset(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, offset: i32) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.set_nvidia_power_offset(offset)
            }

            #[zbus(name = "GetAvailableGovernors")]
            pub async fn get_available_governors(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return String::new();
                }
                self.backend.get_available_governors()
            }

            #[zbus(name = "GetAvailableEPPs")]
            pub async fn get_available_epps(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return String::new();
                }
                self.backend.get_available_epps()
            }

            #[zbus(name = "GetCpuCoreCount")]
            pub async fn get_cpu_core_count(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> i32 {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return 0;
                }
                self.backend.get_cpu_core_count()
            }

            #[zbus(name = "GetCpuTopologyJSON")]
            pub async fn get_cpu_topology_json(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> String {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return "{}".to_string();
                }
                self.backend.get_cpu_topology_json()
            }

            #[zbus(name = "GetWaterCoolerAvailable")]
            pub async fn get_water_cooler_available(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return false;
                }
                self.backend.get_water_cooler_available()
            }

            #[zbus(name = "GetWaterCoolerConnected")]
            pub async fn get_water_cooler_connected(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return false;
                }
                self.backend.get_water_cooler_connected()
            }

            #[zbus(name = "GetWaterCoolerFanSpeed")]
            pub async fn get_water_cooler_fan_speed(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> i32 {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return 0;
                }
                self.backend.get_water_cooler_fan_speed()
            }

            #[zbus(name = "GetWaterCoolerPumpLevel")]
            pub async fn get_water_cooler_pump_level(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> i32 {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return 0;
                }
                self.backend.get_water_cooler_pump_level()
            }

            #[zbus(name = "EnableWaterCooler")]
            pub async fn enable_water_cooler(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, enable: bool) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.enable_water_cooler(enable)
            }

            #[zbus(name = "IsWaterCoolerEnabled")]
            pub async fn is_water_cooler_enabled(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return false;
                }
                self.backend.is_water_cooler_enabled()
            }

            #[zbus(name = "SetWaterCoolerFanSpeed")]
            pub async fn set_water_cooler_fan_speed(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, duty_cycle_percent: i32) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.set_water_cooler_fan_speed(duty_cycle_percent)
            }

            #[zbus(name = "SetWaterCoolerPumpVoltage")]
            pub async fn set_water_cooler_pump_voltage(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, voltage: i32) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.set_water_cooler_pump_voltage(voltage)
            }

            #[zbus(name = "SetWaterCoolerLEDColor")]
            pub async fn set_water_cooler_led_color(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, red: i32, green: i32, blue: i32, mode_val: i32) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.set_water_cooler_led_color(red, green, blue, mode_val)
            }

            #[zbus(name = "TurnOffWaterCoolerLED")]
            pub async fn turn_off_water_cooler_led(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.turn_off_water_cooler_led()
            }

            #[zbus(name = "TurnOffWaterCoolerFan")]
            pub async fn turn_off_water_cooler_fan(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.turn_off_water_cooler_fan()
            }

            #[zbus(name = "TurnOffWaterCoolerPump")]
            pub async fn turn_off_water_cooler_pump(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.turn_off_water_cooler_pump()
            }

            #[zbus(name = "IsWaterCoolerAutoControlEnabled")]
            pub async fn is_water_cooler_auto_control_enabled(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return false;
                }
                self.backend.is_water_cooler_auto_control_enabled()
            }

            #[zbus(name = "SetWaterCoolerAutoControl")]
            pub async fn set_water_cooler_auto_control(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, enabled: bool) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return false;
                }
                self.backend.set_water_cooler_auto_control(enabled)
            }

            #[zbus(name = "GetWaterCoolerSupported")]
            pub async fn get_water_cooler_supported(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return false;
                }
                self.backend.get_water_cooler_supported()
            }

            #[zbus(name = "GetCTGPAdjustmentSupported")]
            pub async fn get_ctgp_adjustment_supported(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return false;
                }
                self.backend.get_ctgp_adjustment_supported()
            }

            #[zbus(name = "GetHwpDynamicBoostSupported")]
            pub async fn get_hwp_dynamic_boost_supported(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> bool {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return false;
                }
                self.backend.get_hwp_dynamic_boost_supported()
            }

            #[zbus(name = "GetMonitorDataSince")]
            pub async fn get_monitor_data_since(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, since_timestamp_ms: i64) -> Vec<u8> {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return Vec::new();
                }
                self.backend.get_monitor_data_since(since_timestamp_ms)
            }

            #[zbus(name = "SetMonitorHistoryHorizon")]
            pub async fn set_monitor_history_horizon(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection, seconds: i32) {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Control).await {
                    return;
                }
                self.backend.set_monitor_history_horizon(seconds);
            }

            #[zbus(name = "GetMonitorHistoryHorizon")]
            pub async fn get_monitor_history_horizon(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> i32 {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return 0;
                }
                self.backend.get_monitor_history_horizon()
            }

            #[zbus(name = "GetCpuFrequencyMHz")]
            pub async fn get_cpu_frequency_mhz(&self, #[zbus(header)] header: zbus::message::Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> i32 {
                let sender = header.sender().map(|s| s.as_str());
                if !self.authorizer.check_auth(conn, sender, AuthAction::Read).await {
                    return 0;
                }
                self.backend.get_cpu_frequency_mhz()
            }

        }
    };
}

impl_uccd_methods!(UccdInterface, "com.uniwill.uccd");
impl_uccd_methods!(UccdLegacyInterface, "de.uniwill.uccd");
