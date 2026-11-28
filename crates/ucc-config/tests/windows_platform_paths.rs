//! Cross-platform configuration and runtime path resolution contract tests (`M15.1`, `M15.5`).
//!
//! Verifies deterministic `PlatformPaths` resolution for both Unix (FHS / XDG) and Windows
//! (`%ProgramData%` / `%APPDATA%` / `%USERPROFILE%`) conventions without touching host files.

use std::path::{Path, PathBuf};
use ucc_config::platform_paths::{
    default_runtime_dir, default_system_settings_path, default_user_config_dir,
    resolve_platform_paths, PathPlatform,
};

#[test]
fn unix_resolves_fhs_and_xdg_config_home() {
    let paths = resolve_platform_paths(PathPlatform::Unix, |key| match key {
        "XDG_CONFIG_HOME" => Some("/home/alice/.config-custom".to_string()),
        "HOME" => Some("/home/alice".to_string()),
        _ => None,
    });

    assert_eq!(paths.system_config_dir, PathBuf::from("/etc/ucc"));
    assert_eq!(
        paths.system_settings_file,
        PathBuf::from("/etc/ucc/settings.json")
    );
    assert_eq!(
        paths.user_config_dir,
        PathBuf::from("/home/alice/.config-custom/ucc")
    );
    assert_eq!(paths.runtime_dir, PathBuf::from("/run/ucc"));
    assert_eq!(
        paths.sleep_state_file,
        PathBuf::from("/run/ucc/sleep-state.json")
    );
}

#[test]
fn unix_falls_back_from_xdg_to_home_and_var_lib() {
    let via_home = resolve_platform_paths(PathPlatform::Unix, |key| match key {
        "XDG_CONFIG_HOME" => Some("   ".to_string()),
        "HOME" => Some("/home/bob".to_string()),
        _ => None,
    });
    assert_eq!(
        via_home.user_config_dir,
        PathBuf::from("/home/bob/.config/ucc")
    );

    let fallback = resolve_platform_paths(PathPlatform::Unix, |key| match key {
        "XDG_CONFIG_HOME" => Some(String::new()),
        "HOME" => Some(" \t ".to_string()),
        _ => None,
    });
    assert_eq!(
        fallback.user_config_dir,
        PathBuf::from("/var/lib/ucc/user-config")
    );
    assert_eq!(fallback.system_config_dir, PathBuf::from("/etc/ucc"));
    assert_eq!(fallback.runtime_dir, PathBuf::from("/run/ucc"));
}

#[test]
fn windows_resolves_programdata_and_appdata_paths() {
    let paths = resolve_platform_paths(PathPlatform::Windows, |key| match key {
        "PROGRAMDATA" => Some(r"C:\ProgramData".to_string()),
        "APPDATA" => Some(r"C:\Users\Alice\AppData\Roaming".to_string()),
        "USERPROFILE" => Some(r"C:\Users\Ignored".to_string()),
        _ => None,
    });

    let expected_sys = Path::new(r"C:\ProgramData").join("ucc-rusted");
    let expected_run = expected_sys.join("run");
    let expected_user = Path::new(r"C:\Users\Alice\AppData\Roaming").join("ucc-rusted");

    assert_eq!(paths.system_config_dir, expected_sys);
    assert_eq!(
        paths.system_settings_file,
        expected_sys.join("settings.json")
    );
    assert_eq!(paths.user_config_dir, expected_user);
    assert_eq!(paths.runtime_dir, expected_run);
    assert_eq!(
        paths.sleep_state_file,
        expected_run.join("sleep-state.json")
    );
}

#[test]
fn windows_falls_back_from_appdata_to_userprofile_and_programdata() {
    let via_profile = resolve_platform_paths(PathPlatform::Windows, |key| match key {
        "PROGRAMDATA" => Some(r"C:\ProgramData".to_string()),
        "APPDATA" => None,
        "USERPROFILE" => Some(r"C:\Users\Bob".to_string()),
        _ => None,
    });
    assert_eq!(
        via_profile.user_config_dir,
        Path::new(r"C:\Users\Bob\AppData\Roaming").join("ucc-rusted")
    );

    let all_missing = resolve_platform_paths(PathPlatform::Windows, |_| None);
    let default_sys = Path::new(r"C:\ProgramData").join("ucc-rusted");
    assert_eq!(all_missing.system_config_dir, default_sys);
    assert_eq!(
        all_missing.system_settings_file,
        default_sys.join("settings.json")
    );
    assert_eq!(all_missing.user_config_dir, default_sys);
    assert_eq!(all_missing.runtime_dir, default_sys.join("run"));
    assert_eq!(
        all_missing.sleep_state_file,
        default_sys.join("run").join("sleep-state.json")
    );
}

#[test]
fn windows_ignores_whitespace_only_env_vars() {
    let paths = resolve_platform_paths(PathPlatform::Windows, |key| match key {
        "PROGRAMDATA" => Some("   ".to_string()),
        "APPDATA" => Some("\t\n".to_string()),
        "USERPROFILE" => Some(String::new()),
        _ => None,
    });

    let default_sys = Path::new(r"C:\ProgramData").join("ucc-rusted");
    assert_eq!(paths.system_config_dir, default_sys);
    assert_eq!(paths.user_config_dir, default_sys);
    assert_eq!(paths.runtime_dir, default_sys.join("run"));
}

#[test]
fn windows_preserves_custom_drive_letters_and_spaces() {
    let paths = resolve_platform_paths(PathPlatform::Windows, |key| match key {
        "PROGRAMDATA" => Some(r"D:\Custom Program Data".to_string()),
        "APPDATA" => Some(r"E:\Users\Jane Doe\AppData\Roaming".to_string()),
        _ => None,
    });

    assert_eq!(
        paths.system_config_dir,
        Path::new(r"D:\Custom Program Data").join("ucc-rusted")
    );
    assert_eq!(
        paths.user_config_dir,
        Path::new(r"E:\Users\Jane Doe\AppData\Roaming").join("ucc-rusted")
    );
    assert_eq!(
        paths.sleep_state_file,
        Path::new(r"D:\Custom Program Data")
            .join("ucc-rusted")
            .join("run")
            .join("sleep-state.json")
    );
}

#[test]
fn platform_paths_structural_invariants_hold_for_both_platforms() {
    for platform in [PathPlatform::Unix, PathPlatform::Windows] {
        let paths = resolve_platform_paths(platform, |key| match key {
            "XDG_CONFIG_HOME" => Some("/tmp/xdg".to_string()),
            "PROGRAMDATA" => Some(r"C:\ProgramData".to_string()),
            "APPDATA" => Some(r"C:\Users\Test\AppData\Roaming".to_string()),
            _ => None,
        });

        assert_eq!(
            paths.system_settings_file,
            paths.system_config_dir.join("settings.json")
        );
        assert_eq!(
            paths.sleep_state_file,
            paths.runtime_dir.join("sleep-state.json")
        );

        match platform {
            PathPlatform::Unix => {
                assert_eq!(paths.runtime_dir, PathBuf::from("/run/ucc"));
                assert_eq!(
                    paths.user_config_dir.file_name().and_then(|s| s.to_str()),
                    Some("ucc")
                );
            }
            PathPlatform::Windows => {
                assert_eq!(paths.runtime_dir, paths.system_config_dir.join("run"));
                assert_eq!(
                    paths.user_config_dir.file_name().and_then(|s| s.to_str()),
                    Some("ucc-rusted")
                );
            }
        }
    }
}

#[test]
fn host_default_helpers_match_host_platform_convention() {
    #[cfg(unix)]
    {
        assert_eq!(PathPlatform::host(), PathPlatform::Unix);
        assert_eq!(
            default_system_settings_path(),
            PathBuf::from("/etc/ucc/settings.json")
        );
        assert_eq!(default_runtime_dir(), PathBuf::from("/run/ucc"));
        let user_dir = default_user_config_dir();
        let leaf = user_dir.file_name().and_then(|s| s.to_str());
        assert!(matches!(leaf, Some("ucc" | "user-config")));
    }

    #[cfg(windows)]
    {
        assert_eq!(PathPlatform::host(), PathPlatform::Windows);
        assert_eq!(
            default_system_settings_path()
                .file_name()
                .and_then(|s| s.to_str()),
            Some("settings.json")
        );
        assert_eq!(
            default_runtime_dir().file_name().and_then(|s| s.to_str()),
            Some("run")
        );
        assert_eq!(
            default_user_config_dir()
                .file_name()
                .and_then(|s| s.to_str()),
            Some("ucc-rusted")
        );
    }
}
