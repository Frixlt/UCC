use tokio::sync::mpsc;
use ucc_desktop::tray::{TrayAction, UccTray};

#[test]
fn test_tray_menu_structure_and_actions() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let profiles = vec![
        "Office".to_string(),
        "Gaming".to_string(),
        "Turbo".to_string(),
    ];
    let mut tray = UccTray::new(profiles, 1, tx);

    // Test title and ID
    assert_eq!(ksni::Tray::id(&tray), "frixit-control-center");
    assert_eq!(ksni::Tray::title(&tray), "Frixit Control Center");
    assert_eq!(
        ksni::Tray::icon_name(&tray),
        "preferences-system-performance"
    );

    // Test activate sends OpenGui action
    ksni::Tray::activate(&mut tray, 0, 0);
    assert_eq!(rx.try_recv().unwrap(), TrayAction::OpenGui);

    // Verify menu items
    let menu = ksni::Tray::menu(&tray);
    assert!(menu.len() >= 5); // OpenGui, Separator, RadioGroup, Separator, Fan, WaterCooler, Separator, Quit

    // Change profile
    tray.set_active_profile(2);
    // Change water cooler status
    tray.set_water_cooler_connected(true);
}
