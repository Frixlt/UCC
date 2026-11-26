//! Keyboard backlight and front lightbar lighting controllers with pluggable [`LightingTransport`].
//!
//! Currently supports Linux sysfs LED class nodes (`tuxedo-keyboard`, `ite_8291`, `ite_8291_lb`,
//! `tuxedo_nb04`, `tuxedo_nb05`) via [`SysfsLedTransport`] and deterministic unit testing via
//! [`MockLightingTransport`], with a clean seam for direct USB `hidraw` transport in `M14`.

pub mod keyboard;
pub mod lightbar;
pub mod transport;

pub use keyboard::{
    map_states_by_led_id, BufferInputGuard, KeyboardBacklightController, MECHREVO_ENTER_LED_ID,
    MECHREVO_EXPECTED_ZONES, MECHREVO_LCTRL_LED_ID, MECHREVO_LSHIFT_LED_ID,
    MECHREVO_PHYSICAL_KEY_COUNT,
};
pub use lightbar::{
    LightbarAnimation, LightbarColorMode, LightbarController, LightbarPowerMode,
    LightbarPowerProfile, LightbarSettings, LIGHTBAR_FRAME_INTERVAL,
};
pub use transport::{
    KeyboardCapabilities, LightbarCapabilities, LightingError, LightingOp, LightingTransport,
    MockLightingTransport, RgbColor, SysfsLedTransport,
};
