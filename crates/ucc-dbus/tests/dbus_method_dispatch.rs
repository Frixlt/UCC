#![cfg(unix)]
use std::{collections::HashMap, sync::Arc};
use ucc_dbus::{
    register_uccd_service, AllowAllAuthorizer, AuthAction, Authorizer, DefaultUccdBackend,
    PolkitAuthorizer, UccdProxy, DBUS_SERVICE,
};
use zbus::{connection::Builder, zvariant::OwnedValue};

struct Rules;
#[zbus::interface(name = "org.freedesktop.PolicyKit1.Authority")]
impl Rules {
    fn check_authorization(
        &self,
        _subject: (String, HashMap<String, OwnedValue>),
        action_id: &str,
        _details: HashMap<String, String>,
        _flags: u32,
        _cancellation_id: &str,
    ) -> (bool, bool, HashMap<String, String>) {
        (action_id.starts_with("de.uniwill."), false, HashMap::new())
    }
}

#[tokio::test]
async fn primary_polkit_denial_does_not_fall_back_to_legacy_allow() {
    assert_ne!(
        std::str::from_utf8(
            &std::process::Command::new("id")
                .arg("-u")
                .output()
                .unwrap()
                .stdout
        )
        .unwrap()
        .trim()
        .parse::<u32>()
        .unwrap(),
        0,
        "audit requires unprivileged caller"
    );
    let conn = Builder::session()
        .unwrap()
        .name("org.freedesktop.PolicyKit1")
        .unwrap()
        .build()
        .await
        .unwrap();
    conn.object_server()
        .at("/org/freedesktop/PolicyKit1/Authority", Rules)
        .await
        .unwrap();
    let client = Builder::session().unwrap().build().await.unwrap();
    let sender = client.unique_name().unwrap().as_str();
    let allowed = PolkitAuthorizer::new()
        .check_auth(&conn, Some(sender), AuthAction::Control)
        .await;
    assert!(
        !allowed,
        "the secondary action reversed an explicit primary denial"
    );
}

#[tokio::test]
async fn default_backend_does_not_report_a_successful_noop_write() {
    let conn = Builder::session()
        .unwrap()
        .name(DBUS_SERVICE)
        .unwrap()
        .build()
        .await
        .unwrap();
    register_uccd_service(
        &conn,
        Arc::new(DefaultUccdBackend),
        Arc::new(AllowAllAuthorizer),
    )
    .await
    .unwrap();
    let client = Builder::session().unwrap().build().await.unwrap();
    let proxy = UccdProxy::new(&client).await.unwrap();
    let result = proxy.set_display_brightness(37).await;
    if matches!(result, Ok(true)) {
        assert_eq!(
            proxy.get_display_brightness().await.unwrap(),
            37,
            "public default backend said it wrote brightness but readback is unchanged"
        );
    }
}
