// SPDX-License-Identifier: GPL-3.0-or-later

//! Reproduction tests for confirmed `xtask` / packaging / CI findings on commit `1d07126`:
//! - R14: `debian/changelog` top version must be monotonic above previous `0.3.0` for same-pkgname packages
//! - R15: `debian/control` must not require removed binary package `policykit-1` without `polkitd`
//! - R16: `crates/xtask/src/maintenance.rs` must use `ucc-rusted` and invoke `install-runtime`
//! - R17: `.github/workflows/release.yml` must verify git tag matches manifest version
//! - R23: `crates/xtask/tests/build_verification.rs` must check all 10 workspace members including `ucc-session-agent`

use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn parse_semver_triple(raw: &str) -> (u64, u64, u64) {
    let base = raw
        .split(|c| ['-', '+', '~'].contains(&c))
        .next()
        .unwrap_or(raw);
    let mut parts = base.split('.');
    let major = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let patch = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    (major, minor, patch)
}

/// R14: `debian/changelog` top version for package `ucc` must be strictly greater than the
/// previous released `0.3.0` (or carry an Epoch) so normal apt upgrades select the new package.
#[test]
fn r14_version_is_monotonic_above_0_3_0_for_same_pkgname_packages() {
    let changelog_path = workspace_root().join("debian/changelog");
    let changelog = fs::read_to_string(&changelog_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", changelog_path.display()));

    let first_line = changelog
        .lines()
        .find(|l| !l.trim().is_empty())
        .expect("debian/changelog must not be empty");
    let start = first_line
        .find('(')
        .expect("debian/changelog header must contain '('");
    let end = first_line[start + 1..]
        .find(')')
        .expect("debian/changelog header must contain ')'");
    let raw_version = &first_line[start + 1..start + 1 + end];

    let has_epoch = raw_version.contains(':');
    let parsed = parse_semver_triple(raw_version);
    assert!(
        has_epoch || parsed > (0, 3, 0),
        "R14: debian/changelog top version '{raw_version}' is not monotonic above previous 0.3.0 for same-pkgname package `ucc`"
    );
}

/// R15: `debian/control` must depend on `polkitd | policykit-1` or `polkitd`, not bare
/// `policykit-1` (which was removed as a binary package in Debian 13 / trixie).
#[test]
fn r15_debian_control_does_not_require_removed_policykit_1_only() {
    let control_path = workspace_root().join("debian/control");
    let control = fs::read_to_string(&control_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", control_path.display()));

    let depends_start = control
        .find("Depends:")
        .expect("debian/control must contain Depends: field");
    let depends_section = control[depends_start..]
        .split("\nDescription:")
        .next()
        .unwrap_or(&control[depends_start..]);

    assert!(
        depends_section.contains("polkitd"),
        "R15: debian/control Depends must include `polkitd` (e.g. `polkitd | policykit-1`) instead of removed bare `policykit-1`; got:\n{depends_section}"
    );
}

/// R16: `crates/xtask/src/maintenance.rs` must use `ucc-rusted` (not legacy `frixit-control-center`)
/// and invoke `install-runtime` when staging the deployment bundle.
#[test]
fn r16_maintenance_prepare_deployment_uses_ucc_rusted_and_install_runtime() {
    assert_ne!(
        xtask::maintenance::PACKAGE_NAME,
        "frixit-control-center",
        "R16: maintenance::PACKAGE_NAME must not hardcode legacy 'frixit-control-center'"
    );
    assert_eq!(
        xtask::maintenance::PACKAGE_NAME,
        "ucc-rusted",
        "R16: maintenance::PACKAGE_NAME should match active package name 'ucc-rusted'"
    );

    let maint_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/maintenance.rs");
    let source = fs::read_to_string(&maint_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", maint_path.display()));
    assert!(
        source.contains("install-runtime") || source.contains("install_runtime"),
        "R16: maintenance.rs deployment staging must invoke install-runtime for Rust binaries"
    );
}

/// R17: `.github/workflows/release.yml` triggers on `v*.*.*` tags and must validate that the
/// pushed git tag matches the manifest version in `Cargo.toml`.
#[test]
fn r17_release_workflow_verifies_git_tag_matches_manifest_version() {
    let workflow_path = workspace_root().join(".github/workflows/release.yml");
    let workflow = fs::read_to_string(&workflow_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", workflow_path.display()));

    let references_tag = workflow.contains("GITHUB_REF_NAME")
        || workflow.contains("github.ref_name")
        || workflow.contains("--tag")
        || workflow.contains("EXPECTED_VERSION");
    assert!(
        references_tag,
        "R17: .github/workflows/release.yml must validate the git tag against Cargo.toml manifest version"
    );
}

/// R23: `crates/xtask/tests/build_verification.rs` must verify `rust-version.workspace = true`
/// across all 10 workspace members declared in root `Cargo.toml`, including `ucc-session-agent`.
#[test]
fn r23_build_verification_checks_all_workspace_members_including_session_agent() {
    let root = workspace_root();
    let root_cargo = fs::read_to_string(root.join("Cargo.toml")).expect("read root Cargo.toml");

    // Extract workspace members from `[workspace] members = [...]`.
    let members_start = root_cargo
        .find("members = [")
        .expect("root Cargo.toml must define workspace members");
    let after = &root_cargo[members_start + "members = [".len()..];
    let members_end = after.find(']').expect("closing bracket for members");
    let members_block = &after[..members_end];

    let workspace_members: Vec<String> = members_block
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim().trim_end_matches(',').trim();
            let unquoted = trimmed.strip_prefix('"')?.strip_suffix('"')?;
            Some(unquoted.trim_start_matches("crates/").to_string())
        })
        .collect();

    assert_eq!(
        workspace_members.len(),
        10,
        "expected 10 workspace members in root Cargo.toml, got {workspace_members:?}"
    );

    let bv_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/build_verification.rs");
    let bv_src = fs::read_to_string(&bv_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", bv_path.display()));

    // Either dynamically parses `members = [` from root Cargo.toml, or lists every member explicitly.
    let dynamically_reads_members = bv_src.contains("members = [") || bv_src.contains("metadata");
    if !dynamically_reads_members {
        for member in &workspace_members {
            let quoted = format!("\"{member}\"");
            assert!(
                bv_src.contains(&quoted),
                "R23: crates/xtask/tests/build_verification.rs omits workspace member {quoted}"
            );
        }
    }
}
