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

    // Resolve symlink targets so atomic writes update the underlying file instead of
    // replacing the symlink itself or inheriting 0o777 symlink metadata (`ISSUE-16`).
    let resolved_storage: PathBuf;
    let target_path = match fs::symlink_metadata(target_path.as_ref()) {
        Ok(meta) if meta.file_type().is_symlink() => {
            resolved_storage = match fs::canonicalize(target_path.as_ref()) {
                Ok(canonical) => canonical,
                Err(_) => {
                    let link_dest = fs::read_link(target_path.as_ref())?;
                    if link_dest.is_absolute() {
                        link_dest
                    } else {
                        target_path
                            .as_ref()
                            .parent()
                            .filter(|p| !p.as_os_str().is_empty())
                            .unwrap_or_else(|| Path::new("."))
                            .join(link_dest)
                    }
                }
            };
            resolved_storage.as_path()
        }
        _ => target_path.as_ref(),
    };

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

    #[cfg(unix)]
    let is_root = {
        extern "C" {
            fn geteuid() -> u32;
        }
        unsafe { geteuid() == 0 }
    };
    #[cfg(not(unix))]
    let is_root = false;

    if !is_root && target_path.exists() {
        let meta = fs::metadata(target_path)?;
        if meta.permissions().readonly() {
            return Err(ConfigError::PermissionDenied(format!(
                "Target file {} is read-only",
                target_path.display()
            )));
        }
    }

    #[cfg(unix)]
    let target_mode: u32 = {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(target_path)
            .map(|m| m.permissions().mode() & 0o666)
            .unwrap_or(0o644)
    };

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
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(target_mode);
            // 0o400000 is O_NOFOLLOW on Linux
            opts.custom_flags(0o400000);
        }
        let mut temp_file = opts.open(&temp_path)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&temp_path, fs::Permissions::from_mode(target_mode))?;
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

    // Helper for safe atomic backup rotation: writes to a temporary file and renames it over the destination,
    // which replaces existing files/symlinks without traversing symlinks.
    let rotate_backup = |src_content: &[u8], dest_path: &Path, mode: u32| -> Result<(), std::io::Error> {
        let parent = dest_path.parent().unwrap_or_else(|| Path::new("."));
        let bak_temp = parent.join(format!(
            ".{}.bak.tmp.{}",
            dest_path.file_name().unwrap_or_default().to_string_lossy(),
            std::process::id()
        ));
        if let Ok(meta) = fs::symlink_metadata(&bak_temp) {
            if meta.file_type().is_symlink() || meta.is_file() {
                let _ = fs::remove_file(&bak_temp);
            }
        }
        {
            use std::io::Write;
            let mut opts = fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(mode);
                // 0o400000 is O_NOFOLLOW on Linux
                opts.custom_flags(0o400000);
            }
            let mut f = opts.open(&bak_temp)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&bak_temp, fs::Permissions::from_mode(mode))?;
            }
            f.write_all(src_content)?;
            f.flush()?;
            f.sync_all()?;
        }
        fs::rename(&bak_temp, dest_path)?;
        Ok(())
    };

    // 5. Create backup of the existing target file (both canonical .bak and legacy .backup)
    if target_path.exists() {
        let target_bytes = fs::read(target_path).unwrap_or_default();
        let target_content = String::from_utf8_lossy(&target_bytes);
        let target_is_valid = crate::manager::is_valid_settings_content(&target_content);

        #[cfg(unix)]
        let mode = target_mode;
        #[cfg(not(unix))]
        let mode = 0o644;

        let bak_path = PathBuf::from(format!("{}.bak", target_path.display()));
        let backup_path = PathBuf::from(format!("{}.backup", target_path.display()));

        if target_is_valid {
            if let Err(e) = rotate_backup(&target_bytes, &bak_path, mode) {
                let _ = fs::remove_file(&temp_path);
                return Err(ConfigError::Io(e));
            }
            let _ = rotate_backup(&target_bytes, &backup_path, mode);
        } else {
            // Target is corrupted or domain-invalid! Do NOT overwrite valid existing backup files with a broken file!
            let has_valid_backup = [&bak_path, &backup_path]
                .iter()
                .any(|p| p.exists() && fs::read_to_string(p).map(|s| crate::manager::is_valid_settings_content(&s)).unwrap_or(false));
            if !has_valid_backup {
                let _ = rotate_backup(&target_bytes, &bak_path, mode);
            }
        }
    }

    // 6. Atomically rename temporary file to target path
    if let Err(e) = fs::rename(&temp_path, target_path) {
        let _ = fs::remove_file(&temp_path);
        return Err(ConfigError::Io(e));
    }

    // 7. Ensure directory durability
    #[cfg(unix)]
    {
        if let Ok(dir_f) = fs::File::open(parent) {
            let _ = dir_f.sync_all();
        }
    }

    Ok(())
}

/// Scans `dir` for any orphaned temporary files (`.*.tmp.*` or `.*.tmp_restore.*`)
/// left from interrupted writes, restores, or crashes, and removes them.
/// Returns the number of removed temporary files.
pub fn cleanup_temp_files(dir: impl AsRef<Path>) -> Result<usize, std::io::Error> {
    let dir = dir.as_ref();
    if !dir.is_dir() {
        return Ok(0);
    }
    let mut count = 0;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.')
            && (name_str.contains(".tmp.") || name_str.contains(".tmp_restore."))
            && fs::remove_file(entry.path()).is_ok()
        {
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn test_atomic_write_new_and_backup() {
        let _guard = TEST_MUTEX.lock().unwrap();
        std::env::remove_var(ENV_READ_ONLY_PREVIEW);

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
        let _guard = TEST_MUTEX.lock().unwrap();
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

