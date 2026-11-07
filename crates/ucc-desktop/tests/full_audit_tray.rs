use ksni::{MenuItem, Tray};
use ucc_desktop::tray::UccTray;

fn fan_item(tray: &UccTray) -> ksni::menu::StandardItem<UccTray> {
    tray.menu()
        .into_iter()
        .find_map(|item| match item {
            MenuItem::Standard(item) if item.label.starts_with("Fan: ") => Some(item),
            _ => None,
        })
        .unwrap()
}

#[test]
fn closed_backend_channel_does_not_show_unapplied_fan_state() {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut tray = UccTray::new(vec!["Quiet".into()], 0, tx);
    drop(rx);
    let item = fan_item(&tray);
    assert!(item.label.starts_with("Fan: Auto"));
    (item.activate)(&mut tray);
    assert!(fan_item(&tray).label.starts_with("Fan: Auto"));
}
