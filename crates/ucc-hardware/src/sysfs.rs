//! Sysfs and Procfs abstraction layer with type safety, path sandbox isolation,
//! and write journaling for non-destructive testing and hardware resilience.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// Error conditions encountered during sysfs/procfs operations.
#[derive(Debug, thiserror::Error)]
pub enum SysfsError {
    #[error("Sysfs node not found: {0}")]
    NotFound(PathBuf),

    #[error("Permission denied accessing sysfs node: {0}")]
    PermissionDenied(PathBuf),

    #[error("Parse error on sysfs node {path}: {reason}")]
    ParseError { path: PathBuf, reason: String },

    #[error("Unsafe path traversal or out-of-sandbox path: {0}")]
    UnsafePath(PathBuf),

    #[error("Sysfs IO error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl SysfsError {
    pub fn from_io(err: io::Error, path: impl Into<PathBuf>) -> Self {
        let p = path.into();
        match err.kind() {
            io::ErrorKind::NotFound => SysfsError::NotFound(p),
            io::ErrorKind::PermissionDenied => SysfsError::PermissionDenied(p),
            _ => SysfsError::Io { path: p, source: err },
        }
    }
}

/// A recorded entry in the sysfs write journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SysfsJournalEntry {
    pub timestamp: SystemTime,
    pub path: PathBuf,
    pub payload: String,
    pub success: bool,
}

/// Thread-safe journal tracking all sysfs write operations for audit and verification.
#[derive(Debug, Default, Clone)]
pub struct SysfsWriteJournal {
    entries: Arc<Mutex<Vec<SysfsJournalEntry>>>,
}

impl SysfsWriteJournal {
    pub fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn record(&self, path: impl Into<PathBuf>, payload: impl Into<String>, success: bool) {
        let mut list = self.entries.lock().unwrap();
        list.push(SysfsJournalEntry {
            timestamp: SystemTime::now(),
            path: path.into(),
            payload: payload.into(),
            success,
        });
    }

    pub fn entries(&self) -> Vec<SysfsJournalEntry> {
        self.entries.lock().unwrap().clone()
    }

    pub fn count(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    pub fn clear(&self) {
        self.entries.lock().unwrap().clear();
    }
}

/// Isolated root directory for sysfs/procfs access.
///
/// In production, defaults to `/`. In test environments, points to a temporary
/// sandbox directory created with `tempfile`, ensuring zero uncontrolled writes
/// to the live host system.
#[derive(Debug, Clone)]
pub struct SysfsRoot {
    root_path: PathBuf,
    journal: SysfsWriteJournal,
}

impl SysfsRoot {
    /// Creates a SysfsRoot rooted at `/` (production mode).
    pub fn production() -> Self {
        let root = std::env::var("UCC_SYSFS_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/"));
        Self {
            root_path: root,
            journal: SysfsWriteJournal::new(),
        }
    }

    /// Creates a SysfsRoot rooted at a custom directory (test mode).
    pub fn custom(path: impl Into<PathBuf>) -> Self {
        Self {
            root_path: path.into(),
            journal: SysfsWriteJournal::new(),
        }
    }

    /// Returns the underlying base path of this sysfs root.
    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    /// Returns the write journal associated with this sysfs root.
    pub fn journal(&self) -> &SysfsWriteJournal {
        &self.journal
    }

    /// Resolves and sanitizes a relative or absolute sysfs/procfs path inside this root.
    /// Rejects any path traversal attempts (e.g. containing `..`) that escape the root.
    pub fn resolve(&self, target_path: impl AsRef<Path>) -> Result<PathBuf, SysfsError> {
        let target = target_path.as_ref();

        // Check for disallowed path traversal components
        for c in target.components() {
            if let Component::ParentDir = c {
                return Err(SysfsError::UnsafePath(target.to_path_buf()));
            }
        }

        // If root is `/`, return the target path directly (preserving absolute paths)
        if self.root_path == Path::new("/") {
            return Ok(target.to_path_buf());
        }

        // Otherwise strip leading slash and join with custom root
        let relative = if target.has_root() {
            target
                .strip_prefix("/")
                .unwrap_or(target)
        } else {
            target
        };

        Ok(self.root_path.join(relative))
    }

    /// Reads raw string content from a sysfs node.
    pub fn read_string(&self, path: impl AsRef<Path>) -> Result<String, SysfsError> {
        let full_path = self.resolve(path.as_ref())?;
        match fs::read_to_string(&full_path) {
            Ok(s) => Ok(s.trim_end_matches(&['\r', '\n'][..]).to_string()),
            Err(e) => Err(SysfsError::from_io(e, full_path)),
        }
    }

    /// Writes raw string content to a sysfs node, logging to the journal.
    pub fn write_string(&self, path: impl AsRef<Path>, data: &str) -> Result<(), SysfsError> {
        let full_path = self.resolve(path.as_ref())?;
        let res = (|| -> io::Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&full_path)?;
            file.write_all(data.as_bytes())?;
            file.flush()?;
            Ok(())
        })();

        let success = res.is_ok();
        self.journal.record(&full_path, data, success);

        match res {
            Ok(()) => Ok(()),
            Err(e) => Err(SysfsError::from_io(e, full_path)),
        }
    }

    /// Checks if a sysfs node exists and is accessible.
    pub fn exists(&self, path: impl AsRef<Path>) -> bool {
        match self.resolve(path) {
            Ok(p) => p.exists(),
            Err(_) => false,
        }
    }
}

/// Trait for types that can be parsed from and serialized to a sysfs node string.
pub trait SysfsValue: Sized {
    fn parse_sysfs(raw: &str) -> Result<Self, String>;
    fn format_sysfs(&self) -> String;
}

impl SysfsValue for bool {
    fn parse_sysfs(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        match trimmed {
            "1" | "true" | "Y" | "y" => Ok(true),
            "0" | "false" | "N" | "n" => Ok(false),
            _ => Err(format!("Invalid boolean sysfs representation: '{trimmed}'")),
        }
    }

    fn format_sysfs(&self) -> String {
        if *self { "1".to_string() } else { "0".to_string() }
    }
}

macro_rules! impl_sysfs_integer {
    ($($t:ty),*) => {
        $(
            impl SysfsValue for $t {
                fn parse_sysfs(raw: &str) -> Result<Self, String> {
                    let trimmed = raw.trim();
                    <$t>::from_str(trimmed).map_err(|e| format!("Failed to parse integer from '{trimmed}': {e}"))
                }

                fn format_sysfs(&self) -> String {
                    self.to_string()
                }
            }
        )*
    };
}

impl_sysfs_integer!(i8, u8, i16, u16, i32, u32, i64, u64, isize, usize);

impl SysfsValue for String {
    fn parse_sysfs(raw: &str) -> Result<Self, String> {
        Ok(raw.trim().to_string())
    }

    fn format_sysfs(&self) -> String {
        self.clone()
    }
}

/// Parser for Linux CPU range syntax (e.g. "0-7" or "0,2,4-7" or "0-3,8-11").
pub fn parse_cpu_range(s: &str) -> Result<Vec<i32>, String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    let mut result = Vec::new();
    for token in trimmed.split(',') {
        let t = token.trim();
        if t.is_empty() {
            return Err("Empty segment in range list".to_string());
        }

        if let Some((start_s, end_s)) = t.split_once('-') {
            let start = start_s
                .trim()
                .parse::<i32>()
                .map_err(|e| format!("Invalid range start '{start_s}': {e}"))?;
            let end = end_s
                .trim()
                .parse::<i32>()
                .map_err(|e| format!("Invalid range end '{end_s}': {e}"))?;

            if start > end {
                return Err(format!("Inverted range: '{start}-{end}'"));
            }
            if !(0..=4096).contains(&start) || !(0..=4096).contains(&end) {
                return Err(format!("Range out of sane CPU boundaries: '{start}-{end}'"));
            }

            for cpu in start..=end {
                result.push(cpu);
            }
        } else {
            let cpu = t
                .parse::<i32>()
                .map_err(|e| format!("Invalid CPU index '{t}': {e}"))?;
            if !(0..=4096).contains(&cpu) {
                return Err(format!("CPU index out of sane boundaries: '{cpu}'"));
            }
            result.push(cpu);
        }
    }

    result.sort_unstable();
    result.dedup();
    Ok(result)
}

/// Formatter for Linux CPU lists into compact range syntax (e.g. [0,1,2,3,6,7] -> "0-3,6-7").
pub fn format_cpu_range(cpus: &[i32]) -> String {
    if cpus.is_empty() {
        return String::new();
    }

    let mut sorted = cpus.to_vec();
    sorted.sort_unstable();
    sorted.dedup();

    let mut ranges: Vec<String> = Vec::new();
    let mut range_start = sorted[0];
    let mut prev = sorted[0];

    for &curr in &sorted[1..] {
        if curr == prev + 1 {
            prev = curr;
        } else {
            if range_start == prev {
                ranges.push(format!("{range_start}"));
            } else {
                ranges.push(format!("{range_start}-{prev}"));
            }
            range_start = curr;
            prev = curr;
        }
    }

    if range_start == prev {
        ranges.push(format!("{range_start}"));
    } else {
        ranges.push(format!("{range_start}-{prev}"));
    }

    ranges.join(",")
}

impl SysfsValue for Vec<i32> {
    fn parse_sysfs(raw: &str) -> Result<Self, String> {
        parse_cpu_range(raw)
    }

    fn format_sysfs(&self) -> String {
        format_cpu_range(self)
    }
}

impl SysfsValue for Vec<String> {
    fn parse_sysfs(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }
        Ok(trimmed
            .split_whitespace()
            .map(|s| s.to_string())
            .collect())
    }

    fn format_sysfs(&self) -> String {
        self.join(" ")
    }
}

/// Type-safe typed sysfs node handle pointing to a specific path within a SysfsRoot.
#[derive(Debug, Clone)]
pub struct SysfsNode<T: SysfsValue> {
    root: SysfsRoot,
    path: PathBuf,
    _phantom: std::marker::PhantomData<T>,
}

impl<T: SysfsValue> SysfsNode<T> {
    /// Creates a new typed sysfs node handle.
    pub fn new(root: &SysfsRoot, path: impl Into<PathBuf>) -> Self {
        Self {
            root: root.clone(),
            path: path.into(),
            _phantom: std::marker::PhantomData,
        }
    }

    /// Path of this sysfs node relative or absolute.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Checks if this node exists and is accessible.
    pub fn is_available(&self) -> bool {
        self.root.exists(&self.path)
    }

    /// Reads and parses the value from this sysfs node.
    pub fn read(&self) -> Result<T, SysfsError> {
        let raw = self.root.read_string(&self.path)?;
        T::parse_sysfs(&raw).map_err(|reason| SysfsError::ParseError {
            path: self.path.clone(),
            reason,
        })
    }

    /// Formats and writes the value to this sysfs node.
    pub fn write(&self, value: &T) -> Result<(), SysfsError> {
        let formatted = value.format_sysfs();
        self.root.write_string(&self.path, &formatted)
    }
}
