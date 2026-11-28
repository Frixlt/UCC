// SPDX-License-Identifier: GPL-3.0-or-later

//! UCC Daemon (uccd) entry point.
//!
//! Provides D-Bus object server for com.uniwill.uccd / de.uniwill.uccd with Polkit authentication,
//! hardware controllers, and power/thermal profile management.

use nix::libc;
use nix::sys::stat::fstat;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use ucc_dbus::PolkitAuthorizer;
use uccd::{DaemonRuntime, LiveUccdBackend};

const DAEMON_NAME: &str = "uccd";
const PRIMARY_PID_FILE: &str = "/run/uccd.pid";

fn resolve_pid_file() -> PathBuf {
    #[cfg(unix)]
    {
        use nix::unistd::{access, AccessFlags};
        if access("/run", AccessFlags::W_OK).is_ok() {
            return PathBuf::from(PRIMARY_PID_FILE);
        }
    }
    if let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        let runtime_path = PathBuf::from(runtime_dir);
        if runtime_path.is_absolute() {
            return runtime_path.join("uccd.pid");
        }
    }
    PathBuf::from(format!("/run/user/{}/uccd.pid", unsafe { libc::geteuid() }))
}

struct PidLock {
    file: File,
    path: PathBuf,
}

impl PidLock {
    fn acquire(path: &Path) -> Result<Self, String> {
        use std::os::unix::fs::MetadataExt;

        const MAX_ATTEMPTS: usize = 5;
        for _ in 0..MAX_ATTEMPTS {
            // Refuse to follow symlinks to protect external files
            if let Ok(meta) = fs::symlink_metadata(path) {
                if meta.file_type().is_symlink() {
                    return Err(format!("PID file {} is a symlink", path.display()));
                }
            }

            let mut opts = OpenOptions::new();
            opts.read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600);
            #[cfg(unix)]
            {
                opts.custom_flags(libc::O_NOFOLLOW);
            }

            let file = opts
                .open(path)
                .map_err(|e| format!("Failed to open/create PID file {}: {e}", path.display()))?;

            let fd = file.as_raw_fd();
            let st = fstat(fd)
                .map_err(|e| format!("Failed to stat PID file {}: {e}", path.display()))?;
            if (st.st_mode & libc::S_IFMT) != libc::S_IFREG
                || st.st_uid != unsafe { libc::geteuid() }
            {
                return Err(format!(
                    "PID file {} is not a regular file owned by current UID",
                    path.display()
                ));
            }
            let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
            if ret != 0 {
                let mut existing_pid = String::new();
                let mut f = &file;
                let _ = f.read_to_string(&mut existing_pid);
                return Err(format!(
                    "Daemon is already running with PID {}",
                    existing_pid.trim()
                ));
            }

            // Verify locked inode still matches the directory entry at `path` (`V2-P2-01`).
            match fs::symlink_metadata(path) {
                Ok(meta)
                    if !meta.file_type().is_symlink()
                        && meta.dev() == st.st_dev
                        && meta.ino() == st.st_ino =>
                {
                    let mut lock = Self {
                        file,
                        path: path.to_path_buf(),
                    };
                    lock.write_pid()?;
                    return Ok(lock);
                }
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(format!("PID file {} is a symlink", path.display()));
                }
                _ => {
                    unsafe {
                        libc::flock(fd, libc::LOCK_UN);
                    }
                    continue;
                }
            }
        }
        Err(format!(
            "Failed to acquire stable PID lock on {} after {} attempts",
            path.display(),
            MAX_ATTEMPTS
        ))
    }

    fn write_pid(&mut self) -> Result<(), String> {
        unsafe {
            libc::ftruncate(self.file.as_raw_fd(), 0);
            libc::lseek(self.file.as_raw_fd(), 0, libc::SEEK_SET);
        }
        let pid_str = format!("{}\n", std::process::id());
        self.file
            .write_all(pid_str.as_bytes())
            .map_err(|e| format!("Failed to write PID to file: {e}"))?;
        let _ = self.file.flush();
        Ok(())
    }
}

impl Drop for PidLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

fn stop_daemon(pid_path: &Path) -> i32 {
    if let Ok(meta) = fs::symlink_metadata(pid_path) {
        if meta.file_type().is_symlink() {
            eprintln!(
                "PID file {} is a symlink; refusing to stop.",
                pid_path.display()
            );
            return 1;
        }
    }

    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        opts.custom_flags(libc::O_NOFOLLOW);
    }

    let Ok(mut file) = opts.open(pid_path) else {
        eprintln!(
            "PID file not found ({}). Daemon may not be running.",
            pid_path.display()
        );
        return 1;
    };

    match fstat(file.as_raw_fd()) {
        Ok(st)
            if (st.st_mode & libc::S_IFMT) == libc::S_IFREG
                && st.st_uid == unsafe { libc::geteuid() } => {}
        _ => {
            eprintln!(
                "PID file {} failed ownership or regular-file verification; refusing to stop.",
                pid_path.display()
            );
            return 1;
        }
    }

    let mut pid_str = String::new();
    if file.read_to_string(&mut pid_str).is_err() {
        eprintln!("Failed to read PID file");
        return 1;
    }

    let Ok(pid) = pid_str.trim().parse::<i32>() else {
        eprintln!("Invalid PID in file: '{}'", pid_str.trim());
        return 1;
    };

    if pid <= 1 {
        eprintln!("Invalid PID ({pid}); refusing to send signal to system processes.");
        return 1;
    }

    // Check if running
    if unsafe { libc::kill(pid, 0) } != 0 {
        eprintln!("Daemon process (PID {pid}) is not running.");
        let _ = fs::remove_file(pid_path);
        return 0;
    }

    // Verify process identity to avoid signaling a recycled PID (fail closed on unreadable comm).
    match fs::read_to_string(format!("/proc/{pid}/comm")) {
        Ok(comm) => {
            let comm_trimmed = comm.trim();
            if comm_trimmed != DAEMON_NAME {
                eprintln!(
                    "PID {pid} comm ('{comm_trimmed}') does not match daemon name '{DAEMON_NAME}'"
                );
                return 1;
            }
        }
        Err(err) => {
            eprintln!("Failed to verify /proc/{pid}/comm identity: {err}; refusing to signal.");
            return 1;
        }
    }

    // Send SIGTERM
    println!("Sending SIGTERM to PID {pid}...");
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }

    // Wait up to 8 seconds (giving the daemon's 5s graceful shutdown budget headroom)
    for _ in 0..80 {
        if unsafe { libc::kill(pid, 0) } != 0 {
            println!("uccd daemon stopped.");
            let _ = fs::remove_file(pid_path);
            return 0;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    // Force kill if still running
    eprintln!("Daemon did not exit gracefully within 8s. Sending SIGKILL...");
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
    let _ = fs::remove_file(pid_path);
    println!("uccd daemon force-killed.");
    0
}

fn print_usage(prog: &str) {
    println!("Usage: {prog} [OPTIONS]");
    println!("Options:");
    println!("  -v, --version  Show version information");
    println!("  -h, --help     Show this help message");
    println!("      --debug    Run in debug mode (foreground with verbose logging)");
    println!("      --start    Start the daemon (default)");
    println!("      --stop     Stop the running daemon");
}

fn print_version() {
    println!("{} version {}", DAEMON_NAME, env!("CARGO_PKG_VERSION"));
    println!("Daemon for ucc-rusted (Uniwill/Clevo/TUXEDO Control Center)");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let mut debug_mode = false;
    let mut stop_daemon_flag = false;

    for arg in args.iter().skip(1) {
        match arg.as_str() {
            "-v" | "--version" => {
                print_version();
                return Ok(());
            }
            "-h" | "--help" => {
                print_usage(&args[0]);
                return Ok(());
            }
            "--debug" => {
                debug_mode = true;
            }
            "--start" => {}
            "--stop" => {
                stop_daemon_flag = true;
            }
            other => {
                eprintln!("Unknown argument: {other}");
                print_usage(&args[0]);
                std::process::exit(1);
            }
        }
    }

    let pid_path = resolve_pid_file();

    if stop_daemon_flag {
        let code = stop_daemon(&pid_path);
        std::process::exit(code);
    }

    // Initialize tracing
    let filter = if debug_mode {
        EnvFilter::new("debug,zbus=info")
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    info!(
        "{} starting - version {}",
        DAEMON_NAME,
        env!("CARGO_PKG_VERSION")
    );

    // Acquire exclusive PID lock
    let _lock = match PidLock::acquire(&pid_path) {
        Ok(l) => {
            info!("Acquired PID lock on {}", pid_path.display());
            l
        }
        Err(e) => {
            error!("{e}");
            std::process::exit(1);
        }
    };

    // Connect to D-Bus without acquiring com.uniwill.uccd yet: register objects BEFORE
    // requesting the well-known name so Type=dbus activation sees ready endpoints immediately (M8.2).
    // Fail closed when the system D-Bus is unavailable unless an explicit non-root test flag opts in.
    let allow_session_fallback = std::env::var("UCC_ALLOW_SESSION_BUS").as_deref() == Ok("1")
        && unsafe { libc::geteuid() } != 0;
    let (connection, bus_label) = match zbus::connection::Builder::system() {
        Ok(builder) => match builder.build().await {
            Ok(conn) => (conn, "system"),
            Err(e) if allow_session_fallback => {
                warn!("System D-Bus unavailable ({e}); using session bus because UCC_ALLOW_SESSION_BUS=1");
                (
                    zbus::connection::Builder::session()?.build().await?,
                    "session",
                )
            }
            Err(e) => {
                return Err(format!("System D-Bus unavailable: {e}").into());
            }
        },
        Err(e) if allow_session_fallback => {
            warn!("System D-Bus builder failed ({e}); using session bus because UCC_ALLOW_SESSION_BUS=1");
            (
                zbus::connection::Builder::session()?.build().await?,
                "session",
            )
        }
        Err(e) => {
            return Err(format!("System D-Bus builder failed: {e}").into());
        }
    };

    // Register SIGINT/SIGTERM BEFORE startup initialization so early termination during slow
    // hardware activation still performs bounded hardware cleanup and fan-auto restoration (`TA-03`).
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let (early_stop_tx, early_stop_rx) = tokio::sync::watch::channel(false);
    let signal_watcher = tokio::spawn(async move {
        tokio::select! {
            _ = sigint.recv() => {
                info!("Received SIGINT, initiating bounded shutdown");
            }
            _ = sigterm.recv() => {
                info!("Received SIGTERM, initiating bounded shutdown");
            }
        }
        let _ = early_stop_tx.send(true);
    });

    let backend = Arc::new(LiveUccdBackend::new_production());
    let authorizer: Arc<dyn ucc_dbus::Authorizer> = Arc::new(PolkitAuthorizer::new());

    let mut shutdown_wait = early_stop_rx.clone();
    let runtime = match DaemonRuntime::start_on_connection_cancellable(
        connection,
        "com.uniwill.uccd",
        backend,
        authorizer,
        true,
        async move {
            let _ = shutdown_wait.wait_for(|&stopped| stopped).await;
        },
    )
    .await?
    {
        Some(rt) => rt,
        None => {
            info!("uccd daemon stopped during startup initialization");
            return Ok(());
        }
    };
    info!(
        "Registered D-Bus interfaces on /com/uniwill/uccd and /de/uniwill/uccd ({bus_label} bus)"
    );

    let mut run_stop_rx = early_stop_rx;
    let _ = run_stop_rx.wait_for(|&stopped| stopped).await;
    let _ = signal_watcher.await;

    runtime.shutdown(Duration::from_secs(5)).await;
    info!("uccd daemon stopped");
    Ok(())
}
