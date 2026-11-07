//! Workspace automation tasks for building assets, completions, and manpages.

use std::fs;
use std::path::PathBuf;
use clap_complete::Shell;

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
        _ => {
            eprintln!("Usage: cargo xtask <dist-cli|completions|manpages>");
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
