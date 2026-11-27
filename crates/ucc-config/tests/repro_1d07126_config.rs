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
