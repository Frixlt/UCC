// SPDX-License-Identifier: GPL-3.0-or-later

//! Reproduction integration tests for confirmed `ucc-desktop` findings on commit `1d07126`:
//! - R09: Preview tray fallback must not launch production `ucc-gui` and must reap spawned children.
//! - M-DESK-04: `pidfd.rs` must re-check `pidfd_exited` after the final sleep before reporting failure.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use ucc_desktop::activate_or_launch_gui;

struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(prefix: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{prefix}_{}_{nanos}", std::process::id()));
        fs::create_dir_all(&path).expect("failed to create temp dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct EnvVarGuard {
    key: &'static str,
    prev: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let prev = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, prev }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => std::env::set_var(self.key, v),
            None => std::env::remove_var(self.key),
        }
    }
}

/// R09: When preview tray activation falls back from D-Bus (`preview = true`), it must NOT launch
/// a production `ucc-gui` without preview mode, and `launch_gui` must reap spawned child processes
/// instead of discarding `Child` with `cmd.spawn().map(|_| ())`.
#[tokio::test]
async fn r09_preview_tray_fallback_must_not_launch_production_gui_and_reaps_children() {
    let tmp = TempDirGuard::new("ucc_repro_r09");
    let marker = tmp.path().join("production_launched.marker");
    let fake_gui = tmp.path().join("ucc-gui");

    let script = format!(
        "#!/bin/sh\n\
         is_preview=0\n\
         if [ \"${{UCC_READ_ONLY_PREVIEW:-0}}\" = \"1\" ] || [ \"${{UCC_READ_ONLY_PREVIEW:-}}\" = \"true\" ]; then\n\
             is_preview=1\n\
         fi\n\
         for arg in \"$@\"; do\n\
             case \"$arg\" in\n\
                 *preview*) is_preview=1 ;;\n\
             esac\n\
         done\n\
         if [ \"$is_preview\" -eq 0 ]; then\n\
             echo \"$*\" > \"{}\"\n\
         fi\n\
         exit 0\n",
        marker.display()
    );
    fs::write(&fake_gui, script).expect("write fake ucc-gui");
    fs::set_permissions(&fake_gui, fs::Permissions::from_mode(0o755)).expect("chmod fake ucc-gui");

    let _gui_env = EnvVarGuard::set("UCC_GUI_BIN", fake_gui.to_str().unwrap());
    let _bus_env = EnvVarGuard::set(
        "DBUS_SESSION_BUS_ADDRESS",
        "unix:path=/nonexistent/ucc_r09_no_preview_owner",
    );

    let _ = activate_or_launch_gui(None, true).await;
    tokio::time::sleep(Duration::from_millis(150)).await;

    assert!(
        !marker.exists(),
        "R09: activate_or_launch_gui(None, true) launched production ucc-gui without preview mode"
    );

    // Source verification: launcher.rs must reap spawned children (not discard Child via `cmd.spawn().map(|_| ())`)
    // and must not drop the `preview` flag in the fallback path.
    let launcher_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/launcher.rs");
    let source = fs::read_to_string(&launcher_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", launcher_path.display()));
    assert!(
        !source.contains("cmd.spawn().map(|_| ())"),
        "R09: launcher.rs must reap spawned GUI child processes instead of dropping Child via `cmd.spawn().map(|_| ())`"
    );
    assert!(
        !source.contains("launch_gui(activation_token)\n}"),
        "R09: activate_or_launch_gui must propagate `preview` mode into the fallback launcher"
    );
}

/// M-DESK-04: After the final `tokio::time::sleep` in the `SIGKILL` wait loop, `pidfd.rs` must
/// re-check `Self::pidfd_exited(&t.fd)` before reporting `"PID {} has not exited"`.
#[test]
fn m_desk_04_pidfd_checks_exited_after_final_sleep() {
    let pidfd_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/pidfd.rs");
    let source = fs::read_to_string(&pidfd_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", pidfd_path.display()));

    let kill_loop_start = source
        .find("let kill_deadline")
        .expect("pidfd.rs must contain `let kill_deadline`");
    let tail = &source[kill_loop_start..];
    let sleep_pos = tail
        .find("tokio::time::sleep")
        .expect("kill_deadline loop must sleep between polls");
    let after_sleep = &tail[sleep_pos..];
    let not_exited_pos = after_sleep
        .find("has not exited")
        .expect("pidfd.rs must report targets that have not exited");
    let final_check_section = &after_sleep[..not_exited_pos];

    assert!(
        final_check_section.contains("pidfd_exited"),
        "M-DESK-04: pidfd.rs must re-check `pidfd_exited` after the final kill-loop sleep before reporting `has not exited`"
    );
}
