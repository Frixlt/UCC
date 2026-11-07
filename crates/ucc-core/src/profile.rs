use crate::charging::ChargingPolicy;
use crate::cpu::ProfileCpu;
use crate::error::ValidationError;
use crate::fan::{FanCurve, FanPoint, PumpPoint, PumpVoltage};
use serde::{Deserialize, Serialize};

/// Display resolution and refresh rate settings for a profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileDisplay {
    pub brightness: i32,
    pub use_brightness: bool,
    pub refresh_rate: i32,
    pub use_ref_rate: bool,
    pub x_resolution: i32,
    pub y_resolution: i32,
    pub use_resolution: bool,
}

impl Default for ProfileDisplay {
    fn default() -> Self {
        Self {
            brightness: 100,
            use_brightness: false,
            refresh_rate: -1,
            use_ref_rate: false,
            x_resolution: -1,
            y_resolution: -1,
            use_resolution: false,
        }
    }
}

impl ProfileDisplay {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.use_brightness && !(0..=100).contains(&self.brightness) {
            return Err(ValidationError::InvalidDisplayBrightness(self.brightness));
        }
        Ok(())
    }
}

/// Webcam toggle status for a profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileWebcam {
    pub status: bool,
    pub use_status: bool,
}

impl Default for ProfileWebcam {
    fn default() -> Self {
        Self {
            status: true,
            use_status: true,
        }
    }
}

/// Fan and water cooler control settings within a profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileFanControl {
    pub use_control: bool,
    pub fan_profile: String,
    pub same_speed: bool,
    pub auto_control_wc: bool,
    pub manual_fan_speed: Option<u8>,
    pub manual_pump_voltage: Option<i32>,
    pub enable_water_cooler: bool,
    pub table_cpu: Vec<FanPoint>,
    pub table_gpu: Vec<FanPoint>,
    pub table_pump: Vec<PumpPoint>,
    pub table_water_cooler_fan: Vec<FanPoint>,
}

impl Default for ProfileFanControl {
    fn default() -> Self {
        Self {
            use_control: true,
            fan_profile: "fan-balanced".to_string(),
            same_speed: true,
            auto_control_wc: true,
            manual_fan_speed: None,
            manual_pump_voltage: None,
            enable_water_cooler: false,
            table_cpu: Vec::new(),
            table_gpu: Vec::new(),
            table_pump: Vec::new(),
            table_water_cooler_fan: Vec::new(),
        }
    }
}

impl ProfileFanControl {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if let Some(speed) = self.manual_fan_speed {
            if speed > 100 {
                return Err(ValidationError::InvalidFanSpeed { speed });
            }
        }

        if let Some(voltage) = self.manual_pump_voltage {
            PumpVoltage::from_discrete_code(voltage)?;
        }

        if !self.table_cpu.is_empty() {
            FanCurve::new(self.table_cpu.clone())?.validate()?;
        }
        if !self.table_gpu.is_empty() {
            FanCurve::new(self.table_gpu.clone())?.validate()?;
        }
        if !self.table_water_cooler_fan.is_empty() {
            FanCurve::new(self.table_water_cooler_fan.clone())?.validate()?;
        }
        for point in &self.table_pump {
            point.validate()?;
        }

        Ok(())
    }
}

/// Keyboard backlight references within a profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProfileKeyboard {
    pub keyboard_profile_name: String,
    pub keyboard_profile_id: String,
    pub keyboard_profile_data: String,
}

/// Complete UCC system performance and hardware configuration profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UccProfile {
    pub id: String,
    pub name: String,
    pub description: String,
    pub display: ProfileDisplay,
    pub cpu: ProfileCpu,
    pub webcam: ProfileWebcam,
    pub fan: ProfileFanControl,
    pub keyboard: ProfileKeyboard,
    pub odm_profile_name: Option<String>,
    pub odm_power_limits: Vec<u32>,
    pub nvidia_ctgp_offset: Option<i32>,
    pub charging: ChargingPolicy,
}

impl UccProfile {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            display: ProfileDisplay::default(),
            cpu: ProfileCpu::default(),
            webcam: ProfileWebcam::default(),
            fan: ProfileFanControl::default(),
            keyboard: ProfileKeyboard::default(),
            odm_profile_name: None,
            odm_power_limits: Vec::new(),
            nvidia_ctgp_offset: None,
            charging: ChargingPolicy::default(),
        }
    }

    /// Validates the profile against all domain invariants:
    /// - Display brightness
    /// - CPU limits, frequency bounds, core selection
    /// - ODM power limits (TDP range and monotonicity)
    /// - Fan speed, pump voltage, and curve monotonicity
    /// - Charging policy thresholds
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.display.validate()?;
        self.cpu.validate(&self.odm_power_limits)?;
        self.fan.validate()?;
        self.charging.validate()?;
        Ok(())
    }
}
