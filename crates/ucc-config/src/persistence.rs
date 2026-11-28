//! Atomic file persistence with backup rotation and permission validation.
//!
//! Implements safe transactional writes via temporary files on the same filesystem
//! and atomic directory renames (`rename(2)`), preventing data corruption on power loss
//! or unexpected application crashes.

use crate::error::ConfigError;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Environment variable indicating read-only preview mode.
pub const ENV_READ_ONLY_PREVIEW: &str = "UCC_READ_ONLY_PREVIEW";

/// Monotonic per-process sequence counter ensuring unique temporary file names
/// across concurrent threads and rapid sequential writes.
static TEMP_FILE_SEQ: AtomicU64 = AtomicU64::new(0);

fn next_temp_id() -> (u32, u64, u128) {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    (
        std::process::id(),
        TEMP_FILE_SEQ.fetch_add(1, Ordering::Relaxed),
        nanos,
    )
}

/// RAII guard that removes a temporary file on drop unless disarmed after a successful rename.
struct TempFileGuard {
    path: Option<PathBuf>,
}

impl TempFileGuard {
    fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    fn path(&self) -> &Path {
        self.path
            .as_deref()
            .expect("TempFileGuard accessed after disarm")
    }

    fn disarm(&mut self) {
        self.path = None;
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if let Some(ref p) = self.path {
            let _ = fs::remove_file(p);
        }
    }
}

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

/// Checks whether a directory is writable by the effective UID/GID on Unix via `faccessat(..., AT_EACCESS)`.
#[cfg(unix)]
fn check_dir_writable(dir: &Path) -> Result<(), ConfigError> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    if let Ok(c_path) = CString::new(dir.as_os_str().as_bytes()) {
        let rc = unsafe {
            libc::faccessat(
                libc::AT_FDCWD,
                c_path.as_ptr(),
                libc::W_OK | libc::X_OK,
                libc::AT_EACCESS,
            )
        };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::PermissionDenied {
                return Err(ConfigError::PermissionDenied(format!(
                    "Directory {} is not writable: {}",
                    dir.display(),
                    err
                )));
            }
            return Err(ConfigError::Io(err));
        }
    }
    Ok(())
}

/// Atomically writes content to `target_path`.
///
/// Workflow:
/// 1. Validates `UCC_READ_ONLY_PREVIEW` - returns `ConfigError::ReadOnlyPreview` if active.
/// 2. Ensures parent directory exists and is writable.
/// 3. Validates target file write permissions if it already exists.
/// 4. Writes content to a uniquely-named hidden temporary file in the same parent directory,
///    applies FD-based permissions (`fchmod`), flushes, and calls `sync_all()`.
/// 5. If `target_path` already exists, atomically rotates `.bak` and `.backup` copies using
///    unique temporary names without extra redundant `fsync` barriers.
/// 6. Atomically replaces `target_path` using `fs::rename`.
/// 7. Syncs the parent directory once for directory-entry durability.
/// 8. Automatically unlinks any temporary files via RAII guards on any error path.
pub fn write_atomic(target_path: impl AsRef<Path>, content: &[u8]) -> Result<(), ConfigError> {
    // 1. Guard against writes during read-only preview mode
    if is_read_only_preview() {
        return Err(ConfigError::ReadOnlyPreview);
    }

    // Resolve symlink targets only when they resolve to an existing file inside the
    // same configuration directory; reject broken or escaping symlinks (`P0-08`).
    let resolved_storage: PathBuf;
    let raw_target = target_path.as_ref();
    let target_path = match fs::symlink_metadata(raw_target) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let canonical = fs::canonicalize(raw_target).map_err(|_| {
                ConfigError::PermissionDenied(format!(
                    "Refusing to write through broken symlink {}",
                    raw_target.display()
                ))
            })?;
            let raw_parent = raw_target
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            let canon_parent = fs::canonicalize(raw_parent).map_err(|_| {
                ConfigError::PermissionDenied(format!(
                    "Refusing to write through symlink with unresolvable parent {}",
                    raw_target.display()
                ))
            })?;
            if !canonical.starts_with(&canon_parent) {
                return Err(ConfigError::PermissionDenied(format!(
                    "Refusing to write through symlink {} escaping directory {}",
                    raw_target.display(),
                    canon_parent.display()
                )));
            }
            resolved_storage = canonical;
            resolved_storage.as_path()
        }
        _ => raw_target,
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

    // 3. Permission verification: check effective UID/GID writability on parent directory
    #[cfg(unix)]
    check_dir_writable(parent)?;

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

    // 4. Create uniquely-named temporary file in the same directory (guarantees same filesystem)
    let file_name = target_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("settings.json");
    let (pid, seq, nanos) = next_temp_id();
    let temp_path = parent.join(format!(".{file_name}.tmp.{pid}.{seq}.{nanos}"));
    let mut temp_guard: Option<TempFileGuard> = None;

    // Write content and synchronize temp file to disk
    let write_res = (|| -> Result<(), std::io::Error> {
        use std::io::Write;
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(target_mode);
            // Refuse to follow symlinks via target-specific O_NOFOLLOW
            opts.custom_flags(libc::O_NOFOLLOW);
        }
        let mut temp_file = opts.open(&temp_path)?;
        temp_guard = Some(TempFileGuard::new(temp_path.clone()));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            temp_file.set_permissions(fs::Permissions::from_mode(target_mode))?;
        }

        temp_file.write_all(content)?;
        temp_file.flush()?;
        temp_file.sync_all()?;
        Ok(())
    })();

    if let Err(e) = write_res {
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
    let mut temp_guard = temp_guard.expect("temp file created above");

    // Helper for safe atomic backup rotation: writes to a uniquely-named temporary file
    // with FD-based permissions and RAII cleanup, then renames it over the destination
    // (replacing existing files/symlinks without traversing symlinks).
    // Syncs backup data before renaming so an existing valid backup cannot be truncated on crash.
    let rotate_backup =
        |src_content: &[u8], dest_path: &Path, mode: u32| -> Result<(), std::io::Error> {
            #[cfg(not(unix))]
            let _ = mode;
            let bak_parent = dest_path.parent().unwrap_or_else(|| Path::new("."));
            let (b_pid, b_seq, b_nanos) = next_temp_id();
            let bak_tmp_path = bak_parent.join(format!(
                ".{}.bak.tmp.{b_pid}.{b_seq}.{b_nanos}",
                dest_path.file_name().unwrap_or_default().to_string_lossy(),
            ));
            let mut bak_guard = {
                use std::io::Write;
                let mut opts = fs::OpenOptions::new();
                opts.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    opts.mode(mode);
                    // Refuse to follow symlinks via target-specific O_NOFOLLOW
                    opts.custom_flags(libc::O_NOFOLLOW);
                }
                let mut f = opts.open(&bak_tmp_path)?;
                let guard = TempFileGuard::new(bak_tmp_path.clone());
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    f.set_permissions(fs::Permissions::from_mode(mode))?;
                }
                f.write_all(src_content)?;
                f.sync_all()?;
                guard
            };
            fs::rename(bak_guard.path(), dest_path)?;
            bak_guard.disarm();
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
            rotate_backup(&target_bytes, &bak_path, mode)?;
            let _ = rotate_backup(&target_bytes, &backup_path, mode);
        } else {
            // Target is corrupted or domain-invalid! Do NOT overwrite valid existing backup files with a broken file!
            let has_valid_backup = [&bak_path, &backup_path].iter().any(|p| {
                p.exists()
                    && fs::read_to_string(p)
                        .map(|s| crate::manager::is_valid_settings_content(&s))
                        .unwrap_or(false)
            });
            if !has_valid_backup {
                let _ = rotate_backup(&target_bytes, &bak_path, mode);
            }
        }
    }

    // 6. Atomically rename temporary file to target path
    fs::rename(temp_guard.path(), target_path)?;
    temp_guard.disarm();

    // 7. Ensure directory durability once (barrier 2 of 2)
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
        assert_eq!(
            fs::read_to_string(&target_file).unwrap(),
            "{\"version\": 1}"
        );

        // Second write - updates file, creates backup of version 1
        write_atomic(&target_file, b"{\"version\": 2}").expect("second write");
        assert!(target_file.exists());
        assert!(bak_file.exists());
        assert_eq!(
            fs::read_to_string(&target_file).unwrap(),
            "{\"version\": 2}"
        );
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
        assert!(
            !target_file.exists(),
            "Target file must not be created in preview mode"
        );

        // Clean up env
        std::env::remove_var(ENV_READ_ONLY_PREVIEW);
        assert!(!is_read_only_preview());
    }
}
