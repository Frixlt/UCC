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

/// Returns the current user's configuration path (`~/.config/ucc/settings` or `$XDG_CONFIG_HOME/ucc/settings`).
pub fn user_config_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.trim().is_empty() {
            let path = PathBuf::from(&xdg).join(USER_CONFIG_REL_PATH);
            if path.exists() {
                return Some(path);
            }
            let json_path = PathBuf::from(xdg).join(USER_CONFIG_JSON_REL_PATH);
            if json_path.exists() {
                return Some(json_path);
            }
            return Some(path);
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            let path = PathBuf::from(&home).join(".config").join(USER_CONFIG_REL_PATH);
            if path.exists() {
                return Some(path);
            }
            let json_path = PathBuf::from(home).join(".config").join(USER_CONFIG_JSON_REL_PATH);
            if json_path.exists() {
                return Some(json_path);
            }
            return Some(path);
        }
    }

    None
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

    /// Determines the active file path to read from:
    /// 1. Custom override path if set.
    /// 2. User config path (`~/.config/ucc/settings.json`) if it exists on disk.
    /// 3. System config path (`/etc/ucc/settings.json`) if it exists on disk.
    /// 4. None (indicating default settings should be generated).
    pub fn resolve_read_path(&self) -> Option<PathBuf> {
        if let Some(ref p) = self.custom_path {
            if p.exists() || Self::backup_paths(p).iter().any(|b| b.exists()) {
                return Some(p.clone());
            }
            return None;
        }

        if let Some(user_path) = user_config_path() {
            if user_path.exists() || Self::backup_paths(&user_path).iter().any(|b| b.exists()) {
                return Some(user_path);
            }
        }

        let sys_path = system_config_path();
        if sys_path.exists() || Self::backup_paths(&sys_path).iter().any(|b| b.exists()) {
            return Some(sys_path);
        }

        let sys_json = PathBuf::from(SYSTEM_CONFIG_JSON_PATH);
        if sys_json.exists() || Self::backup_paths(&sys_json).iter().any(|b| b.exists()) {
            return Some(sys_json);
        }

        None
    }

    /// Determines the active target path to write to:
    /// 1. Custom override path if set.
    /// 2. System config path if executing as root (UID 0).
    /// 3. User config path if available.
    /// 4. Fallback to system config path.
    pub fn resolve_write_path(&self) -> PathBuf {
        if let Some(ref p) = self.custom_path {
            return p.clone();
        }

        #[cfg(unix)]
        {
            extern "C" {
                fn geteuid() -> u32;
            }
            if unsafe { geteuid() == 0 } {
                return system_config_path();
            }
        }

        user_config_path().unwrap_or_else(system_config_path)
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

    /// Loads settings directly from an explicit file path.
    pub fn load_from_path(path: &Path) -> Result<UccSettings, ConfigError> {
        let content = fs::read_to_string(path)?;
        let settings = UccSettings::from_json(&content)?;
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
        for bak_path in Self::backup_paths(path) {
            if bak_path.exists() {
                if let Ok(settings) = Self::load_from_path(&bak_path) {
                    let json = settings.to_json_pretty()?;
                    let temp_path = path.with_extension(format!(
                        "tmp_restore_{}",
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos()
                    ));
                    fs::write(&temp_path, json.as_bytes())?;
                    fs::rename(&temp_path, path)?;
                    return Ok(settings);
                }
            }
        }
        Err(ConfigError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("No valid backup file found (.bak or .backup) for {}", path.display()),
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

        let json = settings.to_json_pretty()?;
        write_atomic(path, json.as_bytes())
    }
}
