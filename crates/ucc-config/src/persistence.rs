//! Atomic file persistence with backup rotation and permission validation.
//!
//! Implements safe transactional writes via temporary files on the same filesystem
//! and atomic directory renames (`rename(2)`), preventing data corruption on power loss
//! or unexpected application crashes.

use crate::error::ConfigError;
use std::fs;
use std::path::{Path, PathBuf};

/// Environment variable indicating read-only preview mode.
pub const ENV_READ_ONLY_PREVIEW: &str = "UCC_READ_ONLY_PREVIEW";

/// Checks if read-only preview mode is active in the environment.
pub fn is_read_only_preview() -> bool {
    match std::env::var(ENV_READ_ONLY_PREVIEW) {
        Ok(val) => {
            let lower = val.trim().to_lowercase();
            !lower.is_empty() && lower != "0" && lower != "false" && lower != "no"
        }
        Err(_) => false,
    }
}

/// Atomically writes content to `target_path`.
///
/// Workflow:
/// 1. Validates `UCC_READ_ONLY_PREVIEW` - returns `ConfigError::ReadOnlyPreview` if active.
/// 2. Ensures parent directory exists and is writable.
/// 3. Validates target file write permissions if it already exists.
/// 4. Writes content to a hidden temporary file in the same parent directory.
/// 5. Flushes and calls `sync_all()` to ensure physical persistence on disk.
/// 6. If `target_path` already exists, creates a `.bak` backup copy.
/// 7. Atomically replaces `target_path` using `fs::rename`.
/// 8. Cleans up temporary file in case of any failure.
pub fn write_atomic(target_path: impl AsRef<Path>, content: &[u8]) -> Result<(), ConfigError> {
    // 1. Guard against writes during read-only preview mode
    if is_read_only_preview() {
        return Err(ConfigError::ReadOnlyPreview);
    }

    let target_path = target_path.as_ref();
    let parent = target_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    // 2. Ensure parent directory exists
    if !parent.exists() {
        fs::create_dir_all(parent).map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                ConfigError::PermissionDenied(format!(
                    "Cannot create configuration directory {}: {}",
                    parent.display(),
                    e
                ))
            } else {
                ConfigError::Io(e)
            }
        })?;
    }

    // 3. Permission verification: check if parent directory is writable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let parent_meta = fs::metadata(parent)?;
        // Check write permission bits (user, group, other)
        if parent_meta.permissions().mode() & 0o222 == 0 {
            return Err(ConfigError::PermissionDenied(format!(
                "Directory {} is read-only (mode: {:o})",
                parent.display(),
                parent_meta.permissions().mode()
            )));
        }
    }

    // Check target file write permissions if target already exists
    if target_path.exists() {
        let meta = fs::metadata(target_path)?;
        if meta.permissions().readonly() {
            return Err(ConfigError::PermissionDenied(format!(
                "Target file {} is read-only",
                target_path.display()
            )));
        }
    }

    // 4. Create temporary file in the same directory (guarantees same filesystem)
    let file_name = target_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("settings.json");
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let temp_name = format!(".{}.tmp.{}.{}", file_name, std::process::id(), timestamp);
    let temp_path = parent.join(temp_name);

    // Write content and synchronize to disk
    let write_res = (|| -> Result<(), std::io::Error> {
        use std::io::Write;
        let mut temp_file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&temp_path, fs::Permissions::from_mode(0o644));
        }

        temp_file.write_all(content)?;
        temp_file.flush()?;
        temp_file.sync_all()?;
        Ok(())
    })();

    if let Err(e) = write_res {
        let _ = fs::remove_file(&temp_path);
        return if e.kind() == std::io::ErrorKind::PermissionDenied {
            Err(ConfigError::PermissionDenied(format!(
                "Permission denied writing temp file {}: {}",
                temp_path.display(),
                e
            )))
        } else {
            Err(ConfigError::Io(e))
        };
    }

    // 5. Create backup (.bak) of the existing target file
    if target_path.exists() {
        let bak_path = PathBuf::from(format!("{}.bak", target_path.display()));
        if let Err(e) = fs::copy(target_path, &bak_path) {
            let _ = fs::remove_file(&temp_path);
            return Err(ConfigError::Io(e));
        }
    }

    // 6. Atomically rename temporary file to target path
    if let Err(e) = fs::rename(&temp_path, target_path) {
        let _ = fs::remove_file(&temp_path);
        return Err(ConfigError::Io(e));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_atomic_write_new_and_backup() {
        let temp_dir = std::env::temp_dir().join(format!("ucc_test_atomic_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("create temp dir");

        let target_file = temp_dir.join("test_settings.json");
        let bak_file = temp_dir.join("test_settings.json.bak");

        // First write - creates file, no backup yet
        write_atomic(&target_file, b"{\"version\": 1}").expect("first write");
        assert!(target_file.exists());
        assert!(!bak_file.exists());
        assert_eq!(fs::read_to_string(&target_file).unwrap(), "{\"version\": 1}");

        // Second write - updates file, creates backup of version 1
        write_atomic(&target_file, b"{\"version\": 2}").expect("second write");
        assert!(target_file.exists());
        assert!(bak_file.exists());
        assert_eq!(fs::read_to_string(&target_file).unwrap(), "{\"version\": 2}");
        assert_eq!(fs::read_to_string(&bak_file).unwrap(), "{\"version\": 1}");

        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_preview_mode_blocks_write_atomic() {
        // Set environment variable
        std::env::set_var(ENV_READ_ONLY_PREVIEW, "1");
        assert!(is_read_only_preview());

        let temp_dir = std::env::temp_dir().join(format!("ucc_test_prev_{}", std::process::id()));
        let target_file = temp_dir.join("should_not_exist.json");

        let res = write_atomic(&target_file, b"forbidden write");
        assert!(matches!(res, Err(ConfigError::ReadOnlyPreview)));
        assert!(!target_file.exists(), "Target file must not be created in preview mode");

        // Clean up env
        std::env::remove_var(ENV_READ_ONLY_PREVIEW);
        assert!(!is_read_only_preview());
    }
}

