use ucc_dbus::*;

#[test]
fn test_dbus_constants() {
    assert_eq!(DBUS_SERVICE, "com.uniwill.uccd");
    assert_eq!(DBUS_PATH, "/com/uniwill/uccd");
    assert_eq!(DBUS_INTERFACE, "com.uniwill.uccd");
}

#[test]
fn test_default_backend_methods() {
    let interface = UccdInterface::default();
    assert_eq!(interface.get_device_name(), "Frixit Control Center Laptop");
    assert!(interface.is_device_supported());
    assert_eq!(interface.get_power_state(), "power_ac");
    assert_eq!(interface.get_display_brightness(), 100);
    assert!(!interface.get_water_cooler_connected());
    assert_eq!(interface.get_current_charging_profile(), "balanced");
}
