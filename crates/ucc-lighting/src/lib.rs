//! Keyboard backlight and front lightbar lighting controllers with pluggable [`LightingTransport`].
//!
//! Currently supports Linux sysfs LED class nodes (`tuxedo-keyboard`, `ite_8291`, `ite_8291_lb`,
//! `tuxedo_nb04`, `tuxedo_nb05`) via [`SysfsLedTransport`], direct USB HID feature reports via
//! [`UniwillHidTransport`] / [`WindowsHidTransport`], and deterministic unit testing via
//! [`MockLightingTransport`].

pub mod hid_transport;
pub mod keyboard;
pub mod lightbar;
pub mod transport;
pub mod windows_hid;

pub use hid_transport::{
    HidDeviceInfo, HidDeviceTransport, MockHidTransport, UniwillHidTransport,
    ITE_KEYBOARD_PRODUCT_ID, ITE_LIGHTBAR_PRODUCT_IDS, ITE_VENDOR_ID,
};

pub use keyboard::{
    map_states_by_led_id, BufferInputGuard, KeyboardBacklightController, KeyboardHardwareSnapshot,
    MECHREVO_ENTER_LED_ID, MECHREVO_EXPECTED_ZONES, MECHREVO_LCTRL_LED_ID, MECHREVO_LSHIFT_LED_ID,
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
#[cfg(windows)]
pub use windows_hid::LiveWinHidSysApi;
pub use windows_hid::{
    frame_windows_feature_report, unframe_windows_feature_report, MockWinHidSysApi,
    WinHidDeviceRecord, WinHidSysApi, WindowsHidTransport, VENDOR_USAGE_PAGE_MIN,
};
