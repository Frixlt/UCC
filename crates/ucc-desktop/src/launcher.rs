// SPDX-License-Identifier: GPL-3.0-or-later

//! Locates and launches `ucc-gui` with optional `XDG_ACTIVATION_TOKEN` handoff.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && (m.permissions().mode() & 0o111) != 0)
}

fn find_binary_named(env_var: &str, name: &str, fallbacks: &[&str]) -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var(env_var) {
        let p = PathBuf::from(explicit);
        if is_executable_file(&p) {
            return Some(p);
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join(name);
            if is_executable_file(&sibling) {
                return Some(sibling);
            }
        }
    }

    if let Ok(path_env) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_env) {
            let candidate = dir.join(name);
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }

    for fallback in fallbacks {
        let p = Path::new(fallback);
        if is_executable_file(p) {
            return Some(p.to_path_buf());
        }
    }

    None
}

pub fn find_gui_binary() -> Option<PathBuf> {
    find_binary_named(
        "UCC_GUI_BIN",
        "ucc-gui",
        &["/usr/bin/ucc-gui", "/usr/local/bin/ucc-gui"],
    )
}

pub fn find_preview_gui_binary() -> Option<PathBuf> {
    find_binary_named("UCC_GUI_PREVIEW_BIN", "ucc-gui-preview", &[])
}

pub const GUI_DBUS_SERVICE: &str = "com.uniwill.UccGui";
pub const GUI_DBUS_PREVIEW_SERVICE: &str = "com.uniwill.UccGuiPreview";
pub const GUI_DBUS_PATH: &str = "/com/uniwill/UccGui";
pub const GUI_DBUS_PREVIEW_PATH: &str = "/com/uniwill/UccGuiPreview";
pub const GUI_DBUS_INTERFACE: &str = "com.uniwill.UccGui";

fn spawn_gui_binary(bin: &Path, activation_token: Option<&str>) -> std::io::Result<()> {
    let mut cmd = Command::new(bin);
    cmd.arg("--show")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(token) = activation_token.filter(|t| !t.trim().is_empty()) {
        cmd.env("XDG_ACTIVATION_TOKEN", token);
    }
    let mut child = cmd.spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

pub fn launch_gui(activation_token: Option<&str>) -> std::io::Result<()> {
    let bin = find_gui_binary().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "ucc-gui executable not found in UCC_GUI_BIN, sibling directory, or PATH",
        )
    })?;
    spawn_gui_binary(&bin, activation_token)
}

pub async fn activate_gui_dbus_on(
    conn: &zbus::Connection,
    service_name: &str,
    activation_token: Option<&str>,
) -> zbus::Result<()> {
    let object_path = if service_name == GUI_DBUS_PREVIEW_SERVICE {
        GUI_DBUS_PREVIEW_PATH
    } else {
        GUI_DBUS_PATH
    };
    let proxy = zbus::Proxy::new(conn, service_name, object_path, GUI_DBUS_INTERFACE).await?;
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

pub async fn activate_gui_dbus(activation_token: Option<&str>, preview: bool) -> zbus::Result<()> {
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
    if preview {
        let bin = find_preview_gui_binary().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Preview ucc-gui instance not running and no preview binary available",
            )
        })?;
        return spawn_gui_binary(&bin, activation_token);
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

    #[tokio::test]
    async fn preview_activation_never_falls_back_to_production_gui() {
        let prev_bin = std::env::var_os("UCC_GUI_BIN");
        let prev_preview = std::env::var_os("UCC_GUI_PREVIEW_BIN");
        let prev_path = std::env::var_os("PATH");

        let dir = PathBuf::from(format!("/tmp/ucc-r09-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let prod_marker = dir.join("prod-launched");
        let prod_bin = dir.join("ucc-gui");
        std::fs::write(
            &prod_bin,
            format!("#!/bin/sh\ntouch {}\n", prod_marker.display()),
        )
        .unwrap();
        std::fs::set_permissions(&prod_bin, std::fs::Permissions::from_mode(0o755)).unwrap();

        std::env::set_var("UCC_GUI_BIN", &prod_bin);
        std::env::remove_var("UCC_GUI_PREVIEW_BIN");
        std::env::set_var("PATH", &dir);

        let res = activate_or_launch_gui(None, true).await;
        assert!(res.is_err());
        assert!(!prod_marker.exists());

        match prev_bin {
            Some(v) => std::env::set_var("UCC_GUI_BIN", v),
            None => std::env::remove_var("UCC_GUI_BIN"),
        }
        match prev_preview {
            Some(v) => std::env::set_var("UCC_GUI_PREVIEW_BIN", v),
            None => std::env::remove_var("UCC_GUI_PREVIEW_BIN"),
        }
        match prev_path {
            Some(v) => std::env::set_var("PATH", v),
            None => std::env::remove_var("PATH"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
