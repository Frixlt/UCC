// SPDX-License-Identifier: GPL-3.0-or-later

//! Workspace automation tasks for building assets, completions, manpages, dist bundles, and test fixtures.

use clap_complete::Shell;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use xtask::maintenance::{
    self, CaptureGalleryArgs, DeploymentManager, DesktopTransition, PrepareDeploymentArgs,
    SystemCommandRunner,
};
use xtask::sha256_hex;

fn repo_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .map(Path::to_path_buf)
        .unwrap_or(manifest_dir)
}

const RUNTIME_BINARIES: [(&str, &str); 7] = [
    ("uccd", "bin"),
    ("ucc-cli", "bin"),
    ("ucc-desktop-tray", "bin"),
    ("ucc-session-agent", "bin"),
    ("ucc-hardware", "lib/ucc"),
    ("ucc-sleep", "lib/ucc"),
    ("ucc-import-check", "lib/ucc"),
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let task = args
        .next()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "help".to_string());

    match task.as_str() {
        "dist" => {
            let target = std::env::var_os("CARGO_TARGET_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| repo_root().join("target"));
            let dest = args
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(|| target.join("dist"));
            let bins = args.next().map(PathBuf::from).unwrap_or_else(|| {
                let release = target.join("release");
                if release.join("uccd").is_file() {
                    release
                } else {
                    target.join("debug")
                }
            });
            install_runtime(&dest, &bins)?;
            stage_dist_assets(&dest)?;
            println!("Distribution assets staged in {}", dest.display());
        }
        "install-runtime" => {
            let dest = args
                .next()
                .ok_or("install-runtime requires a destination")?;
            let bins = args
                .next()
                .ok_or("install-runtime requires a binary directory")?;
            install_runtime(Path::new(&dest), Path::new(&bins))?;
            stage_cli_assets(Path::new(&dest))?;
        }
        "dist-cli" | "assets" => {
            generate_all_completions(Path::new("target/completions"))?;
            generate_all_manpages(Path::new("target/man"))?;
            println!("CLI assets successfully generated in target/completions and target/man");
        }
        "completions" => {
            let out_dir = args
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("target/completions"));
            generate_all_completions(&out_dir)?;
            println!("Shell completions generated in {}", out_dir.display());
        }
        "manpages" => {
            let out_dir = args
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("target/man"));
            generate_all_manpages(&out_dir)?;
            println!("Man pages generated in {}", out_dir.display());
        }
        "build-fixtures" | "fixtures" => {
            let base = build_fixtures()?;
            println!("Test fixtures compiled to {}", base.display());
        }
        "check-versions" => {
            let root = repo_root();
            let versions = check_versions_in_root(&root)?;
            for (file, ver) in &versions {
                println!("{file}: {ver}");
            }
            println!(
                "All {} versioned manifests match {}",
                versions.len(),
                versions[0].1
            );
        }
        "release" => {
            let rest: Vec<String> = args.map(|s| s.to_string_lossy().into_owned()).collect();
            let dry_run = rest.iter().any(|a| a == "--dry-run");
            let version = rest
                .iter()
                .find(|a| !a.starts_with('-'))
                .ok_or("Usage: cargo xtask release <X.Y.Z[-prerelease]> [--dry-run]")?;
            let root = repo_root();
            let changed = bump_versions_in_root(&root, version, dry_run)?;
            if dry_run {
                println!(
                    "Dry-run complete for version {version} ({} files checked):",
                    changed.len()
                );
                for p in &changed {
                    println!("  would update {}", p.display());
                }
            } else {
                println!("Updated {} manifests to version {version}:", changed.len());
                for p in &changed {
                    println!("  {}", p.display());
                }
                maybe_run_git_cliff(&root, version);
                println!("Next steps:");
                println!("  1. cargo update --workspace --offline");
                println!(
                    "  2. cargo clippy --workspace --all-targets --locked --offline -- -D warnings"
                );
                println!("  3. dbus-run-session -- cargo test --workspace --locked --offline --no-fail-fast");
                println!("  4. git commit -am \"chore(release): v{version}\" && git tag -a v{version} -m \"v{version}\"");
            }
        }
        "import-offline" => {
            let rest: Vec<String> = args.map(|s| s.to_string_lossy().into_owned()).collect();
            let parsed = maintenance::parse_import_offline_args(&rest)?;
            let report = maintenance::run_import_offline(&parsed)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        "maintenance" => {
            let sub = args
                .next()
                .map(|s| s.to_string_lossy().into_owned())
                .ok_or(
                "Usage: cargo xtask maintenance <prepare-deployment|deploy|desktop-transition> ...",
            )?;
            let rest: Vec<String> = args.map(|s| s.to_string_lossy().into_owned()).collect();
            let runner = SystemCommandRunner;
            let root = repo_root();
            match sub.as_str() {
                "prepare-deployment" => {
                    let parsed = parse_prepare_deployment_args(&rest)?;
                    let out = maintenance::prepare_deployment(&parsed, &root, &runner, false)?;
                    println!("{}", serde_json::to_string(&out)?);
                }
                "deploy" => {
                    run_deploy_cli(&rest, &runner)?;
                }
                "desktop-transition" => {
                    run_desktop_transition_cli(&rest, &runner)?;
                }
                other => {
                    return Err(format!("Unknown maintenance subcommand: {other}").into());
                }
            }
        }
        "capture-gallery" => {
            let rest: Vec<String> = args.map(|s| s.to_string_lossy().into_owned()).collect();
            let root = repo_root();
            let parsed = parse_capture_gallery_args(&rest, &root)?;
            let runner = SystemCommandRunner;
            let manifest = maintenance::capture_gallery(&parsed, &root, &runner)?;
            let count = manifest["files"].as_object().map_or(0, |m| m.len());
            println!(
                "Captured and verified {count} images at {}",
                parsed.output_dir.display()
            );
        }
        "parser-check" => {
            let root = repo_root();
            let versions = check_versions_in_root(&root)?;
            let sbom = generate_sbom_json(&root, &versions[0].1)?;
            let report = maintenance::run_parser_check(&root, versions.len(), &sbom)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        _ => {
            eprintln!("Usage: cargo xtask <dist [dir [bin-dir]]|install-runtime <dir> <bin-dir>|dist-cli|completions|manpages|build-fixtures|check-versions|release <ver> [--dry-run]|import-offline|maintenance|capture-gallery|parser-check>");
            std::process::exit(1);
        }
    }

    Ok(())
}

fn stage_cli_assets(dest: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let man_dir = dest.join("share/man/man1");
    fs::create_dir_all(&man_dir)?;
    let mut man_file = fs::File::create(man_dir.join("ucc-cli.1"))?;
    ucc_cli::cli::generate_manpage(&mut man_file)?;

    let man8_dir = dest.join("share/man/man8");
    fs::create_dir_all(&man8_dir)?;
    fs::copy(repo_root().join("uccd/uccd.8"), man8_dir.join("uccd.8"))?;

    for (shell, rel) in [
        (Shell::Bash, "share/bash-completion/completions/ucc-cli"),
        (Shell::Zsh, "share/zsh/site-functions/_ucc-cli"),
        (Shell::Fish, "share/fish/vendor_completions.d/ucc-cli.fish"),
    ] {
        let out_path = dest.join(rel);
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut f = fs::File::create(&out_path)?;
        ucc_cli::cli::generate_completions(shell, &mut f);
    }

    Ok(())
}

fn stage_dist_assets(dest: &Path) -> Result<(), Box<dyn std::error::Error>> {
    stage_cli_assets(dest)?;
    let root = repo_root();
    for (src_rel, dst_rel) in [
        ("uccd/uccd.service", "lib/systemd/system/uccd.service"),
        (
            "uccd/uccd-sleep.service",
            "lib/systemd/system/uccd-sleep.service",
        ),
        (
            "uccd/com.uniwill.uccd.conf",
            "share/dbus-1/system.d/com.uniwill.uccd.conf",
        ),
        (
            "uccd/com.uniwill.uccd.service",
            "share/dbus-1/system-services/com.uniwill.uccd.service",
        ),
        (
            "uccd/com.uniwill.uccd.policy",
            "share/polkit-1/actions/com.uniwill.uccd.policy",
        ),
    ] {
        let dst = dest.join(dst_rel);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(root.join(src_rel), dst)?;
    }
    let sleep_alias = dest.join("lib/systemd/system/uccd-pre-sleep.service");
    match fs::remove_file(&sleep_alias) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    std::os::unix::fs::symlink("uccd-sleep.service", sleep_alias)?;
    stage_release_metadata(&root, dest)?;
    Ok(())
}

fn json_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

fn parse_prepare_deployment_args(args: &[String]) -> Result<PrepareDeploymentArgs, String> {
    let mut build_dir = None;
    let mut data_dir = None;
    let mut tcc_dir = PathBuf::from("/etc/tcc");
    let mut tcc_archive = PathBuf::from("/opt/tuxedo-control-center/resources/app.asar");
    let mut normal_tests = None;
    let mut preview_tests = None;
    let mut sysfs_leds = PathBuf::from("/sys/class/leds");
    let mut allow_gui_audit_failure = false;
    let mut output_dir = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--build" => {
                i += 1;
                build_dir = Some(PathBuf::from(args.get(i).ok_or("--build requires a path")?));
            }
            "--data" => {
                i += 1;
                data_dir = Some(PathBuf::from(args.get(i).ok_or("--data requires a path")?));
            }
            "--tcc-dir" => {
                i += 1;
                tcc_dir = PathBuf::from(args.get(i).ok_or("--tcc-dir requires a path")?);
            }
            "--tcc-archive" => {
                i += 1;
                tcc_archive = PathBuf::from(args.get(i).ok_or("--tcc-archive requires a path")?);
            }
            "--normal-tests" => {
                i += 1;
                normal_tests = Some(PathBuf::from(
                    args.get(i).ok_or("--normal-tests requires a path")?,
                ));
            }
            "--preview-tests" => {
                i += 1;
                preview_tests = Some(PathBuf::from(
                    args.get(i).ok_or("--preview-tests requires a path")?,
                ));
            }
            "--sysfs-leds" => {
                i += 1;
                sysfs_leds = PathBuf::from(args.get(i).ok_or("--sysfs-leds requires a path")?);
            }
            "--allow-gui-audit-failure" => allow_gui_audit_failure = true,
            other if other.starts_with('-') => {
                return Err(format!("Unknown prepare-deployment flag: {other}"));
            }
            other => {
                if output_dir.is_some() {
                    return Err(format!("Unexpected extra argument: {other}"));
                }
                output_dir = Some(PathBuf::from(other));
            }
        }
        i += 1;
    }

    Ok(PrepareDeploymentArgs {
        build_dir: build_dir.ok_or("--build is required")?,
        data_dir: data_dir.ok_or("--data is required")?,
        tcc_dir,
        tcc_archive,
        normal_tests: normal_tests.ok_or("--normal-tests is required")?,
        preview_tests: preview_tests.ok_or("--preview-tests is required")?,
        sysfs_leds,
        allow_gui_audit_failure,
        output_dir: output_dir.ok_or("output directory is required")?,
    })
}

fn run_deploy_cli(args: &[String], runner: &SystemCommandRunner) -> Result<(), String> {
    let mut install: Option<PathBuf> = None;
    let mut rollback: Option<PathBuf> = None;
    let mut root = PathBuf::from("/");

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--install" => {
                i += 1;
                install = Some(PathBuf::from(
                    args.get(i).ok_or("--install requires a path")?,
                ));
            }
            "--rollback" => {
                i += 1;
                rollback = Some(PathBuf::from(
                    args.get(i).ok_or("--rollback requires a path")?,
                ));
            }
            "--root" => {
                i += 1;
                root = PathBuf::from(args.get(i).ok_or("--root requires a path")?);
            }
            other => return Err(format!("Unknown deploy argument: {other}")),
        }
        i += 1;
    }

    if install.is_some() == rollback.is_some() {
        return Err("Specify exactly one of --install <bundle> or --rollback <backup>".to_string());
    }

    if root == Path::new("/") {
        let euid_root = fs::read_to_string("/proc/self/status").ok().and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Uid:"))
                .and_then(|l| l.split_whitespace().nth(2))
                .and_then(|u| u.parse::<u32>().ok())
        }) == Some(0);
        if !euid_root {
            return Err("System installation requires root (use pkexec)".to_string());
        }
    }

    if let Some(bundle) = install {
        let mut mgr = DeploymentManager::new(&bundle, &root, runner);
        let backup = mgr.install()?;
        println!(
            "{}",
            serde_json::json!({ "installed": true, "backup": backup.display().to_string() })
        );
    } else if let Some(backup) = rollback {
        let resolved = fs::canonicalize(&backup)
            .map_err(|e| format!("Failed resolving backup {}: {e}", backup.display()))?;
        let mut mgr = DeploymentManager::from_backup(&resolved, &root, runner)?;
        mgr.rollback()?;
        println!(
            "{}",
            serde_json::json!({ "rolledBack": true, "backup": resolved.display().to_string() })
        );
    }
    Ok(())
}

fn run_desktop_transition_cli(args: &[String], runner: &SystemCommandRunner) -> Result<(), String> {
    let mut positional = Vec::new();
    let mut source: Option<PathBuf> = None;
    let mut preferences: Option<PathBuf> = None;
    let mut home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--source" => {
                i += 1;
                source = Some(PathBuf::from(
                    args.get(i).ok_or("--source requires a path")?,
                ));
            }
            "--preferences" => {
                i += 1;
                preferences = Some(PathBuf::from(
                    args.get(i).ok_or("--preferences requires a path")?,
                ));
            }
            "--home" => {
                i += 1;
                home = PathBuf::from(args.get(i).ok_or("--home requires a path")?);
            }
            other if other.starts_with('-') => {
                return Err(format!("Unknown desktop-transition flag: {other}"));
            }
            other => positional.push(other.to_string()),
        }
        i += 1;
    }

    if positional.len() != 2 {
        return Err(
            "Usage: cargo xtask maintenance desktop-transition <quiesce|activate|restore> <state-dir> [--source <dir>] [--preferences <file>] [--home <dir>]"
                .to_string(),
        );
    }
    let action = positional[0].as_str();
    let state_dir = PathBuf::from(&positional[1]);
    let transition = DesktopTransition::new(&state_dir, &home, runner);
    match action {
        "quiesce" => {
            let src = source.ok_or("--source is required for quiesce")?;
            let state = transition.quiesce(&src)?;
            println!("{}", serde_json::to_string_pretty(&state).unwrap());
        }
        "activate" => {
            let prefs = preferences.ok_or("--preferences is required for activate")?;
            let state = transition.activate(&prefs)?;
            println!("{}", serde_json::to_string_pretty(&state).unwrap());
        }
        "restore" => {
            let state = transition.restore()?;
            println!("{}", serde_json::to_string_pretty(&state).unwrap());
        }
        other => return Err(format!("Unknown desktop-transition action: {other}")),
    }
    Ok(())
}

fn parse_capture_gallery_args(
    args: &[String],
    repo_root: &Path,
) -> Result<CaptureGalleryArgs, String> {
    let mut build_dir = None;
    let mut output_dir = repo_root.join("screenshots/current");

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--build" => {
                i += 1;
                build_dir = Some(PathBuf::from(args.get(i).ok_or("--build requires a path")?));
            }
            "--output" => {
                i += 1;
                output_dir = PathBuf::from(args.get(i).ok_or("--output requires a path")?);
            }
            other => return Err(format!("Unknown capture-gallery argument: {other}")),
        }
        i += 1;
    }

    Ok(CaptureGalleryArgs {
        build_dir: build_dir.ok_or("--build is required")?,
        output_dir,
    })
}

struct LockPkg {
    name: String,
    version: String,
    source: Option<String>,
    checksum: Option<String>,
}

fn parse_cargo_lock_packages(lock_content: &str) -> Vec<LockPkg> {
    let mut pkgs = Vec::new();
    let mut current: Option<LockPkg> = None;
    let extract_quoted = |line: &str, key: &str| -> Option<String> {
        line.strip_prefix(key)
            .and_then(|rest| rest.trim().strip_prefix('"'))
            .and_then(|rest| rest.strip_suffix('"'))
            .map(ToString::to_string)
    };
    for raw in lock_content.lines() {
        let line = raw.trim();
        if line == "[[package]]" {
            if let Some(pkg) = current.take() {
                if !pkg.name.is_empty() && !pkg.version.is_empty() {
                    pkgs.push(pkg);
                }
            }
            current = Some(LockPkg {
                name: String::new(),
                version: String::new(),
                source: None,
                checksum: None,
            });
            continue;
        }
        if let Some(ref mut pkg) = current {
            if let Some(v) = extract_quoted(line, "name = ") {
                pkg.name = v;
            } else if let Some(v) = extract_quoted(line, "version = ") {
                pkg.version = v;
            } else if let Some(v) = extract_quoted(line, "source = ") {
                pkg.source = Some(v);
            } else if let Some(v) = extract_quoted(line, "checksum = ") {
                pkg.checksum = Some(v);
            }
        }
    }
    if let Some(pkg) = current {
        if !pkg.name.is_empty() && !pkg.version.is_empty() {
            pkgs.push(pkg);
        }
    }
    pkgs
}

pub fn generate_sbom_json(
    root: &Path,
    app_version: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let lock_raw = fs::read_to_string(root.join("Cargo.lock"))?;
    let pkgs = parse_cargo_lock_packages(&lock_raw);
    let mut out = String::from("{\n  \"bomFormat\": \"CycloneDX\",\n  \"specVersion\": \"1.5\",\n  \"metadata\": {\n    \"component\": {\n      \"type\": \"application\",\n      \"name\": \"ucc-rusted\",\n      \"version\": \"");
    out.push_str(&json_escape(app_version));
    out.push_str("\"\n    }\n  },\n  \"components\": [\n");
    for (idx, pkg) in pkgs.iter().enumerate() {
        let comma = if idx + 1 < pkgs.len() { "," } else { "" };
        let purl = format!("pkg:cargo/{}@{}", pkg.name, pkg.version);
        let source_line = pkg
            .source
            .as_ref()
            .map(|s| format!(", \"description\": \"{}\"", json_escape(s)))
            .unwrap_or_default();
        let hash_line = pkg
            .checksum
            .as_ref()
            .map(|h| {
                format!(
                    ", \"hashes\": [{{\"alg\": \"SHA-256\", \"content\": \"{}\"}}]",
                    json_escape(h)
                )
            })
            .unwrap_or_default();
        out.push_str(&format!(
            "    {{\"type\": \"library\", \"name\": \"{}\", \"version\": \"{}\", \"purl\": \"{}\"{source_line}{hash_line}}}{comma}\n",
            json_escape(&pkg.name),
            json_escape(&pkg.version),
            json_escape(&purl),
        ));
    }
    out.push_str("  ]\n}\n");
    Ok(out)
}

fn current_git_short_sha(root: &Path) -> String {
    if let Ok(env_commit) = std::env::var("UCC_BUILD_COMMIT") {
        let trimmed = env_commit.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if let Some(git_sha) = std::process::Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
    {
        return git_sha;
    }
    if let Ok(file_sha) = fs::read_to_string(root.join(".release-commit")) {
        let trimmed = file_sha.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    "unknown".to_string()
}

fn stage_release_metadata(root: &Path, dest: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let cargo_raw = fs::read_to_string(root.join("Cargo.toml"))?;
    let version = parse_manifest_version(ManifestKind::CargoToml, &cargo_raw)?;
    let commit = current_git_short_sha(root);
    let doc_dir = dest.join("share/doc/ucc");
    fs::create_dir_all(&doc_dir)?;

    let mut manifest = format!(
        "{{\n  \"package\": \"ucc-rusted\",\n  \"version\": \"{}\",\n  \"commit\": \"{}\",\n  \"binaries\": [\n",
        json_escape(&version),
        json_escape(&commit)
    );
    for (idx, (name, subdir)) in RUNTIME_BINARIES.iter().enumerate() {
        let rel = format!("{subdir}/{name}");
        let bytes = fs::read(dest.join(&rel))?;
        let digest = sha256_hex(&bytes);
        let comma = if idx + 1 < RUNTIME_BINARIES.len() {
            ","
        } else {
            ""
        };
        manifest.push_str(&format!(
            "    {{\"path\": \"{}\", \"bytes\": {}, \"sha256\": \"{}\"}}{comma}\n",
            json_escape(&rel),
            bytes.len(),
            digest
        ));
    }
    manifest.push_str("  ]\n}\n");
    fs::write(doc_dir.join("RELEASE_MANIFEST.json"), manifest)?;

    let sbom = generate_sbom_json(root, &version)?;
    fs::write(doc_dir.join("SBOM.json"), sbom)?;
    Ok(())
}

fn install_runtime(dest: &Path, bins: &Path) -> io::Result<()> {
    for (name, _) in RUNTIME_BINARIES {
        let src = bins.join(name);
        if !src
            .metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        {
            return Err(io::Error::other(format!(
                "required executable missing or not executable: {}",
                src.display()
            )));
        }
    }
    for (name, subdir) in RUNTIME_BINARIES {
        let out = dest.join(subdir);
        fs::create_dir_all(&out)?;
        fs::copy(bins.join(name), out.join(name))?;
    }
    Ok(())
}

fn generate_all_completions(out_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(out_dir)?;

    let shells = [
        (Shell::Bash, "ucc-cli.bash"),
        (Shell::Zsh, "_ucc-cli"),
        (Shell::Fish, "ucc-cli.fish"),
    ];

    for (shell, filename) in shells {
        let path = out_dir.join(filename);
        let mut file = fs::File::create(&path)?;
        ucc_cli::cli::generate_completions(shell, &mut file);
        println!("Generated completion: {}", path.display());
    }

    Ok(())
}

fn generate_all_manpages(out_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(out_dir)?;

    let path = out_dir.join("ucc-cli.1");
    let mut file = fs::File::create(&path)?;
    ucc_cli::cli::generate_manpage(&mut file)?;
    println!("Generated man page: {}", path.display());

    let daemon_man = out_dir.join("uccd.8");
    fs::copy(repo_root().join("uccd/uccd.8"), &daemon_man)?;
    println!("Staged man page: {}", daemon_man.display());

    Ok(())
}

fn build_fixtures() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let root = repo_root();
    let base_dir = std::env::var_os("UCC_FIXTURES_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("ucc-fixtures-{}", std::process::id()))
        });

    let stand_dir = base_dir.join("stand");
    let extra_dir = base_dir.join("extra");
    fs::create_dir_all(&stand_dir)?;
    fs::create_dir_all(extra_dir.join("index"))?;
    fs::create_dir_all(extra_dir.join("bdf"))?;

    let stand_src = root.join("stand-fixtures");
    run_cmd(
        "cc",
        [
            OsString::from("-shared"),
            OsString::from("-fPIC"),
            stand_src.join("nvml.c").into_os_string(),
            OsString::from("-o"),
            stand_dir.join("libnvidia-ml.so.1").into_os_string(),
        ],
    )?;
    run_cmd(
        "cc",
        [
            OsString::from("-shared"),
            OsString::from("-fPIC"),
            stand_src.join("open-swap.c").into_os_string(),
            OsString::from("-ldl"),
            OsString::from("-o"),
            stand_dir.join("open-swap.so").into_os_string(),
        ],
    )?;
    run_cmd(
        "cc",
        [
            OsString::from("-shared"),
            OsString::from("-fPIC"),
            stand_src.join("open-ancestor.c").into_os_string(),
            OsString::from("-ldl"),
            OsString::from("-o"),
            stand_dir.join("open-ancestor.so").into_os_string(),
        ],
    )?;

    let extra_src = root.join("extra-fixtures");
    run_cmd(
        "cc",
        [
            OsString::from("-shared"),
            OsString::from("-fPIC"),
            extra_src.join("nvml.c").into_os_string(),
            OsString::from("-o"),
            extra_dir.join("index/libnvidia-ml.so.1").into_os_string(),
        ],
    )?;
    run_cmd(
        "cc",
        [
            OsString::from("-DAUDIT_WITH_BDF"),
            OsString::from("-shared"),
            OsString::from("-fPIC"),
            extra_src.join("nvml.c").into_os_string(),
            OsString::from("-o"),
            extra_dir.join("bdf/libnvidia-ml.so.1").into_os_string(),
        ],
    )?;
    run_cmd(
        "cc",
        [
            OsString::from("-shared"),
            OsString::from("-fPIC"),
            extra_src.join("open-ancestor.c").into_os_string(),
            OsString::from("-ldl"),
            OsString::from("-o"),
            extra_dir.join("open-ancestor.so").into_os_string(),
        ],
    )?;

    Ok(base_dir)
}

fn run_cmd<I, S>(prog: &str, args: I) -> Result<(), Box<dyn std::error::Error>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let collected: Vec<OsString> = args
        .into_iter()
        .map(|s| s.as_ref().to_os_string())
        .collect();
    let status = std::process::Command::new(prog).args(&collected).status()?;
    if !status.success() {
        return Err(format!("Command '{prog}' failed with status {status}").into());
    }
    Ok(())
}

/// Validates strict SemVer `MAJOR.MINOR.PATCH` with optional `-prerelease` identifier.
pub fn validate_semver(version: &str) -> Result<(), String> {
    if version.is_empty() || version.starts_with('v') {
        return Err(format!(
            "Invalid SemVer '{version}': must not be empty or start with 'v'"
        ));
    }
    let (core, pre) = match version.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (version, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() != 3 {
        return Err(format!(
            "Invalid SemVer '{version}': expected MAJOR.MINOR.PATCH"
        ));
    }
    for part in parts {
        if part.is_empty()
            || !part.bytes().all(|b| b.is_ascii_digit())
            || (part.len() > 1 && part.starts_with('0'))
        {
            return Err(format!(
                "Invalid SemVer '{version}': invalid numeric component '{part}'"
            ));
        }
    }
    if let Some(pre_tag) = pre {
        if pre_tag.is_empty()
            || !pre_tag.split('.').all(|seg| {
                !seg.is_empty() && seg.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
        {
            return Err(format!(
                "Invalid SemVer '{version}': invalid prerelease suffix '{pre_tag}'"
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum ManifestKind {
    CargoToml,
    CMakeLists,
    Pkgbuild,
    RpmSpec,
    PackageNix,
    DebianChangelog,
}

const VERSIONED_MANIFESTS: [(&str, ManifestKind); 6] = [
    ("Cargo.toml", ManifestKind::CargoToml),
    ("CMakeLists.txt", ManifestKind::CMakeLists),
    ("PKGBUILD", ManifestKind::Pkgbuild),
    ("ucc.spec", ManifestKind::RpmSpec),
    ("package.nix", ManifestKind::PackageNix),
    ("debian/changelog", ManifestKind::DebianChangelog),
];

fn parse_manifest_version(kind: ManifestKind, content: &str) -> Result<String, String> {
    match kind {
        ManifestKind::CargoToml => {
            let mut in_workspace_pkg = false;
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with('[') {
                    in_workspace_pkg = trimmed == "[workspace.package]";
                    continue;
                }
                if in_workspace_pkg && trimmed.starts_with("version = \"") {
                    if let Some(val) = trimmed
                        .strip_prefix("version = \"")
                        .and_then(|s| s.strip_suffix('"'))
                    {
                        return Ok(val.to_string());
                    }
                }
            }
            Err("Missing [workspace.package] version in Cargo.toml".to_string())
        }
        ManifestKind::CMakeLists => {
            let mut in_project = false;
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("project(") {
                    in_project = true;
                    continue;
                }
                if in_project {
                    if let Some(rest) = trimmed.strip_prefix("VERSION ") {
                        return Ok(rest.trim().to_string());
                    }
                    if trimmed.contains(')') {
                        break;
                    }
                }
            }
            Err("Missing project(ucc VERSION ...) in CMakeLists.txt".to_string())
        }
        ManifestKind::Pkgbuild => {
            for line in content.lines() {
                if let Some(rest) = line.strip_prefix("pkgver=") {
                    return Ok(rest.trim().to_string());
                }
            }
            Err("Missing pkgver= in PKGBUILD".to_string())
        }
        ManifestKind::RpmSpec => {
            for line in content.lines() {
                if let Some(rest) = line.strip_prefix("Version:") {
                    return Ok(rest.trim().to_string());
                }
            }
            Err("Missing Version: in ucc.spec".to_string())
        }
        ManifestKind::PackageNix => {
            for line in content.lines() {
                if let Some(pos) = line.find("version ? \"") {
                    let rest = &line[pos + "version ? \"".len()..];
                    if let Some(ver) = rest.split('"').next() {
                        return Ok(ver.to_string());
                    }
                }
            }
            Err("Missing version parameter in package.nix".to_string())
        }
        ManifestKind::DebianChangelog => {
            let first = content.lines().next().unwrap_or("");
            if let Some(open) = first.find('(') {
                if let Some(close) = first[open + 1..].find(')') {
                    return Ok(first[open + 1..open + 1 + close].trim().to_string());
                }
            }
            Err("Missing top version entry in debian/changelog".to_string())
        }
    }
}

fn rewrite_manifest_version(
    kind: ManifestKind,
    content: &str,
    new_version: &str,
) -> Result<String, String> {
    let trailing_nl = content.ends_with('\n');
    let mut lines: Vec<String> = content.lines().map(ToString::to_string).collect();
    match kind {
        ManifestKind::CargoToml => {
            let mut in_workspace_pkg = false;
            let mut replaced = false;
            for line in &mut lines {
                let trimmed = line.trim();
                if trimmed.starts_with('[') {
                    in_workspace_pkg = trimmed == "[workspace.package]";
                    continue;
                }
                if in_workspace_pkg && trimmed.starts_with("version = \"") {
                    *line = format!("version = \"{new_version}\"");
                    replaced = true;
                    break;
                }
            }
            if !replaced {
                return Err("Could not update version in Cargo.toml".to_string());
            }
        }
        ManifestKind::CMakeLists => {
            let mut in_project = false;
            let mut replaced = false;
            for line in &mut lines {
                let trimmed = line.trim();
                if trimmed.starts_with("project(") {
                    in_project = true;
                    continue;
                }
                if in_project {
                    if trimmed.starts_with("VERSION ") {
                        let indent: String =
                            line.chars().take_while(|c| c.is_whitespace()).collect();
                        *line = format!("{indent}VERSION {new_version}");
                        replaced = true;
                        break;
                    }
                    if trimmed.contains(')') {
                        break;
                    }
                }
            }
            if !replaced {
                return Err("Could not update VERSION in CMakeLists.txt".to_string());
            }
        }
        ManifestKind::Pkgbuild => {
            let mut found_ver = false;
            for line in &mut lines {
                if line.starts_with("pkgver=") {
                    *line = format!("pkgver={new_version}");
                    found_ver = true;
                } else if line.starts_with("pkgrel=") {
                    *line = "pkgrel=1".to_string();
                }
            }
            if !found_ver {
                return Err("Could not update pkgver= in PKGBUILD".to_string());
            }
        }
        ManifestKind::RpmSpec => {
            let mut found_ver = false;
            for line in &mut lines {
                if line.starts_with("Version:") {
                    *line = format!("Version:        {new_version}");
                    found_ver = true;
                } else if line.starts_with("Release:") {
                    *line = "Release:        1%{?dist}".to_string();
                }
            }
            if !found_ver {
                return Err("Could not update Version: in ucc.spec".to_string());
            }
        }
        ManifestKind::PackageNix => {
            let mut found = false;
            for line in &mut lines {
                if let Some(pos) = line.find("version ? \"") {
                    let prefix = &line[..pos + "version ? \"".len()];
                    let rest = &line[pos + "version ? \"".len()..];
                    if let Some(end_q) = rest.find('"') {
                        let suffix = &rest[end_q..];
                        *line = format!("{prefix}{new_version}{suffix}");
                        found = true;
                        break;
                    }
                }
            }
            if !found {
                return Err("Could not update version in package.nix".to_string());
            }
        }
        ManifestKind::DebianChangelog => {
            let current = parse_manifest_version(ManifestKind::DebianChangelog, content)?;
            if current == new_version {
                return Ok(content.to_string());
            }
            let rfc_date = current_rfc2822_date();
            let entry = vec![
                format!("ucc ({new_version}) unstable; urgency=medium"),
                String::new(),
                format!("  * Release v{new_version}."),
                String::new(),
                format!(" -- Frixit <Frixlt@users.noreply.github.com>  {rfc_date}"),
                String::new(),
            ];
            let mut combined = entry;
            combined.extend(lines);
            lines = combined;
        }
    }
    let mut out = lines.join("\n");
    if trailing_nl {
        out.push('\n');
    }
    Ok(out)
}

fn current_rfc2822_date() -> String {
    if let Ok(output) = std::process::Command::new("date").arg("-R").output() {
        if output.status.success() {
            let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    "Thu, 01 Jan 1970 00:00:00 +0000".to_string()
}

pub fn read_versions_in_root(
    root: &Path,
) -> Result<Vec<(&'static str, String)>, Box<dyn std::error::Error>> {
    let mut result = Vec::with_capacity(VERSIONED_MANIFESTS.len());
    for (rel, kind) in VERSIONED_MANIFESTS {
        let content = fs::read_to_string(root.join(rel))?;
        let ver = parse_manifest_version(kind, &content)?;
        validate_semver(&ver)?;
        result.push((rel, ver));
    }
    Ok(result)
}

pub fn check_versions_in_root(
    root: &Path,
) -> Result<Vec<(&'static str, String)>, Box<dyn std::error::Error>> {
    let versions = read_versions_in_root(root)?;
    let expected = &versions[0].1;
    let mismatches: Vec<String> = versions
        .iter()
        .filter(|(_, v)| v != expected)
        .map(|(file, v)| format!("{file}={v}"))
        .collect();
    if !mismatches.is_empty() {
        return Err(format!(
            "Version mismatch across manifests (expected {expected}): {}",
            mismatches.join(", ")
        )
        .into());
    }
    Ok(versions)
}

pub fn bump_versions_in_root(
    root: &Path,
    new_version: &str,
    dry_run: bool,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    validate_semver(new_version)?;
    let mut updated = Vec::with_capacity(VERSIONED_MANIFESTS.len());
    for (rel, kind) in VERSIONED_MANIFESTS {
        let path = root.join(rel);
        let original = fs::read_to_string(&path)?;
        let rewritten = rewrite_manifest_version(kind, &original, new_version)?;
        if !dry_run && rewritten != original {
            fs::write(&path, rewritten)?;
        }
        updated.push(path);
    }
    Ok(updated)
}

fn maybe_run_git_cliff(root: &Path, version: &str) {
    let tag = format!("v{version}");
    match std::process::Command::new("git-cliff")
        .current_dir(root)
        .args(["--tag", &tag, "-o", "CHANGELOG.md"])
        .status()
    {
        Ok(status) if status.success() => {
            println!("Updated CHANGELOG.md with git-cliff ({tag})");
        }
        Ok(status) => {
            eprintln!(
                "git-cliff exited with status {status}; update CHANGELOG.md manually if needed"
            );
        }
        Err(_) => {
            println!("git-cliff not installed on PATH; skipping automatic CHANGELOG.md update");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_semver_accepts_and_rejects_correctly() {
        assert!(validate_semver("0.3.0").is_ok());
        assert!(validate_semver("1.0.0").is_ok());
        assert!(validate_semver("0.4.0-rc.1").is_ok());
        assert!(validate_semver("v0.3.0").is_err());
        assert!(validate_semver("0.3").is_err());
        assert!(validate_semver("01.2.3").is_err());
        assert!(validate_semver("0.3.0-").is_err());
        assert!(validate_semver("").is_err());
    }

    #[test]
    fn live_repo_manifests_have_matching_versions() {
        let versions = check_versions_in_root(&repo_root())
            .expect("all 6 repo manifests must match on valid SemVer");
        assert_eq!(versions.len(), 6);
    }

    #[test]
    fn bump_versions_dry_run_and_write_are_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("debian")).unwrap();

        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"0.3.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::write(
            root.join("CMakeLists.txt"),
            "project( ucc\n  VERSION 0.3.0\n  LANGUAGES CXX\n)\n",
        )
        .unwrap();
        fs::write(
            root.join("PKGBUILD"),
            "pkgname=ucc-rusted\npkgver=0.3.0\npkgrel=4\n",
        )
        .unwrap();
        fs::write(
            root.join("ucc.spec"),
            "Name:           ucc\nVersion:        0.3.0\nRelease:        3%{?dist}\n",
        )
        .unwrap();
        fs::write(
            root.join("package.nix"),
            "{ stdenv, version ? \"0.3.0\", }:\nstdenv.mkDerivation { pname = \"ucc\"; }\n",
        )
        .unwrap();
        fs::write(
            root.join("debian/changelog"),
            "ucc (0.3.0) unstable; urgency=medium\n\n  * Initial.\n\n -- Frixit <Frixlt@users.noreply.github.com>  Thu, 22 Oct 2026 22:00:00 +0300\n",
        )
        .unwrap();

        // Dry run leaves 0.3.0 intact
        bump_versions_in_root(root, "0.4.0", true).unwrap();
        assert_eq!(check_versions_in_root(root).unwrap()[0].1, "0.3.0");

        // Actual bump updates all 6 manifests and resets pkgrel/Release
        bump_versions_in_root(root, "0.4.0", false).unwrap();
        let bumped = check_versions_in_root(root).unwrap();
        assert!(bumped.iter().all(|(_, v)| v == "0.4.0"));
        assert!(fs::read_to_string(root.join("PKGBUILD"))
            .unwrap()
            .contains("pkgrel=1"));
        assert!(fs::read_to_string(root.join("ucc.spec"))
            .unwrap()
            .contains("Release:        1%{?dist}"));

        // Idempotent re-run does not duplicate debian/changelog entry
        let changelog_once = fs::read_to_string(root.join("debian/changelog")).unwrap();
        bump_versions_in_root(root, "0.4.0", false).unwrap();
        let changelog_twice = fs::read_to_string(root.join("debian/changelog")).unwrap();
        assert_eq!(changelog_once, changelog_twice);
    }

    #[test]
    fn dist_emits_release_manifest_and_cyclonedx_sbom() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let sbom = generate_sbom_json(&repo_root(), "0.3.0").unwrap();
        assert!(sbom.contains("\"bomFormat\": \"CycloneDX\""));
        assert!(sbom.contains("\"name\": \"ucc-rusted\""));
        assert!(sbom.contains("\"purl\": \"pkg:cargo/zbus@"));

        let tmp = tempfile::tempdir().unwrap();
        let bins = tmp.path().join("bins");
        let dest = tmp.path().join("dist");
        fs::create_dir_all(&bins).unwrap();
        for (name, _) in RUNTIME_BINARIES {
            let f = bins.join(name);
            fs::write(&f, format!("bin:{name}")).unwrap();
            fs::set_permissions(&f, fs::Permissions::from_mode(0o755)).unwrap();
        }
        install_runtime(&dest, &bins).unwrap();
        stage_dist_assets(&dest).unwrap();

        let manifest =
            fs::read_to_string(dest.join("share/doc/ucc/RELEASE_MANIFEST.json")).unwrap();
        assert!(manifest.contains("\"package\": \"ucc-rusted\""));
        assert!(manifest.contains("lib/ucc/ucc-sleep"));
        assert!(dest.join("share/doc/ucc/SBOM.json").is_file());
    }

    #[test]
    fn commit_sha_falls_back_to_release_commit_file_when_git_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let outside_git = tmp.path();
        fs::write(outside_git.join(".release-commit"), "abc1234\n").unwrap();
        assert_eq!(current_git_short_sha(outside_git), "abc1234");
    }
}
