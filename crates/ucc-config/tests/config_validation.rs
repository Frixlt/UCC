// SPDX-License-Identifier: GPL-3.0-or-later
//! Reproduction integration tests for confirmed `ucc-config` findings on commit `1d07126`:
//! - N19: `effective_tcc_fan_table` accepts inverted fan speed bounds (`minimum > maximum`).
//! - R13: `persistence.rs` hardcodes `0o400000` (x86_64-only `O_NOFOLLOW`) instead of `libc::O_NOFOLLOW`.

use std::fs;
use std::path::Path;
use ucc_config::{effective_tcc_fan_table, TccFanPoint};

#[test]
fn n19_tcc_import_rejects_inverted_fan_speed_bounds() {
    let base_points = vec![
        TccFanPoint {
            temp: 40,
            speed: 20,
        },
        TccFanPoint {
            temp: 80,
            speed: 90,
        },
    ];

    let result = effective_tcc_fan_table(&base_points, 80, 30, 0);
    assert!(
        result.is_err(),
        "N19: effective_tcc_fan_table with inverted bounds (minimum=80, maximum=30) must return Err, \
         got Ok with {} points",
        result.as_ref().map_or(0, Vec::len)
    );
}

#[test]
fn r13_persistence_uses_libc_o_nofollow_not_hardcoded_octal() {
    let persistence_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/persistence.rs");
    let source = fs::read_to_string(&persistence_path).unwrap_or_else(|e| {
        panic!("Failed to read {}: {e}", persistence_path.display());
    });

    let code_without_line_comments: String = source
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        !code_without_line_comments.contains("0o400000"),
        "R13: {} must not hardcode x86_64 octal 0o400000 for O_NOFOLLOW",
        persistence_path.display()
    );
    assert!(
        code_without_line_comments.contains("O_NOFOLLOW"),
        "R13: {} must use target-specific libc::O_NOFOLLOW",
        persistence_path.display()
    );
}

#[cfg(unix)]
#[test]
fn p0_08_write_atomic_rejects_broken_and_untrusted_symlinks() {
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;
    use ucc_config::write_atomic;

    let tmp = TempDir::new().unwrap();
    let cfg_dir = tmp.path().join("cfg");
    let outside_dir = tmp.path().join("outside");
    fs::create_dir_all(&cfg_dir).unwrap();
    fs::create_dir_all(&outside_dir).unwrap();

    // Create a broken symlink inside cfg_dir pointing outside to a non-existent victim file.
    let victim = outside_dir.join("victim.json");
    let broken_link = cfg_dir.join("settings.json");
    symlink(&victim, &broken_link).unwrap();

    let res = write_atomic(&broken_link, br#"{"attacker":true}"#);
    assert!(
        res.is_err(),
        "P0-08: write_atomic must reject broken symlink pointing outside target directory, got {:?}",
        res
    );
    assert!(
        !victim.exists(),
        "P0-08: write_atomic must not create victim file {} through broken symlink",
        victim.display()
    );
}

#[test]
fn m_cfg_02_platform_paths_never_falls_back_to_tmp_ucc() {
    use ucc_config::{resolve_platform_paths, PathPlatform};

    let unix = resolve_platform_paths(PathPlatform::Unix, |_| None);
    assert_ne!(
        unix.user_config_dir,
        Path::new("/tmp/ucc"),
        "M-CFG-02: Unix user_config_dir must never fall back to world-writable /tmp/ucc"
    );
    assert!(
        !unix.user_config_dir.starts_with("/tmp"),
        "M-CFG-02: Unix user_config_dir must not fall back under /tmp, got {}",
        unix.user_config_dir.display()
    );

    let win = resolve_platform_paths(PathPlatform::Windows, |_| None);
    let win_str = win.user_config_dir.to_string_lossy();
    assert!(
        !win_str.contains(r"ucc-rusted\ucc-rusted") && !win_str.contains("ucc-rusted/ucc-rusted"),
        "M-CFG-02: Windows user_config_dir fallback must not double `ucc-rusted\\ucc-rusted`, got {win_str}"
    );
}

#[test]
fn m_cfg_06_load_from_path_enforces_max_size_limit() {
    use tempfile::TempDir;
    use ucc_config::{create_default_settings, ConfigManager};

    let tmp = TempDir::new().unwrap();
    let oversized_path = tmp.path().join("oversized_settings.json");

    // Build a valid JSON settings payload padded with >16 MiB of leading whitespace.
    let valid_json = create_default_settings().to_json().unwrap();
    let mut payload = vec![b' '; 16 * 1024 * 1024];
    payload.extend_from_slice(valid_json.as_bytes());
    fs::write(&oversized_path, &payload).unwrap();

    let res = ConfigManager::load_from_path(&oversized_path);
    assert!(
        res.is_err(),
        "M-CFG-06: ConfigManager::load_from_path must reject oversized (>16 MiB) config files to prevent OOM"
    );
}
