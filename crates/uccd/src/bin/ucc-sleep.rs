// SPDX-License-Identifier: GPL-3.0-or-later

//! Root sleep/resume helper saving and restoring volatile runtime state across suspend (`ISSUE-04`, `M14`).

use nix::libc;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};
use ucc_config::{is_read_only_preview, ConfigManager};
use ucc_dbus::{DBUS_INTERFACE, DBUS_PATH};
use ucc_hardware::{DmiInfo, RealSysfs, Sysfs};
use uccd::{
    prepare_sleep, resume_sleep, select_platform_backend, SleepOperations, SleepState,
    SleepStateStore, WakeupPolicy, ENV_PLATFORM_BACKEND,
};

const DBUS_SERVICE_NAME: &str = "com.uniwill.uccd";

fn systemctl_bin() -> String {
    if let Ok(custom) = std::env::var("UCC_SYSTEMCTL") {
        if !custom.trim().is_empty() {
            return custom;
        }
    }
    if Path::new("/usr/bin/systemctl").exists() {
        "/usr/bin/systemctl".to_string()
    } else {
        "systemctl".to_string()
    }
}

fn run_systemctl(verb: &str) -> Result<String, String> {
    let mut cmd = Command::new(systemctl_bin());
    cmd.arg(verb).arg("uccd.service");
    if verb == "show" {
        cmd.arg("--property=ActiveState").arg("--value");
    }
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|_| "Cannot start systemctl".to_string())?;

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                let output = child
                    .wait_with_output()
                    .map_err(|_| "Failed collecting systemctl output".to_string())?;
                if output.status.success() {
                    return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
                }
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!("systemctl {verb} failed: {}", stderr.trim()));
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("systemctl timed out".to_string());
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => return Err(format!("systemctl wait failed: {e}")),
        }
    }
}

/// Ensures firmware automatic fan control (`RamFan 1.5` auto on `uwcc` or EC auto on `tuxedo_io`)
/// and disables `PWUP` safely across suspend/resume transitions.
fn restore_platform_safe_sleep_state(settings_choice: Option<&str>) {
    let sysfs: Arc<dyn Sysfs> = Arc::new(RealSysfs);
    let dmi = DmiInfo::read_from_sysfs(sysfs.as_ref());
    let env_override = std::env::var(ENV_PLATFORM_BACKEND).ok();
    let backend = select_platform_backend(env_override.as_deref(), settings_choice, &dmi, &sysfs);
    if !backend.is_available() {
        return;
    }
    let _ = backend.set_pwup(false, 0);
    let _ = backend.set_fans_auto();
}

struct SystemdSleepOps;

impl SleepOperations for SystemdSleepOps {
    fn is_daemon_active(&self) -> Result<bool, String> {
        let state = run_systemctl("show")?;
        match state.as_str() {
            "active" | "activating" | "reloading" => Ok(true),
            "inactive" | "failed" => Ok(false),
            other => Err(format!("Daemon state is unstable: {other}")),
        }
    }

    fn capture_runtime_state(&self) -> Result<SleepState, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("ucc-sleep-dbus".to_string())
            .spawn(move || {
                let res = (|| -> Result<SleepState, String> {
                    let conn = zbus::blocking::Connection::system()
                        .map_err(|e| format!("Cannot connect to system D-Bus: {e}"))?;
                    let proxy = zbus::blocking::proxy::Builder::<zbus::blocking::Proxy>::new(&conn)
                        .destination(DBUS_SERVICE_NAME)
                        .map_err(|e| format!("Invalid D-Bus destination: {e}"))?
                        .path(DBUS_PATH)
                        .map_err(|e| format!("Invalid D-Bus path: {e}"))?
                        .interface(DBUS_INTERFACE)
                        .map_err(|e| format!("Invalid D-Bus interface: {e}"))?
                        .cache_properties(zbus::proxy::CacheProperties::No)
                        .build()
                        .map_err(|e| format!("Cannot build uccd D-Bus proxy: {e}"))?;
                    let policy_json: String = proxy
                        .call("GetPowerPolicyJSON", &())
                        .map_err(|e| format!("Cannot capture GetPowerPolicyJSON: {e}"))?;
                    let cooler_enabled: bool = proxy
                        .call("IsWaterCoolerEnabled", &())
                        .map_err(|e| format!("Cannot capture IsWaterCoolerEnabled: {e}"))?;
                    let parsed: serde_json::Value = serde_json::from_str(&policy_json)
                        .map_err(|e| format!("Invalid GetPowerPolicyJSON: {e}"))?;
                    let manual_mode = parsed
                        .get("manual_mode")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if !SleepState::valid_mode(&manual_mode) {
                        return Err("Invalid daemon runtime state".to_string());
                    }
                    Ok(SleepState {
                        was_active: true,
                        manual_mode,
                        cooler_enabled,
                    })
                })();
                let _ = tx.send(res);
            })
            .map_err(|e| format!("Cannot spawn D-Bus state capture thread: {e}"))?;
        rx.recv_timeout(Duration::from_secs(5))
            .map_err(|_| "Timed out capturing daemon runtime state over D-Bus".to_string())?
    }

    fn stop_daemon(&self) -> Result<(), String> {
        run_systemctl("stop").map(|_| ())
    }

    fn start_daemon(&self) -> Result<(), String> {
        run_systemctl("start").map(|_| ())
    }
}

fn run() -> Result<(), String> {
    if is_read_only_preview() {
        return Err("Sleep integration is disabled in read-only previews".to_string());
    }
    let args: Vec<String> = std::env::args().collect();
    if unsafe { libc::geteuid() } != 0
        || args.len() != 2
        || (args[1] != "prepare" && args[1] != "resume")
    {
        return Err("Usage (root only): ucc-sleep prepare|resume".to_string());
    }

    let store = SleepStateStore::default();
    let ops = SystemdSleepOps;

    let mut wake_only = true;
    let mut platform_choice: Option<String> = None;
    if let Some(read_path) = ConfigManager::new().resolve_read_path() {
        if let Ok((settings, _)) = ConfigManager::load_with_recovery(&read_path) {
            platform_choice = settings.platform_backend;
            if let Some(pd) = settings.power_devices {
                if let Some(w) = pd
                    .get("lid")
                    .and_then(|l| l.get("wakeOnlyLidAndKeyboard"))
                    .and_then(|v| v.as_bool())
                {
                    wake_only = w;
                }
            }
        }
    }

    if args[1] == "prepare" {
        let _ = WakeupPolicy::default().apply(wake_only, &RealSysfs);
        let res = prepare_sleep(&store, &ops);
        restore_platform_safe_sleep_state(platform_choice.as_deref());
        res
    } else {
        let was_active = store.load().ok().flatten().is_some_and(|s| s.was_active);
        let res = resume_sleep(&store, &ops);
        if !was_active {
            restore_platform_safe_sleep_state(platform_choice.as_deref());
        }
        res
    }
}

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
