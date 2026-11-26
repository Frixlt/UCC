use ksni::menu::MenuItem;
use ksni::Tray;
use tokio::sync::mpsc;
use ucc_desktop::tray::{TrayAction, UccTray};

#[test]
fn queued_fan_command_is_not_displayed_as_an_applied_state() {
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let mut tray = UccTray::new(vec!["Office".to_string()], 0, sender);
    let menu = tray.menu();
    let item = menu
        .iter()
        .find_map(|item| match item {
            MenuItem::Standard(item) if item.label.starts_with("Fan:") => Some(item),
            _ => None,
        })
        .unwrap();
    (item.activate)(&mut tray);
    assert_eq!(receiver.try_recv().unwrap(), TrayAction::ToggleFanAuto);
    // Receiver has not issued the command and can still reject it.
    assert!(tray.menu().iter().any(
        |item| matches!(item, MenuItem::Standard(item) if item.label.starts_with("Fan: Auto"))
    ));
}
