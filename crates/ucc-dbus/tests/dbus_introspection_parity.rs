use std::sync::Arc;
use ucc_dbus::{register_uccd_service, AllowAllAuthorizer, DefaultUccdBackend};
use zbus::{connection::Builder, fdo::IntrospectableProxy};

#[tokio::test]
async fn export_introspection() {
    let name = "com.uniwill.uccd.v4_audit";
    let conn = Builder::session()
        .unwrap()
        .name(name)
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
    for (path, filename) in [
        ("/com/uniwill/uccd", "rust-dbus-primary.xml"),
        ("/de/uniwill/uccd", "rust-dbus-alias.xml"),
    ] {
        let proxy = IntrospectableProxy::builder(&client)
            .destination(name)
            .unwrap()
            .path(path)
            .unwrap()
            .build()
            .await
            .unwrap();
        let xml = proxy.introspect().await.unwrap();
        std::fs::write(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join(filename),
            xml,
        )
        .unwrap();
    }
}
