// SPDX-License-Identifier: GPL-3.0-or-later

//! Locates and launches `ucc-gui` with optional `XDG_ACTIVATION_TOKEN` handoff.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .is_ok_and(|m| m.is_file() && (m.permissions().mode() & 0o111) != 0)
}

pub fn find_gui_binary() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("UCC_GUI_BIN") {
        let p = PathBuf::from(explicit);
        if is_executable_file(&p) {
            return Some(p);
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("ucc-gui");
            if is_executable_file(&sibling) {
                return Some(sibling);
            }
        }
    }

    if let Ok(path_env) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_env) {
            let candidate = dir.join("ucc-gui");
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }

    for fallback in ["/usr/bin/ucc-gui", "/usr/local/bin/ucc-gui"] {
        let p = Path::new(fallback);
        if is_executable_file(p) {
            return Some(p.to_path_buf());
        }
    }

    None
}

pub const GUI_DBUS_SERVICE: &str = "com.uniwill.UccGui";
pub const GUI_DBUS_PREVIEW_SERVICE: &str = "com.uniwill.UccGuiPreview";
pub const GUI_DBUS_PATH: &str = "/com/uniwill/UccGui";
pub const GUI_DBUS_INTERFACE: &str = "com.uniwill.UccGui";

pub fn launch_gui(activation_token: Option<&str>) -> std::io::Result<()> {
    let bin = find_gui_binary().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "ucc-gui executable not found in UCC_GUI_BIN, sibling directory, or PATH",
        )
    })?;

    let mut cmd = Command::new(bin);
    cmd.arg("--show")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(token) = activation_token.filter(|t| !t.trim().is_empty()) {
        cmd.env("XDG_ACTIVATION_TOKEN", token);
    }
    cmd.spawn().map(|_| ())
}

pub async fn activate_gui_dbus_on(
    conn: &zbus::Connection,
    service_name: &str,
    activation_token: Option<&str>,
) -> zbus::Result<()> {
    let proxy = zbus::Proxy::new(conn, service_name, GUI_DBUS_PATH, GUI_DBUS_INTERFACE).await?;
    if let Some(token) = activation_token.filter(|t| !t.trim().is_empty()) {
        if proxy
            .call::<_, _, ()>("ActivateWithToken", &(token,))
            .await
            .is_ok()
        {
            return Ok(());
        }
    }
    proxy.call::<_, _, ()>("Activate", &()).await
}

pub async fn activate_gui_dbus(
    activation_token: Option<&str>,
    preview: bool,
) -> zbus::Result<()> {
    let conn = zbus::Connection::session().await?;
    let service = if preview {
        GUI_DBUS_PREVIEW_SERVICE
    } else {
        GUI_DBUS_SERVICE
    };
    activate_gui_dbus_on(&conn, service, activation_token).await
}

pub async fn activate_or_launch_gui(
    activation_token: Option<&str>,
    preview: bool,
) -> std::io::Result<()> {
    if activate_gui_dbus(activation_token, preview).await.is_ok() {
        return Ok(());
    }
    launch_gui(activation_token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_gui_binary_returns_none_or_executable() {
        if let Some(path) = find_gui_binary() {
            assert!(is_executable_file(&path));
        }
    }
}
