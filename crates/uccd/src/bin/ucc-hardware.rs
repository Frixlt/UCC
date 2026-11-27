// SPDX-License-Identifier: GPL-3.0-or-later

//! Privileged hardware probe / fail-safe fan restore helper (`ISSUE-04`, `ISSUE-05`, `M14`).
//!
//! Uses the unified [`uccd::select_platform_backend`] selection (`UCC_PLATFORM_BACKEND` ->
//! `settings.platformBackend` -> `"auto"` `/dev/uwcc` -> `/dev/tuxedo_io`) so `ExecStopPost`
//! restores firmware automatic fan control (`RamFan 1.5` auto on `uwcc` or EC auto on
//! `tuxedo_io`) on any supported laptop.

use std::sync::Arc;
use ucc_config::ConfigManager;
use ucc_hardware::{DmiInfo, RealSysfs, Sysfs};
use uccd::{select_platform_backend, ENV_PLATFORM_BACKEND};

fn run() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 || (args[1] != "--audit" && args[1] != "--fan-auto") {
        eprintln!("Usage: ucc-hardware --audit | --fan-auto");
        return 2;
    }

    let sysfs: Arc<dyn Sysfs> = Arc::new(RealSysfs);
    let dmi = DmiInfo::read_from_sysfs(sysfs.as_ref());
    let settings_choice = ConfigManager::new()
        .resolve_read_path()
        .and_then(|path| ConfigManager::load_with_recovery(&path).ok())
        .and_then(|(settings, _)| settings.platform_backend);
    let env_override = std::env::var(ENV_PLATFORM_BACKEND).ok();
    let backend = select_platform_backend(
        env_override.as_deref(),
        settings_choice.as_deref(),
        &dmi,
        &sysfs,
    );
    if !backend.is_available() {
        return 4;
    }

    if args[1] == "--fan-auto" {
        let _ = backend.set_pwup(false, 0);
        let ok = backend.set_fans_auto().is_ok();
        println!(
            "Firmware fan control restore: {}",
            if ok { "OK" } else { "FAILED" }
        );
        return if ok { 0 } else { 1 };
    }

    let fan_count = backend.capabilities().device.number_of_fans.max(2);
    let mut ok = true;
    for fan in 0..fan_count {
        let speed = match backend.read_fan_speed_percent(fan) {
            Ok(v) => i32::from(v),
            Err(_) => {
                ok = false;
                -1
            }
        };
        let temp = match backend.read_fan_temp(fan) {
            Ok(v) => v,
            Err(_) => {
                ok = false;
                -1
            }
        };
        match backend.read_fan_rpm(fan) {
            Ok(rpm) => println!("fan={fan} EC-duty={speed} temp={temp} rpm={rpm}"),
            Err(_) => println!("fan={fan} EC-duty={speed} temp={temp}"),
        }
    }
    if ok {
        0
    } else {
        1
    }
}

fn main() {
    std::process::exit(run());
}
