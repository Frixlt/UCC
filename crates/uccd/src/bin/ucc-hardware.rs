// SPDX-License-Identifier: GPL-3.0-or-later

//! Privileged hardware probe / fail-safe fan restore helper (`ISSUE-04`, `ISSUE-05`).
//!
//! Replaces `uccd/src/hardware-helper.cpp` WITHOUT the personal laptop DMI gate so `ExecStopPost`
//! restores firmware automatic fan control on any supported `/dev/tuxedo_io` laptop.

use ucc_hardware::{PlatformBackend, TuxedoIoBackend, TuxedoIoDevice};

fn run() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 || (args[1] != "--audit" && args[1] != "--fan-auto") {
        eprintln!("Usage: ucc-hardware --audit | --fan-auto");
        return 2;
    }

    let Ok(dev) = TuxedoIoDevice::open(TuxedoIoDevice::DEFAULT_PATH) else {
        return 4;
    };
    let backend = TuxedoIoBackend::new(dev, None, None);
    if !backend.is_available() {
        return 4;
    }

    if args[1] == "--fan-auto" {
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
        println!("fan={fan} EC-duty={speed} temp={temp}");
    }
    if ok { 0 } else { 1 }
}

fn main() {
    std::process::exit(run());
}
