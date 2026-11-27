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

#[tokio::test]
async fn ta_14_tray_sigterm_exits_promptly_during_hung_dbus_rpc() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    use std::time::Duration;

    let mut dbus = std::process::Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--nopidfile", "--print-address=1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("dbus-daemon required");
    let mut reader = BufReader::new(dbus.stdout.take().unwrap());
    let mut addr = String::new();
    reader.read_line(&mut addr).unwrap();
    let addr = addr.trim().to_string();

    struct HungUccd;
    #[zbus::interface(name = "com.uniwill.uccd")]
    impl HungUccd {
        #[zbus(name = "GetDefaultProfilesJSON")]
        async fn get_default_profiles_json(&self) -> String {
            tokio::time::sleep(Duration::from_secs(30)).await;
            "[]".to_string()
        }
    }

    let svc = "org.test.uccd.tray.hung";
    let _server = zbus::connection::Builder::address(addr.as_str())
        .unwrap()
        .name(svc)
        .unwrap()
        .serve_at("/com/uniwill/uccd", HungUccd)
        .unwrap()
        .build()
        .await
        .unwrap();

    let mut tray_proc = tokio::process::Command::new(env!("CARGO_BIN_EXE_ucc-desktop-tray"))
        .env("DBUS_SESSION_BUS_ADDRESS", &addr)
        .env("UCCD_DBUS_SERVICE", svc)
        .env("UCC_DBUS_TIMEOUT_MS", "5000")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    tokio::time::sleep(Duration::from_millis(120)).await;
    let pid = nix::unistd::Pid::from_raw(tray_proc.id().unwrap() as i32);
    nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM).unwrap();

    let status = tokio::time::timeout(Duration::from_secs(1), tray_proc.wait())
        .await
        .expect("TA-14: tray must exit within 1s on SIGTERM despite hung D-Bus call")
        .unwrap();
    let _ = dbus.kill();
    let _ = dbus.wait();
    assert_eq!(status.code(), Some(0));
}
