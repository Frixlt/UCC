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

pub fn launch_gui(activation_token: Option<&str>) -> std::io::Result<()> {
    let bin = find_gui_binary().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "ucc-gui executable not found in UCC_GUI_BIN, sibling directory, or PATH",
        )
    })?;

    let mut cmd = Command::new(bin);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(token) = activation_token.filter(|t| !t.trim().is_empty()) {
        cmd.env("XDG_ACTIVATION_TOKEN", token);
    }
    cmd.spawn().map(|_| ())
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
