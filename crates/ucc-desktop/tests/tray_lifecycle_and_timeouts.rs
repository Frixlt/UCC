// SPDX-License-Identifier: GPL-3.0-or-later

//! Regression tests for V2-P2-16 in `ucc-desktop`:
//! - Event loop breaks when `rx.recv()` returns `None` (no headless hang when ksni exits).
//! - `activate_or_launch_gui` and D-Bus activation calls are bounded by `tokio::time::timeout`.
//! - `SelectProfile`, `ToggleFanAuto`, and `ToggleWaterCooler` distinguish `Ok(false)` vs `Err(err)`.

use std::fs;
use std::path::Path;

#[test]
fn v2_p2_16_tray_event_loop_handles_channel_close_timeouts_and_distinct_error_logs() {
    let main_src = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"))
        .expect("read src/main.rs");

    assert!(
        !main_src.contains("Some(action) = rx.recv()"),
        "V2-P2-16: tokio::select! must handle rx.recv() returning None instead of ignoring it"
    );
    assert!(
        main_src.contains("action = rx.recv() =>")
            && main_src.contains("let Some(action) = action else"),
        "V2-P2-16: rx.recv() arm must break when channel closes"
    );
    assert!(
        main_src.contains("tokio::time::timeout(\n                            rpc_timeout_duration(),\n                            activate_or_launch_gui(")
            || main_src.contains("tokio::time::timeout(")
                && main_src.contains("activate_or_launch_gui(token.as_deref(), preview)"),
        "V2-P2-16: OpenGui must wrap activate_or_launch_gui in tokio::time::timeout"
    );

    assert!(
        main_src.contains("Some(Ok(false)) => {\n                                    warn!(\"Daemon rejected profile switch to {id}\");")
            && main_src.contains("Some(Err(err)) => {\n                                    warn!(\"Failed to switch profile to {id}: {err}\");"),
        "V2-P2-16: SelectProfile must distinguish Ok(false) vs Err(err)"
    );
    assert!(
        main_src.contains("Some(Ok(false)) => {\n                                warn!(\"Daemon rejected fan Auto/Max toggle\");")
            && main_src.contains("Some(Err(err)) => {\n                                warn!(\"Failed to toggle fan Auto/Max: {err}\");"),
        "V2-P2-16: ToggleFanAuto must distinguish Ok(false) vs Err(err)"
    );
    assert!(
        main_src.contains("Some(Ok(false)) => {\n                                warn!(\"Daemon rejected EnableWaterCooler({target})\");")
            && main_src.contains("Some(Err(err)) => {\n                                warn!(\"Failed to toggle EnableWaterCooler({target}): {err}\");"),
        "V2-P2-16: ToggleWaterCooler must distinguish Ok(false) vs Err(err)"
    );

    let launcher_src =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/launcher.rs"))
            .expect("read src/launcher.rs");
    assert!(
        launcher_src.contains("tokio::time::timeout("),
        "V2-P2-16: launcher.rs must wrap D-Bus activation calls in tokio::time::timeout"
    );
}
