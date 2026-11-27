// SPDX-License-Identifier: GPL-3.0-or-later

//! Volatile suspend/resume state persistence in `/run/ucc/sleep-state.json` (`ISSUE-04`).
//!
//! Ported from `uccd/inc/SleepState.hpp`. The directory (`0700`) and file (`0600`) must be owned
//! by `owner_uid` (`0` in production); `uccd` never trusts a user-owned snapshot.

use nix::libc;
use nix::sys::stat::fstat;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

pub const DEFAULT_SLEEP_STATE_PATH: &str = "/run/ucc/sleep-state.json";
const MAX_SLEEP_SNAPSHOT_BYTES: u64 = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SleepState {
    pub was_active: bool,
    pub manual_mode: String,
    pub cooler_enabled: bool,
}

impl Default for SleepState {
    fn default() -> Self {
        Self {
            was_active: false,
            manual_mode: "auto".to_string(),
            cooler_enabled: false,
        }
    }
}

impl SleepState {
    pub fn valid_mode(mode: &str) -> bool {
        matches!(mode, "auto" | "ac" | "type_c" | "bat")
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SleepStatePayload {
    version: u32,
    was_active: bool,
    manual_mode: String,
    cooler_enabled: bool,
}

#[derive(Debug, Clone)]
pub struct SleepStateStore {
    path: PathBuf,
    owner_uid: u32,
}

impl Default for SleepStateStore {
    fn default() -> Self {
        Self::new(DEFAULT_SLEEP_STATE_PATH, 0)
    }
}

impl SleepStateStore {
    pub fn new(path: impl Into<PathBuf>, owner_uid: u32) -> Self {
        Self {
            path: path.into(),
            owner_uid,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<Option<SleepState>, String> {
        if let Err(err) = fs::symlink_metadata(&self.path) {
            if err.kind() == std::io::ErrorKind::NotFound {
                return Ok(None);
            }
            return Err("Cannot inspect sleep snapshot".to_string());
        }
        self.verify_directory(false)?;

        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&self.path)
            .map_err(|_| "Cannot open sleep snapshot".to_string())?;

        let st = fstat(file.as_raw_fd()).map_err(|_| "Cannot stat sleep snapshot".to_string())?;
        let is_reg = (st.st_mode & libc::S_IFMT) == libc::S_IFREG;
        let bad_perms = (st.st_mode & 0o077) != 0;
        let size = u64::try_from(st.st_size).unwrap_or(u64::MAX);
        if !is_reg || st.st_uid != self.owner_uid || bad_perms || size > MAX_SLEEP_SNAPSHOT_BYTES {
            return Err("Unsafe sleep snapshot ownership, permissions or size".to_string());
        }

        let mut buf = Vec::new();
        file.take(MAX_SLEEP_SNAPSHOT_BYTES + 1)
            .read_to_end(&mut buf)
            .map_err(|_| "Cannot read sleep snapshot".to_string())?;
        if buf.len() as u64 > MAX_SLEEP_SNAPSHOT_BYTES {
            return Err("Sleep snapshot exceeds maximum size".to_string());
        }

        let payload: SleepStatePayload =
            serde_json::from_slice(&buf).map_err(|_| "Invalid sleep snapshot".to_string())?;
        if payload.version != 1 || !SleepState::valid_mode(&payload.manual_mode) {
            return Err("Invalid sleep snapshot".to_string());
        }

        Ok(Some(SleepState {
            was_active: payload.was_active,
            manual_mode: payload.manual_mode,
            cooler_enabled: payload.cooler_enabled,
        }))
    }

    pub fn save(&self, state: &SleepState) -> Result<(), String> {
        if !SleepState::valid_mode(&state.manual_mode) {
            return Err("Invalid manual power mode".to_string());
        }
        self.verify_directory(true)?;
        let _ = self.load()?;

        let parent = self
            .path
            .parent()
            .ok_or_else(|| "Invalid sleep snapshot path".to_string())?;
        let tmp_path = parent.join(format!(
            ".sleep-state.tmp.{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));

        let payload = SleepStatePayload {
            version: 1,
            was_active: state.was_active,
            manual_mode: state.manual_mode.clone(),
            cooler_enabled: state.cooler_enabled,
        };
        let bytes =
            serde_json::to_vec(&payload).map_err(|_| "Cannot encode sleep snapshot".to_string())?;

        let mut tmp: File = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&tmp_path)
            .map_err(|_| "Cannot create private sleep snapshot".to_string())?;

        if let Err(err) = tmp
            .write_all(&bytes)
            .and_then(|_| tmp.sync_all())
        {
            let _ = fs::remove_file(&tmp_path);
            return Err(format!("Cannot save sleep snapshot: {err}"));
        }
        drop(tmp);

        if let Err(err) = fs::rename(&tmp_path, &self.path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(format!("Cannot commit sleep snapshot: {err}"));
        }
        Ok(())
    }

    pub fn clear(&self) -> Result<(), String> {
        if self.load()?.is_none() {
            return Ok(());
        }
        fs::remove_file(&self.path).map_err(|_| "Cannot remove sleep snapshot".to_string())
    }

    fn verify_directory(&self, create: bool) -> Result<(), String> {
        let dir = self
            .path
            .parent()
            .ok_or_else(|| "Invalid sleep snapshot path".to_string())?;
        if create {
            if let Err(err) = fs::create_dir(dir) {
                if err.kind() != std::io::ErrorKind::AlreadyExists {
                    return Err("Cannot create sleep snapshot directory".to_string());
                }
            } else {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
                }
            }
        }
        let meta = fs::symlink_metadata(dir)
            .map_err(|_| "Unsafe sleep snapshot directory".to_string())?;
        let bad_perms = (meta.mode() & 0o022) != 0;
        if !meta.file_type().is_dir() || meta.uid() != self.owner_uid || bad_perms {
            return Err("Unsafe sleep snapshot directory".to_string());
        }
        Ok(())
    }
}

pub trait SleepOperations {
    fn is_daemon_active(&self) -> Result<bool, String>;
    fn capture_runtime_state(&self) -> Result<SleepState, String>;
    fn stop_daemon(&self) -> Result<(), String>;
    fn start_daemon(&self) -> Result<(), String>;
}

pub fn prepare_sleep(store: &SleepStateStore, ops: &dyn SleepOperations) -> Result<(), String> {
    let previous = store.load()?;
    let active = ops.is_daemon_active()?;
    if !active {
        return if previous.is_some() {
            Ok(())
        } else {
            store.save(&SleepState::default())
        };
    }
    let mut state = ops.capture_runtime_state()?;
    state.was_active = true;
    store.save(&state)?;
    ops.stop_daemon()
}

pub fn resume_sleep(store: &SleepStateStore, ops: &dyn SleepOperations) -> Result<(), String> {
    let Some(state) = store.load()? else {
        return Ok(());
    };
    if state.was_active {
        // With systemd Type=dbus, `start_daemon()` finishes as soon as `BusName` is owned —
        // potentially before `uccd` reads & activates the snapshot. Leave deletion to `uccd`
        // once it has consumed the snapshot and completed hardware activation (`FULL-27-01`).
        ops.start_daemon()
    } else {
        store.clear()
    }
}
