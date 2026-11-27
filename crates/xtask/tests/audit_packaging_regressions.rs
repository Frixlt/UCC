// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn t_pkg_02_xtask_hygiene_and_pinned_toolchain() {
    let root = repo_root();
    let toolchain = std::fs::read_to_string(root.join("rust-toolchain.toml"))
        .expect("rust-toolchain.toml must exist");
    assert!(
        !toolchain.contains("channel = \"stable\""),
        "rust-toolchain.toml must pin an exact Rust version rather than floating 'stable':\n{toolchain}"
    );

    let xtask_src = std::fs::read_to_string(root.join("crates/xtask/src/main.rs")).unwrap();
    assert!(
        xtask_src.contains("\"dist\""),
        "xtask must expose a 'dist' subcommand for packaging staging"
    );
    assert!(
        !xtask_src.contains("/tmp/ucc-stand-fixtures")
            && !xtask_src.contains("/tmp/ucc-extra-fixtures"),
        "xtask must not hardcode shared /tmp/ucc-* fixture build paths"
    );
    assert!(
        !xtask_src.contains(".to_str().unwrap()"),
        "xtask must avoid .to_str().unwrap() on filesystem paths"
    );
}

#[test]
fn t_pkg_03_cmake_adr01_and_no_qml_ghosts() {
    let root = repo_root();
    let top_cmake = std::fs::read_to_string(root.join("CMakeLists.txt")).unwrap();
    let gui_cmake = std::fs::read_to_string(root.join("ucc-gui/CMakeLists.txt")).unwrap();
    let dbus_cmake = std::fs::read_to_string(root.join("libucc-dbus/CMakeLists.txt")).unwrap();

    assert!(
        !root.join("ucc-gui/qml").exists(),
        "retired ucc-gui/qml/ directory should be removed per ADR-01"
    );
    assert!(
        !top_cmake.contains("QuickControls2") && !top_cmake.contains("Quick\n"),
        "top-level CMakeLists.txt must not unconditionally require Qt6 Qml/Quick/QuickControls2 when BUILD_TRAY=OFF"
    );
    assert!(
        !gui_cmake.contains("Bluetooth"),
        "ADR-01: ucc-gui must not link Qt6::Bluetooth (moved to uccd)"
    );
    assert!(
        dbus_cmake.contains("STATIC"),
        "ADR-01: libucc-dbus should build as STATIC internal helper library without SO ABI split"
    );

    let win_act = std::fs::read_to_string(root.join("include/WindowActivation.hpp")).unwrap();
    assert!(
        win_act.contains("XDG_ACTIVATION_TOKEN"),
        "WindowActivation.hpp must forward XDG_ACTIVATION_TOKEN even when KF6::WindowSystem is absent"
    );
}

#[test]
fn t_pkg_04_packaging_manifests_consistent() {
    let root = repo_root();
    let deb_rules = std::fs::read_to_string(root.join("debian/rules")).unwrap();
    assert!(
        !deb_rules.contains("obj-x86_64-linux-gnu"),
        "debian/rules must not hardcode obj-x86_64-linux-gnu on Architecture: any"
    );
    assert!(
        deb_rules.contains("cargo"),
        "debian/rules must build Rust workspace binaries"
    );

    let spec = std::fs::read_to_string(root.join("ucc.spec")).unwrap();
    assert!(
        spec.contains("cargo") && spec.contains("ucc-hardware") && spec.contains("ucc-sleep"),
        "ucc.spec must build Rust components and package ucc-hardware/ucc-sleep helpers"
    );

    let nix = std::fs::read_to_string(root.join("package.nix")).unwrap();
    let sleep_svc = std::fs::read_to_string(root.join("uccd/uccd-sleep.service")).unwrap();
    if !sleep_svc.contains("/usr/bin/systemctl") {
        assert!(
            !nix.contains("for svc in uccd-pre-sleep.service uccd-sleep.service; do")
                || !nix.contains("--replace-fail \"/usr/bin/systemctl\""),
            "package.nix postFixup runs --replace-fail \"/usr/bin/systemctl\" on uccd-sleep.service where that string does not exist"
        );
    }

    let nixos_mod = std::fs::read_to_string(root.join("nix/nixos-module.nix")).unwrap();
    assert!(
        nixos_mod.contains("ExecStopPost") && nixos_mod.contains("--fan-auto"),
        "nix/nixos-module.nix must mirror ExecStopPost --fan-auto safety restore"
    );
}

#[test]
fn t_pkg_08_systemd_units_and_hardware_helper_dmi_gate() {
    let root = repo_root();
    let uccd_svc = std::fs::read_to_string(root.join("uccd/uccd.service")).unwrap();
    assert!(
        uccd_svc.contains("ExecStopPost=-/usr/lib/ucc/ucc-hardware --fan-auto"),
        "uccd.service ExecStopPost must use '-' prefix so unsupported laptops don't mark unit failed"
    );

    let sleep_svc = std::fs::read_to_string(root.join("uccd/uccd-sleep.service")).unwrap();
    assert!(
        sleep_svc.contains("After=dbus.service"),
        "uccd-sleep.service must order After=dbus.service"
    );

    // Neither legacy C++ hardware-helper nor new Rust ucc-hardware may restrict --fan-auto to one personal laptop DMI
    let hw_helper = std::fs::read_to_string(root.join("uccd/src/hardware-helper.cpp")).unwrap();
    assert!(
        !hw_helper.contains("YAOSHI Series-X6AR55xY"),
        "hardware-helper must not hardcode one personal laptop DMI board_name blocking fan auto restore everywhere else"
    );
    assert!(
        root.join("crates/uccd/src/bin").exists()
            || std::fs::read_to_string(root.join("crates/uccd/Cargo.toml"))
                .unwrap()
                .contains("ucc-hardware"),
        "Rust workspace must provide ucc-hardware binary (ISSUE-04)"
    );
}

#[test]
fn t_client_09_gnome_extension_contract() {
    let root = repo_root();
    let meta = std::fs::read_to_string(root.join("ucc-gnome/metadata.json")).unwrap();
    assert!(
        !meta.contains("\"settings-schema\""),
        "ucc-gnome/metadata.json must not reference nonexistent settings-schema"
    );

    let client_js = std::fs::read_to_string(root.join("ucc-gnome/uccdClient.js")).unwrap();
    assert!(
        client_js.contains("GetKeyboardBacklightControlSupported")
            && !client_js.contains("GetKeyboardBacklightControlEnabled"),
        "uccdClient.js must call GetKeyboardBacklightControlSupported instead of removed GetKeyboardBacklightControlEnabled"
    );

    let ext_js = std::fs::read_to_string(root.join("ucc-gnome/extension.js")).unwrap();
    assert!(
        ext_js.contains("getWaterCoolerConnected")
            || ext_js.contains("GetWaterCoolerConnected"),
        "ucc-gnome/extension.js must derive wcConnected from GetWaterCoolerConnected rather than GetPowerState == power_wc"
    );
    assert!(
        ext_js.contains("fanProfileId"),
        "ucc-gnome/extension.js ApplyFanProfiles must include fanProfileId so radio button state persists"
    );
}

#[test]
fn t_client_01_cpp_libucc_dbus_bool_methods_use_call_method_bool() {
    let root = repo_root();
    let cpp = std::fs::read_to_string(root.join("libucc-dbus/UccdClient.cpp")).unwrap();

    for method in [
        "SetStateMap",
        "SetBatchStateMap",
        "SetActiveProfile",
        "ApplyProfile",
        "SaveCustomProfile",
        "DeleteCustomProfile",
        "SetWebcam",
        "SetDisplayRefreshRate",
        "SetChargingProfile",
        "SetChargingPriority",
        "SetChargeStartThreshold",
        "SetChargeEndThreshold",
        "SetChargeType",
        "SetNVIDIAPowerOffset",
    ] {
        let bad = format!("callVoidMethod( \"{method}\"");
        assert!(
            !cpp.contains(&bad),
            "UccdClient.cpp discards bool return value of {method} via callVoidMethod"
        );
    }
}

#[test]
fn dist_stages_all_runtime_binaries_from_cargo_target_dir() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("custom target");
    let bins = target.join("release");
    let dest = target.join("dist");
    fs::create_dir_all(&bins).unwrap();
    let expected = [
        ("uccd", "bin"),
        ("ucc-cli", "bin"),
        ("ucc-desktop-tray", "bin"),
        ("ucc-session-agent", "bin"),
        ("ucc-hardware", "lib/ucc"),
        ("ucc-sleep", "lib/ucc"),
        ("ucc-import-check", "lib/ucc"),
    ];
    let create_executable = |name: &str| {
        let file = bins.join(name);
        fs::write(&file, name).unwrap();
        fs::set_permissions(file, fs::Permissions::from_mode(0o755)).unwrap();
    };
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_xtask"))
            .arg("dist")
            .env("CARGO_TARGET_DIR", &target)
            .output()
            .unwrap()
    };

    create_executable("uccd");
    let incomplete = run();
    assert!(!incomplete.status.success());
    assert!(String::from_utf8_lossy(&incomplete.stderr).contains("ucc-cli"));
    assert!(!dest.exists());

    for (name, _) in expected {
        create_executable(name);
    }
    let complete = run();
    assert!(
        complete.status.success(),
        "{}",
        String::from_utf8_lossy(&complete.stderr)
    );
    for (name, subdir) in expected {
        let file = dest.join(subdir).join(name);
        assert_eq!(fs::read_to_string(&file).unwrap(), name);
        assert_ne!(file.metadata().unwrap().permissions().mode() & 0o111, 0);
    }
    assert!(dest.join("share/man/man1/ucc-cli.1").is_file());
    assert!(dest.join("lib/systemd/system/uccd.service").is_file());
    assert_eq!(
        fs::read_link(dest.join("lib/systemd/system/uccd-pre-sleep.service")).unwrap(),
        PathBuf::from("uccd-sleep.service")
    );
}

#[test]
fn t_m11_6_shell_adapter_boundary_no_cpp_backend_and_no_plasma_tray() {
    let root = repo_root();

    for manifest in ["PKGBUILD", "ucc.spec", "package.nix", "debian/rules"] {
        let text = std::fs::read_to_string(root.join(manifest)).unwrap();
        assert!(
            text.contains("-DBUILD_TRAY=OFF"),
            "{manifest} must disable legacy C++/QML Plasma tray with -DBUILD_TRAY=OFF"
        );
        assert!(
            text.contains("-DUCC_PACKAGE_RUST_RUNTIME=ON"),
            "{manifest} must enable -DUCC_PACKAGE_RUST_RUNTIME=ON"
        );
    }

    let deb_install = std::fs::read_to_string(root.join("debian/ucc.install")).unwrap();
    for gjs_file in [
        "usr/share/gnome-shell/extensions/ucc@uniwill.com/metadata.json",
        "usr/share/gnome-shell/extensions/ucc@uniwill.com/extension.js",
        "usr/share/gnome-shell/extensions/ucc@uniwill.com/uccdClient.js",
        "usr/share/gnome-shell/extensions/ucc@uniwill.com/stylesheet.css",
    ] {
        assert!(
            deb_install.contains(gjs_file),
            "debian/ucc.install must ship pure-GJS adapter file {gjs_file}"
        );
    }
    assert!(
        !deb_install.contains("plasma/plasmoids") && !deb_install.contains("libucc_tray"),
        "debian/ucc.install must not ship retired Plasma tray plugin or QML C++ backend"
    );

    for entry in std::fs::read_dir(root.join("ucc-gnome")).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        assert_ne!(
            name, "CMakeLists.txt",
            "ucc-gnome must not have native CMake build"
        );
        for forbidden_ext in [".c", ".cpp", ".h", ".hpp", ".so"] {
            assert!(
                !name.ends_with(forbidden_ext),
                "ucc-gnome must remain pure GJS without native file {name}"
            );
        }
    }

    let adr03 = std::fs::read_to_string(
        root.join("docs/rust/execution/decisions/ADR-03-shell-embedding-policy.md"),
    )
    .unwrap();
    assert!(
        adr03.contains("ucc-desktop-tray") && adr03.contains("ucc-gnome"),
        "ADR-03 must document ucc-desktop-tray SNI replacement and thin GJS ucc-gnome exception"
    );
}

#[test]
fn t_m11_8_research_probes_excluded_from_runtime_and_attribution_preserved() {
    let root = repo_root();

    for packaging_file in [
        "CMakeLists.txt",
        "PKGBUILD",
        "ucc.spec",
        "package.nix",
        "debian/ucc.install",
        "crates/xtask/src/main.rs",
    ] {
        let text = std::fs::read_to_string(root.join(packaging_file)).unwrap();
        assert!(
            !text.contains("nvidia_findings"),
            "{packaging_file} must never package or install research probes from nvidia_findings"
        );
    }

    for doc in [
        "COPYING",
        "LICENSE",
        "PROJECT_ORIGINS.md",
        "THIRD_PARTY.md",
        "nvidia_findings/README.md",
        "nvidia_findings/nvidia_undocumented_api_findings.md",
    ] {
        let path = root.join(doc);
        assert!(path.is_file(), "missing attribution/historical doc {doc}");
        assert!(
            std::fs::metadata(&path).unwrap().len() > 0,
            "attribution/historical doc {doc} must not be empty"
        );
    }
}
