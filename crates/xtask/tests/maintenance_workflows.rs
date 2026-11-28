// SPDX-License-Identifier: GPL-3.0-or-later

//! Rootless workflow tests for `M11.3` and `M11.4` maintenance subcommands:
//! `import-offline`, `prepare-deployment`, `deploy` / rollback, `desktop-transition`,
//! `capture-gallery`, and `parser-check`.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};
use tempfile::TempDir;
use xtask::maintenance::{
    capture_gallery, find_duplicate_json_keys, prepare_deployment, run_parser_check,
    verify_cyclonedx_sbom_json, verify_gallery_output, CaptureGalleryArgs, CmdOutput,
    CommandRunner, DeploymentManager, DesktopTransition, PrepareDeploymentArgs,
    DESKTOP_ACTIVATE_UNITS, DESKTOP_STOP_UNITS, GALLERY_PAGES,
};
use xtask::sha256_hex;

type RecordedCall = (String, Vec<String>, Vec<(String, String)>);
type CallHook = Box<dyn Fn(&str, &[String], &[(&str, &str)]) -> Option<CmdOutput>>;

#[derive(Default)]
struct RecordingRunner {
    calls: RefCell<Vec<RecordedCall>>,
    scripted: RefCell<BTreeMap<String, VecDeque<CmdOutput>>>,
    on_call: Option<CallHook>,
}

impl RecordingRunner {
    fn push_reply(&self, prog: &str, reply: CmdOutput) {
        self.scripted
            .borrow_mut()
            .entry(prog.to_string())
            .or_default()
            .push_back(reply);
    }

    fn calls(&self) -> Vec<RecordedCall> {
        self.calls.borrow().clone()
    }
}

impl CommandRunner for RecordingRunner {
    fn run_with_env(
        &self,
        program: &str,
        args: &[String],
        envs: &[(&str, &str)],
    ) -> Result<CmdOutput, String> {
        self.calls.borrow_mut().push((
            program.to_string(),
            args.to_vec(),
            envs.iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        ));
        if let Some(cb) = &self.on_call {
            if let Some(res) = cb(program, args, envs) {
                return Ok(res);
            }
        }
        if let Some(queue) = self.scripted.borrow_mut().get_mut(program) {
            if let Some(res) = queue.pop_front() {
                return Ok(res);
            }
        }
        Ok(CmdOutput::ok(""))
    }
}

fn build_asar(path: &Path) {
    let literal = b"export const defaultFanProfiles = [\n{name: 'Balanced', tableCPU: [{temp: 0, speed: 10}, {temp: 100, speed: 90}], tableGPU: [{temp: 0, speed: 10}, {temp: 100, speed: 90}]},\n];\n";
    let mut tree = json!({ "size": literal.len(), "offset": "0" });
    for part in [
        "dist",
        "tuxedo-control-center",
        "service-app",
        "common",
        "models",
        "TccFanTable.js",
    ]
    .into_iter()
    .rev()
    {
        tree = json!({ "files": { (part): tree } });
    }
    let header = serde_json::to_vec(&tree).unwrap();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&4_u32.to_le_bytes());
    bytes.extend_from_slice(&((header.len() + 8) as u32).to_le_bytes());
    bytes.extend_from_slice(&((header.len() + 4) as u32).to_le_bytes());
    bytes.extend_from_slice(&(header.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&header);
    bytes.extend_from_slice(literal);
    fs::write(path, bytes).unwrap();
}

fn write_sample_inputs(
    root: &Path,
) -> (
    PathBuf,
    PathBuf,
    PathBuf,
    PathBuf,
    PathBuf,
    PathBuf,
    PathBuf,
) {
    let data = root.join("Data");
    fs::create_dir_all(&data).unwrap();
    fs::write(
        data.join("config.json"),
        serde_json::to_string_pretty(&json!({
            "settings": {
                "ac": { "tuxedo_profile_id": "prof1", "cpu_profile": "performance" },
                "type_c": { "tuxedo_profile_id": "prof1", "cpu_profile": "balanced" },
                "bat": { "tuxedo_profile_id": "prof1", "cpu_profile": "power-saver" }
            },
            "devices": []
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(data.join("keyboard_colors.json"), r#"{"0":[10,20,30]}"#).unwrap();
    fs::write(data.join("lightbar.json"), r#"{"silent_mode":true}"#).unwrap();
    fs::write(
        data.join("gui_settings.json"),
        r#"{"theme":"system","start_minimized":false}"#,
    )
    .unwrap();

    let tcc = root.join("etc/tcc");
    fs::create_dir_all(&tcc).unwrap();
    fs::write(tcc.join("settings"), r#"{"chargingProfile":"stationary"}"#).unwrap();
    fs::write(
        tcc.join("profiles"),
        serde_json::to_string_pretty(&json!([{
            "id": "prof1",
            "name": "Balanced Base",
            "fan": { "useControl": true, "fanProfile": "Balanced" }
        }]))
        .unwrap(),
    )
    .unwrap();

    let asar = root.join("app.asar");
    build_asar(&asar);

    let sysfs = root.join("sys/class/leds/rgb:kbd_backlight");
    fs::create_dir_all(&sysfs).unwrap();
    fs::write(sysfs.join("multi_intensity"), "0 0 0\n").unwrap();
    fs::write(sysfs.join("max_brightness"), "50\n").unwrap();
    fs::write(sysfs.join("brightness"), "25\n").unwrap();

    let build = root.join("build");
    fs::create_dir_all(build.join("bin")).unwrap();
    fs::create_dir_all(build.join("include")).unwrap();
    fs::write(
        build.join("include/version.h"),
        "#define UCC_VERSION_FULL \"0.0.1-test\"\n",
    )
    .unwrap();
    fs::write(
        build.join("CMakeCache.txt"),
        "UCC_READ_ONLY_PREVIEW:BOOL=OFF\nCMAKE_INSTALL_PREFIX:PATH=/usr\nBUILD_GUI:BOOL=ON\nBUILD_CLI:BOOL=ON\nBUILD_DESKTOP_TRAY:BOOL=ON\nBUILD_SESSION_AGENT:BOOL=ON\n",
    )
    .unwrap();
    fs::write(build.join("bin/ucc-import-check"), "#!/bin/sh\nexit 0\n").unwrap();

    let normal_xml = root.join("normal.xml");
    let preview_xml = root.join("preview.xml");
    fs::write(
        &normal_xml,
        "<testsuite><testcase name=\"test_ok\"/><testcase name=\"test_gui_audit\"/></testsuite>",
    )
    .unwrap();
    fs::write(
        &preview_xml,
        "<testsuite><testcase name=\"test_preview_ok\"/></testsuite>",
    )
    .unwrap();

    (
        data,
        tcc,
        asar,
        root.join("sys/class/leds"),
        build,
        normal_xml,
        preview_xml,
    )
}

#[test]
fn cli_import_offline_creates_private_bundle_without_python() {
    let tmp = TempDir::new().unwrap();
    let (data, tcc, asar, _, _, _, _) = write_sample_inputs(tmp.path());
    let caps = tmp.path().join("caps.json");
    fs::write(&caps, r#"{"zones":1,"maxBrightness":50,"ledIds":[0]}"#).unwrap();
    let out_dir = tmp.path().join("import-out");

    let status = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args([
            "import-offline",
            data.to_str().unwrap(),
            out_dir.to_str().unwrap(),
            "--capabilities",
            caps.to_str().unwrap(),
            "--tcc-dir",
            tcc.to_str().unwrap(),
            "--tcc-archive",
            asar.to_str().unwrap(),
            "--brightness",
            "30",
        ])
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert!(out_dir.join("settings").is_file());
    assert!(out_dir.join("report.json").is_file());
    assert!(out_dir.join("pending.json").is_file());
    let prefs: Value = serde_json::from_str(
        &fs::read_to_string(out_dir.join("desktop-preferences.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(prefs, json!({ "theme": "auto", "startMinimized": false }));
}

#[test]
fn prepare_deployment_stages_package_and_validates_junit() {
    let tmp = TempDir::new().unwrap();
    let (data, tcc_dir, tcc_archive, sysfs_leds, build_dir, normal_tests, preview_tests) =
        write_sample_inputs(tmp.path());
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();

    let runner = RecordingRunner {
        on_call: Some(Box::new(|prog, args, envs| match prog {
            "cmake" => {
                let dest = envs
                    .iter()
                    .find(|(k, _)| *k == "DESTDIR")
                    .map(|(_, v)| PathBuf::from(v))
                    .unwrap();
                fs::create_dir_all(dest.join("usr/bin")).unwrap();
                fs::write(dest.join("usr/bin/uccd"), b"elf").unwrap();
                Some(CmdOutput::ok("installed"))
            }
            "git" => {
                if args.iter().any(|a| a == "rev-parse") {
                    Some(CmdOutput::ok("deadbeefcafebabe\n"))
                } else {
                    Some(CmdOutput::ok("42\n"))
                }
            }
            "bsdtar" => {
                let pkg_idx = args.iter().position(|a| a == "--file").unwrap() + 1;
                fs::write(&args[pkg_idx], b"fake-zstd-pkg").unwrap();
                Some(CmdOutput::ok(""))
            }
            "pacman" => Some(CmdOutput::ok("Name : ucc-rusted\n")),
            _ => Some(CmdOutput::ok("")),
        })),
        ..Default::default()
    };

    let bundle_dir = tmp.path().join("bundle");
    let args = PrepareDeploymentArgs {
        build_dir: build_dir.clone(),
        data_dir: data.clone(),
        tcc_dir: tcc_dir.clone(),
        tcc_archive: tcc_archive.clone(),
        normal_tests: normal_tests.clone(),
        preview_tests: preview_tests.clone(),
        sysfs_leds: sysfs_leds.clone(),
        allow_gui_audit_failure: false,
        output_dir: bundle_dir.clone(),
    };
    let res = prepare_deployment(&args, &repo_root, &runner, true).unwrap();
    let pkg_name = res["package"].as_str().unwrap();
    assert!(bundle_dir.join(pkg_name).is_file());
    assert!(bundle_dir.join("deployment.json").is_file());
    assert!(!bundle_dir.join("deploy-system.py").exists());

    // Failing test in JUnit XML blocks prepare-deployment unless explicitly allowed for audit only.
    let failing_xml = tmp.path().join("failing.xml");
    fs::write(
        &failing_xml,
        "<testsuite><testcase name=\"test_gui_audit\"><failure message=\"boom\"/></testcase></testsuite>",
    )
    .unwrap();
    let bad_args = PrepareDeploymentArgs {
        normal_tests: failing_xml.clone(),
        output_dir: tmp.path().join("bundle-fail"),
        ..args.clone()
    };
    assert!(prepare_deployment(&bad_args, &repo_root, &runner, true).is_err());

    let allowed_args = PrepareDeploymentArgs {
        normal_tests: failing_xml,
        allow_gui_audit_failure: true,
        output_dir: tmp.path().join("bundle-allowed"),
        ..args
    };
    assert!(prepare_deployment(&allowed_args, &repo_root, &runner, true).is_ok());
}

#[test]
fn deployment_install_rollback_and_checksum_verification() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("sysroot");
    fs::create_dir_all(root.join("etc/tcc")).unwrap();
    fs::write(root.join("etc/tcc/settings"), b"{}").unwrap();

    let input_file = root.join("home/user/config.json");
    fs::create_dir_all(input_file.parent().unwrap()).unwrap();
    fs::write(&input_file, b"{\"ok\":true}").unwrap();
    let input_sha = sha256_hex(b"{\"ok\":true}");

    let bundle = tmp.path().join("bundle");
    fs::create_dir_all(&bundle).unwrap();
    let pkg_bytes = b"pkg-payload";
    let pkg_name = "ucc-rusted-0.0.1-1-x86_64.pkg.tar.zst";
    fs::write(bundle.join(pkg_name), pkg_bytes).unwrap();
    fs::write(bundle.join("capabilities.json"), b"{}").unwrap();
    fs::write(
        bundle.join("deployment.json"),
        serde_json::to_string_pretty(&json!({
            "package": pkg_name,
            "packageSha256": sha256_hex(pkg_bytes),
            "inputs": [{ "path": "/home/user/config.json", "sha256": input_sha }]
        }))
        .unwrap(),
    )
    .unwrap();

    // 1) Happy install + explicit rollback.
    let runner = RecordingRunner::default();
    runner.push_reply("pacman", CmdOutput::fail(1, "not installed")); // -Q check
    for _ in 0..5 {
        runner.push_reply("systemctl", CmdOutput::ok("active\n"));
        runner.push_reply("systemctl", CmdOutput::ok("enabled\n"));
    }
    runner.push_reply("pacman", CmdOutput::ok("installed")); // -U
    runner.push_reply("/usr/lib/ucc/ucc-import-check", CmdOutput::ok(""));
    runner.push_reply("systemd-analyze", CmdOutput::ok(""));
    for _ in 0..7 {
        runner.push_reply("systemctl", CmdOutput::ok(""));
    }
    runner.push_reply("busctl", CmdOutput::ok("b true\n"));
    runner.push_reply(
        "ucc-cli",
        CmdOutput::ok(r#"{"current_mode":"ac","manual_mode":null}"#),
    );
    runner.push_reply("systemctl", CmdOutput::ok("active\n")); // health is-active
    runner.push_reply(
        "ucc-cli",
        CmdOutput::ok(r#"{"current_mode":"ac","manual_mode":"Balanced"}"#),
    );
    runner.push_reply("/usr/lib/ucc/ucc-sleep", CmdOutput::ok(""));
    runner.push_reply("/usr/lib/ucc/ucc-sleep", CmdOutput::ok(""));
    runner.push_reply("busctl", CmdOutput::ok("b true\n"));
    runner.push_reply(
        "ucc-cli",
        CmdOutput::ok(r#"{"current_mode":"ac","manual_mode":"Balanced"}"#),
    );
    runner.push_reply("systemctl", CmdOutput::ok("active\n"));
    runner.push_reply(
        "ucc-cli",
        CmdOutput::ok(r#"{"current_mode":"ac","manual_mode":"Balanced"}"#),
    );

    let mut mgr = DeploymentManager::new(&bundle, &root, &runner);
    let backup = mgr.install().expect("install succeeds");
    assert_eq!(mgr.state.as_ref().unwrap().phase, "installed");
    assert!(backup.join("etc/tcc/settings").is_file());

    // Now test explicit rollback from backup directory.
    let rb_runner = RecordingRunner::default();
    let mut rb_mgr = DeploymentManager::from_backup(&backup, &root, &rb_runner).unwrap();
    rb_mgr.rollback().unwrap();
    assert_eq!(rb_mgr.state.as_ref().unwrap().phase, "rolled-back");
    // Re-running rollback on already rolled-back deployment fails cleanly.
    assert!(DeploymentManager::from_backup(&backup, &root, &rb_runner).is_err());

    // 2) Auto-rollback triggered when sleep resume loses manual mode.
    let fail_runner = RecordingRunner::default();
    fail_runner.push_reply("pacman", CmdOutput::fail(1, "not installed"));
    for _ in 0..5 {
        fail_runner.push_reply("systemctl", CmdOutput::ok("active\n"));
        fail_runner.push_reply("systemctl", CmdOutput::ok("enabled\n"));
    }
    fail_runner.push_reply("pacman", CmdOutput::ok("installed"));
    fail_runner.push_reply("/usr/lib/ucc/ucc-import-check", CmdOutput::ok(""));
    fail_runner.push_reply("systemd-analyze", CmdOutput::ok(""));
    for _ in 0..7 {
        fail_runner.push_reply("systemctl", CmdOutput::ok(""));
    }
    fail_runner.push_reply("busctl", CmdOutput::ok("b true\n"));
    fail_runner.push_reply("ucc-cli", CmdOutput::ok(r#"{"current_mode":"ac"}"#));
    fail_runner.push_reply("systemctl", CmdOutput::ok("active\n"));
    fail_runner.push_reply(
        "ucc-cli",
        CmdOutput::ok(r#"{"current_mode":"ac","manual_mode":"Custom"}"#),
    );
    fail_runner.push_reply("/usr/lib/ucc/ucc-sleep", CmdOutput::ok(""));
    fail_runner.push_reply("/usr/lib/ucc/ucc-sleep", CmdOutput::ok(""));
    fail_runner.push_reply("busctl", CmdOutput::ok("b true\n"));
    fail_runner.push_reply("ucc-cli", CmdOutput::ok(r#"{"current_mode":"ac"}"#));
    fail_runner.push_reply("systemctl", CmdOutput::ok("active\n"));
    fail_runner.push_reply(
        "ucc-cli",
        CmdOutput::ok(r#"{"current_mode":"ac","manual_mode":null}"#),
    );

    let mut auto_rb_mgr = DeploymentManager::new(&bundle, &root, &fail_runner);
    let err = auto_rb_mgr.install().unwrap_err();
    assert!(err.contains("manual power mode"), "got {err}");
    assert_eq!(auto_rb_mgr.state.as_ref().unwrap().phase, "rolled-back");

    // 3) Tampered source config fails verification before touching system.
    fs::write(&input_file, b"tampered").unwrap();
    let tamper_runner = RecordingRunner::default();
    let tamper_mgr = DeploymentManager::new(&bundle, &root, &tamper_runner);
    assert!(tamper_mgr
        .verify_bundle()
        .unwrap_err()
        .contains("Source configuration changed"));
}

#[test]
fn desktop_transition_quiesce_activate_and_restore_follows_adr01() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let source = tmp.path().join("old-tcc");
    let state_dir = tmp.path().join("state");
    fs::create_dir_all(home.join(".config/autostart")).unwrap();
    fs::create_dir_all(&source).unwrap();
    let orig_desktop = "[Desktop Entry]\nName=Old CC\nExec=/opt/old\n";
    fs::write(
        home.join(".config/autostart/control-center.desktop"),
        orig_desktop,
    )
    .unwrap();

    let runner = RecordingRunner::default();
    runner.push_reply(
        "systemctl",
        CmdOutput::ok(r#"[{"unit":"tuxedo-power-tray.service","active":"active"},{"unit":"unrelated.service","active":"active"}]"#),
    );
    runner.push_reply("systemctl", CmdOutput::ok("")); // stop old unit
    runner.push_reply("systemctl", CmdOutput::ok("active\n")); // uccd.service active
    runner.push_reply("/usr/bin/ucc-gui", CmdOutput::ok(""));
    runner.push_reply("systemctl", CmdOutput::ok("")); // user daemon-reload
    runner.push_reply("systemd-run", CmdOutput::ok("")); // ucc-desktop-tray
    runner.push_reply("systemd-run", CmdOutput::ok("")); // ucc-session-agent
    runner.push_reply("systemctl", CmdOutput::ok("active\n")); // is-active tray
    runner.push_reply("systemctl", CmdOutput::ok("active\n")); // is-active agent

    // restore calls:
    runner.push_reply("systemctl", CmdOutput::ok("")); // stop DESKTOP_STOP_UNITS
    runner.push_reply("systemctl", CmdOutput::ok("")); // user daemon-reload
    runner.push_reply("systemctl", CmdOutput::ok("active\n")); // tuxedo-power.service active
    runner.push_reply("bash", CmdOutput::ok("")); // launch.sh

    let mut transition = DesktopTransition::new(&state_dir, &home, &runner);
    transition.poll_interval = std::time::Duration::ZERO;

    let quiesced = transition.quiesce(&source).unwrap();
    assert_eq!(quiesced.old_units, vec!["tuxedo-power-tray.service"]);

    let prefs_file = tmp.path().join("desktop-preferences.json");
    fs::write(&prefs_file, r#"{"theme":"auto","startMinimized":false}"#).unwrap();
    let activated = transition.activate(&prefs_file).unwrap();
    assert_eq!(activated.phase, "activated");
    assert!(
        fs::read_to_string(home.join(".config/autostart/control-center.desktop"))
            .unwrap()
            .contains("Hidden=true")
    );

    // Verify ADR-01: only DESKTOP_ACTIVATE_UNITS (tray + agent, NOT permanent ucc-gui.service) started!
    let calls = runner.calls();
    let launched_units: Vec<&str> = calls
        .iter()
        .filter(|(prog, _, _)| prog == "systemd-run")
        .filter_map(|(_, args, _)| args.iter().find(|a| a.starts_with("--unit=")))
        .map(|s| s.trim_start_matches("--unit="))
        .collect();
    assert_eq!(launched_units, DESKTOP_ACTIVATE_UNITS);
    assert!(!launched_units.contains(&"ucc-gui.service"));

    let restored = transition.restore().unwrap();
    assert_eq!(restored.phase, "restored");
    assert_eq!(
        fs::read_to_string(home.join(".config/autostart/control-center.desktop")).unwrap(),
        orig_desktop
    );
    let stop_call = runner
        .calls()
        .into_iter()
        .find(|(prog, args, _)| {
            prog == "systemctl" && args.contains(&"ucc-gui.service".to_string())
        })
        .expect("restore stops all DESKTOP_STOP_UNITS including ucc-gui.service");
    for unit in DESKTOP_STOP_UNITS {
        assert!(stop_call.1.contains(&unit.to_string()));
    }
}

#[test]
fn capture_gallery_verifies_all_pages_and_writes_manifest() {
    let tmp = TempDir::new().unwrap();
    let build_dir = tmp.path().join("build");
    fs::create_dir_all(build_dir.join("include")).unwrap();
    fs::create_dir_all(build_dir.join("bin")).unwrap();
    fs::write(
        build_dir.join("include/version.h"),
        "#define UCC_VERSION_FULL \"0.0.1-audit\"\n",
    )
    .unwrap();

    let output_dir = tmp.path().join("gallery");
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();

    let runner = RecordingRunner {
        on_call: Some(Box::new(|prog, args, envs| match prog {
            "dbus-run-session" => {
                let dir = envs
                    .iter()
                    .find(|(k, _)| *k == "UCC_SCREENSHOT_DIR")
                    .map(|(_, v)| PathBuf::from(v))
                    .unwrap();
                let theme = envs
                    .iter()
                    .find(|(k, _)| *k == "UCC_AUDIT_THEME")
                    .map(|(_, v)| *v)
                    .unwrap();
                for page in GALLERY_PAGES {
                    fs::write(dir.join(format!("{page}.png")), format!("{theme}:{page}")).unwrap();
                }
                Some(CmdOutput::ok(""))
            }
            "git" if args.iter().any(|a| a == "rev-parse") => Some(CmdOutput::ok("cafebabe1234\n")),
            "pkg-config" => Some(CmdOutput::ok("6.8.1\n")),
            _ => None,
        })),
        ..Default::default()
    };

    let manifest = capture_gallery(
        &CaptureGalleryArgs {
            build_dir,
            output_dir: output_dir.clone(),
        },
        &repo_root,
        &runner,
    )
    .unwrap();
    assert_eq!(manifest["files"].as_object().unwrap().len(), 22);
    assert!(output_dir.join("manifest.json").is_file());

    // Remove one page and ensure verify_gallery_output rejects incomplete captures.
    fs::remove_file(output_dir.join("light/overview.png")).unwrap();
    assert!(verify_gallery_output(&output_dir.join("light")).is_err());
}

#[test]
fn parser_check_rejects_duplicate_json_keys_and_passes_on_repo() {
    assert!(!find_duplicate_json_keys(r#"{"a": 1, "a": 2}"#)
        .unwrap()
        .is_empty());
    assert!(
        !find_duplicate_json_keys(r#"{"nested": [{"x": 1, "x": 2}]}"#)
            .unwrap()
            .is_empty()
    );
    assert!(
        find_duplicate_json_keys(r#"{"a": {"x": 1}, "b": {"x": 2}}"#)
            .unwrap()
            .is_empty()
    );

    assert!(verify_cyclonedx_sbom_json(r#"{"bomFormat":"CycloneDX","specVersion":"1.5","metadata":{"component":{"name":"ucc-rusted"}},"components":[{"purl":"pkg:cargo/foo@0.1.0"}]}"#).is_ok());
    assert!(verify_cyclonedx_sbom_json(r#"{"bomFormat":"CycloneDX","specVersion":"1.5","metadata":{"component":{"name":"wrong"}},"components":[]}"#).is_err());

    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let sbom = r#"{"bomFormat":"CycloneDX","specVersion":"1.5","metadata":{"component":{"name":"ucc-rusted"}},"components":[{"purl":"pkg:cargo/ucc-core@0.0.1"}]}"#;
    let report = run_parser_check(&repo_root, 6, sbom).unwrap();
    assert_eq!(report.versions_matched, 6);
    assert_eq!(report.json_files_checked, 11);
    assert!(report.importer_self_test);
}

#[test]
fn deployment_rejects_path_traversal_and_symlinks_and_installs_private_package_copy() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("sysroot");
    fs::create_dir_all(root.join("etc/tcc")).unwrap();
    fs::write(root.join("etc/tcc/settings"), b"{}").unwrap();

    let outside_file = tmp.path().join("outside-secret.json");
    fs::write(&outside_file, b"{\"secret\":true}").unwrap();
    let outside_sha = sha256_hex(b"{\"secret\":true}");

    let pkg_bytes = b"pkg-payload-v2";
    let pkg_name = "ucc-rusted-0.0.1-1-x86_64.pkg.tar.zst";

    // 1) Reject `..` path traversal in deployment.json inputs.
    let traversal_bundle = tmp.path().join("bundle-traversal");
    fs::create_dir_all(&traversal_bundle).unwrap();
    fs::write(traversal_bundle.join(pkg_name), pkg_bytes).unwrap();
    fs::write(
        traversal_bundle.join("deployment.json"),
        serde_json::to_string_pretty(&json!({
            "package": pkg_name,
            "packageSha256": sha256_hex(pkg_bytes),
            "inputs": [{ "path": "/../outside-secret.json", "sha256": outside_sha }]
        }))
        .unwrap(),
    )
    .unwrap();
    let runner = RecordingRunner::default();
    let mgr = DeploymentManager::new(&traversal_bundle, &root, &runner);
    let err = mgr.verify_bundle().unwrap_err();
    assert!(
        err.contains("traversal") || err.contains("invalid component"),
        "expected path traversal rejection, got: {err}"
    );

    // 2) Reject symlink file and symlink ancestor directory escaping root.
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let sym_input = root.join("home/user/linked.json");
        fs::create_dir_all(sym_input.parent().unwrap()).unwrap();
        symlink(&outside_file, &sym_input).unwrap();

        let sym_bundle = tmp.path().join("bundle-symlink-file");
        fs::create_dir_all(&sym_bundle).unwrap();
        fs::write(sym_bundle.join(pkg_name), pkg_bytes).unwrap();
        fs::write(
            sym_bundle.join("deployment.json"),
            serde_json::to_string_pretty(&json!({
                "package": pkg_name,
                "packageSha256": sha256_hex(pkg_bytes),
                "inputs": [{ "path": "/home/user/linked.json", "sha256": outside_sha }]
            }))
            .unwrap(),
        )
        .unwrap();
        let sym_mgr = DeploymentManager::new(&sym_bundle, &root, &runner);
        let sym_err = sym_mgr.verify_bundle().unwrap_err();
        assert!(
            sym_err.contains("symlink"),
            "expected symlink file rejection, got: {sym_err}"
        );

        let outside_dir = tmp.path().join("outside-dir");
        fs::create_dir_all(&outside_dir).unwrap();
        fs::write(outside_dir.join("cfg.json"), b"{\"secret\":true}").unwrap();
        symlink(&outside_dir, root.join("home/escape")).unwrap();

        let dir_sym_bundle = tmp.path().join("bundle-symlink-dir");
        fs::create_dir_all(&dir_sym_bundle).unwrap();
        fs::write(dir_sym_bundle.join(pkg_name), pkg_bytes).unwrap();
        fs::write(
            dir_sym_bundle.join("deployment.json"),
            serde_json::to_string_pretty(&json!({
                "package": pkg_name,
                "packageSha256": sha256_hex(pkg_bytes),
                "inputs": [{ "path": "/home/escape/cfg.json", "sha256": outside_sha }]
            }))
            .unwrap(),
        )
        .unwrap();
        let dir_sym_mgr = DeploymentManager::new(&dir_sym_bundle, &root, &runner);
        let dir_sym_err = dir_sym_mgr.verify_bundle().unwrap_err();
        assert!(
            dir_sym_err.contains("symlink"),
            "expected symlink ancestor rejection, got: {dir_sym_err}"
        );
    }

    // 3) Verify install() copies package into private 0700 backup dir and passes that copy to pacman -U.
    let valid_input = root.join("home/user/valid.json");
    fs::create_dir_all(valid_input.parent().unwrap()).unwrap();
    fs::write(&valid_input, b"{\"valid\":true}").unwrap();
    let valid_sha = sha256_hex(b"{\"valid\":true}");

    let bundle = tmp.path().join("bundle-private-copy");
    fs::create_dir_all(&bundle).unwrap();
    fs::write(bundle.join(pkg_name), pkg_bytes).unwrap();
    fs::write(bundle.join("capabilities.json"), b"{}").unwrap();
    fs::write(
        bundle.join("deployment.json"),
        serde_json::to_string_pretty(&json!({
            "package": pkg_name,
            "packageSha256": sha256_hex(pkg_bytes),
            "inputs": [{ "path": "/home/user/valid.json", "sha256": valid_sha }]
        }))
        .unwrap(),
    )
    .unwrap();

    let install_runner = RecordingRunner::default();
    install_runner.push_reply("pacman", CmdOutput::fail(1, "not installed")); // -Q check
    for _ in 0..5 {
        install_runner.push_reply("systemctl", CmdOutput::ok("active\n"));
        install_runner.push_reply("systemctl", CmdOutput::ok("enabled\n"));
    }
    install_runner.push_reply("pacman", CmdOutput::ok("installed")); // -U
    install_runner.push_reply("/usr/lib/ucc/ucc-import-check", CmdOutput::ok(""));
    install_runner.push_reply("systemd-analyze", CmdOutput::ok(""));
    for _ in 0..7 {
        install_runner.push_reply("systemctl", CmdOutput::ok(""));
    }
    install_runner.push_reply("busctl", CmdOutput::ok("b true\n"));
    install_runner.push_reply(
        "ucc-cli",
        CmdOutput::ok(r#"{"current_mode":"ac","manual_mode":null}"#),
    );
    install_runner.push_reply("systemctl", CmdOutput::ok("active\n"));
    install_runner.push_reply(
        "ucc-cli",
        CmdOutput::ok(r#"{"current_mode":"ac","manual_mode":"Balanced"}"#),
    );
    install_runner.push_reply("/usr/lib/ucc/ucc-sleep", CmdOutput::ok(""));
    install_runner.push_reply("/usr/lib/ucc/ucc-sleep", CmdOutput::ok(""));
    install_runner.push_reply("busctl", CmdOutput::ok("b true\n"));
    install_runner.push_reply(
        "ucc-cli",
        CmdOutput::ok(r#"{"current_mode":"ac","manual_mode":"Balanced"}"#),
    );
    install_runner.push_reply("systemctl", CmdOutput::ok("active\n"));
    install_runner.push_reply(
        "ucc-cli",
        CmdOutput::ok(r#"{"current_mode":"ac","manual_mode":"Balanced"}"#),
    );

    let mut install_mgr = DeploymentManager::new(&bundle, &root, &install_runner);
    let backup = install_mgr.install().expect("install succeeds");

    let expected_private_pkg = backup.join(pkg_name);
    assert!(
        expected_private_pkg.is_file(),
        "private package copy must exist inside backup dir"
    );
    assert_eq!(fs::read(&expected_private_pkg).unwrap(), pkg_bytes);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let backup_mode = fs::metadata(&backup).unwrap().permissions().mode() & 0o777;
        assert_eq!(backup_mode, 0o700, "backup dir must be mode 0700");
        let pkg_mode = fs::metadata(&expected_private_pkg)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(pkg_mode, 0o600, "private package copy must be mode 0600");
    }

    let pacman_u = install_runner
        .calls()
        .into_iter()
        .find(|(prog, args, _)| prog == "pacman" && args.first().map(String::as_str) == Some("-U"))
        .expect("pacman -U call recorded");
    assert_eq!(
        pacman_u.1,
        vec![
            "-U".to_string(),
            "--noconfirm".to_string(),
            expected_private_pkg.display().to_string(),
        ],
        "pacman -U must install from the private 0700 backup copy, not the untrusted bundle"
    );
}

#[test]
fn prepare_deployment_uses_offline_manifest_and_rust_import_check_fallback() {
    let tmp = TempDir::new().unwrap();
    let (data, tcc_dir, tcc_archive, sysfs_leds, build_dir, normal_tests, preview_tests) =
        write_sample_inputs(tmp.path());
    // Remove C++ ucc-import-check to exercise Rust fallback.
    fs::remove_file(build_dir.join("bin/ucc-import-check")).unwrap();

    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();

    let runner = RecordingRunner {
        on_call: Some(Box::new(|prog, args, envs| match prog {
            "cmake" => {
                let dest = envs
                    .iter()
                    .find(|(k, _)| *k == "DESTDIR")
                    .map(|(_, v)| PathBuf::from(v))
                    .unwrap();
                fs::create_dir_all(dest.join("usr/bin")).unwrap();
                fs::write(dest.join("usr/bin/uccd"), b"elf").unwrap();
                Some(CmdOutput::ok("installed"))
            }
            "git" => {
                if args.iter().any(|a| a == "rev-parse") {
                    Some(CmdOutput::ok("deadbeefcafebabe\n"))
                } else {
                    Some(CmdOutput::ok("42\n"))
                }
            }
            "bsdtar" => {
                let pkg_idx = args.iter().position(|a| a == "--file").unwrap() + 1;
                fs::write(&args[pkg_idx], b"fake-zstd-pkg").unwrap();
                Some(CmdOutput::ok(""))
            }
            "pacman" => Some(CmdOutput::ok("Name : ucc-rusted\n")),
            _ => Some(CmdOutput::ok("")),
        })),
        ..Default::default()
    };

    let bundle_dir = tmp.path().join("bundle-rust-fallback");
    let args = PrepareDeploymentArgs {
        build_dir,
        data_dir: data,
        tcc_dir,
        tcc_archive,
        normal_tests,
        preview_tests,
        sysfs_leds,
        allow_gui_audit_failure: false,
        output_dir: bundle_dir,
    };
    prepare_deployment(&args, &repo_root, &runner, true).unwrap();

    let calls = runner.calls();
    let import_call = calls
        .iter()
        .find(|(prog, _, _)| prog.ends_with("ucc-import-check"))
        .expect("ucc-import-check call recorded");
    assert!(
        !import_call.0.contains("/build/bin/ucc-import-check"),
        "expected Rust ucc-import-check fallback, got: {}",
        import_call.0
    );

    assert!(
        calls.iter().all(|(prog, _, _)| prog != "cargo"),
        "prepare_deployment must not invoke nested `cargo run -p xtask`: {calls:?}"
    );
    let runtime_call = calls
        .iter()
        .find(|(_, args, _)| args.first().map(String::as_str) == Some("install-runtime"))
        .expect("direct install-runtime call via current_exe recorded");
    assert_eq!(
        runtime_call.0,
        std::env::current_exe().unwrap().display().to_string()
    );
    assert_eq!(runtime_call.1.len(), 3);

    let pkginfo = fs::read_to_string(args.output_dir.join("stage/.PKGINFO"))
        .expect("stage/.PKGINFO must be written");
    assert!(pkginfo.contains("depend = qt6-charts"));
    assert!(pkginfo.contains("depend = libxcb"));
    assert!(!pkginfo.contains("depend = qt6-connectivity"));
}
