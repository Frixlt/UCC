//! Keyboard backlight and front lightbar lighting controllers with pluggable [`LightingTransport`].
//!
//! Supports Linux sysfs LED class nodes (`tuxedo-keyboard`, `ite_8291`, `ite_8291_lb`,
//! `tuxedo_nb04`, `tuxedo_nb05`) via [`SysfsLedTransport`], direct USB HID feature reports via
//! [`UniwillHidTransport`] (Linux `/dev/hidraw*` [`HidrawDeviceTransport`] and Windows
//! [`WindowsHidTransport`]), and deterministic unit testing via [`MockLightingTransport`] and
//! [`MockHidTransport`].

pub mod hid_transport;
pub mod hidraw_linux;
pub mod keyboard;
pub mod lightbar;
pub mod transport;
pub mod windows_hid;

pub use hid_transport::{
    create_hidraw_lighting_transport, create_hidraw_lighting_transport_with_ec_power,
    open_best_lighting_transport, read_sysfs_hidraw_interface_number,
    select_best_lighting_transport, EcLidLogoHook, EcLightbarPowerHook, HidDeviceInfo, HidDeviceTransport,
    MockHidTransport, UniwillHidTransport, EC_LIGHTBAR_POWER_REG_PRIMARY,
    EC_LIGHTBAR_POWER_REG_SECONDARY, ITE_KEYBOARD_MAX_HW_BRIGHTNESS, ITE_KEYBOARD_PRODUCT_ID,
    ITE_LIGHTBAR_PRODUCT_IDS, ITE_LIGHTBAR_PRODUCT_ID_4ZONE, ITE_LIGHTBAR_PRODUCT_ID_5ZONE,
    ITE_VENDOR_ID, LIGHTBAR_FRONT_ZONES, LIGHTBAR_LID_LOGO_ZONE_ID,
};
pub use hidraw_linux::{
    hidiocgfeature_request, hidiocsfeature_request, parse_hid_id_from_uevent,
    parse_hid_report_descriptor_usage_page, read_sysfs_hidraw_usage_page, HidrawDeviceTransport,
    DEFAULT_DEV_ROOT, DEFAULT_SYSFS_HIDRAW_ROOT, HIDRAW_MAX_FEATURE_REPORT_LEN,
};

pub use keyboard::{
    map_states_by_led_id, BufferInputGuard, KeyboardBacklightController, KeyboardHardwareSnapshot,
    MECHREVO_ENTER_LED_ID, MECHREVO_EXPECTED_ZONES, MECHREVO_LCTRL_LED_ID, MECHREVO_LSHIFT_LED_ID,
    MECHREVO_PHYSICAL_KEY_COUNT,
};
pub use lightbar::{
    LightbarAnimation, LightbarColorMode, LightbarController, LightbarPowerMode,
    LightbarPowerProfile, LidLogoSettings, LightbarSettings, LIGHTBAR_FRAME_INTERVAL,
};
pub use transport::{
    KeyboardCapabilities, LightbarCapabilities, LightingError, LightingOp, LightingTransport,
    MockLightingTransport, RgbColor, SysfsLedTransport,
};
#[cfg(windows)]
pub use windows_hid::LiveWinHidSysApi;
pub use windows_hid::{
    frame_windows_feature_report, frame_windows_output_report, unframe_windows_feature_report,
    MockWinHidSysApi, WinHidDeviceRecord, WinHidSysApi, WindowsHidTransport, VENDOR_USAGE_PAGE_MIN,
};
