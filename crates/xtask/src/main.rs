// SPDX-License-Identifier: GPL-3.0-or-later

//! Workspace automation tasks for building assets, completions, manpages, dist bundles, and test fixtures.

use clap_complete::Shell;
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::fs;
use std::path::{Path, PathBuf};

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
            let dest = args.next().ok_or("install-runtime requires a destination")?;
            let bins = args.next().ok_or("install-runtime requires a binary directory")?;
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
        _ => {
            eprintln!("Usage: cargo xtask <dist [dir [bin-dir]]|install-runtime <dir> <bin-dir>|dist-cli|completions|manpages|build-fixtures>");
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
        ("uccd/uccd-sleep.service", "lib/systemd/system/uccd-sleep.service"),
        ("uccd/com.uniwill.uccd.conf", "share/dbus-1/system.d/com.uniwill.uccd.conf"),
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
    Ok(())
}

fn install_runtime(dest: &Path, bins: &Path) -> io::Result<()> {
    for (name, _) in RUNTIME_BINARIES {
        let src = bins.join(name);
        if !src.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0) {
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
    let collected: Vec<OsString> = args.into_iter().map(|s| s.as_ref().to_os_string()).collect();
    let status = std::process::Command::new(prog).args(&collected).status()?;
    if !status.success() {
        return Err(format!("Command '{prog}' failed with status {status}").into());
    }
    Ok(())
}
