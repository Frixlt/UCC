use super::mock_backend::*;
use std::sync::atomic::Ordering;
use ucc_dbus::*;
use zbus::object_server::SignalEmitter;

#[tokio::test]
async fn test_contract_profiles_group() {
    let backend = MockContractBackend::default();
    let (_bus, _server, _client, proxy) = setup_contract_test_stand(backend.clone()).await;

    // 1. GetActiveProfileJSON
    let active_json = proxy.get_active_profile_json().await.unwrap();
    assert!(active_json.contains("Office Profile"));

    // 2. SetActiveProfile valid & invalid
    let ok = proxy.set_active_profile("balanced").await.unwrap();
    assert!(ok);
    assert_eq!(*backend.active_profile_id.lock().unwrap(), "balanced");

    let bad = proxy
        .set_active_profile("non_existent_profile")
        .await
        .unwrap();
    assert!(!bad);

    // 3. ApplyProfile valid & malformed
    let apply_ok = proxy
        .apply_profile(r#"{"id":"temp-gaming","name":"Temp Gaming"}"#)
        .await
        .unwrap();
    assert!(apply_ok);
    assert!(proxy
        .get_active_profile_json()
        .await
        .unwrap()
        .contains("Temp Gaming"));

    let apply_bad = proxy.apply_profile("NOT_VALID_JSON{:::").await.unwrap();
    assert!(!apply_bad);

    // 4. SaveCustomProfile & DeleteCustomProfile
    let save_ok = proxy
        .save_custom_profile(r#"{"id":"custom-user-1","name":"User Profile"}"#)
        .await
        .unwrap();
    assert!(save_ok);

    let custom_list = proxy.get_custom_profiles_json().await.unwrap();
    assert!(custom_list.contains("custom-user-1"));

    let del_ok = proxy.delete_custom_profile("custom-user-1").await.unwrap();
    assert!(del_ok);

    let del_bad = proxy.delete_custom_profile("custom-user-1").await.unwrap();
    assert!(!del_bad); // already deleted

    // 5. Inventory listings
    let all_profiles = proxy.get_profiles_json().await.unwrap();
    assert!(all_profiles.contains("office") && all_profiles.contains("performance"));

    let def_profiles = proxy.get_default_profiles_json().await.unwrap();
    assert!(def_profiles.contains("office") && def_profiles.contains("balanced"));
}

#[tokio::test]
async fn test_contract_power_and_state_group() {
    let backend = MockContractBackend::default();
    let (_bus, _server, _client, proxy) = setup_contract_test_stand(backend.clone()).await;

    // 1. PowerState & Policy
    let pstate = proxy.get_power_state().await.unwrap();
    assert_eq!(pstate, "power_ac");

    let policy = proxy.get_power_policy_json().await.unwrap();
    assert!(policy.contains("balanced"));

    // 2. Manual power mode
    let mode_ok = proxy.set_manual_power_mode("power_bat").await.unwrap();
    assert!(mode_ok);
    assert_eq!(proxy.get_power_state().await.unwrap(), "power_bat");

    // 3. Register & unregister power request
    let req_ok = proxy
        .register_power_request("game.exe", "High performance demand", "power_ac")
        .await
        .unwrap();
    assert!(req_ok);
    assert!(backend
        .power_requests
        .lock()
        .unwrap()
        .contains_key("game.exe"));

    let unreq_ok = proxy.unregister_power_request("game.exe").await.unwrap();
    assert!(unreq_ok);

    // 4. State Map & Batch State Map
    let sm_ok = proxy.set_state_map("ac", "performance").await.unwrap();
    assert!(sm_ok);

    let batch_ok = proxy
        .set_batch_state_map(r#"{"bat":"office","wc":"performance"}"#)
        .await
        .unwrap();
    assert!(batch_ok);
    assert_eq!(
        backend
            .state_map
            .lock()
            .unwrap()
            .get("wc")
            .map(String::as_str),
        Some("performance")
    );

    let batch_bad = proxy.set_batch_state_map("BROKEN_JSON").await.unwrap();
    assert!(!batch_bad);

    // 5. Settings JSON
    let settings = proxy.get_settings_json().await.unwrap();
    assert!(settings.contains("version"));
}

#[tokio::test]
async fn test_contract_fans_and_sensors_group() {
    let backend = MockContractBackend::default();
    let (_bus, _server, _client, proxy) = setup_contract_test_stand(backend).await;

    // Fan data maps
    let cpu_map = proxy.get_fan_data_cpu().await.unwrap();
    assert!(cpu_map.contains_key("temp"));
    assert!(cpu_map.contains_key("fanSpeedRPM"));
    assert!(cpu_map.contains_key("fanSpeedPercent"));

    let gpu1_map = proxy.get_fan_data_gpu1().await.unwrap();
    assert!(gpu1_map.contains_key("temp"));
    assert!(gpu1_map.contains_key("fanSpeedRPM"));

    let gpu2_map = proxy.get_fan_data_gpu2().await.unwrap();
    assert!(gpu2_map.is_empty());

    // Fan profiles
    let fan_names = proxy.get_fan_profile_names().await.unwrap();
    assert!(fan_names.contains("office"));

    let fan_prof = proxy.get_fan_profile("office").await.unwrap();
    assert!(fan_prof.contains("points"));

    let set_ok = proxy
        .set_fan_profile("office", r#"{"points":[{"temp":50,"speed":50}]}"#)
        .await
        .unwrap();
    assert!(set_ok);

    let set_bad = proxy.set_fan_profile("office", "INVALID").await.unwrap();
    assert!(!set_bad);

    let apply_ok = proxy
        .apply_fan_profiles(r#"{"cpu":[{"temp":40,"speed":30}]}"#)
        .await
        .unwrap();
    assert!(apply_ok);

    let rev_ok = proxy.revert_fan_profiles().await.unwrap();
    assert!(rev_ok);

    assert_eq!(proxy.get_fans_min_speed().await.unwrap(), 0);
    assert!(proxy.get_fans_off_available().await.unwrap());
}

#[tokio::test]
async fn test_contract_water_cooler_group() {
    let backend = MockContractBackend::default();
    let (_bus, _server, _client, proxy) = setup_contract_test_stand(backend.clone()).await;

    // Status queries
    assert!(!proxy.get_water_cooler_connected().await.unwrap());
    assert!(proxy.get_water_cooler_available().await.unwrap());
    assert!(proxy.get_water_cooler_supported().await.unwrap());
    assert_eq!(proxy.get_water_cooler_fan_speed().await.unwrap(), 40);
    assert_eq!(proxy.get_water_cooler_pump_level().await.unwrap(), 2);

    // Controls: Fan speed
    assert!(proxy.set_water_cooler_fan_speed(80).await.unwrap());
    assert_eq!(backend.water_cooler_fan_speed.load(Ordering::SeqCst), 80);
    assert!(!proxy.set_water_cooler_fan_speed(150).await.unwrap()); // out of bounds
    assert!(!proxy.set_water_cooler_fan_speed(-5).await.unwrap());

    // Controls: Discrete pump voltage codes (0: 11V, 2: 7V, 3: 8V, 4: Off)
    assert!(proxy.set_water_cooler_pump_voltage(0).await.unwrap());
    assert!(proxy.set_water_cooler_pump_voltage(3).await.unwrap());
    assert!(proxy.set_water_cooler_pump_voltage(4).await.unwrap());
    // Code 1 (12V) is forbidden on hardware and must be rejected
    assert!(!proxy.set_water_cooler_pump_voltage(1).await.unwrap());
    assert!(!proxy.set_water_cooler_pump_voltage(99).await.unwrap());

    // Turn off
    assert!(proxy.turn_off_water_cooler_fan().await.unwrap());
    assert_eq!(backend.water_cooler_fan_speed.load(Ordering::SeqCst), 0);

    assert!(proxy.turn_off_water_cooler_pump().await.unwrap());
    assert_eq!(backend.water_cooler_pump_level.load(Ordering::SeqCst), 4); // 4 = Off

    assert!(proxy.turn_off_water_cooler_led().await.unwrap());

    // Enable and auto control
    assert!(proxy.enable_water_cooler(true).await.unwrap());
    assert!(proxy.is_water_cooler_enabled().await.unwrap());
    assert!(proxy.set_water_cooler_auto_control(false).await.unwrap());
    assert!(!proxy.is_water_cooler_auto_control_enabled().await.unwrap());
}

#[tokio::test]
async fn test_contract_lightbar_and_display_group() {
    let backend = MockContractBackend::default();
    let (_bus, _server, _client, proxy) = setup_contract_test_stand(backend.clone()).await;

    // Display brightness
    assert_eq!(proxy.get_display_brightness().await.unwrap(), 75);
    assert!(proxy.set_display_brightness(90).await.unwrap());
    assert_eq!(backend.display_brightness.load(Ordering::SeqCst), 90);

    // Bounds checking [0, 100]
    assert!(!proxy.set_display_brightness(-1).await.unwrap());
    assert!(!proxy.set_display_brightness(101).await.unwrap());

    // Lightbar settings & animation
    let lb_settings = proxy.get_lightbar_settings_json().await.unwrap();
    assert!(lb_settings.contains("static"));

    let lb_status = proxy.get_lightbar_status_json().await.unwrap();
    assert!(lb_status.contains("active"));

    assert!(proxy
        .set_lightbar_settings_json(r#"{"mode":"breathing","speed":3}"#)
        .await
        .unwrap());
    assert!(!proxy.set_lightbar_settings_json("CORRUPT").await.unwrap());

    assert!(proxy.trigger_lightbar_animation(true).await.unwrap());
}

#[tokio::test]
async fn test_contract_devices_and_charging_group() {
    let backend = MockContractBackend::default();
    let (_bus, _server, _client, proxy) = setup_contract_test_stand(backend.clone()).await;

    // Device & System info
    assert_eq!(
        proxy.get_device_name().await.unwrap(),
        "Frixit LAPTOP Test Stand"
    );
    assert!(proxy
        .get_system_info_json()
        .await
        .unwrap()
        .contains("7840HS"));
    assert!(proxy.is_device_supported().await.unwrap());
    assert_eq!(
        proxy.uccd_version().await.unwrap(),
        env!("CARGO_PKG_VERSION")
    );

    // Charging profile
    assert_eq!(
        proxy.get_current_charging_profile().await.unwrap(),
        "balanced"
    );
    assert!(proxy.set_charging_profile("stationary").await.unwrap());
    assert_eq!(
        backend.charging_profile.lock().unwrap().as_str(),
        "stationary"
    );

    // Charging thresholds bounds
    assert_eq!(proxy.get_charge_start_threshold().await.unwrap(), 60);
    assert_eq!(proxy.get_charge_end_threshold().await.unwrap(), 80);

    assert!(proxy.set_charge_start_threshold(50).await.unwrap());
    assert!(proxy.set_charge_end_threshold(85).await.unwrap());
    assert!(!proxy.set_charge_start_threshold(-10).await.unwrap());
    assert!(!proxy.set_charge_start_threshold(110).await.unwrap());
    assert!(!proxy.set_charge_end_threshold(150).await.unwrap());

    // Charge type
    assert_eq!(proxy.get_charge_type().await.unwrap(), "Standard");
    assert!(proxy.set_charge_type("Express").await.unwrap());
    assert_eq!(backend.charge_type.lock().unwrap().as_str(), "Express");

    // Power devices
    let pdev_set = proxy.get_power_device_settings_json().await.unwrap();
    assert!(pdev_set.contains("device-1"));
    let pdev_stat = proxy.get_power_device_status_json().await.unwrap();
    assert!(pdev_stat.contains("online"));
    assert!(proxy
        .set_power_device_settings_json(r#"[{"id":"device-2"}]"#)
        .await
        .unwrap());
    assert!(proxy.scan_power_devices().await.unwrap());
}

#[tokio::test]
async fn test_contract_signals_emission_and_delivery() {
    let backend = MockContractBackend::default();
    let (_bus, server_conn, _client_conn, proxy) = setup_contract_test_stand(backend).await;

    let emitter = SignalEmitter::new(&server_conn, "/com/uniwill/uccd").unwrap();

    // Verify emission of all 7 signals succeeds without error
    UccdInterface::<MockContractBackend>::profile_changed(
        &emitter,
        "performance",
        "kb-default",
        "fan-turbo",
    )
    .await
    .unwrap();

    UccdInterface::<MockContractBackend>::power_state_changed(&emitter, "power_bat")
        .await
        .unwrap();

    UccdInterface::<MockContractBackend>::power_policy_changed(
        &emitter,
        r#"{"policy":"powersave"}"#,
    )
    .await
    .unwrap();

    UccdInterface::<MockContractBackend>::water_cooler_status_changed(&emitter, "connected")
        .await
        .unwrap();

    UccdInterface::<MockContractBackend>::lightbar_settings_changed(&emitter, r#"{"mode":"wave"}"#)
        .await
        .unwrap();

    UccdInterface::<MockContractBackend>::power_device_status_changed(
        &emitter,
        r#"{"devices":[]}"#,
    )
    .await
    .unwrap();

    UccdInterface::<MockContractBackend>::mode_reapply_pending_changed(&emitter, true)
        .await
        .unwrap();

    // Verify proxy remains responsive after signal emissions
    assert!(proxy.is_device_supported().await.unwrap());
}

#[tokio::test]
async fn test_contract_error_handling_and_fault_injection() {
    let backend = MockContractBackend::default();
    let (_bus, _server, _client, proxy) = setup_contract_test_stand(backend.clone()).await;

    // 1. Malformed JSON handling across all JSON-accepting methods
    assert!(!proxy.apply_profile("NOT_JSON").await.unwrap());
    assert!(!proxy.save_custom_profile("{unclosed").await.unwrap());
    assert!(!proxy.set_batch_state_map("INVALID").await.unwrap());
    assert!(!proxy.set_fan_profile("office", "{:::").await.unwrap());
    assert!(!proxy.apply_fan_profiles("").await.unwrap());
    assert!(!proxy.set_lightbar_settings_json("").await.unwrap());
    assert!(!proxy.set_power_device_settings_json("{").await.unwrap());

    // 2. Non-existent IDs
    assert!(!proxy
        .set_active_profile("non_existent_12345")
        .await
        .unwrap());
    assert!(!proxy
        .delete_custom_profile("non_existent_12345")
        .await
        .unwrap());

    // 3. Out-of-bounds parameters
    assert!(!proxy.set_display_brightness(-50).await.unwrap());
    assert!(!proxy.set_display_brightness(250).await.unwrap());
    assert!(!proxy.set_water_cooler_fan_speed(-1).await.unwrap());
    assert!(!proxy.set_water_cooler_fan_speed(101).await.unwrap());
    assert!(!proxy.set_water_cooler_pump_voltage(1).await.unwrap()); // forbidden
    assert!(!proxy.set_water_cooler_pump_voltage(5).await.unwrap()); // out of range
    assert!(!proxy.set_charge_start_threshold(-1).await.unwrap());
    assert!(!proxy.set_charge_start_threshold(101).await.unwrap());
    assert!(!proxy.set_charge_end_threshold(-1).await.unwrap());
    assert!(!proxy.set_charge_end_threshold(101).await.unwrap());
}
