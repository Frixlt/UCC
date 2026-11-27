// SPDX-License-Identifier: GPL-3.0-or-later

//! Maintenance, deployment, offline import, desktop transition, gallery, and
//! parser-check workflows ported to pure Rust (`M11.1–M11.4`).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use ucc_config::{
    convert_offline_import, desktop_preferences, merge_tcc, read_tcc_fan_presets,
    write_offline_import_bundle, OfflineUserData,
};

use crate::sha256_hex;

pub const OLD_UNITS: [&str; 3] = ["tccd-sleep.service", "tccd.service", "tuxedo-power.service"];
pub const NEW_UNITS: [&str; 2] = ["uccd.service", "uccd-sleep.service"];
pub const PACKAGE_NAME: &str = "ucc-rusted";

pub const DESKTOP_FILES: [&str; 3] = [
    ".config/autostart/control-center.desktop",
    ".config/UniwillControlCenter/desktop.conf",
    ".config/UniwillControlCenter/appearance.conf",
];

/// Background user units started on activation under ADR-01 / ADR-03:
/// `ucc-gui` runs on demand rather than as a permanent session service.
pub const DESKTOP_ACTIVATE_UNITS: [&str; 2] =
    ["ucc-desktop-tray.service", "ucc-session-agent.service"];

/// All user units stopped cleanly on desktop restore.
pub const DESKTOP_STOP_UNITS: [&str; 3] = [
    "ucc-desktop-tray.service",
    "ucc-session-agent.service",
    "ucc-gui.service",
];

pub const GALLERY_PAGES: [&str; 11] = [
    "overview",
    "profiles",
    "cooler-settings",
    "watercool-settings",
    "monitor",
    "keyboard-hardware",
    "about",
    "power-devices",
    "lightbar",
    "battery-processes",
    "driver-warning",
];

/// Captured process output returned by [`CommandRunner`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CmdOutput {
    pub fn ok(stdout: impl Into<String>) -> Self {
        Self {
            status: 0,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    pub fn fail(status: i32, stderr: impl Into<String>) -> Self {
        Self {
            status,
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }
}

/// Abstraction over external system tools (`systemctl`, `pacman`, `cmake`, etc.)
/// so deployment and transition workflows can be tested end-to-end without root.
pub trait CommandRunner {
    fn run_with_env(
        &self,
        program: &str,
        args: &[String],
        envs: &[(&str, &str)],
    ) -> Result<CmdOutput, String>;

    fn run(&self, program: &str, args: &[String]) -> Result<CmdOutput, String> {
        self.run_with_env(program, args, &[])
    }
}

/// Production [`CommandRunner`] delegating to `std::process::Command`.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemCommandRunner;

impl CommandRunner for SystemCommandRunner {
    fn run_with_env(
        &self,
        program: &str,
        args: &[String],
        envs: &[(&str, &str)],
    ) -> Result<CmdOutput, String> {
        let mut cmd = Command::new(program);
        cmd.args(args);
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let out = cmd
            .output()
            .map_err(|e| format!("Failed to spawn {program}: {e}"))?;
        Ok(CmdOutput {
            status: out.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

fn run_checked(
    runner: &dyn CommandRunner,
    program: &str,
    args: &[&str],
    envs: &[(&str, &str)],
) -> Result<CmdOutput, String> {
    let owned: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    let res = runner.run_with_env(program, &owned, envs)?;
    if res.status != 0 {
        let out_tail: String = res
            .stdout
            .chars()
            .rev()
            .take(2000)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        let err_tail: String = res
            .stderr
            .chars()
            .rev()
            .take(2000)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        return Err(format!(
            "Command failed ({}): {program} {}\n{out_tail}{err_tail}",
            res.status,
            owned.join(" ")
        ));
    }
    Ok(res)
}

fn run_unchecked(
    runner: &dyn CommandRunner,
    program: &str,
    args: &[&str],
) -> Result<CmdOutput, String> {
    let owned: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    runner.run(program, &owned)
}

fn read_json_value(path: &Path) -> Result<Value, String> {
    let content =
        fs::read_to_string(path).map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    serde_json::from_str(&content).map_err(|e| format!("Invalid JSON in {}: {e}", path.display()))
}

fn write_private_file(path: &Path, content: &str, mode: u32) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, content).map_err(|e| format!("Failed to write {}: {e}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))
            .map_err(|e| format!("Failed to chmod {}: {e}", tmp.display()))?;
    }
    fs::rename(&tmp, path).map_err(|e| format!("Failed to replace {}: {e}", path.display()))?;
    Ok(())
}

fn set_dir_mode(path: &Path, mode: u32) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .map_err(|e| format!("Failed to chmod {}: {e}", path.display()))?;
    }
    let _ = mode;
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("Failed to create {}: {e}", dst.display()))?;
    for entry in fs::read_dir(src).map_err(|e| format!("Failed to read {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| format!("Dir entry error in {}: {e}", src.display()))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let meta = fs::symlink_metadata(&src_path)
            .map_err(|e| format!("Stat failed on {}: {e}", src_path.display()))?;
        if meta.file_type().is_symlink() {
            #[cfg(unix)]
            {
                let target = fs::read_link(&src_path)
                    .map_err(|e| format!("read_link failed on {}: {e}", src_path.display()))?;
                std::os::unix::fs::symlink(&target, &dst_path)
                    .map_err(|e| format!("symlink failed on {}: {e}", dst_path.display()))?;
            }
        } else if meta.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if meta.is_file() {
            fs::copy(&src_path, &dst_path).map_err(|e| {
                format!(
                    "Copy failed from {} to {}: {e}",
                    src_path.display(),
                    dst_path.display()
                )
            })?;
        }
    }
    Ok(())
}

fn file_sha256(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|e| format!("Failed to hash {}: {e}", path.display()))?;
    Ok(sha256_hex(&bytes))
}

#[cfg(unix)]
fn current_euid_is_root() -> bool {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("Uid:"))
                .and_then(|line| line.split_whitespace().nth(2))
                .and_then(|uid| uid.parse::<u32>().ok())
        })
        == Some(0)
}

#[cfg(not(unix))]
fn current_euid_is_root() -> bool {
    false
}

fn unix_secs_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn format_utc_stamp(secs: u64) -> String {
    // Civil date conversion (Howard Hinnant's algorithm) for `YYYYMMDDTHHMMSSZ`.
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let hour = rem / 3600;
    let minute = (rem % 3600) / 60;
    let second = rem % 60;

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    format!("{year:04}{m:02}{d:02}T{hour:02}{minute:02}{second:02}Z")
}

// ---------------------------------------------------------------------------
// 1. Offline import (`import-offline`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ImportOfflineArgs {
    pub data_dir: PathBuf,
    pub output_dir: PathBuf,
    pub capabilities: PathBuf,
    pub settings: Option<PathBuf>,
    pub tcc_dir: Option<PathBuf>,
    pub tcc_archive: Option<PathBuf>,
    pub brightness: Option<i64>,
}

pub fn run_import_offline(args: &ImportOfflineArgs) -> Result<Value, String> {
    if args.tcc_archive.is_some() && args.tcc_dir.is_none() {
        return Err("--tcc-archive requires --tcc-dir".to_string());
    }
    let data =
        OfflineUserData::from_dir(&args.data_dir).map_err(|e| format!("Import failed: {e}"))?;
    let mut baseline = match &args.settings {
        Some(path) => read_json_value(path)?,
        None => json!({}),
    };
    let mut tcc_report: Option<Value> = None;
    if let Some(tcc_dir) = &args.tcc_dir {
        let tcc_settings = read_json_value(&tcc_dir.join("settings"))?;
        let tcc_profiles = read_json_value(&tcc_dir.join("profiles"))?;
        let presets = match &args.tcc_archive {
            Some(archive) => {
                read_tcc_fan_presets(archive).map_err(|e| format!("Import failed: {e}"))?
            }
            None => BTreeMap::new(),
        };
        let (merged, report) = merge_tcc(&baseline, &tcc_settings, &tcc_profiles, &presets)
            .map_err(|e| format!("Import failed: {e}"))?;
        baseline = merged;
        tcc_report = Some(report);
    }
    let capabilities = read_json_value(&args.capabilities)?;
    let mut output = convert_offline_import(&data, &baseline, &capabilities, args.brightness)
        .map_err(|e| format!("Import failed: {e}"))?;
    if let Some(tcc_rep) = tcc_report {
        if let Some(obj) = output.report.as_object_mut() {
            obj.insert("tccImport".to_string(), tcc_rep);
        }
    }
    let preferences = desktop_preferences(&data).map_err(|e| format!("Import failed: {e}"))?;
    write_offline_import_bundle(&args.output_dir, &output, &preferences)
        .map_err(|e| format!("Import failed: {e}"))?;
    Ok(output.report)
}

pub fn parse_import_offline_args(args: &[String]) -> Result<ImportOfflineArgs, String> {
    let mut positional = Vec::new();
    let mut capabilities = None;
    let mut settings = None;
    let mut tcc_dir = None;
    let mut tcc_archive = None;
    let mut brightness = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--capabilities" => {
                i += 1;
                capabilities = Some(PathBuf::from(
                    args.get(i).ok_or("--capabilities requires a path")?,
                ));
            }
            "--settings" => {
                i += 1;
                settings = Some(PathBuf::from(
                    args.get(i).ok_or("--settings requires a path")?,
                ));
            }
            "--tcc-dir" => {
                i += 1;
                tcc_dir = Some(PathBuf::from(
                    args.get(i).ok_or("--tcc-dir requires a path")?,
                ));
            }
            "--tcc-archive" => {
                i += 1;
                tcc_archive = Some(PathBuf::from(
                    args.get(i).ok_or("--tcc-archive requires a path")?,
                ));
            }
            "--brightness" => {
                i += 1;
                let raw = args.get(i).ok_or("--brightness requires an integer")?;
                brightness = Some(
                    raw.parse::<i64>()
                        .map_err(|_| format!("Invalid --brightness value: {raw}"))?,
                );
            }
            other if other.starts_with('-') => {
                return Err(format!("Unknown import-offline flag: {other}"));
            }
            other => positional.push(PathBuf::from(other)),
        }
        i += 1;
    }

    if positional.len() != 2 {
        return Err(
            "Usage: cargo xtask import-offline <data-dir> <output-dir> --capabilities <path> [--settings <path>] [--tcc-dir <dir>] [--tcc-archive <asar>] [--brightness <n>]".to_string(),
        );
    }

    Ok(ImportOfflineArgs {
        data_dir: positional.remove(0),
        output_dir: positional.remove(0),
        capabilities: capabilities.ok_or("--capabilities is required")?,
        settings,
        tcc_dir,
        tcc_archive,
        brightness,
    })
}

// ---------------------------------------------------------------------------
// 2. Prepare deployment (`maintenance prepare-deployment`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PrepareDeploymentArgs {
    pub build_dir: PathBuf,
    pub data_dir: PathBuf,
    pub tcc_dir: PathBuf,
    pub tcc_archive: PathBuf,
    pub normal_tests: PathBuf,
    pub preview_tests: PathBuf,
    pub sysfs_leds: PathBuf,
    pub allow_gui_audit_failure: bool,
    pub output_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JUnitSummary {
    pub total: usize,
    pub failed_names: Vec<String>,
}

/// Parses JUnit XML `<testcase>` elements without external XML dependencies.
pub fn parse_junit_xml(path: &Path) -> Result<JUnitSummary, String> {
    let xml =
        fs::read_to_string(path).map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    if !xml.trim_start().starts_with('<') {
        return Err(format!("Malformed XML in {}", path.display()));
    }

    let mut total = 0_usize;
    let mut failed_names = Vec::new();
    let mut pos = 0_usize;
    while let Some(rel) = xml[pos..].find("<testcase") {
        let tag_start = pos + rel;
        let after_kw = tag_start + "<testcase".len();
        let next_b = xml.as_bytes().get(after_kw).copied().unwrap_or(0);
        if !(next_b.is_ascii_whitespace() || next_b == b'>' || next_b == b'/') {
            pos = after_kw;
            continue;
        }
        let open_end = xml[tag_start..]
            .find('>')
            .map(|r| tag_start + r)
            .ok_or_else(|| format!("Unterminated <testcase> tag in {}", path.display()))?;
        let open_tag = &xml[tag_start..=open_end];
        let self_closing = open_tag.trim_end().ends_with("/>");
        let name = extract_xml_attr(open_tag, "name").unwrap_or_else(|| "unnamed".to_string());
        total += 1;

        if self_closing {
            pos = open_end + 1;
        } else {
            let close_start = xml[open_end + 1..]
                .find("</testcase>")
                .map(|r| open_end + 1 + r)
                .ok_or_else(|| format!("Missing </testcase> in {}", path.display()))?;
            let body = &xml[open_end + 1..close_start];
            if has_child_tag(body, "failure") || has_child_tag(body, "error") {
                failed_names.push(name);
            }
            pos = close_start + "</testcase>".len();
        }
    }

    Ok(JUnitSummary {
        total,
        failed_names,
    })
}

fn has_child_tag(body: &str, tag: &str) -> bool {
    let pat = format!("<{tag}");
    let mut pos = 0;
    while let Some(rel) = body[pos..].find(&pat) {
        let after = pos + rel + pat.len();
        let next_b = body.as_bytes().get(after).copied().unwrap_or(0);
        if next_b.is_ascii_whitespace() || next_b == b'>' || next_b == b'/' {
            return true;
        }
        pos = after;
    }
    false
}

fn extract_xml_attr(tag: &str, attr: &str) -> Option<String> {
    let mut pos = 0;
    while let Some(rel) = tag[pos..].find(attr) {
        let start = pos + rel;
        let before_ok = start == 0 || tag.as_bytes()[start - 1].is_ascii_whitespace();
        let after = start + attr.len();
        if before_ok && tag[after..].starts_with('=') {
            let rest = &tag[after + 1..];
            if let Some(q) = rest.as_bytes().first().copied() {
                if q == b'"' || q == b'\'' {
                    if let Some(end) = rest[1..].find(q as char) {
                        return Some(rest[1..1 + end].to_string());
                    }
                }
            }
        }
        pos = after;
    }
    None
}

pub fn verify_junit_report(
    path: &Path,
    allow_gui_audit_failure: bool,
) -> Result<JUnitSummary, String> {
    let summary = parse_junit_xml(path)?;
    let ok = summary.total > 0
        && (summary.failed_names.is_empty()
            || (allow_gui_audit_failure && summary.failed_names == ["test_gui_audit"]));
    if !ok {
        return Err(format!(
            "Missing or failing test results: {}",
            path.display()
        ));
    }
    Ok(summary)
}

fn discover_sysfs_keyboard(sysfs_leds: &Path) -> Result<(Value, i64), String> {
    let mut nodes: Vec<(i64, PathBuf)> = Vec::new();
    let entries = fs::read_dir(sysfs_leds)
        .map_err(|e| format!("Failed to inspect {}: {e}", sysfs_leds.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed reading led entry: {e}"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let id = if name == "rgb:kbd_backlight" {
            Some(0_i64)
        } else if let Some(suffix) = name.strip_prefix("rgb:kbd_backlight_") {
            suffix
                .parse::<i64>()
                .ok()
                .filter(|n| *n >= 0 && suffix.bytes().all(|b| b.is_ascii_digit()))
        } else {
            None
        };
        if let Some(led_id) = id {
            let path = entry.path();
            if path.join("multi_intensity").exists() {
                nodes.push((led_id, path));
            }
        }
    }
    nodes.sort_by_key(|(id, _)| *id);
    let unique: BTreeSet<i64> = nodes.iter().map(|(id, _)| *id).collect();
    if nodes.is_empty() || unique.len() != nodes.len() {
        return Err("Cannot identify physical RGB keyboard LEDs".to_string());
    }

    let mut maxima = BTreeSet::new();
    for (_, path) in &nodes {
        let max_text = fs::read_to_string(path.join("max_brightness"))
            .map_err(|e| format!("Failed reading max_brightness in {}: {e}", path.display()))?;
        let max_val = max_text
            .trim()
            .parse::<i64>()
            .map_err(|_| format!("Invalid max_brightness in {}", path.display()))?;
        maxima.insert(max_val);
    }
    if maxima.len() != 1 {
        return Err("Inconsistent keyboard brightness limits".to_string());
    }
    let max_brightness = *maxima.iter().next().unwrap();

    let bright_text = fs::read_to_string(nodes[0].1.join("brightness"))
        .map_err(|e| format!("Failed reading brightness in {}: {e}", nodes[0].1.display()))?;
    let brightness = bright_text
        .trim()
        .parse::<i64>()
        .map_err(|_| format!("Invalid brightness in {}", nodes[0].1.display()))?;

    let led_ids: Vec<i64> = nodes.iter().map(|(id, _)| *id).collect();
    let capabilities = json!({
        "zones": nodes.len(),
        "maxBrightness": max_brightness,
        "ledIds": led_ids,
    });
    Ok((capabilities, brightness))
}

fn dir_files_total_size(root: &Path) -> u64 {
    let mut total = 0_u64;
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Ok(meta) = fs::symlink_metadata(&path) {
                if meta.is_dir() {
                    total += dir_files_total_size(&path);
                } else if meta.is_file() {
                    total += meta.len();
                }
            }
        }
    }
    total
}

fn parse_cmake_version(cmake_lists: &str) -> Result<String, String> {
    for line in cmake_lists.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("VERSION") {
            let tok = rest.split_whitespace().next().unwrap_or("");
            if !tok.is_empty() && tok.chars().all(|c| c.is_ascii_digit() || c == '.') {
                return Ok(tok.to_string());
            }
        }
    }
    Err("Could not determine VERSION in CMakeLists.txt".to_string())
}

pub fn prepare_deployment(
    args: &PrepareDeploymentArgs,
    repo_root: &Path,
    runner: &dyn CommandRunner,
    allow_root_for_tests: bool,
) -> Result<Value, String> {
    if !allow_root_for_tests && current_euid_is_root() {
        return Err("Prepare the bundle as the desktop user".to_string());
    }

    let normal_summary = verify_junit_report(&args.normal_tests, args.allow_gui_audit_failure)?;
    let preview_summary = verify_junit_report(&args.preview_tests, args.allow_gui_audit_failure)?;

    let cache_text = fs::read_to_string(args.build_dir.join("CMakeCache.txt"))
        .map_err(|e| format!("Failed reading CMakeCache.txt: {e}"))?;
    let cache_lines: Vec<&str> = cache_text.lines().collect();
    for required in [
        "UCC_READ_ONLY_PREVIEW:BOOL=OFF",
        "CMAKE_INSTALL_PREFIX:PATH=/usr",
        "BUILD_GUI:BOOL=ON",
        "BUILD_CLI:BOOL=ON",
        "BUILD_DESKTOP_TRAY:BOOL=ON",
        "BUILD_SESSION_AGENT:BOOL=ON",
    ] {
        if !cache_lines.contains(&required) {
            return Err(format!("Unsuitable installation build: {required}"));
        }
    }

    fs::create_dir(&args.output_dir).map_err(|e| {
        format!(
            "Failed creating private bundle dir {}: {e}",
            args.output_dir.display()
        )
    })?;
    set_dir_mode(&args.output_dir, 0o700)?;
    let bundle = fs::canonicalize(&args.output_dir)
        .map_err(|e| format!("Failed resolving bundle path: {e}"))?;

    let (capabilities, brightness) = discover_sysfs_keyboard(&args.sysfs_leds)?;
    write_private_file(
        &bundle.join("capabilities.json"),
        &(serde_json::to_string_pretty(&capabilities).unwrap() + "\n"),
        0o600,
    )?;

    let snapshot = bundle.join("source");
    fs::create_dir(&snapshot).map_err(|e| format!("mkdir source failed: {e}"))?;
    set_dir_mode(&snapshot, 0o700)?;

    let data_origins: Vec<PathBuf> = ["config", "keyboard_colors", "lightbar", "gui_settings"]
        .into_iter()
        .map(|k| args.data_dir.join(format!("{k}.json")))
        .collect();
    let tcc_origins = [args.tcc_dir.join("settings"), args.tcc_dir.join("profiles")];

    let mut fingerprints = Vec::new();
    for origin in data_origins
        .iter()
        .chain(tcc_origins.iter())
        .chain(std::iter::once(&args.tcc_archive))
    {
        let resolved = fs::canonicalize(origin)
            .map_err(|e| format!("Failed resolving {}: {e}", origin.display()))?;
        fingerprints.push(json!({
            "path": resolved.display().to_string(),
            "sha256": file_sha256(&resolved)?,
        }));
    }

    let snap_data = snapshot.join("Data");
    fs::create_dir(&snap_data).map_err(|e| format!("mkdir Data failed: {e}"))?;
    set_dir_mode(&snap_data, 0o700)?;
    for origin in &data_origins {
        fs::copy(origin, snap_data.join(origin.file_name().unwrap()))
            .map_err(|e| format!("Copy {} failed: {e}", origin.display()))?;
    }

    let snap_tcc = snapshot.join("tcc");
    fs::create_dir(&snap_tcc).map_err(|e| format!("mkdir tcc failed: {e}"))?;
    set_dir_mode(&snap_tcc, 0o700)?;
    for origin in &tcc_origins {
        fs::copy(origin, snap_tcc.join(origin.file_name().unwrap()))
            .map_err(|e| format!("Copy {} failed: {e}", origin.display()))?;
    }

    let snap_asar = snapshot.join("app.asar");
    fs::copy(&args.tcc_archive, &snap_asar)
        .map_err(|e| format!("Copy {} failed: {e}", args.tcc_archive.display()))?;

    let import_dir = bundle.join("import");
    let report = run_import_offline(&ImportOfflineArgs {
        data_dir: snap_data,
        output_dir: import_dir.clone(),
        capabilities: bundle.join("capabilities.json"),
        settings: None,
        tcc_dir: Some(snap_tcc),
        tcc_archive: Some(snap_asar),
        brightness: Some(brightness),
    })?;
    write_private_file(
        &bundle.join("migration.log"),
        &(serde_json::to_string_pretty(&report).unwrap() + "\n"),
        0o600,
    )?;

    if report
        .get("unresolvedProfiles")
        .and_then(Value::as_array)
        .is_some_and(|arr| !arr.is_empty())
    {
        return Err("Some original profiles remain unresolved".to_string());
    }

    let import_check_bin = args.build_dir.join("bin/ucc-import-check");
    let import_check_str = import_check_bin.display().to_string();
    let import_settings_str = import_dir.join("settings").display().to_string();
    let caps_path_str = bundle.join("capabilities.json").display().to_string();
    run_checked(
        runner,
        &import_check_str,
        &[&import_settings_str, &caps_path_str],
        &[],
    )?;

    let stage = bundle.join("stage");
    let build_str = args.build_dir.display().to_string();
    let stage_str = stage.display().to_string();
    let install_out = run_checked(
        runner,
        "cmake",
        &["--install", &build_str],
        &[("DESTDIR", &stage_str)],
    )?;
    let stage_usr_str = stage.join("usr").display().to_string();
    let rust_bin_str = repo_root.join("target/release").display().to_string();
    let runtime_out = run_checked(
        runner,
        "cargo",
        &[
            "run",
            "-p",
            "xtask",
            "--locked",
            "--",
            "install-runtime",
            &stage_usr_str,
            &rust_bin_str,
        ],
        &[],
    )?;
    write_private_file(
        &bundle.join("install.log"),
        &format!(
            "{}{}{}{}",
            install_out.stdout, install_out.stderr, runtime_out.stdout, runtime_out.stderr
        ),
        0o600,
    )?;

    let etc_ucc = stage.join("etc/ucc");
    fs::create_dir_all(&etc_ucc).map_err(|e| format!("mkdir etc/ucc failed: {e}"))?;
    set_dir_mode(&etc_ucc, 0o755)?;
    let staged_settings = etc_ucc.join("settings");
    fs::copy(import_dir.join("settings"), &staged_settings)
        .map_err(|e| format!("Copy settings failed: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staged_settings, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("chmod staged settings failed: {e}"))?;
    }

    let repo_str = repo_root.display().to_string();
    let commit = run_checked(runner, "git", &["-C", &repo_str, "rev-parse", "HEAD"], &[])?
        .stdout
        .trim()
        .to_string();
    let revision = run_checked(
        runner,
        "git",
        &["-C", &repo_str, "rev-list", "--count", "HEAD"],
        &[],
    )?
    .stdout
    .trim()
    .to_string();
    let cmake_lists = fs::read_to_string(repo_root.join("CMakeLists.txt"))
        .map_err(|e| format!("Failed reading CMakeLists.txt: {e}"))?;
    let base_version = parse_cmake_version(&cmake_lists)?;
    let commit_prefix: String = commit.chars().take(8).collect();
    let version = format!("{base_version}.r{revision}.g{commit_prefix}-1");

    let mut metadata = vec![
        format!("pkgname = {PACKAGE_NAME}"),
        format!("pkgbase = {PACKAGE_NAME}"),
        format!("pkgver = {version}"),
        "pkgdesc = Frixit Control Center fork with original personal configuration".to_string(),
        format!("builddate = {}", unix_secs_now()),
        "packager = Local Frixit build".to_string(),
        format!("size = {}", dir_files_total_size(&stage)),
        "arch = x86_64".to_string(),
        "license = GPL-3.0-or-later".to_string(),
        "backup = etc/ucc/settings".to_string(),
        format!("provides = uniwill-control-center={base_version}"),
        "conflict = uniwill-control-center".to_string(),
    ];
    for dep in [
        "qt6-base",
        "qt6-connectivity",
        "qt6-svg",
        "dbus",
        "polkit",
        "systemd-libs",
        "hicolor-icon-theme",
    ] {
        metadata.push(format!("depend = {dep}"));
    }
    write_private_file(
        &stage.join(".PKGINFO"),
        &(metadata.join("\n") + "\n"),
        0o644,
    )?;

    let package_name = format!("{PACKAGE_NAME}-{version}-x86_64.pkg.tar.zst");
    let package_path = bundle.join(&package_name);
    let package_str = package_path.display().to_string();
    let mut entries: Vec<String> = fs::read_dir(&stage)
        .map_err(|e| format!("Failed reading stage dir: {e}"))?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    entries.sort();

    let mut tar_args: Vec<String> = vec![
        "--create",
        "--zstd",
        "--uid",
        "0",
        "--gid",
        "0",
        "--uname",
        "root",
        "--gname",
        "root",
        "--file",
        &package_str,
        "-C",
        &stage_str,
    ]
    .into_iter()
    .map(String::from)
    .collect();
    tar_args.extend(entries);
    let tar_res = runner.run("bsdtar", &tar_args)?;
    if tar_res.status != 0 {
        return Err(format!("bsdtar failed: {}", tar_res.stderr));
    }
    if package_path.exists() {
        set_dir_mode(&package_path, 0o600)?;
    }

    let qip = run_checked(runner, "pacman", &["-Qip", &package_str], &[])?;
    write_private_file(&bundle.join("package-info.txt"), &qip.stdout, 0o600)?;

    fs::copy(&args.normal_tests, bundle.join("normal-tests.xml"))
        .map_err(|e| format!("Copy normal-tests.xml failed: {e}"))?;
    fs::copy(&args.preview_tests, bundle.join("preview-tests.xml"))
        .map_err(|e| format!("Copy preview-tests.xml failed: {e}"))?;

    let manifest = json!({
        "commit": commit,
        "version": version,
        "package": package_name,
        "packageSha256": file_sha256(&package_path)?,
        "inputs": fingerprints,
        "tests": {
            "normal": normal_summary.total,
            "preview": preview_summary.total,
        },
        "migration": report,
        "testFailures": {
            "normal": normal_summary.failed_names,
            "preview": preview_summary.failed_names,
        },
        "guiAuditFailureAcceptedForLaunch": args.allow_gui_audit_failure,
    });
    write_private_file(
        &bundle.join("deployment.json"),
        &(serde_json::to_string_pretty(&manifest).unwrap() + "\n"),
        0o600,
    )?;

    Ok(json!({
        "bundle": bundle.display().to_string(),
        "package": package_name,
        "tests": [normal_summary.total, preview_summary.total],
    }))
}

// ---------------------------------------------------------------------------
// 3. System deploy & rollback (`maintenance deploy`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnitSnapshot {
    pub active: String,
    pub enabled: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentState {
    pub phase: String,
    pub package: String,
    pub manifest: Value,
    pub package_installed: bool,
    pub package_install_started: bool,
    pub services_touched: bool,
    pub units: BTreeMap<String, UnitSnapshot>,
}

/// Manages staged package installation, post-install verification, and atomic rollback.
pub struct DeploymentManager<'a> {
    pub bundle: PathBuf,
    pub root: PathBuf,
    pub runner: &'a dyn CommandRunner,
    pub backup: Option<PathBuf>,
    pub state: Option<DeploymentState>,
}

impl<'a> DeploymentManager<'a> {
    pub fn new(bundle: &Path, root: &Path, runner: &'a dyn CommandRunner) -> Self {
        Self {
            bundle: fs::canonicalize(bundle).unwrap_or_else(|_| bundle.to_path_buf()),
            root: root.to_path_buf(),
            runner,
            backup: None,
            state: None,
        }
    }

    pub fn from_backup(
        backup: &Path,
        root: &Path,
        runner: &'a dyn CommandRunner,
    ) -> Result<Self, String> {
        let state_text = fs::read_to_string(backup.join("state.json"))
            .map_err(|e| format!("Failed reading backup state.json: {e}"))?;
        let state: DeploymentState = serde_json::from_str(&state_text)
            .map_err(|e| format!("Invalid backup state.json: {e}"))?;
        if state.phase == "rolled-back" {
            return Err("This deployment was already rolled back".to_string());
        }
        Ok(Self {
            bundle: backup.to_path_buf(),
            root: root.to_path_buf(),
            runner,
            backup: Some(backup.to_path_buf()),
            state: Some(state),
        })
    }

    fn path(&self, abs_path: &str) -> PathBuf {
        self.root.join(abs_path.trim_start_matches('/'))
    }

    fn save(&self) -> Result<(), String> {
        let backup = self.backup.as_ref().ok_or("Missing backup path")?;
        let state = self.state.as_ref().ok_or("Missing deployment state")?;
        let target = backup.join("state.json");
        let content = serde_json::to_string_pretty(state)
            .map_err(|e| format!("Failed serializing deployment state: {e}"))?
            + "\n";
        write_private_file(&target, &content, 0o600)
    }

    pub fn verify_bundle(&self) -> Result<(Value, PathBuf), String> {
        let manifest = read_json_value(&self.bundle.join("deployment.json"))?;
        let name = manifest
            .get("package")
            .and_then(Value::as_str)
            .ok_or("Missing package name in deployment.json")?;
        if Path::new(name).file_name().and_then(|s| s.to_str()) != Some(name)
            || !name.starts_with(&format!("{PACKAGE_NAME}-"))
        {
            return Err("Invalid package filename".to_string());
        }
        let package = self.bundle.join(name);
        let meta = fs::symlink_metadata(&package)
            .map_err(|e| format!("Missing package {}: {e}", package.display()))?;
        let expected_sha = manifest
            .get("packageSha256")
            .and_then(Value::as_str)
            .ok_or("Missing packageSha256 in deployment.json")?;
        if meta.file_type().is_symlink() || file_sha256(&package)? != expected_sha {
            return Err("Package checksum mismatch".to_string());
        }

        let inputs = manifest
            .get("inputs")
            .and_then(Value::as_array)
            .ok_or("Missing inputs array in deployment.json")?;
        for item in inputs {
            let path_str = item
                .get("path")
                .and_then(Value::as_str)
                .ok_or("Missing input path")?;
            let expected = item
                .get("sha256")
                .and_then(Value::as_str)
                .ok_or("Missing input sha256")?;
            if file_sha256(&self.path(path_str))? != expected {
                return Err(format!("Source configuration changed: {path_str}"));
            }
        }

        if run_unchecked(self.runner, "pacman", &["-Q", PACKAGE_NAME])?.status == 0 {
            return Err(
                "This first-install tool refuses to overwrite an installed UCC package".to_string(),
            );
        }
        if self.path("/usr/bin/uccd").exists() || self.path("/etc/ucc/settings").exists() {
            return Err(
                "Existing unmanaged UCC installation requires a separate upgrade migration"
                    .to_string(),
            );
        }

        Ok((manifest, package))
    }

    pub fn install(&mut self) -> Result<PathBuf, String> {
        let (manifest, package) = self.verify_bundle()?;
        let stamp = format_utc_stamp(unix_secs_now());
        let short_hash: String = file_sha256(&package)?.chars().take(8).collect();
        let backup = self
            .path("/var/lib/ucc/deployments")
            .join(format!("{stamp}-{short_hash}"));
        fs::create_dir_all(&backup)
            .map_err(|e| format!("Failed creating backup {}: {e}", backup.display()))?;
        set_dir_mode(&backup, 0o700)?;
        self.backup = Some(backup.clone());

        let mut units = BTreeMap::new();
        for unit in OLD_UNITS.iter().chain(NEW_UNITS.iter()) {
            let active = run_unchecked(self.runner, "systemctl", &["is-active", unit])?
                .stdout
                .trim()
                .to_string();
            let enabled = run_unchecked(self.runner, "systemctl", &["is-enabled", unit])?
                .stdout
                .trim()
                .to_string();
            units.insert((*unit).to_string(), UnitSnapshot { active, enabled });
        }

        self.state = Some(DeploymentState {
            phase: "prepared".to_string(),
            package: PACKAGE_NAME.to_string(),
            manifest: manifest.clone(),
            package_installed: false,
            package_install_started: false,
            services_touched: false,
            units,
        });

        for dir_abs in ["/etc/tcc", "/etc/ucc"] {
            let source = self.path(dir_abs);
            if source.is_dir() {
                copy_dir_recursive(&source, &backup.join(dir_abs.trim_start_matches('/')))?;
            }
        }
        if let Some(inputs) = manifest.get("inputs").and_then(Value::as_array) {
            for item in inputs {
                if let Some(p) = item.get("path").and_then(Value::as_str) {
                    let src = self.path(p);
                    let dst = backup.join("inputs").join(p.trim_start_matches('/'));
                    if let Some(parent) = dst.parent() {
                        fs::create_dir_all(parent)
                            .map_err(|e| format!("mkdir backup input failed: {e}"))?;
                    }
                    fs::copy(&src, &dst)
                        .map_err(|e| format!("Backup copy failed for {}: {e}", src.display()))?;
                }
            }
        }
        self.save()?;

        let install_res = self.perform_install_steps(&package);
        if let Err(err) = install_res {
            if let Err(rb_err) = self.rollback() {
                return Err(format!("{err}; ROLLBACK ERROR: {rb_err}"));
            }
            return Err(err);
        }

        Ok(backup)
    }

    fn perform_install_steps(&mut self, package: &Path) -> Result<(), String> {
        let pkg_str = package.display().to_string();
        if let Some(state) = self.state.as_mut() {
            state.package_install_started = true;
        }
        self.save()?;

        run_checked(self.runner, "pacman", &["-U", "--noconfirm", &pkg_str], &[])?;
        if let Some(state) = self.state.as_mut() {
            state.package_installed = true;
        }
        self.save()?;

        let caps_str = self.bundle.join("capabilities.json").display().to_string();
        run_checked(
            self.runner,
            "/usr/lib/ucc/ucc-import-check",
            &["/etc/ucc/settings", &caps_str],
            &[],
        )?;
        run_checked(
            self.runner,
            "systemd-analyze",
            &["--man=no", "verify", "uccd.service", "uccd-sleep.service"],
            &[],
        )?;

        if let Some(state) = self.state.as_mut() {
            state.services_touched = true;
        }
        self.save()?;

        // Stop the old sleep hook first: its ExecStop may start tccd.
        run_checked(self.runner, "systemctl", &["stop", OLD_UNITS[0]], &[])?;
        run_checked(
            self.runner,
            "systemctl",
            &["stop", OLD_UNITS[1], OLD_UNITS[2]],
            &[],
        )?;
        run_checked(
            self.runner,
            "systemctl",
            &["disable", OLD_UNITS[0], OLD_UNITS[1], OLD_UNITS[2]],
            &[],
        )?;
        run_checked(
            self.runner,
            "systemctl",
            &["mask", "--runtime", "tccd.service", "tccd-sleep.service"],
            &[],
        )?;
        run_checked(self.runner, "systemctl", &["daemon-reload"], &[])?;
        run_checked(self.runner, "systemctl", &["reload", "dbus.service"], &[])?;
        run_checked(
            self.runner,
            "systemctl",
            &["enable", NEW_UNITS[0], NEW_UNITS[1]],
            &[],
        )?;
        run_checked(self.runner, "systemctl", &["start", "uccd.service"], &[])?;

        self.health()?;

        let prev_out = run_checked(self.runner, "ucc-cli", &["--json", "power", "status"], &[])?;
        let previous: Value = serde_json::from_str(&prev_out.stdout)
            .map_err(|e| format!("Invalid JSON from ucc-cli power status: {e}"))?;
        run_checked(self.runner, "/usr/lib/ucc/ucc-sleep", &["prepare"], &[])?;
        run_checked(self.runner, "/usr/lib/ucc/ucc-sleep", &["resume"], &[])?;
        self.health()?;

        let res_out = run_checked(self.runner, "ucc-cli", &["--json", "power", "status"], &[])?;
        let resumed: Value = serde_json::from_str(&res_out.stdout)
            .map_err(|e| format!("Invalid JSON from ucc-cli power status after resume: {e}"))?;
        if resumed.get("manual_mode") != previous.get("manual_mode") {
            return Err("Resume lifecycle did not preserve the manual power mode".to_string());
        }

        if let Some(state) = self.state.as_mut() {
            state.phase = "installed".to_string();
        }
        self.save()?;
        Ok(())
    }

    pub fn health(&self) -> Result<(), String> {
        let supported = run_checked(
            self.runner,
            "busctl",
            &[
                "--timeout=15",
                "call",
                "com.uniwill.uccd",
                "/com/uniwill/uccd",
                "com.uniwill.uccd",
                "IsDeviceSupported",
            ],
            &[],
        )?;
        if supported.stdout.trim() != "b true" {
            return Err(format!(
                "UCC does not report a supported device: {}",
                supported.stdout.trim()
            ));
        }

        let policy_out = run_checked(self.runner, "ucc-cli", &["--json", "power", "status"], &[])?;
        let policy: Value = serde_json::from_str(&policy_out.stdout)
            .map_err(|e| format!("Invalid live power status JSON: {e}"))?;
        let mode = policy.get("current_mode").and_then(Value::as_str);
        if !matches!(mode, Some("ac" | "type_c" | "bat")) {
            return Err("Invalid live power policy".to_string());
        }

        run_checked(
            self.runner,
            "systemctl",
            &["is-active", "uccd.service"],
            &[],
        )?;
        Ok(())
    }

    pub fn rollback(&mut self) -> Result<(), String> {
        let mut state = self.state.clone().ok_or("Missing deployment state")?;
        let backup = self.backup.clone().ok_or("Missing backup path")?;

        if state.package_install_started && !state.package_installed {
            state.package_installed =
                run_unchecked(self.runner, "pacman", &["-Q", PACKAGE_NAME])?.status == 0;
            self.state = Some(state.clone());
            self.save()?;
        }

        if state.services_touched {
            let _ = run_unchecked(self.runner, "systemctl", &["stop", "uccd-sleep.service"]);
            let _ = run_unchecked(self.runner, "systemctl", &["stop", "uccd.service"]);
            let _ = run_unchecked(
                self.runner,
                "systemctl",
                &["disable", NEW_UNITS[0], NEW_UNITS[1]],
            );
        }

        if state.package_installed {
            run_checked(
                self.runner,
                "pacman",
                &["-R", "--noconfirm", PACKAGE_NAME],
                &[],
            )?;
            let settings_dir = self.path("/etc/ucc");
            if settings_dir.exists() {
                fs::rename(&settings_dir, backup.join("retired-ucc-settings"))
                    .map_err(|e| format!("Failed retiring /etc/ucc during rollback: {e}"))?;
            }
            let previous = backup.join("etc/ucc");
            if previous.exists() {
                copy_dir_recursive(&previous, &settings_dir)?;
            }
        }

        if state.services_touched {
            let _ = run_unchecked(
                self.runner,
                "systemctl",
                &["unmask", "--runtime", "tccd.service", "tccd-sleep.service"],
            );
            run_checked(self.runner, "systemctl", &["daemon-reload"], &[])?;
            let _ = run_unchecked(self.runner, "systemctl", &["reload", "dbus.service"]);

            // Restore sleep hook before main daemon: sleep hook start may stop tccd.
            for unit in OLD_UNITS {
                if let Some(saved) = state.units.get(unit) {
                    match saved.enabled.as_str() {
                        "enabled" => {
                            run_checked(self.runner, "systemctl", &["enable", unit], &[])?;
                        }
                        "enabled-runtime" => {
                            run_checked(
                                self.runner,
                                "systemctl",
                                &["enable", "--runtime", unit],
                                &[],
                            )?;
                        }
                        _ => {}
                    }
                    if saved.active == "active" {
                        run_checked(self.runner, "systemctl", &["start", unit], &[])?;
                    }
                }
            }
        }

        state.phase = "rolled-back".to_string();
        self.state = Some(state);
        self.save()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 4. Desktop transition (`maintenance desktop-transition`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopTransitionState {
    pub source: String,
    pub phase: String,
    pub files: BTreeMap<String, bool>,
    pub old_units: Vec<String>,
}

/// Handles user desktop session quiesce, activation (`ADR-01` on-demand GUI), and restore.
pub struct DesktopTransition<'a> {
    pub directory: PathBuf,
    pub home: PathBuf,
    pub runner: &'a dyn CommandRunner,
    pub poll_attempts: usize,
    pub poll_interval: Duration,
}

impl<'a> DesktopTransition<'a> {
    pub fn new(directory: &Path, home: &Path, runner: &'a dyn CommandRunner) -> Self {
        Self {
            directory: directory.to_path_buf(),
            home: home.to_path_buf(),
            runner,
            poll_attempts: 10,
            poll_interval: Duration::from_secs(1),
        }
    }

    fn save(&self, state: &DesktopTransitionState) -> Result<(), String> {
        let text = serde_json::to_string_pretty(state)
            .map_err(|e| format!("Failed serializing transition state: {e}"))?
            + "\n";
        write_private_file(&self.directory.join("state.json"), &text, 0o600)
    }

    fn load(&self) -> Result<DesktopTransitionState, String> {
        let text = fs::read_to_string(self.directory.join("state.json"))
            .map_err(|e| format!("Failed reading transition state.json: {e}"))?;
        serde_json::from_str(&text).map_err(|e| format!("Invalid transition state.json: {e}"))
    }

    pub fn quiesce(&self, source: &Path) -> Result<DesktopTransitionState, String> {
        fs::create_dir(&self.directory).map_err(|e| {
            format!(
                "Failed creating state dir {}: {e}",
                self.directory.display()
            )
        })?;
        set_dir_mode(&self.directory, 0o700)?;

        let resolved_source = fs::canonicalize(source)
            .unwrap_or_else(|_| source.to_path_buf())
            .display()
            .to_string();

        let mut files = BTreeMap::new();
        for relative in DESKTOP_FILES {
            let origin = self.home.join(relative);
            let exists = origin.exists();
            files.insert(relative.to_string(), exists);
            if exists {
                let target = self.directory.join("files").join(relative);
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|e| format!("mkdir backup dir failed: {e}"))?;
                }
                fs::copy(&origin, &target)
                    .map_err(|e| format!("Copy {} failed: {e}", origin.display()))?;
            }
        }

        let list_out = run_checked(
            self.runner,
            "systemctl",
            &["--user", "list-units", "--all", "--output=json"],
            &[],
        )?;
        let units: Vec<Value> = serde_json::from_str(&list_out.stdout)
            .map_err(|e| format!("Invalid JSON from systemctl list-units: {e}"))?;

        let mut old_units = Vec::new();
        for unit in units {
            let name = unit.get("unit").and_then(Value::as_str).unwrap_or("");
            let active = unit.get("active").and_then(Value::as_str).unwrap_or("");
            let matches_legacy = matches!(
                name,
                "tuxedo-power-tray.service" | "tuxedo-lightbar-tray.service"
            ) || name.starts_with("app-tuxedo-control-center-")
                || name.starts_with("app-tuxedo\\x2dcontrol\\x2dcenter@");
            if matches_legacy && matches!(active, "active" | "activating") {
                old_units.push(name.to_string());
            }
        }

        let state = DesktopTransitionState {
            source: resolved_source,
            phase: "quiesced".to_string(),
            files,
            old_units: old_units.clone(),
        };
        self.save(&state)?;

        if !old_units.is_empty() {
            let mut stop_args: Vec<&str> = vec!["--user", "stop"];
            stop_args.extend(old_units.iter().map(String::as_str));
            run_checked(self.runner, "systemctl", &stop_args, &[])?;
        }
        Ok(state)
    }

    pub fn activate(&self, preferences: &Path) -> Result<DesktopTransitionState, String> {
        let mut state = self.load()?;
        if run_unchecked(self.runner, "systemctl", &["is-active", "uccd.service"])?.status != 0 {
            return Err("The system UCC daemon is not active".to_string());
        }

        let desktop_override = self.home.join(".config/autostart/control-center.desktop");
        write_private_file(
            &desktop_override,
            "[Desktop Entry]\nType=Application\nName=Control Center (replaced by UCC)\nHidden=true\n",
            0o644,
        )?;

        let pref_str = preferences.display().to_string();
        run_checked(
            self.runner,
            "/usr/bin/ucc-gui",
            &["--import-preferences", &pref_str, "--preferences-only"],
            &[],
        )?;
        run_checked(self.runner, "systemctl", &["--user", "daemon-reload"], &[])?;

        for unit in DESKTOP_ACTIVATE_UNITS {
            let unit_flag = format!("--unit={unit}");
            let bin_name = unit.strip_suffix(".service").unwrap_or(unit);
            let bin_path = format!("/usr/bin/{bin_name}");
            run_checked(
                self.runner,
                "systemd-run",
                &["--user", "--collect", &unit_flag, &bin_path],
                &[],
            )?;
        }

        for attempt in 0..self.poll_attempts {
            let mut all_active = true;
            for unit in DESKTOP_ACTIVATE_UNITS {
                if run_unchecked(self.runner, "systemctl", &["--user", "is-active", unit])?.status
                    != 0
                {
                    all_active = false;
                    break;
                }
            }
            if all_active {
                state.phase = "activated".to_string();
                self.save(&state)?;
                return Ok(state);
            }
            if attempt + 1 < self.poll_attempts && !self.poll_interval.is_zero() {
                std::thread::sleep(self.poll_interval);
            }
        }

        Err("One or more UCC desktop clients failed to start".to_string())
    }

    pub fn restore(&self) -> Result<DesktopTransitionState, String> {
        let mut state = self.load()?;
        let mut stop_args: Vec<&str> = vec!["--user", "stop"];
        stop_args.extend(DESKTOP_STOP_UNITS);
        let _ = run_unchecked(self.runner, "systemctl", &stop_args);

        for (relative, existed) in &state.files {
            let target = self.home.join(relative);
            if *existed {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(|e| format!("mkdir restore failed: {e}"))?;
                }
                fs::copy(self.directory.join("files").join(relative), &target)
                    .map_err(|e| format!("Restore copy failed for {}: {e}", target.display()))?;
            } else {
                match fs::remove_file(&target) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(format!("Remove {} failed: {e}", target.display())),
                }
            }
        }
        run_checked(self.runner, "systemctl", &["--user", "daemon-reload"], &[])?;

        if state.old_units.iter().any(|u| u.starts_with("tuxedo-")) {
            if run_unchecked(
                self.runner,
                "systemctl",
                &["is-active", "tuxedo-power.service"],
            )?
            .status
                != 0
            {
                return Err(
                    "Restore the system deployment before restarting original desktop clients"
                        .to_string(),
                );
            }
            let launch_sh = Path::new(&state.source)
                .join("launch.sh")
                .display()
                .to_string();
            run_checked(self.runner, "bash", &[&launch_sh], &[])?;
        }

        state.phase = "restored".to_string();
        self.save(&state)?;
        Ok(state)
    }
}

// ---------------------------------------------------------------------------
// 5. Capture gallery (`capture-gallery`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct CaptureGalleryArgs {
    pub build_dir: PathBuf,
    pub output_dir: PathBuf,
}

/// Validates that a theme directory contains exact PNG stems matching `GALLERY_PAGES`.
pub fn verify_gallery_output(theme_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut actual_stems = BTreeSet::new();
    let mut files = Vec::new();
    for entry in fs::read_dir(theme_dir)
        .map_err(|e| format!("Failed reading gallery dir {}: {e}", theme_dir.display()))?
    {
        let entry = entry.map_err(|e| format!("Gallery dir entry error: {e}"))?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("png") {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or("Invalid PNG stem")?
                .to_string();
            actual_stems.insert(stem);
            files.push(path);
        }
    }
    let expected: BTreeSet<String> = GALLERY_PAGES.iter().map(|s| (*s).to_string()).collect();
    if actual_stems != expected {
        return Err(format!(
            "Unexpected gallery files in {}: {actual_stems:?}",
            theme_dir.display()
        ));
    }
    files.sort();
    Ok(files)
}

fn read_build_version(version_h: &Path) -> Result<String, String> {
    let text = fs::read_to_string(version_h)
        .map_err(|e| format!("Failed reading {}: {e}", version_h.display()))?;
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix("#define UCC_VERSION_FULL") {
            let trimmed = rest.trim().trim_matches('"');
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }
    }
    Ok(text.trim().to_string())
}

pub fn capture_gallery(
    args: &CaptureGalleryArgs,
    repo_root: &Path,
    runner: &dyn CommandRunner,
) -> Result<Value, String> {
    let build = fs::canonicalize(&args.build_dir).map_err(|e| {
        format!(
            "Failed resolving build dir {}: {e}",
            args.build_dir.display()
        )
    })?;
    fs::create_dir_all(&args.output_dir)
        .map_err(|e| format!("Failed creating {}: {e}", args.output_dir.display()))?;
    let output = fs::canonicalize(&args.output_dir)
        .map_err(|e| format!("Failed resolving output dir: {e}"))?;

    let build_version = read_build_version(&build.join("include/version.h"))?;
    let audit_script = repo_root
        .join("tests/run_gui_audit_test.sh")
        .display()
        .to_string();
    let audit_bin = build.join("bin/test_gui_audit").display().to_string();

    let mut image_hashes = Map::new();
    for theme in ["light", "dark"] {
        let folder = output.join(theme);
        fs::create_dir_all(&folder)
            .map_err(|e| format!("Failed creating theme dir {}: {e}", folder.display()))?;
        let folder_str = folder.display().to_string();
        run_checked(
            runner,
            "dbus-run-session",
            &[
                "--",
                "bash",
                &audit_script,
                &audit_bin,
                "renderReleaseGallery",
            ],
            &[
                ("UCC_AUDIT_THEME", theme),
                ("UCC_SCREENSHOT_DIR", &folder_str),
            ],
        )?;
        for png in verify_gallery_output(&folder)? {
            let rel = png
                .strip_prefix(&output)
                .map_err(|e| format!("Strip prefix failed: {e}"))?
                .display()
                .to_string();
            image_hashes.insert(rel, Value::String(file_sha256(&png)?));
        }
    }

    let repo_str = repo_root.display().to_string();
    let commit = run_checked(runner, "git", &["-C", &repo_str, "rev-parse", "HEAD"], &[])?
        .stdout
        .trim()
        .to_string();
    let qt_version = run_checked(runner, "pkg-config", &["--modversion", "Qt6Core"], &[])?
        .stdout
        .trim()
        .to_string();

    let manifest = json!({
        "release": env!("CARGO_PKG_VERSION"),
        "buildVersion": build_version,
        "sourceCommit": commit,
        "qtVersion": qt_version,
        "viewport": [1440, 1000],
        "platform": "offscreen",
        "data": "isolated test fixture; no live hardware writes",
        "files": Value::Object(image_hashes),
    });
    write_private_file(
        &output.join("manifest.json"),
        &(serde_json::to_string_pretty(&manifest).unwrap() + "\n"),
        0o644,
    )?;
    Ok(manifest)
}

// ---------------------------------------------------------------------------
// 6. Strict parser-check (`parser-check`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParserCheckReport {
    pub versions_matched: usize,
    pub json_files_checked: usize,
    pub sbom_components: usize,
    pub importer_self_test: bool,
}

struct JsonScanner<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> JsonScanner<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            bytes: input.as_bytes(),
            pos: 0,
        }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len()
            && matches!(self.bytes[self.pos], b' ' | b'\t' | b'\r' | b'\n')
        {
            self.pos += 1;
        }
    }

    fn parse_string(&mut self) -> Result<String, String> {
        self.skip_ws();
        if self.bytes.get(self.pos) != Some(&b'"') {
            return Err("Expected JSON string".to_string());
        }
        self.pos += 1;
        let start = self.pos;
        let mut decoded = String::new();
        let mut raw_from = start;
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if b == b'"' {
                let tail = std::str::from_utf8(&self.bytes[raw_from..self.pos])
                    .map_err(|_| "Invalid UTF-8 in JSON string".to_string())?;
                decoded.push_str(tail);
                self.pos += 1;
                return Ok(decoded);
            }
            if b == b'\\' {
                let chunk = std::str::from_utf8(&self.bytes[raw_from..self.pos])
                    .map_err(|_| "Invalid UTF-8 in JSON string".to_string())?;
                decoded.push_str(chunk);
                self.pos += 1;
                let esc = *self
                    .bytes
                    .get(self.pos)
                    .ok_or("Unterminated escape in JSON string")?;
                match esc {
                    b'"' => decoded.push('"'),
                    b'\\' => decoded.push('\\'),
                    b'/' => decoded.push('/'),
                    b'b' => decoded.push('\x08'),
                    b'f' => decoded.push('\x0c'),
                    b'n' => decoded.push('\n'),
                    b'r' => decoded.push('\r'),
                    b't' => decoded.push('\t'),
                    b'u' => {
                        let hex = self
                            .bytes
                            .get(self.pos + 1..self.pos + 5)
                            .and_then(|s| std::str::from_utf8(s).ok())
                            .ok_or("Invalid unicode escape in JSON string")?;
                        let code = u32::from_str_radix(hex, 16)
                            .map_err(|_| "Invalid hex unicode escape in JSON".to_string())?;
                        if let Some(ch) = char::from_u32(code) {
                            decoded.push(ch);
                        }
                        self.pos += 4;
                    }
                    _ => return Err("Invalid escape in JSON string".to_string()),
                }
                self.pos += 1;
                raw_from = self.pos;
                continue;
            }
            self.pos += 1;
        }
        Err("Unterminated JSON string".to_string())
    }

    fn scan_value(&mut self, path: &str, duplicates: &mut Vec<String>) -> Result<(), String> {
        self.skip_ws();
        let b = *self.bytes.get(self.pos).ok_or("Unexpected end of JSON")?;
        match b {
            b'{' => {
                self.pos += 1;
                self.skip_ws();
                let mut seen = BTreeSet::new();
                if self.bytes.get(self.pos) == Some(&b'}') {
                    self.pos += 1;
                    return Ok(());
                }
                loop {
                    let key = self.parse_string()?;
                    let child_path = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    if !seen.insert(key) {
                        duplicates.push(child_path.clone());
                    }
                    self.skip_ws();
                    if self.bytes.get(self.pos) != Some(&b':') {
                        return Err("Expected ':' in JSON object".to_string());
                    }
                    self.pos += 1;
                    self.scan_value(&child_path, duplicates)?;
                    self.skip_ws();
                    match self.bytes.get(self.pos) {
                        Some(&b',') => self.pos += 1,
                        Some(&b'}') => {
                            self.pos += 1;
                            return Ok(());
                        }
                        _ => return Err("Expected ',' or '}' in JSON object".to_string()),
                    }
                }
            }
            b'[' => {
                self.pos += 1;
                self.skip_ws();
                if self.bytes.get(self.pos) == Some(&b']') {
                    self.pos += 1;
                    return Ok(());
                }
                let mut idx = 0_usize;
                loop {
                    let child_path = format!("{path}[{idx}]");
                    self.scan_value(&child_path, duplicates)?;
                    idx += 1;
                    self.skip_ws();
                    match self.bytes.get(self.pos) {
                        Some(&b',') => self.pos += 1,
                        Some(&b']') => {
                            self.pos += 1;
                            return Ok(());
                        }
                        _ => return Err("Expected ',' or ']' in JSON array".to_string()),
                    }
                }
            }
            b'"' => {
                self.parse_string()?;
                Ok(())
            }
            b't' => {
                if self.bytes.get(self.pos..self.pos + 4) == Some(b"true") {
                    self.pos += 4;
                    Ok(())
                } else {
                    Err("Invalid JSON literal".to_string())
                }
            }
            b'f' => {
                if self.bytes.get(self.pos..self.pos + 5) == Some(b"false") {
                    self.pos += 5;
                    Ok(())
                } else {
                    Err("Invalid JSON literal".to_string())
                }
            }
            b'n' => {
                if self.bytes.get(self.pos..self.pos + 4) == Some(b"null") {
                    self.pos += 4;
                    Ok(())
                } else {
                    Err("Invalid JSON literal".to_string())
                }
            }
            b'-' | b'0'..=b'9' => {
                let start = self.pos;
                while self.pos < self.bytes.len()
                    && matches!(
                        self.bytes[self.pos],
                        b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E'
                    )
                {
                    self.pos += 1;
                }
                let num_str = std::str::from_utf8(&self.bytes[start..self.pos])
                    .map_err(|_| "Invalid JSON number".to_string())?;
                serde_json::from_str::<Value>(num_str)
                    .map_err(|e| format!("Invalid JSON number {num_str}: {e}"))?;
                Ok(())
            }
            other => Err(format!("Unexpected JSON byte: {other}")),
        }
    }
}

/// Scans a JSON string and returns a sorted list of duplicate object key paths.
pub fn find_duplicate_json_keys(input: &str) -> Result<Vec<String>, String> {
    let _: Value = serde_json::from_str(input).map_err(|e| format!("Invalid JSON syntax: {e}"))?;
    let mut scanner = JsonScanner::new(input);
    let mut duplicates = Vec::new();
    scanner.scan_value("", &mut duplicates)?;
    scanner.skip_ws();
    if scanner.pos != scanner.bytes.len() {
        return Err("Trailing characters after JSON document".to_string());
    }
    Ok(duplicates)
}

/// Verifies CycloneDX 1.5 SBOM structure and unique component PURLs.
pub fn verify_cyclonedx_sbom_json(sbom_text: &str) -> Result<usize, String> {
    let duplicates = find_duplicate_json_keys(sbom_text)?;
    if !duplicates.is_empty() {
        return Err(format!("Duplicate keys in SBOM JSON: {duplicates:?}"));
    }
    let val: Value =
        serde_json::from_str(sbom_text).map_err(|e| format!("Invalid SBOM JSON: {e}"))?;
    if val.get("bomFormat").and_then(Value::as_str) != Some("CycloneDX")
        || val.get("specVersion").and_then(Value::as_str) != Some("1.5")
    {
        return Err("SBOM missing CycloneDX 1.5 header".to_string());
    }
    if val
        .get("metadata")
        .and_then(|m| m.get("component"))
        .and_then(|c| c.get("name"))
        .and_then(Value::as_str)
        != Some("ucc-rusted")
    {
        return Err("SBOM metadata.component.name must be ucc-rusted".to_string());
    }
    let components = val
        .get("components")
        .and_then(Value::as_array)
        .ok_or("SBOM missing components array")?;
    if components.is_empty() {
        return Err("SBOM components array must not be empty".to_string());
    }
    let mut seen_purls = BTreeSet::new();
    for comp in components {
        let purl = comp
            .get("purl")
            .and_then(Value::as_str)
            .ok_or("SBOM component missing purl")?;
        if !seen_purls.insert(purl.to_string()) {
            return Err(format!("Duplicate SBOM purl: {purl}"));
        }
    }
    Ok(components.len())
}

pub fn run_parser_check(
    repo_root: &Path,
    versions_matched: usize,
    sbom_text: &str,
) -> Result<ParserCheckReport, String> {
    let json_targets = [
        "docs/rust/baseline-manifest.json",
        "docs/rust/execution/backlog.json",
        "docs/rust/execution/dbus-status.json",
        "docs/rust/execution/environment.json",
        "docs/rust/execution/feature-status.json",
        "docs/rust/execution/findings.json",
        "docs/rust/execution/gui-status.json",
        "docs/rust/execution/intentional-differences.json",
        "docs/rust/execution/ownership.json",
        "docs/rust/execution/state.json",
        "ucc-gnome/metadata.json",
    ];
    for rel in json_targets {
        let path = repo_root.join(rel);
        let text = fs::read_to_string(&path)
            .map_err(|e| format!("Failed reading {}: {e}", path.display()))?;
        let dups = find_duplicate_json_keys(&text)
            .map_err(|e| format!("{} failed JSON validation: {e}", path.display()))?;
        if !dups.is_empty() {
            return Err(format!(
                "Duplicate JSON keys in {}: {dups:?}",
                path.display()
            ));
        }
    }

    let sbom_components = verify_cyclonedx_sbom_json(sbom_text)?;

    // Self-check offline importer invariant in memory (reject unknown LED, accept balanced conversion).
    let data = OfflineUserData {
        config: json!({
            "settings": {
                "ac": { "tuxedo_profile_id": "p", "cpu_profile": "balanced" },
                "type_c": { "tuxedo_profile_id": "p", "cpu_profile": "balanced" },
                "bat": { "tuxedo_profile_id": "p", "cpu_profile": "power-saver" }
            },
            "devices": []
        }),
        keyboard_colors: json!({ "0": [10, 20, 30] }),
        lightbar: json!({}),
        gui_settings: json!({ "theme": "system", "start_minimized": false }),
    };
    let caps = json!({ "zones": 1, "maxBrightness": 50, "ledIds": [0] });
    let base = json!({ "profiles": { "p": { "id": "p", "name": "Base", "fan": {} } } });
    let converted = convert_offline_import(&data, &base, &caps, None)
        .map_err(|e| format!("Offline importer self-check failed: {e}"))?;
    if converted.report["importedKeys"].as_u64() != Some(1) {
        return Err("Offline importer self-check returned unexpected count".to_string());
    }
    let bad_caps = json!({ "zones": 1, "maxBrightness": 50, "ledIds": [7] });
    if convert_offline_import(&data, &base, &bad_caps, None).is_ok() {
        return Err("Offline importer self-check failed to reject mismatched LED".to_string());
    }

    Ok(ParserCheckReport {
        versions_matched,
        json_files_checked: json_targets.len(),
        sbom_components,
        importer_self_test: true,
    })
}
