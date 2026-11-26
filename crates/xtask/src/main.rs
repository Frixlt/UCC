//! Workspace automation tasks for building assets, completions, manpages, and test fixtures.

use clap_complete::Shell;
use std::fs;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let task = args.next().unwrap_or_else(|| "help".to_string());

    match task.as_str() {
        "dist-cli" | "assets" => {
            generate_all_completions("target/completions")?;
            generate_all_manpages("target/man")?;
            println!("CLI assets successfully generated in target/completions and target/man");
        }
        "completions" => {
            let out_dir = args.next().unwrap_or_else(|| "target/completions".to_string());
            generate_all_completions(&out_dir)?;
            println!("Shell completions generated in {}", out_dir);
        }
        "manpages" => {
            let out_dir = args.next().unwrap_or_else(|| "target/man".to_string());
            generate_all_manpages(&out_dir)?;
            println!("Man pages generated in {}", out_dir);
        }
        "build-fixtures" | "fixtures" => {
            build_fixtures()?;
            println!("Test fixtures successfully compiled to /tmp/ucc-stand-fixtures and /tmp/ucc-extra-fixtures");
        }
        _ => {
            eprintln!("Usage: cargo xtask <dist-cli|completions|manpages|build-fixtures>");
            std::process::exit(1);
        }
    }

    Ok(())
}

fn generate_all_completions(out_dir_str: &str) -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = PathBuf::from(out_dir_str);
    fs::create_dir_all(&out_dir)?;

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

fn generate_all_manpages(out_dir_str: &str) -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = PathBuf::from(out_dir_str);
    fs::create_dir_all(&out_dir)?;

    let path = out_dir.join("ucc-cli.1");
    let mut file = fs::File::create(&path)?;
    ucc_cli::cli::generate_manpage(&mut file)?;
    println!("Generated man page: {}", path.display());

    Ok(())
}

fn build_fixtures() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(&manifest_dir);

    let stand_dir = PathBuf::from("/tmp/ucc-stand-fixtures");
    let extra_dir = PathBuf::from("/tmp/ucc-extra-fixtures");
    fs::create_dir_all(&stand_dir)?;
    fs::create_dir_all(extra_dir.join("index"))?;
    fs::create_dir_all(extra_dir.join("bdf"))?;

    // 1. stand-fixtures
    let stand_src = root.join("stand-fixtures");
    run_cmd(
        "cc",
        &[
            "-shared",
            "-fPIC",
            stand_src.join("nvml.c").to_str().unwrap(),
            "-o",
            stand_dir.join("libnvidia-ml.so.1").to_str().unwrap(),
        ],
    )?;
    run_cmd(
        "cc",
        &[
            "-shared",
            "-fPIC",
            stand_src.join("open-swap.c").to_str().unwrap(),
            "-ldl",
            "-o",
            stand_dir.join("open-swap.so").to_str().unwrap(),
        ],
    )?;
    run_cmd(
        "cc",
        &[
            "-shared",
            "-fPIC",
            stand_src.join("open-ancestor.c").to_str().unwrap(),
            "-ldl",
            "-o",
            stand_dir.join("open-ancestor.so").to_str().unwrap(),
        ],
    )?;

    // 2. extra-fixtures
    let extra_src = root.join("extra-fixtures");
    run_cmd(
        "cc",
        &[
            "-shared",
            "-fPIC",
            extra_src.join("nvml.c").to_str().unwrap(),
            "-o",
            extra_dir.join("index/libnvidia-ml.so.1").to_str().unwrap(),
        ],
    )?;
    run_cmd(
        "cc",
        &[
            "-DAUDIT_WITH_BDF",
            "-shared",
            "-fPIC",
            extra_src.join("nvml.c").to_str().unwrap(),
            "-o",
            extra_dir.join("bdf/libnvidia-ml.so.1").to_str().unwrap(),
        ],
    )?;
    run_cmd(
        "cc",
        &[
            "-shared",
            "-fPIC",
            extra_src.join("open-ancestor.c").to_str().unwrap(),
            "-ldl",
            "-o",
            extra_dir.join("open-ancestor.so").to_str().unwrap(),
        ],
    )?;

    Ok(())
}

fn run_cmd(prog: &str, args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    let status = std::process::Command::new(prog).args(args).status()?;
    if !status.success() {
        return Err(format!(
            "Command '{prog} {}' failed with status {status}",
            args.join(" ")
        )
        .into());
    }
    Ok(())
}
