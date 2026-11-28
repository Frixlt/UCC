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
    assert!(
        fs::metadata(dest.join("share/man/man8/uccd.8"))
            .map(|m| m.is_file() && m.len() > 0)
            .unwrap_or(false),
        "dist must stage non-empty share/man/man8/uccd.8"
    );
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

#[test]
fn t_pkg_09_gui_ldd_invariant_script_and_ci_wiring() {
    let root = repo_root();

    let script_path = root.join("scripts/check_gui_ldd.sh");
    assert!(
        script_path.is_file(),
        "scripts/check_gui_ldd.sh must exist to enforce ucc-gui shared-library invariants"
    );
    let script = std::fs::read_to_string(&script_path)
        .expect("scripts/check_gui_ldd.sh must be readable UTF-8");
    assert!(
        script.contains("ldd"),
        "scripts/check_gui_ldd.sh must inspect ucc-gui dynamic dependencies via ldd"
    );
    assert!(
        script.contains("not found"),
        "scripts/check_gui_ldd.sh must fail on unresolved 'not found' shared libraries"
    );
    assert!(
        script.contains("KF6"),
        "scripts/check_gui_ldd.sh must forbid KF6 linkage in ucc-gui"
    );
    let checks_qml_quick = (script.contains("Qt6Qml") && script.contains("Qt6Quick"))
        || (script.contains("Qt6") && script.contains("Qml") && script.contains("Quick"));
    assert!(
        checks_qml_quick,
        "scripts/check_gui_ldd.sh must forbid Qt6Qml/Qt6Quick linkage in ucc-gui"
    );
    assert!(
        script.contains("Qt6Bluetooth"),
        "scripts/check_gui_ldd.sh must forbid Qt6Bluetooth linkage in ucc-gui"
    );
    assert!(
        script.contains("libucc-dbus.so") || script.contains(r"libucc-dbus\.so"),
        "scripts/check_gui_ldd.sh must forbid dynamic libucc-dbus.so linkage in ucc-gui"
    );

    let build_yml = std::fs::read_to_string(root.join(".github/workflows/build.yml"))
        .expect(".github/workflows/build.yml must exist");
    assert!(
        build_yml.contains("scripts/check_gui_ldd.sh"),
        ".github/workflows/build.yml must invoke scripts/check_gui_ldd.sh in CI"
    );
}

#[test]
fn check_versions_cli_and_kernel_dkms_packaging_contracts() {
    use std::process::Command;

    let root = repo_root();
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("check-versions")
        .output()
        .expect("xtask check-versions must run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "xtask check-versions failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let workspace_version = stdout
        .lines()
        .find_map(|line| line.strip_prefix("Cargo.toml: "))
        .map(str::trim)
        .expect("check-versions output must report Cargo.toml version");
    assert!(
        !workspace_version.is_empty(),
        "Cargo.toml workspace version must not be empty"
    );

    for manifest in [
        "Cargo.toml",
        "CMakeLists.txt",
        "PKGBUILD",
        "ucc.spec",
        "package.nix",
        "debian/changelog",
    ] {
        let expected_line = format!("{manifest}: {workspace_version}");
        assert!(
            stdout.lines().any(|line| line.trim() == expected_line),
            "check-versions output missing '{expected_line}':\n{stdout}"
        );
    }
    let summary = format!("All 6 versioned manifests match {workspace_version}");
    assert!(
        stdout.contains(&summary),
        "check-versions summary missing '{summary}':\n{stdout}"
    );

    let dkms_conf = std::fs::read_to_string(root.join("kernel/uwcc-rs/dkms.conf"))
        .expect("kernel/uwcc-rs/dkms.conf must exist");
    assert!(
        dkms_conf.contains("PACKAGE_NAME=\"uwcc-rs\""),
        "kernel/uwcc-rs/dkms.conf must set PACKAGE_NAME=\"uwcc-rs\""
    );
    let expected_dkms_ver = format!("PACKAGE_VERSION=\"{workspace_version}\"");
    assert!(
        dkms_conf.contains(&expected_dkms_ver),
        "kernel/uwcc-rs/dkms.conf must contain {expected_dkms_ver}"
    );
    assert!(
        dkms_conf.contains("BUILT_MODULE_NAME[0]=\"uwcc_rs\""),
        "kernel/uwcc-rs/dkms.conf must build uwcc_rs module"
    );
    assert!(
        dkms_conf.contains("AUTOINSTALL=\"yes\""),
        "kernel/uwcc-rs/dkms.conf must set AUTOINSTALL=\"yes\""
    );

    let kernel_pkgbuild =
        std::fs::read_to_string(root.join("kernel/uwcc-rs/packaging/arch/PKGBUILD"))
            .expect("kernel/uwcc-rs/packaging/arch/PKGBUILD must exist");
    assert!(
        kernel_pkgbuild.contains("pkgname=uwcc-dkms"),
        "kernel PKGBUILD must define pkgname=uwcc-dkms"
    );
    let expected_pkgver = format!("pkgver={workspace_version}");
    assert!(
        kernel_pkgbuild.contains(&expected_pkgver),
        "kernel PKGBUILD must contain {expected_pkgver}"
    );
    assert!(
        kernel_pkgbuild.contains("conflicts=('tuxedo-drivers-dkms')"),
        "kernel PKGBUILD must conflict with tuxedo-drivers-dkms"
    );
    assert!(
        kernel_pkgbuild.contains("provides=('uwcc-driver')"),
        "kernel PKGBUILD must provide uwcc-driver"
    );

    for rel in [
        "Kbuild",
        "Makefile",
        "dkms.conf",
        "include/uapi/uwcc.h",
        "src/acpi_shim.h",
        "src/acpi_shim.c",
        "src/uwcc_rs.rs",
        "src/ring_buffer.rs",
        "src/validation.rs",
        "udev/99-uwcc.rules",
        "modprobe/uwcc-blacklist-tuxedo.conf",
    ] {
        assert!(
            kernel_pkgbuild.contains(rel),
            "kernel PKGBUILD must install {rel}"
        );
        assert!(
            root.join("kernel/uwcc-rs").join(rel).is_file(),
            "kernel/uwcc-rs/{rel} referenced by kernel PKGBUILD must exist"
        );
    }
}

#[test]
fn migration_plan_and_russian_readme_packaging_contracts() {
    let root = repo_root();

    let migration_plan_path = root.join("docs/rust/MIGRATION_PLAN.md");
    assert!(
        migration_plan_path.is_file(),
        "docs/rust/MIGRATION_PLAN.md must exist"
    );
    let migration_plan = std::fs::read_to_string(&migration_plan_path)
        .expect("docs/rust/MIGRATION_PLAN.md must be readable UTF-8");
    assert!(
        !migration_plan.trim().is_empty(),
        "docs/rust/MIGRATION_PLAN.md must not be empty"
    );
    assert!(
        migration_plan.contains("kernel/uwcc-rs/") && migration_plan.contains("uccd"),
        "docs/rust/MIGRATION_PLAN.md must document uccd and kernel/uwcc-rs/"
    );

    let readme_ru_path = root.join("README.ru.md");
    assert!(readme_ru_path.is_file(), "README.ru.md must exist");
    let readme_ru =
        std::fs::read_to_string(&readme_ru_path).expect("README.ru.md must be readable UTF-8");
    assert!(
        !readme_ru.trim().is_empty(),
        "README.ru.md must not be empty"
    );

    let cmake = std::fs::read_to_string(root.join("CMakeLists.txt"))
        .expect("CMakeLists.txt must exist");
    let installs_readme_ru = cmake.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.starts_with("install(FILES")
            && trimmed.contains("README.ru.md")
            && trimmed.contains("DESTINATION share/doc/ucc")
    });
    assert!(
        installs_readme_ru,
        "CMakeLists.txt must install README.ru.md to share/doc/ucc"
    );
}
