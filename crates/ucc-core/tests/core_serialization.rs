use ucc_core::LegacyProfileDto;

#[test]
fn legacy_profile_rejects_negative_core_ids() {
    let doc = r#"{"id":"p","name":"P","cpu":{"enabledCores":[0,-1]}}"#;
    let result = LegacyProfileDto::parse_json(doc);
    assert!(
        result.is_err(),
        "negative core ID accepted and discarded: {result:?}"
    );
}

#[test]
fn legacy_profile_never_shifts_tdp_slot_by_filtering_entries() {
    let doc = r#"{"id":"p","name":"P","odmPowerLimits":{"tdpValues":[45,0,80]}}"#;
    if let Ok(profile) = LegacyProfileDto::parse_json(doc) {
        assert_eq!(
            profile.odm_power_limits,
            [45, 0, 80],
            "PL4 silently moved into PL2"
        );
    }
}
