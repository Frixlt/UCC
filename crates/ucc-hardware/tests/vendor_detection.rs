use ucc_hardware::detection::{
    build_laptop_model, classify_manufacturer, detect_device_capabilities, detect_vendor,
    identify_device, is_ctgp_supported, is_device_supported, is_hwp_dynamic_boost_supported,
    is_water_cooler_supported, DeviceVendor, DmiInfo, DmiQuirks, DmiTuple, LaptopManufacturer,
    UniwillDeviceID, SUPPORTED_DEVICE_SKUS,
};
use ucc_hardware::ioctl::requests;
use ucc_hardware::traits::{MockHardwareIo, MockSysfs};
use std::path::Path;

#[test]
fn test_ft_f01_01_exact_mechrevo_tuple_and_near_misses() {
    // Exact 4-field Mechrevo tuple matching baseline test_device_identification.py
    let exact_dmi = DmiInfo {
        sys_vendor: "MECHREVO".to_string(),
        board_vendor: "".to_string(),
        product_name: "YAOSHI Series".to_string(),
        board_name: "YAOSHI Series-X6AR55xY".to_string(),
        board_version: "Standard".to_string(),
        product_sku: "0001".to_string(),
        bios_version: "N.1.04".to_string(),
    };

    let required_tuple = DmiTuple {
        sys_vendor: Some("MECHREVO"),
        product_name: None,
        board_name: Some("YAOSHI Series-X6AR55xY"),
        board_version: Some("Standard"),
        product_sku: Some("0001"),
        board_vendor: None,
    };

    // 1. Positive exact match passes quirk gate and whitelist
    assert!(DmiQuirks::exact_dmi_match(&exact_dmi, &required_tuple));
    assert!(is_device_supported(&exact_dmi));
    assert_eq!(
        identify_device(&exact_dmi, None),
        Some(UniwillDeviceID::STELLARIS16I07)
    );

    // 2. Near-misses: mutating each field must strictly fail the quirk gate and identify_device
    let fields = ["sys_vendor", "board_name", "board_version", "product_sku"];
    for field in fields {
        let mut mutated = exact_dmi.clone();
        match field {
            "sys_vendor" => mutated.sys_vendor.push_str("-different"),
            "board_name" => mutated.board_name.push_str("-different"),
            "board_version" => mutated.board_version.push_str("-different"),
            "product_sku" => mutated.product_sku.push_str("-different"),
            _ => unreachable!(),
        }

        assert!(
            !DmiQuirks::exact_dmi_match(&mutated, &required_tuple),
            "Near-miss mutation of {} should fail DmiQuirks",
            field
        );
        assert!(
            !is_device_supported(&mutated),
            "Near-miss mutation of {} should not be supported",
            field
        );
        assert_eq!(
            identify_device(&mutated, None),
            None,
            "Near-miss mutation of {} should not identify as any device",
            field
        );

        // Erasing the field
        let mut erased = exact_dmi.clone();
        match field {
            "sys_vendor" => erased.sys_vendor.clear(),
            "board_name" => erased.board_name.clear(),
            "board_version" => erased.board_version.clear(),
            "product_sku" => erased.product_sku.clear(),
            _ => unreachable!(),
        }
        assert!(
            !DmiQuirks::exact_dmi_match(&erased, &required_tuple),
            "Erased field {} should fail DmiQuirks",
            field
        );
        assert!(
            !is_device_supported(&erased),
            "Erased field {} should fail is_device_supported",
            field
        );
        assert_eq!(identify_device(&erased, None), None);
    }
}

#[test]
fn test_ft_f01_01_supported_skus_whitelist() {
    for &sku in SUPPORTED_DEVICE_SKUS {
        let dmi = DmiInfo {
            sys_vendor: "TUXEDO".to_string(),
            product_sku: sku.to_string(),
            ..Default::default()
        };
        assert!(
            is_device_supported(&dmi),
            "Whitelisted SKU {} must be supported",
            sku
        );
    }

    let unsupported_dmi = DmiInfo {
        sys_vendor: "GenericOEM".to_string(),
        product_sku: "GENERIC_999".to_string(),
        ..Default::default()
    };
    assert!(!is_device_supported(&unsupported_dmi));
}

#[test]
fn test_ft_f01_02_vendor_selected_from_capabilities_not_marketing_name() {
    let mock = MockHardwareIo::new();

    // 1. Marketing name says Clevo, but capabilities return Uniwill
    let clevo_dmi = DmiInfo {
        sys_vendor: "Clevo".to_string(),
        product_name: "Clevo PB50".to_string(),
        ..Default::default()
    };
    assert_eq!(classify_manufacturer(&clevo_dmi.sys_vendor, ""), LaptopManufacturer::Unknown);

    // Inject Uniwill capability: R_HWCHECK_UW = 1, R_HWCHECK_CL = 0
    mock.set_i32_read(requests::R_HWCHECK_CL, 0);
    mock.set_i32_read(requests::R_HWCHECK_UW, 1);
    assert_eq!(detect_vendor(&mock), DeviceVendor::Uniwill);

    // 2. Marketing name says Tuxedo, but capabilities return Clevo
    let mock_clevo = MockHardwareIo::new();
    mock_clevo.set_i32_read(requests::R_HWCHECK_CL, 1);
    mock_clevo.set_i32_read(requests::R_HWCHECK_UW, 0);
    assert_eq!(detect_vendor(&mock_clevo), DeviceVendor::Clevo);

    // 3. Neither capability responds -> Unsupported
    let mock_unsupported = MockHardwareIo::new();
    mock_unsupported.set_i32_read(requests::R_HWCHECK_CL, 0);
    mock_unsupported.set_i32_read(requests::R_HWCHECK_UW, 0);
    assert_eq!(detect_vendor(&mock_unsupported), DeviceVendor::Unsupported);

    // Marketing name cannot override missing capabilities
    let tuxedo_dmi = DmiInfo {
        sys_vendor: "TUXEDO Computers GmbH".to_string(),
        product_name: "TUXEDO Stellaris 16 Gen6".to_string(),
        ..Default::default()
    };
    let caps = detect_device_capabilities(&mock_unsupported, Some(&tuxedo_dmi), None);
    assert_eq!(caps.vendor, DeviceVendor::Unsupported);
    assert_eq!(caps.number_of_fans, 0);
    assert!(!caps.fan_control);
}

#[test]
fn test_ft_f01_03_missing_driver_and_disappearing_device_produce_unsupported_state() {
    // Hardware is marked unavailable (e.g. /dev/tuxedo_io not present or failed to open)
    let mock = MockHardwareIo::new();
    mock.set_available(false);

    assert_eq!(detect_vendor(&mock), DeviceVendor::Unsupported);

    let dmi = DmiInfo {
        sys_vendor: "TUXEDO".to_string(),
        product_sku: "STELLARIS16I07".to_string(),
        ..Default::default()
    };
    let sysfs = MockSysfs::new();

    let caps = detect_device_capabilities(&mock, Some(&dmi), Some(&sysfs));
    assert_eq!(caps.vendor, DeviceVendor::Unsupported);
    assert_eq!(caps.number_of_fans, 0);
    assert!(!caps.fan_control);
    assert!(!caps.tdp_control);
    assert!(!caps.webcam_switch);
    assert!(!caps.flightmode_switch);
    assert!(!caps.touchpad_switch);
    assert!(!caps.water_cooler_supported);
    assert!(!caps.charging_thresholds);
    assert!(!caps.ctgp_supported);
    assert!(!caps.hwp_dynamic_boost_supported);

    // Crucial: No reads or writes were guessed or recorded
    assert!(mock.journal().is_empty());
}

#[test]
fn test_dmi_quirks_gating() {
    // 1. ACPI platform profile quirk on IBPG10AMD
    assert!(DmiQuirks::has_acpi_platform_profile_quirk(Some(UniwillDeviceID::IBPG10AMD)));
    assert!(!DmiQuirks::has_acpi_platform_profile_quirk(Some(UniwillDeviceID::STELLARIS16I07)));
    assert!(!DmiQuirks::has_acpi_platform_profile_quirk(None));

    // 2. Display MUX quirk on IBM15A10
    assert!(DmiQuirks::has_display_mux_quirk(Some(UniwillDeviceID::IBM15A10)));
    assert!(!DmiQuirks::has_display_mux_quirk(Some(UniwillDeviceID::PULSE1404)));
    assert!(!DmiQuirks::has_display_mux_quirk(None));

    // 3. EPP write quirk on AMD with known buggy DMI
    let bug_dmi = DmiInfo {
        board_name: "NO_EPP_SUPPORT_BOARD".to_string(),
        ..Default::default()
    };
    let ok_dmi = DmiInfo {
        board_name: "STANDARD_BOARD".to_string(),
        ..Default::default()
    };
    assert!(DmiQuirks::has_no_epp_write_quirk(&bug_dmi, "AMD Ryzen 7 7840HS"));
    assert!(!DmiQuirks::has_no_epp_write_quirk(&ok_dmi, "AMD Ryzen 7 7840HS"));
    assert!(!DmiQuirks::has_no_epp_write_quirk(&bug_dmi, "13th Gen Intel Core i7-13700H"));
}

#[test]
fn test_manufacturer_classification_and_mechrevo_naming_format() {
    assert_eq!(classify_manufacturer("TUXEDO Computers", ""), LaptopManufacturer::Tuxedo);
    assert_eq!(classify_manufacturer("", "Schenker Technologies"), LaptopManufacturer::Xmg);
    assert_eq!(classify_manufacturer("XMG", ""), LaptopManufacturer::Xmg);
    assert_eq!(classify_manufacturer("PCSpecialist", ""), LaptopManufacturer::PcSpecialist);
    assert_eq!(classify_manufacturer("Uniwill", ""), LaptopManufacturer::Uniwill);
    assert_eq!(classify_manufacturer("MECHREVO", ""), LaptopManufacturer::Mechrevo);
    assert_eq!(classify_manufacturer("OtherBrand", ""), LaptopManufacturer::Unknown);

    // Exact Mechrevo naming quirk: "MECHREVO " + board_name
    let mechrevo_dmi = DmiInfo {
        sys_vendor: "MECHREVO".to_string(),
        board_name: "YAOSHI Series-X6AR55xY".to_string(),
        product_name: "YAOSHI Series".to_string(),
        ..Default::default()
    };
    let model = build_laptop_model(
        Some(UniwillDeviceID::STELLARIS16I07),
        LaptopManufacturer::Mechrevo,
        &mechrevo_dmi,
    );
    assert_eq!(model, "MECHREVO YAOSHI Series-X6AR55xY");

    // Standard Tuxedo naming
    let tux_dmi = DmiInfo {
        sys_vendor: "TUXEDO".to_string(),
        board_name: "Pulse14".to_string(),
        ..Default::default()
    };
    let tux_model = build_laptop_model(
        Some(UniwillDeviceID::PULSE1404),
        LaptopManufacturer::Tuxedo,
        &tux_dmi,
    );
    assert_eq!(tux_model, "TUXEDO Pulse 14 Gen4 (2024)");
}

#[test]
fn test_water_cooler_and_ctgp_capabilities() {
    // Stellaris 16 Gen7 supports water cooler
    assert!(is_water_cooler_supported(Some(UniwillDeviceID::STELLARIS16I07)));
    assert!(is_water_cooler_supported(Some(UniwillDeviceID::XNE16A25)));
    assert!(is_water_cooler_supported(Some(UniwillDeviceID::PCS_X6FR559Y)));
    // Pulse 14 does not
    assert!(!is_water_cooler_supported(Some(UniwillDeviceID::PULSE1404)));
    assert!(!is_water_cooler_supported(None));

    // cTGP is hidden for IBP series
    assert!(!is_ctgp_supported(Some(UniwillDeviceID::IBPG8), None));
    assert!(!is_ctgp_supported(Some(UniwillDeviceID::IBPG10AMD), None));
    // cTGP is supported for Stellaris
    assert!(is_ctgp_supported(Some(UniwillDeviceID::STELLARIS16I07), None));

    // Dynamic boost detection via sysfs
    let sysfs = MockSysfs::new();
    assert!(!is_hwp_dynamic_boost_supported(&sysfs));
    sysfs.set_file(
        Path::new("/sys/devices/system/cpu/intel_pstate/hwp_dynamic_boost"),
        "1\n",
    );
    assert!(is_hwp_dynamic_boost_supported(&sysfs));
}
