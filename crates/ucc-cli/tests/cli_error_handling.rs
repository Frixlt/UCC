use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    process::{Command, Output},
    sync::{Arc, Mutex as StdMutex},
};
use tokio::sync::Mutex as TokioMutex;
use ucc_dbus::{
    register_uccd_service, AllowAllAuthorizer, Authorizer, DenyAllAuthorizer, UccdBackend,
    DBUS_SERVICE,
};
use zbus::{
    connection::{Builder, Connection},
    zvariant::{OwnedValue, Value as DbusValue},
};

static CLI_TEST_LOCK: TokioMutex<()> = TokioMutex::const_new(());

#[derive(Default)]
struct Backend {
    actions: StdMutex<Vec<String>>,
}

fn sample(value: i32) -> OwnedValue {
    let map = HashMap::from([
        ("timestamp".to_string(), OwnedValue::from(123_i64)),
        ("data".to_string(), OwnedValue::from(value)),
    ]);
    OwnedValue::try_from(DbusValue::new(map)).unwrap()
}

impl UccdBackend for Backend {
    fn get_fan_profile(&self, _: &str) -> String {
        r#"{"tableCPU":[{"temp":50,"speed":65}],"tableGPU":[{"temp":60,"speed":75}]}"#.into()
    }

    fn apply_fan_profiles(&self, input: &str) -> bool {
        let parsed: Value = serde_json::from_str(input).unwrap();
        let valid = parsed.get("cpu").is_some() && parsed.get("gpu").is_some();
        self.actions.lock().unwrap().push(format!("apply:{valid}"));
        valid
    }

    fn set_fan_profile(&self, _: &str, _: &str) -> bool {
        self.actions
            .lock()
            .unwrap()
            .push("unsupported:SetFanProfile".into());
        false
    }

    fn apply_profile(&self, _: &str) -> bool {
        self.actions.lock().unwrap().push("apply-profile".into());
        true
    }

    fn set_power_device_settings_json(&self, _: &str) -> bool {
        self.actions
            .lock()
            .unwrap()
            .push("apply-power-policy".into());
        true
    }

    fn get_fan_data_cpu(&self) -> HashMap<String, OwnedValue> {
        HashMap::from([("speed".into(), sample(65)), ("temp".into(), sample(72))])
    }
}

async fn serve(backend: Arc<Backend>, auth: Arc<dyn Authorizer>) -> Connection {
    let conn = Builder::session()
        .unwrap()
        .name(DBUS_SERVICE)
        .unwrap()
        .build()
        .await
        .unwrap();
    register_uccd_service(&conn, backend, auth).await.unwrap();
    conn
}

fn cpp(args: &[&str]) -> Option<Output> {
    let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").expect("Use dbus-run-session");
    let bin = std::env::var("UCC_AUDIT_CPP_CLI")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
            let root = manifest_dir
                .parent()
                .and_then(|p| p.parent())
                .unwrap_or(manifest_dir);
            let candidate = root.join("audit-cpp-build/bin/ucc-cli");
            if candidate.exists() {
                Some(candidate)
            } else {
                None
            }
        });

    let bin = match bin {
        Some(b) => b,
        None => {
            eprintln!("Notice: Skipping C++ baseline CLI parity step (binary not present)");
            return None;
        }
    };

    Some(
        Command::new(bin)
            .args(args)
            .env("DBUS_SYSTEM_BUS_ADDRESS", address)
            .output()
            .unwrap(),
    )
}

fn rust(args: &[&str]) -> Output {
    Command::new("timeout")
        .arg("8s")
        .arg(env!("CARGO_BIN_EXE_ucc-cli"))
        .args(args)
        .env("UCCD_DBUS_SERVICE", DBUS_SERVICE)
        .output()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fan_set_uses_baseline_apply_protocol() {
    let _lock = CLI_TEST_LOCK.lock().await;
    let backend = Arc::new(Backend::default());
    let _conn = serve(backend.clone(), Arc::new(AllowAllAuthorizer)).await;
    if let Some(cpp_output) = cpp(&["fan", "set", "fixture"]) {
        assert!(cpp_output.status.success(), "{cpp_output:?}");
        assert_eq!(backend.actions.lock().unwrap().as_slice(), ["apply:true"]);
        backend.actions.lock().unwrap().clear();
    }
    let output = rust(&["fan", "set", "fixture"]);
    let journal = backend.actions.lock().unwrap();
    assert!(
        output.status.success() && journal.as_slice() == ["apply:true"],
        "{output:?}, journal={journal:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_decodes_baseline_nested_fan_sample() {
    let _lock = CLI_TEST_LOCK.lock().await;
    let _conn = serve(Arc::new(Backend::default()), Arc::new(AllowAllAuthorizer)).await;
    let rust_output = rust(&["status", "--json"]);
    let rust_json: Value =
        serde_json::from_slice(&rust_output.stdout).unwrap_or_else(|_| panic!("{rust_output:?}"));
    assert_eq!(rust_json["cpu"]["fanPercent"], json!(65));
    assert_eq!(rust_json["cpu"]["temperature"], json!(72));

    if let Some(cpp_output) = cpp(&["status", "--json"]) {
        let cpp_json: Value =
            serde_json::from_slice(&cpp_output.stdout).unwrap_or_else(|_| panic!("{cpp_output:?}"));
        assert_eq!(
            rust_json["cpu"]["fanPercent"],
            cpp_json["cpu"]["fanPercent"]
        );
        assert_eq!(
            rust_json["cpu"]["temperature"],
            cpp_json["cpu"]["temperature"]
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsupported_cpu_mutations_do_not_report_success() {
    let _lock = CLI_TEST_LOCK.lock().await;
    let backend = Arc::new(Backend::default());
    let _conn = serve(backend.clone(), Arc::new(AllowAllAuthorizer)).await;
    let mut false_success = Vec::new();
    for args in [
        ["cpu", "epp", "power"].as_slice(),
        ["cpu", "frequency", "800", "2500"].as_slice(),
    ] {
        backend.actions.lock().unwrap().clear();
        let output = rust(args);
        if output.status.success() && backend.actions.lock().unwrap().is_empty() {
            false_success.push((
                args.to_vec(),
                String::from_utf8_lossy(&output.stdout).into_owned(),
            ));
        }
    }
    assert!(false_success.is_empty(), "{false_success:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn denied_fnlock_has_error_reply_and_cli_failure() {
    let _lock = CLI_TEST_LOCK.lock().await;
    let _conn = serve(Arc::new(Backend::default()), Arc::new(DenyAllAuthorizer)).await;
    let client = Builder::session().unwrap().build().await.unwrap();
    let proxy = ucc_dbus::UccdProxy::new(&client).await.unwrap();
    let response = proxy.set_fn_lock_status(true).await;
    let output = rust(&["fnlock", "set", "on"]);
    assert!(
        response.is_err() && !output.status.success(),
        "D-Bus={response:?}, CLI={output:?}"
    );
}

struct Unrelated;
#[zbus::interface(name = "com.uniwill.audit.Unrelated")]
impl Unrelated {
    fn ping(&self) {}
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_bus_name_without_api_is_not_connected_status() {
    let _lock = CLI_TEST_LOCK.lock().await;
    let _conn = Builder::session()
        .unwrap()
        .name(DBUS_SERVICE)
        .unwrap()
        .build()
        .await
        .unwrap();
    _conn
        .object_server()
        .at(ucc_dbus::DBUS_PATH, Unrelated)
        .await
        .unwrap();
    let output = rust(&["status", "--json"]);
    assert_ne!(
        output.status.code(),
        Some(124),
        "CLI exceeded the audit timeout"
    );
    let response: Option<Value> = serde_json::from_slice(&output.stdout).ok();
    assert!(
        !output.status.success()
            || response.as_ref().and_then(|j| j["connected"].as_bool()) != Some(true),
        "{output:?}, response={response:?}"
    );
}
