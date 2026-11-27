//! Cross-platform configuration, user preference, and runtime state directory paths (`M15.1`).
//!
//! Resolves standard FHS paths on Unix (`/etc/ucc`, `~/.config/ucc`, `/run/ucc`) and standard
//! `%ProgramData%` / `%APPDATA%` paths on Windows (`M15`).

use std::env;
use std::path::{Path, PathBuf};

/// Identifies the host path convention used by [`resolve_platform_paths`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PathPlatform {
    Unix,
    Windows,
}

impl PathPlatform {
    #[cfg(unix)]
    pub const fn host() -> Self {
        Self::Unix
    }

    #[cfg(windows)]
    pub const fn host() -> Self {
        Self::Windows
    }
}

/// Canonical runtime and configuration locations for a target OS convention.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformPaths {
    pub system_settings_file: PathBuf,
    pub system_config_dir: PathBuf,
    pub user_config_dir: PathBuf,
    pub runtime_dir: PathBuf,
    pub sleep_state_file: PathBuf,
}

/// Resolves platform paths using injected environment lookup (for deterministic testing on any OS).
pub fn resolve_platform_paths<F>(platform: PathPlatform, get_env: F) -> PlatformPaths
where
    F: Fn(&str) -> Option<String>,
{
    let non_empty = |key: &str| get_env(key).filter(|v| !v.trim().is_empty());

    match platform {
        PathPlatform::Unix => {
            let system_config_dir = PathBuf::from("/etc/ucc");
            let user_config_dir = non_empty("XDG_CONFIG_HOME")
                .map(|p| PathBuf::from(p).join("ucc"))
                .or_else(|| non_empty("HOME").map(|h| Path::new(&h).join(".config").join("ucc")))
                .unwrap_or_else(|| PathBuf::from("/var/lib/ucc/user-config"));
            let runtime_dir = PathBuf::from("/run/ucc");
            PlatformPaths {
                system_settings_file: system_config_dir.join("settings.json"),
                system_config_dir,
                user_config_dir,
                sleep_state_file: runtime_dir.join("sleep-state.json"),
                runtime_dir,
            }
        }
        PathPlatform::Windows => {
            let program_data =
                non_empty("PROGRAMDATA").unwrap_or_else(|| r"C:\ProgramData".to_string());
            let app_data = non_empty("APPDATA")
                .or_else(|| non_empty("USERPROFILE").map(|p| format!(r"{p}\AppData\Roaming")))
                .unwrap_or_else(|| program_data.clone());
            let system_config_dir = Path::new(&program_data).join("ucc-rusted");
            let runtime_dir = system_config_dir.join("run");
            PlatformPaths {
                system_settings_file: system_config_dir.join("settings.json"),
                system_config_dir,
                user_config_dir: Path::new(&app_data).join("ucc-rusted"),
                sleep_state_file: runtime_dir.join("sleep-state.json"),
                runtime_dir,
            }
        }
    }
}

/// Returns the default system settings file path on the current host OS.
pub fn default_system_settings_path() -> PathBuf {
    resolve_platform_paths(PathPlatform::host(), |k| env::var(k).ok()).system_settings_file
}

/// Returns the default per-user configuration directory on the current host OS.
pub fn default_user_config_dir() -> PathBuf {
    resolve_platform_paths(PathPlatform::host(), |k| env::var(k).ok()).user_config_dir
}

/// Returns the default runtime state directory on the current host OS.
pub fn default_runtime_dir() -> PathBuf {
    resolve_platform_paths(PathPlatform::host(), |k| env::var(k).ok()).runtime_dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_unix_and_windows_paths_cleanly() {
        let unix = resolve_platform_paths(PathPlatform::Unix, |k| match k {
            "XDG_CONFIG_HOME" => Some("/home/tester/.cfg".to_string()),
            _ => None,
        });
        assert_eq!(
            unix.system_settings_file,
            PathBuf::from("/etc/ucc/settings.json")
        );
        assert_eq!(unix.user_config_dir, PathBuf::from("/home/tester/.cfg/ucc"));
        assert_eq!(
            unix.sleep_state_file,
            PathBuf::from("/run/ucc/sleep-state.json")
        );

        let win = resolve_platform_paths(PathPlatform::Windows, |k| match k {
            "PROGRAMDATA" => Some(r"C:\ProgramData".to_string()),
            "APPDATA" => Some(r"C:\Users\Tester\AppData\Roaming".to_string()),
            _ => None,
        });
        assert!(
            win.system_settings_file
                .to_string_lossy()
                .ends_with("ucc-rusted/settings.json")
                || win
                    .system_settings_file
                    .to_string_lossy()
                    .ends_with(r"ucc-rusted\settings.json")
        );
        assert!(win.user_config_dir.to_string_lossy().contains("Tester"));
        assert!(win
            .sleep_state_file
            .to_string_lossy()
            .contains("sleep-state.json"));
    }
}
