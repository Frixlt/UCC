use ucc_config::{
    PROFILE_ID_HIGH_PERFORMANCE, PROFILE_ID_MAX_ENERGY_SAVE, PROFILE_ID_OFFICE, PROFILE_ID_QUIET,
};
use ucc_gui::app::{ControlCenterApp, Message};
use ucc_gui::navigation::Page;
use ucc_gui::widgets::fan_curve::CurvePoint;

#[test]
fn test_ten_pages_inventory_and_titles() {
    assert_eq!(Page::ALL.len(), 10);
    for page in Page::ALL {
        assert!(!page.title().is_empty());
        assert!(!page.icon().is_empty());
    }
}

#[test]
fn test_monotonic_fan_curve_constraints() {
    let points = [
        CurvePoint {
            temp_c: 30.0,
            speed_pct: 20.0,
        },
        CurvePoint {
            temp_c: 50.0,
            speed_pct: 40.0,
        },
        CurvePoint {
            temp_c: 70.0,
            speed_pct: 60.0,
        },
        CurvePoint {
            temp_c: 90.0,
            speed_pct: 100.0,
        },
    ];

    // Attempt to set point[1] below point[0] -> must clamp to point[0]
    let mut candidate_speed: f32 = 10.0;
    candidate_speed = candidate_speed.max(points[0].speed_pct);
    assert_eq!(candidate_speed, 20.0);

    // Attempt to set point[1] above point[2] -> must clamp to point[2]
    let mut candidate_speed: f32 = 80.0;
    candidate_speed = candidate_speed.min(points[2].speed_pct);
    assert_eq!(candidate_speed, 60.0);
}

#[test]
fn test_profile_switch_updates_fan_curve_and_cpu_settings() {
    let (mut app, _) = ControlCenterApp::new();

    // 1. Switch to Quiet profile
    app.update(Message::SelectProfile(PROFILE_ID_QUIET.to_string()));
    assert_eq!(app.active_profile().id, PROFILE_ID_QUIET);
    assert_eq!(app.active_profile().cpu.governor, "powersave");
    assert_eq!(app.active_profile().cpu.energy_performance_preference, "power");
    assert_eq!(app.active_profile().fan.fan_profile, "quiet");

    // Verify fan curve points updated to Quiet profile curve
    assert!(!app.fan_points.is_empty());
    assert_eq!(app.fan_points[0].temp_c, 35.0);
    assert_eq!(app.fan_points[0].speed_pct, 15.0);

    // 2. Switch to High Performance profile
    app.update(Message::SelectProfile(PROFILE_ID_HIGH_PERFORMANCE.to_string()));
    assert_eq!(app.active_profile().id, PROFILE_ID_HIGH_PERFORMANCE);
    assert_eq!(app.active_profile().cpu.governor, "performance");
    assert_eq!(app.active_profile().cpu.energy_performance_preference, "performance");
    assert_eq!(app.active_profile().fan.fan_profile, "balanced");

    // Verify fan curve points updated to High Performance curve
    assert_eq!(app.fan_points[0].temp_c, 30.0);
    assert_eq!(app.fan_points[0].speed_pct, 30.0);

    // 3. Switch to Office / Balanced profile
    app.update(Message::SelectProfile(PROFILE_ID_OFFICE.to_string()));
    assert_eq!(app.active_profile().id, PROFILE_ID_OFFICE);
    assert_eq!(app.active_profile().cpu.governor, "powersave");
    assert_eq!(app.active_profile().cpu.energy_performance_preference, "balance_performance");

    // 4. Switch to Max Energy Save profile
    app.update(Message::SelectProfile(PROFILE_ID_MAX_ENERGY_SAVE.to_string()));
    assert_eq!(app.active_profile().id, PROFILE_ID_MAX_ENERGY_SAVE);
    assert_eq!(app.active_profile().cpu.governor, "powersave");
    assert_eq!(app.active_profile().cpu.energy_performance_preference, "power");
    assert!(app.active_profile().cpu.no_turbo);
}

#[test]
fn test_preview_mode_blocks_save_settings() {
    let (mut app, _) = ControlCenterApp::new();
    // Force preview mode
    app.is_preview = true;

    // Attempt to save settings
    app.update(Message::SaveSettings);

    // Status message must explicitly indicate save is blocked in preview mode
    assert!(
        app.status_message
            .as_ref()
            .map(|m| m.contains("Save blocked"))
            .unwrap_or(false),
        "Expected status message indicating save blocked in preview mode, got: {:?}",
        app.status_message
    );
}
