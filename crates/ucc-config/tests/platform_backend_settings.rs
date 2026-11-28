use ucc_config::{create_default_settings, UccSettings};

#[test]
fn platform_backend_omitted_when_none_and_defaults_to_none() {
    let settings = create_default_settings();
    assert_eq!(settings.platform_backend, None);
    let json = settings.to_json().expect("serialize default settings");
    assert!(
        !json.contains("\"platformBackend\""),
        "None platform_backend should be omitted from JSON"
    );
    let restored = UccSettings::from_json(&json).expect("deserialize default settings");
    assert_eq!(restored.platform_backend, None);
}

#[test]
fn platform_backend_round_trips_auto_uwcc_and_tuxedo_io() {
    for value in ["auto", "uwcc", "tuxedo_io"] {
        let mut settings = create_default_settings();
        settings.platform_backend = Some(value.to_string());
        let json = settings.to_json().expect("serialize settings");
        assert!(
            json.contains(&format!("\"platformBackend\":\"{value}\"")),
            "expected camelCase platformBackend={value} in JSON: {json}"
        );
        let restored = UccSettings::from_json(&json).expect("deserialize settings");
        assert_eq!(restored.platform_backend.as_deref(), Some(value));
        assert!(
            !restored.extra.contains_key("platformBackend"),
            "platformBackend must be parsed into typed field, not extra map"
        );
    }
}

#[test]
fn platform_backend_missing_in_legacy_json_deserializes_as_none() {
    let legacy_json = r#"{
        "stateMap": { "power_ac": "__office__" },
        "profiles": {}
    }"#;
    let settings = UccSettings::from_json(legacy_json).expect("deserialize legacy json");
    assert_eq!(settings.platform_backend, None);
}
