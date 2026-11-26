use std::fs;
use std::path::Path;

#[test]
fn test_build_01_workspace_rust_version_is_harmonized() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir.parent().unwrap().parent().unwrap();

    let root_cargo = fs::read_to_string(root.join("Cargo.toml")).expect("root Cargo.toml must exist");
    assert!(
        root_cargo.contains("rust-version = \"1.89\"") || root_cargo.contains("rust-version = \"1.90\""),
        "Workspace rust-version must be at least 1.89 for dependencies"
    );

    let crates = [
        "ucc-core",
        "ucc-config",
        "ucc-hardware",
        "ucc-dbus",
        "uccd",
        "ucc-cli",
        "ucc-desktop",
        "ucc-lighting",
        "xtask",
    ];

    for krate in crates {
        let manifest = root.join("crates").join(krate).join("Cargo.toml");
        let content = fs::read_to_string(&manifest)
            .unwrap_or_else(|e| panic!("Failed to read {}: {e}", manifest.display()));
        assert!(
            content.contains("rust-version.workspace = true"),
            "{} must inherit rust-version.workspace = true",
            manifest.display()
        );
    }
}

#[test]
fn test_build_02_cargo_alias_for_xtask_exists() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir.parent().unwrap().parent().unwrap();
    let config = fs::read_to_string(root.join(".cargo/config.toml"))
        .expect(".cargo/config.toml must exist");
    assert!(
        config.contains("xtask = \"run --package xtask --\""),
        ".cargo/config.toml must configure cargo xtask alias"
    );
}
