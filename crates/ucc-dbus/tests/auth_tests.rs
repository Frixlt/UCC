use std::sync::Arc;
use ucc_dbus::*;

#[test]
fn test_auth_action_ids() {
    assert_eq!(AuthAction::Read.action_id(), ACTION_READ);
    assert_eq!(AuthAction::Read.legacy_action_id(), LEGACY_ACTION_READ);
    assert_eq!(ACTION_READ, "com.uniwill.uccd.read");
    assert_eq!(LEGACY_ACTION_READ, "de.uniwill.uccd.read");

    assert_eq!(AuthAction::Control.action_id(), ACTION_CONTROL);
    assert_eq!(
        AuthAction::Control.legacy_action_id(),
        LEGACY_ACTION_CONTROL
    );
    assert_eq!(ACTION_CONTROL, "com.uniwill.uccd.control");
    assert_eq!(LEGACY_ACTION_CONTROL, "de.uniwill.uccd.control");

    assert_eq!(
        AuthAction::ManageHardware.action_id(),
        ACTION_MANAGE_HARDWARE
    );
    assert_eq!(
        AuthAction::ManageHardware.legacy_action_id(),
        LEGACY_ACTION_MANAGE_HARDWARE
    );
    assert_eq!(ACTION_MANAGE_HARDWARE, "com.uniwill.uccd.manage-hardware");
    assert_eq!(
        LEGACY_ACTION_MANAGE_HARDWARE,
        "de.uniwill.uccd.manage-hardware"
    );
}

#[tokio::test]
async fn test_authorizer_allow_and_deny() {
    let conn = zbus::connection::Builder::session()
        .unwrap()
        .build()
        .await
        .unwrap();

    let allow = AllowAllAuthorizer;
    assert!(
        allow
            .check_auth(&conn, Some(":1.100"), AuthAction::Read)
            .await
    );
    assert!(
        allow
            .check_auth(&conn, Some(":1.100"), AuthAction::Control)
            .await
    );
    assert!(
        allow
            .check_auth(&conn, Some(":1.100"), AuthAction::ManageHardware)
            .await
    );

    let deny = DenyAllAuthorizer;
    assert!(
        !deny
            .check_auth(&conn, Some(":1.100"), AuthAction::Read)
            .await
    );
    assert!(
        !deny
            .check_auth(&conn, Some(":1.100"), AuthAction::Control)
            .await
    );
    assert!(
        !deny
            .check_auth(&conn, Some(":1.100"), AuthAction::ManageHardware)
            .await
    );
}

#[tokio::test]
async fn test_tracking_authorizer_records_action() {
    let conn = zbus::connection::Builder::session()
        .unwrap()
        .build()
        .await
        .unwrap();

    let tracker = TrackingAuthorizer::new(true);
    assert!(
        tracker
            .check_auth(&conn, Some(":1.42"), AuthAction::Control)
            .await
    );
    assert!(
        tracker
            .check_auth(&conn, Some(":1.42"), AuthAction::ManageHardware)
            .await
    );
    assert!(
        tracker
            .check_auth(&conn, Some(":1.42"), AuthAction::Read)
            .await
    );

    let recorded = tracker.recorded_actions();
    assert_eq!(recorded.len(), 3);
    assert_eq!(recorded[0], (Some(":1.42".to_string()), AuthAction::Control));
    assert_eq!(
        recorded[1],
        (Some(":1.42".to_string()), AuthAction::ManageHardware)
    );
    assert_eq!(recorded[2], (Some(":1.42".to_string()), AuthAction::Read));
}

#[tokio::test]
async fn test_async_concurrent_authorization_non_blocking() {
    let conn = zbus::connection::Builder::session()
        .unwrap()
        .build()
        .await
        .unwrap();

    let authorizer = Arc::new(AllowAllAuthorizer);

    // Spawn 50 concurrent async tasks checking authorization to ensure non-blocking M3.4 behavior
    let mut handles = Vec::new();
    for i in 0..50 {
        let conn_clone = conn.clone();
        let auth_clone = authorizer.clone();
        let handle = tokio::spawn(async move {
            let sender = format!(":1.{i}");
            auth_clone
                .check_auth(&conn_clone, Some(&sender), AuthAction::Control)
                .await
        });
        handles.push(handle);
    }

    for handle in handles {
        let res = handle.await.expect("Task panicked");
        assert!(res);
    }
}
