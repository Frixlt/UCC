use std::path::Path;
use ucc_config::{ConfigManager, UccSettings};
use ucc_gui::app::{ControlCenterApp, Message};

#[test]
fn switching_to_empty_curve_never_copies_previous_profile_on_save() {
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../audit-gui-saved-settings");
    let (mut app, _) = ControlCenterApp::new();
    app.config_manager = ConfigManager::with_custom_path(&out);
    app.config_load_error = None;
    app.settings = UccSettings::from_json(r#"{"profiles":{"first":{"id":"first","name":"First","fan":{"tableCPU":[{"temp":44,"speed":37}]}},"second":{"id":"second","name":"Second"}}}"#).unwrap();
    app.update(Message::SelectProfile("first".into()));
    assert_eq!(app.fan_points.len(), 1);
    app.update(Message::SelectProfile("second".into()));
    app.update(Message::SaveSettings);
    let saved = ConfigManager::load_from_path(&out).unwrap();
    std::fs::remove_file(&out).unwrap();
    assert!(
        saved.profiles["second"].fan.table_cpu.is_empty(),
        "{:?}",
        saved.profiles["second"].fan.table_cpu
    );
}

#[test]
fn canvas_drag_preserves_temperature_order() {
    use iced::{
        mouse,
        widget::canvas::{Event, Program},
        Point, Rectangle,
    };
    use ucc_gui::widgets::fan_curve::{CurvePoint, FanCurveCanvas, FanCurveMessage};
    let widget = FanCurveCanvas::new(vec![
        CurvePoint {
            temp_c: 30.0,
            speed_pct: 20.0,
        },
        CurvePoint {
            temp_c: 70.0,
            speed_pct: 80.0,
        },
    ]);
    let bounds = Rectangle {
        x: 0.0,
        y: 0.0,
        width: 340.0,
        height: 260.0,
    };
    let mut state = Default::default();
    widget.update(
        &mut state,
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
        bounds,
        mouse::Cursor::Available(Point::new(30.0, 190.0)),
    );
    let position = Point::new(290.0, 160.0);
    let (_, message) = widget.update(
        &mut state,
        Event::Mouse(mouse::Event::CursorMoved { position }),
        bounds,
        mouse::Cursor::Available(position),
    );
    if let Some(FanCurveMessage::PointChanged(0, temp, _)) = message {
        assert!(
            temp < 70.0,
            "dragged first point to {temp}°C past its neighbor"
        );
    } else {
        panic!("fixture must exercise drag: {message:?}");
    }
}

#[test]
fn canvas_release_outside_cancels_drag() {
    use iced::{
        mouse,
        widget::canvas::{Event, Program},
        Point, Rectangle,
    };
    use ucc_gui::widgets::fan_curve::{CurvePoint, FanCurveCanvas};
    let widget = FanCurveCanvas::new(vec![CurvePoint {
        temp_c: 30.0,
        speed_pct: 20.0,
    }]);
    let bounds = Rectangle {
        x: 0.0,
        y: 0.0,
        width: 340.0,
        height: 260.0,
    };
    let mut state = Default::default();
    widget.update(
        &mut state,
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
        bounds,
        mouse::Cursor::Available(Point::new(30.0, 190.0)),
    );
    widget.update(
        &mut state,
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
        bounds,
        mouse::Cursor::Available(Point::new(360.0, 190.0)),
    );
    let position = Point::new(100.0, 160.0);
    let (_, message) = widget.update(
        &mut state,
        Event::Mouse(mouse::Event::CursorMoved { position }),
        bounds,
        mouse::Cursor::Available(position),
    );
    assert!(
        message.is_none(),
        "movement without pressed button edited a point: {message:?}"
    );
}
