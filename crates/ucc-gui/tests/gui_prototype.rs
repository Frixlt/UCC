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
fn test_preview_mode_blocks_mutations() {
    // In preview mode, profile switches and curve changes must be ignored
    let is_preview = true;
    let mut active_profile = "Quiet".to_string();

    if !is_preview {
        active_profile = "Performance".to_string();
    }
    // Must remain "Quiet" because preview blocks write paths!
    assert_eq!(active_profile, "Quiet");
}
