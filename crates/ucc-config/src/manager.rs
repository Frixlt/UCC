//! Configuration manager orchestrating path resolution, loading, and persistence.
//!
//! Supports system-wide (`/etc/ucc/settings.json`) and per-user
//! (`~/.config/ucc/settings.json`) configuration paths.

use crate::error::ConfigError;
use crate::persistence::{is_read_only_preview, write_atomic};
use crate::settings::{create_default_settings, UccSettings};
use std::fs;
use std::path::{Path, PathBuf};

/// Standard system-wide settings path.
pub const SYSTEM_CONFIG_PATH: &str = "/etc/ucc/settings";

/// Alternate system-wide settings path (with .json extension).
pub const SYSTEM_CONFIG_JSON_PATH: &str = "/etc/ucc/settings.json";

/// Relative path under user config directory.
pub const USER_CONFIG_REL_PATH: &str = "ucc/settings";

/// Alternate relative path under user config directory.
pub const USER_CONFIG_JSON_REL_PATH: &str = "ucc/settings.json";

/// Returns the standard system configuration path (`/etc/ucc/settings`).
pub fn system_config_path() -> PathBuf {
    PathBuf::from(SYSTEM_CONFIG_PATH)
}

fn resolve_user_config_in_base(base: &Path) -> PathBuf {
    let path = base.join(USER_CONFIG_REL_PATH);
    if path.exists() {
        return path;
    }
    let json_path = base.join(USER_CONFIG_JSON_REL_PATH);
    if json_path.exists() {
        return json_path;
    }
    if ConfigManager::backup_paths(&path)
        .iter()
        .any(|b| b.exists())
    {
        return path;
    }
    if ConfigManager::backup_paths(&json_path)
        .iter()
        .any(|b| b.exists())
    {
        return json_path;
    }
    path
}

/// Returns the current user's configuration path (`~/.config/ucc/settings` or `$XDG_CONFIG_HOME/ucc/settings`).
/// Also checks `settings.json` and `.bak`/`.backup` recovery candidates when primary files are missing.
pub fn user_config_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.trim().is_empty() {
            return Some(resolve_user_config_in_base(Path::new(&xdg)));
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            let base = PathBuf::from(home).join(".config");
            return Some(resolve_user_config_in_base(&base));
        }
    }

    None
}

/// Checks whether raw JSON content represents valid loadable settings under the same
/// global + profile validation/quarantine rules as `ConfigManager::load_from_path`.
pub fn is_valid_settings_content(content: &str) -> bool {
    match UccSettings::from_json(content) {
        Ok(mut settings) => ConfigManager::validate_or_quarantine_settings(&mut settings).is_ok(),
        Err(_) => false,
    }
}

/// High-level configuration manager for loading and saving settings.
#[derive(Debug, Clone, Default)]
pub struct ConfigManager {
    custom_path: Option<PathBuf>,
}

impl ConfigManager {
    /// Creates a new ConfigManager using standard path resolution.
    pub fn new() -> Self {
        Self { custom_path: None }
    }

    /// Creates a ConfigManager with an explicit custom file path (ideal for testing or previews).
    pub fn with_custom_path(path: impl Into<PathBuf>) -> Self {
        Self {
            custom_path: Some(path.into()),
        }
    }

    /// Returns candidate backup paths for a given config path in order of preference (.bak, .backup).
    pub fn backup_paths(path: &Path) -> [PathBuf; 2] {
        [
            PathBuf::from(format!("{}.bak", path.display())),
            PathBuf::from(format!("{}.backup", path.display())),
        ]
    }

    /// Resolves the active system-wide write path given the canonical extensionless path
    /// (`/etc/ucc/settings`) and the `.json` path (`/etc/ucc/settings.json`), preventing
    /// root read/write path splits when `/etc/ucc/settings.json` is the active file on disk.
    pub(crate) fn resolve_system_write_path(sys_path: &Path, sys_json: &Path) -> PathBuf {
        if sys_path.exists() {
            return sys_path.to_path_buf();
        }
        if sys_json.exists() {
            return sys_json.to_path_buf();
        }
        if Self::backup_paths(sys_path).iter().any(|b| b.exists()) {
            return sys_path.to_path_buf();
        }
        if Self::backup_paths(sys_json).iter().any(|b| b.exists()) {
            return sys_json.to_path_buf();
        }
        sys_path.to_path_buf()
    }

    /// Determines the active file path to read from:
    /// 1. Custom override path if set.
    /// 2. User config path (`~/.config/ucc/settings` or `settings.json`, including `.bak`/`.backup` recovery)
    ///    when not running as root.
    /// 3. System config path (`/etc/ucc/settings`, falling back to `/etc/ucc/settings.json`).
    /// 4. None (indicating default settings should be generated).
    pub fn resolve_read_path(&self) -> Option<PathBuf> {
        if let Some(ref p) = self.custom_path {
            if p.exists() || Self::backup_paths(p).iter().any(|b| b.exists()) {
                return Some(p.clone());
            }
            return None;
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

        if !is_root {
            if let Some(user_path) = user_config_path() {
                if user_path.exists() || Self::backup_paths(&user_path).iter().any(|b| b.exists()) {
                    return Some(user_path);
                }
            }
        }

        let sys_path = system_config_path();
        let sys_json = PathBuf::from(SYSTEM_CONFIG_JSON_PATH);
        if sys_path.exists() {
            return Some(sys_path);
        }
        if sys_json.exists() {
            return Some(sys_json);
        }
        if Self::backup_paths(&sys_path).iter().any(|b| b.exists()) {
            return Some(sys_path);
        }
        if Self::backup_paths(&sys_json).iter().any(|b| b.exists()) {
            return Some(sys_json);
        }

        None
    }

    /// Determines the active target path to write to:
    /// 1. Custom override path if set.
    /// 2. Active system config path if executing as root (UID 0), matching whichever of
    ///    `/etc/ucc/settings` or `/etc/ucc/settings.json` is present on disk.
    /// 3. User config path if available.
    /// 4. Fallback to active system config path.
    pub fn resolve_write_path(&self) -> PathBuf {
        if let Some(ref p) = self.custom_path {
            return p.clone();
        }

        let sys_path = system_config_path();
        let sys_json = Path::new(SYSTEM_CONFIG_JSON_PATH);

        #[cfg(unix)]
        {
            extern "C" {
                fn geteuid() -> u32;
            }
            if unsafe { geteuid() == 0 } {
                return Self::resolve_system_write_path(&sys_path, sys_json);
            }
        }

        user_config_path().unwrap_or_else(|| Self::resolve_system_write_path(&sys_path, sys_json))
    }

    /// Loads settings from disk or returns defaults if no configuration file exists.
    /// If the configuration file is corrupted, attempts to recover from the `.bak` backup file.
    pub fn load(&self) -> Result<UccSettings, ConfigError> {
        match self.resolve_read_path() {
            Some(path) => {
                let (settings, _recovered) = Self::load_with_recovery(&path)?;
                Ok(settings)
            }
            None => Ok(create_default_settings()),
        }
    }

    /// Validates unified configuration invariants including global charging thresholds and profile structures.
    pub fn validate_settings(settings: &UccSettings) -> Result<(), ConfigError> {
        Self::validate_global_settings(settings)?;
        for profile in settings.profiles.values() {
            profile.validate()?;
        }
        Ok(())
    }

    /// Validates global settings and quarantines unreferenced invalid legacy profiles
    /// as long as at least one healthy profile remains in the catalog (`ISSUE-26`).
    pub(crate) fn validate_or_quarantine_settings(
        settings: &mut UccSettings,
    ) -> Result<(), ConfigError> {
        Self::validate_global_settings(settings)?;

        let referenced: std::collections::HashSet<String> =
            settings.state_map.values().cloned().collect();
        let total_profiles = settings.profiles.len();
        let mut invalid_unreferenced = Vec::new();
        for (key, profile) in &settings.profiles {
            if let Err(err) = profile.validate() {
                if referenced.contains(key) || referenced.contains(&profile.id) {
                    return Err(err.into());
                }
                invalid_unreferenced.push((key.clone(), err));
            }
        }
        // Only quarantine unreferenced quirky profiles when at least one healthy profile
        // remains in the catalog; otherwise fail so backup recovery / schema checks trigger.
        if !invalid_unreferenced.is_empty() && invalid_unreferenced.len() == total_profiles {
            return Err(invalid_unreferenced.into_iter().next().unwrap().1.into());
        }
        for (key, _err) in invalid_unreferenced {
            settings.profiles.remove(&key);
        }
        Ok(())
    }

    fn validate_global_settings(settings: &UccSettings) -> Result<(), ConfigError> {
        if let Some(start) = settings.charge_start_threshold {
            if !(0..=100).contains(&start) {
                return Err(ConfigError::Validation(format!(
                    "Invalid charge start threshold: {start} not in 0..=100"
                )));
            }
        }
        if let Some(end) = settings.charge_end_threshold {
            if !(0..=100).contains(&end) {
                return Err(ConfigError::Validation(format!(
                    "Invalid charge end threshold: {end} not in 0..=100"
                )));
            }
        }
        if let (Some(start), Some(end)) = (
            settings.charge_start_threshold,
            settings.charge_end_threshold,
        ) {
            if start >= end {
                return Err(ConfigError::Validation(format!(
                    "Invalid global charge thresholds: start ({start}) >= end ({end})"
                )));
            }
        }
        Ok(())
    }

    /// Loads settings directly from an explicit file path.
    ///
    /// Quarantine invalid non-referenced legacy profiles rather than discarding an entire valid
    /// settings file when a single unreferenced user profile has legacy field quirks (`ISSUE-26`).
    pub fn load_from_path(path: &Path) -> Result<UccSettings, ConfigError> {
        use std::io::Read as _;
        const MAX_CONFIG_FILE_BYTES: u64 = 8 * 1024 * 1024;
        let meta = fs::metadata(path)?;
        if meta.len() > MAX_CONFIG_FILE_BYTES {
            return Err(ConfigError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "Configuration file {} exceeds maximum allowed size of {} bytes",
                    path.display(),
                    MAX_CONFIG_FILE_BYTES
                ),
            )));
        }
        let file = fs::File::open(path)?;
        let mut buf = Vec::new();
        file.take(MAX_CONFIG_FILE_BYTES + 1).read_to_end(&mut buf)?;
        if buf.len() as u64 > MAX_CONFIG_FILE_BYTES {
            return Err(ConfigError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "Configuration file {} exceeds maximum allowed size of {} bytes",
                    path.display(),
                    MAX_CONFIG_FILE_BYTES
                ),
            )));
        }
        let content = String::from_utf8(buf).map_err(|e| {
            ConfigError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
        })?;
        let mut settings = UccSettings::from_json(&content)?;
        Self::validate_or_quarantine_settings(&mut settings)?;
        Ok(settings)
    }

    /// Loads settings from `path`. If `path` is corrupted (syntax error, truncated write,
    /// invalid types) or unreadable, attempts recovery by reading from `<path>.bak` or `<path>.backup`.
    /// Returns `Ok((settings, was_recovered))` where `was_recovered == true` indicates that
    /// the configuration was successfully restored from a backup file.
    pub fn load_with_recovery(path: &Path) -> Result<(UccSettings, bool), ConfigError> {
        match Self::load_from_path(path) {
            Ok(settings) => Ok((settings, false)),
            Err(primary_err) => {
                for bak_path in Self::backup_paths(path) {
                    if bak_path.exists() {
                        if let Ok(settings) = Self::load_from_path(&bak_path) {
                            return Ok((settings, true));
                        }
                    }
                }
                Err(primary_err)
            }
        }
    }

    /// Recovers settings from `<path>.bak` or `<path>.backup` and writes them back to `path` atomically,
    /// strictly preserving the valid backup file.
    pub fn restore_from_backup(path: &Path) -> Result<UccSettings, ConfigError> {
        if is_read_only_preview() {
            return Err(ConfigError::ReadOnlyPreview);
        }

        let existing_meta = fs::symlink_metadata(path).ok();
        if let Some(ref meta) = existing_meta {
            if meta.file_type().is_symlink() {
                return Err(ConfigError::PermissionDenied(format!(
                    "Refusing to restore backup over symlink target {}",
                    path.display()
                )));
            }
        }

        #[cfg(unix)]
        let primary_mode = {
            use std::os::unix::fs::PermissionsExt;
            existing_meta
                .as_ref()
                .map(|m| m.permissions().mode() & 0o777)
        };

        for bak_path in Self::backup_paths(path) {
            if bak_path.exists() {
                if let Ok(settings) = Self::load_from_path(&bak_path) {
                    let json = settings.to_json_pretty()?;
                    let parent = path
                        .parent()
                        .filter(|p| !p.as_os_str().is_empty())
                        .unwrap_or_else(|| Path::new("."));
                    let file_name = path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("settings.json");
                    let temp_path = parent.join(format!(
                        ".{file_name}.tmp_restore.{}.{}",
                        std::process::id(),
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos()
                    ));
                    let write_res = (|| -> std::io::Result<()> {
                        use std::io::Write;
                        let mut opts = fs::OpenOptions::new();
                        opts.write(true).create_new(true);
                        #[cfg(unix)]
                        let mode = {
                            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
                            let m = primary_mode.unwrap_or_else(|| {
                                fs::metadata(&bak_path)
                                    .map(|m| m.permissions().mode() & 0o777)
                                    .unwrap_or(0o600)
                            });
                            opts.mode(m);
                            opts.custom_flags(libc::O_NOFOLLOW);
                            m
                        };
                        let mut temp_file = opts.open(&temp_path)?;
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            temp_file.set_permissions(fs::Permissions::from_mode(mode))?;
                        }
                        temp_file.write_all(json.as_bytes())?;
                        temp_file.flush()?;
                        temp_file.sync_all()?;
                        Ok(())
                    })();
                    if let Err(e) = write_res {
                        let _ = fs::remove_file(&temp_path);
                        return Err(ConfigError::Io(e));
                    }
                    if let Err(e) = fs::rename(&temp_path, path) {
                        let _ = fs::remove_file(&temp_path);
                        return Err(ConfigError::Io(e));
                    }
                    #[cfg(unix)]
                    if let Ok(dir_f) = fs::File::open(parent) {
                        let _ = dir_f.sync_all();
                    }
                    return Ok(settings);
                }
            }
        }
        Err(ConfigError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "No valid backup file found (.bak or .backup) for {}",
                path.display()
            ),
        )))
    }

    /// Saves settings to the resolved write path using atomic persistence.
    /// Returns the path where settings were written.
    pub fn save(&self, settings: &UccSettings) -> Result<PathBuf, ConfigError> {
        if is_read_only_preview() {
            return Err(ConfigError::ReadOnlyPreview);
        }

        let target_path = self.resolve_write_path();
        self.save_to(&target_path, settings)?;
        Ok(target_path)
    }

    /// Saves settings directly to an explicit target path using atomic persistence.
    pub fn save_to(&self, path: &Path, settings: &UccSettings) -> Result<(), ConfigError> {
        if is_read_only_preview() {
            return Err(ConfigError::ReadOnlyPreview);
        }

        Self::validate_settings(settings)?;

        let json = settings.to_json_pretty()?;
        write_atomic(path, json.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_system_write_path_avoids_settings_json_split() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sys_path = dir.path().join("settings");
        let sys_json = dir.path().join("settings.json");

        // Neither exists -> defaults to extensionless /etc/ucc/settings.
        assert_eq!(
            ConfigManager::resolve_system_write_path(&sys_path, &sys_json),
            sys_path
        );

        // Only settings.json exists -> writes back to settings.json (no split).
        fs::write(&sys_json, "{}").unwrap();
        assert_eq!(
            ConfigManager::resolve_system_write_path(&sys_path, &sys_json),
            sys_json
        );

        // Legacy settings exists -> prefers legacy settings (matching resolve_read_path).
        fs::write(&sys_path, "{}").unwrap();
        assert_eq!(
            ConfigManager::resolve_system_write_path(&sys_path, &sys_json),
            sys_path
        );

        // Only settings.json.bak exists -> resolves to settings.json for recovery/write consistency.
        fs::remove_file(&sys_path).unwrap();
        fs::remove_file(&sys_json).unwrap();
        fs::write(dir.path().join("settings.json.bak"), "{}").unwrap();
        assert_eq!(
            ConfigManager::resolve_system_write_path(&sys_path, &sys_json),
            sys_json
        );
    }
}
