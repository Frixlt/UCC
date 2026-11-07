//! Hardware abstraction traits and test mock implementations with write journals.
//!
//! Provides `HardwareIo`, `Sysfs`, `ProcessRunner`, `Clock`, and `GpuBackend` traits
//! allowing all higher-level services to run against real Linux kernel interfaces
//! or fully recorded, deterministic in-memory mocks.

use crate::ioctl::IoctlError;
use crate::nvml::NvmlError;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

// ============================================================================
// 1. HardwareIo
// ============================================================================

/// Low-level character device / ioctl interface (e.g. `/dev/tuxedo_io`).
pub trait HardwareIo: Send + Sync {
    /// Read an `i32` value via ioctl.
    fn read_i32(&self, request: u64) -> Result<i32, IoctlError>;

    /// Write an `i32` value via ioctl.
    fn write_i32(&self, request: u64, val: i32) -> Result<(), IoctlError>;

    /// Execute an ioctl without arguments (e.g. `_IO`).
    fn call_none(&self, request: u64) -> Result<(), IoctlError>;

    /// Read a fixed-length C-string via ioctl.
    fn read_str(&self, request: u64, max_len: usize) -> Result<String, IoctlError>;

    /// Checks if the hardware interface device node is open and accessible.
    fn is_available(&self) -> bool;
}

/// A recorded write operation on the hardware character device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HardwareIoWrite {
    WriteI32 {
        request: u64,
        val: i32,
        timestamp: Instant,
    },
    CallNone {
        request: u64,
        timestamp: Instant,
    },
}

/// Thread-safe mock implementation of `HardwareIo` with a write journal.
#[derive(Debug, Default)]
pub struct MockHardwareIo {
    pub available: AtomicBool,
    pub i32_reads: Mutex<HashMap<u64, i32>>,
    pub str_reads: Mutex<HashMap<u64, String>>,
    pub error_requests: Mutex<HashSet<u64>>,
    pub journal: Arc<Mutex<Vec<HardwareIoWrite>>>,
}

impl MockHardwareIo {
    pub fn new() -> Self {
        Self {
            available: AtomicBool::new(true),
            i32_reads: Mutex::new(HashMap::new()),
            str_reads: Mutex::new(HashMap::new()),
            error_requests: Mutex::new(HashSet::new()),
            journal: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn set_available(&self, avail: bool) {
        self.available.store(avail, Ordering::SeqCst);
    }

    pub fn set_i32_read(&self, request: u64, val: i32) {
        self.i32_reads.lock().unwrap().insert(request, val);
    }

    pub fn set_str_read(&self, request: u64, val: impl Into<String>) {
        self.str_reads.lock().unwrap().insert(request, val.into());
    }

    pub fn set_error(&self, request: u64) {
        self.error_requests.lock().unwrap().insert(request);
    }

    pub fn clear_error(&self, request: u64) {
        self.error_requests.lock().unwrap().remove(&request);
    }

    /// Returns a copy of all recorded hardware writes.
    pub fn journal(&self) -> Vec<HardwareIoWrite> {
        self.journal.lock().unwrap().clone()
    }

    /// Returns the shared journal handle.
    pub fn journal_ref(&self) -> Arc<Mutex<Vec<HardwareIoWrite>>> {
        Arc::clone(&self.journal)
    }

    /// Clears the write journal.
    pub fn clear_journal(&self) {
        self.journal.lock().unwrap().clear();
    }
}

impl HardwareIo for MockHardwareIo {
    fn read_i32(&self, request: u64) -> Result<i32, IoctlError> {
        if !self.is_available() {
            return Err(IoctlError::DeviceUnavailable);
        }
        if self.error_requests.lock().unwrap().contains(&request) {
            return Err(IoctlError::IoctlFailed {
                request,
                source: nix::Error::EIO,
            });
        }
        self.i32_reads
            .lock()
            .unwrap()
            .get(&request)
            .copied()
            .ok_or(IoctlError::IoctlFailed {
                request,
                source: nix::Error::ENOTTY,
            })
    }

    fn write_i32(&self, request: u64, val: i32) -> Result<(), IoctlError> {
        if !self.is_available() {
            return Err(IoctlError::DeviceUnavailable);
        }
        if self.error_requests.lock().unwrap().contains(&request) {
            return Err(IoctlError::IoctlFailed {
                request,
                source: nix::Error::EIO,
            });
        }
        self.journal.lock().unwrap().push(HardwareIoWrite::WriteI32 {
            request,
            val,
            timestamp: Instant::now(),
        });
        Ok(())
    }

    fn call_none(&self, request: u64) -> Result<(), IoctlError> {
        if !self.is_available() {
            return Err(IoctlError::DeviceUnavailable);
        }
        if self.error_requests.lock().unwrap().contains(&request) {
            return Err(IoctlError::IoctlFailed {
                request,
                source: nix::Error::EIO,
            });
        }
        self.journal.lock().unwrap().push(HardwareIoWrite::CallNone {
            request,
            timestamp: Instant::now(),
        });
        Ok(())
    }

    fn read_str(&self, request: u64, _max_len: usize) -> Result<String, IoctlError> {
        if !self.is_available() {
            return Err(IoctlError::DeviceUnavailable);
        }
        if self.error_requests.lock().unwrap().contains(&request) {
            return Err(IoctlError::IoctlFailed {
                request,
                source: nix::Error::EIO,
            });
        }
        self.str_reads
            .lock()
            .unwrap()
            .get(&request)
            .cloned()
            .ok_or(IoctlError::IoctlFailed {
                request,
                source: nix::Error::ENOTTY,
            })
    }

    fn is_available(&self) -> bool {
        self.available.load(Ordering::SeqCst)
    }
}

// ============================================================================
// 2. Sysfs
// ============================================================================

/// Trait for reading and writing sysfs / procfs virtual filesystem nodes.
pub trait Sysfs: Send + Sync {
    /// Read entire content of a file to string.
    fn read(&self, path: &Path) -> Result<String, std::io::Error>;

    /// Write string content into a file.
    fn write(&self, path: &Path, content: &str) -> Result<(), std::io::Error>;

    /// Check if the path exists.
    fn exists(&self, path: &Path) -> bool;

    /// Read directory entries.
    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>, std::io::Error>;
}

/// A recorded write operation into a sysfs attribute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SysfsWrite {
    pub path: PathBuf,
    pub content: String,
    pub timestamp: Instant,
}

/// Production implementation of `Sysfs` accessing real Linux filesystem.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealSysfs;

impl Sysfs for RealSysfs {
    fn read(&self, path: &Path) -> Result<String, std::io::Error> {
        std::fs::read_to_string(path)
    }

    fn write(&self, path: &Path, content: &str) -> Result<(), std::io::Error> {
        std::fs::write(path, content)
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(path)? {
            entries.push(entry?.path());
        }
        Ok(entries)
    }
}

/// In-memory mock implementation of `Sysfs` with a write journal.
#[derive(Debug, Default)]
pub struct MockSysfs {
    pub files: Mutex<HashMap<PathBuf, String>>,
    pub fail_reads: Mutex<HashSet<PathBuf>>,
    pub fail_writes: Mutex<HashSet<PathBuf>>,
    pub journal: Arc<Mutex<Vec<SysfsWrite>>>,
}

impl MockSysfs {
    pub fn new() -> Self {
        Self {
            files: Mutex::new(HashMap::new()),
            fail_reads: Mutex::new(HashSet::new()),
            fail_writes: Mutex::new(HashSet::new()),
            journal: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn set_file(&self, path: impl AsRef<Path>, content: impl Into<String>) {
        self.files
            .lock()
            .unwrap()
            .insert(path.as_ref().to_path_buf(), content.into());
    }

    pub fn get_file(&self, path: impl AsRef<Path>) -> Option<String> {
        self.files.lock().unwrap().get(path.as_ref()).cloned()
    }

    pub fn set_read_error(&self, path: impl AsRef<Path>) {
        self.fail_reads
            .lock()
            .unwrap()
            .insert(path.as_ref().to_path_buf());
    }

    pub fn set_write_error(&self, path: impl AsRef<Path>) {
        self.fail_writes
            .lock()
            .unwrap()
            .insert(path.as_ref().to_path_buf());
    }

    pub fn journal(&self) -> Vec<SysfsWrite> {
        self.journal.lock().unwrap().clone()
    }

    pub fn journal_ref(&self) -> Arc<Mutex<Vec<SysfsWrite>>> {
        Arc::clone(&self.journal)
    }

    pub fn clear_journal(&self) {
        self.journal.lock().unwrap().clear();
    }
}

impl Sysfs for MockSysfs {
    fn read(&self, path: &Path) -> Result<String, std::io::Error> {
        if self.fail_reads.lock().unwrap().contains(path) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Simulated read permission denied",
            ));
        }
        self.files.lock().unwrap().get(path).cloned().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("File not found in MockSysfs: {}", path.display()),
            )
        })
    }

    fn write(&self, path: &Path, content: &str) -> Result<(), std::io::Error> {
        if self.fail_writes.lock().unwrap().contains(path) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Simulated write permission denied",
            ));
        }
        let p = path.to_path_buf();
        self.files
            .lock()
            .unwrap()
            .insert(p.clone(), content.to_string());
        self.journal.lock().unwrap().push(SysfsWrite {
            path: p,
            content: content.to_string(),
            timestamp: Instant::now(),
        });
        Ok(())
    }

    fn exists(&self, path: &Path) -> bool {
        self.files.lock().unwrap().contains_key(path)
    }

    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
        let files = self.files.lock().unwrap();
        let mut results = HashSet::new();
        for key in files.keys() {
            if let Ok(rel) = key.strip_prefix(path) {
                if let Some(first_comp) = rel.components().next() {
                    results.insert(path.join(first_comp.as_os_str()));
                }
            }
        }
        if results.is_empty() && !files.keys().any(|k| k == path) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Directory not found: {}", path.display()),
            ));
        }
        Ok(results.into_iter().collect())
    }
}

// ============================================================================
// 3. ProcessRunner
// ============================================================================

/// Output of an executed external program.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProcessOutput {
    pub status_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl ProcessOutput {
    pub fn success(stdout: impl Into<String>) -> Self {
        Self {
            status_code: 0,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    pub fn failure(status_code: i32, stderr: impl Into<String>) -> Self {
        Self {
            status_code,
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }
}

/// An execution record of a process invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessExecution {
    pub program: String,
    pub args: Vec<String>,
    pub timestamp: Instant,
}

/// Trait for executing child processes safely without shell string interpolation.
pub trait ProcessRunner: Send + Sync {
    fn run(&self, program: &str, args: &[&str]) -> Result<ProcessOutput, std::io::Error>;
}

/// Production implementation of `ProcessRunner` using `std::process::Command`.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealProcessRunner;

impl ProcessRunner for RealProcessRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<ProcessOutput, std::io::Error> {
        let output = std::process::Command::new(program).args(args).output()?;
        Ok(ProcessOutput {
            status_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }
}

/// Mock process runner with configurable responses and an execution journal.
#[derive(Debug, Default)]
pub struct MockProcessRunner {
    pub responses: Mutex<HashMap<(String, Vec<String>), ProcessOutput>>,
    pub default_response: Mutex<Option<ProcessOutput>>,
    pub journal: Arc<Mutex<Vec<ProcessExecution>>>,
}

impl MockProcessRunner {
    pub fn new() -> Self {
        Self {
            responses: Mutex::new(HashMap::new()),
            default_response: Mutex::new(None),
            journal: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn set_response(&self, program: &str, args: &[&str], output: ProcessOutput) {
        let args_vec: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        self.responses
            .lock()
            .unwrap()
            .insert((program.to_string(), args_vec), output);
    }

    pub fn set_default_response(&self, output: ProcessOutput) {
        *self.default_response.lock().unwrap() = Some(output);
    }

    pub fn journal(&self) -> Vec<ProcessExecution> {
        self.journal.lock().unwrap().clone()
    }

    pub fn journal_ref(&self) -> Arc<Mutex<Vec<ProcessExecution>>> {
        Arc::clone(&self.journal)
    }

    pub fn clear_journal(&self) {
        self.journal.lock().unwrap().clear();
    }
}

impl ProcessRunner for MockProcessRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<ProcessOutput, std::io::Error> {
        let args_vec: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        self.journal.lock().unwrap().push(ProcessExecution {
            program: program.to_string(),
            args: args_vec.clone(),
            timestamp: Instant::now(),
        });

        if let Some(resp) = self
            .responses
            .lock()
            .unwrap()
            .get(&(program.to_string(), args_vec))
        {
            return Ok(resp.clone());
        }

        if let Some(ref def) = *self.default_response.lock().unwrap() {
            return Ok(def.clone());
        }

        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("No configured mock response for command: {program}"),
        ))
    }
}

// ============================================================================
// 4. Clock
// ============================================================================

/// Trait providing deterministic control over time in production and test environments.
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
    fn system_time(&self) -> SystemTime;
    fn elapsed_since(&self, instant: Instant) -> Duration {
        self.now().saturating_duration_since(instant)
    }
}

/// Real monotonic system clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealClock;

impl Clock for RealClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn system_time(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// Controllable mock clock for deterministic rate limiting and telemetry tests.
#[derive(Debug)]
pub struct MockClock {
    now: Mutex<Instant>,
    system_time: Mutex<SystemTime>,
}

impl Default for MockClock {
    fn default() -> Self {
        Self::new()
    }
}

impl MockClock {
    pub fn new() -> Self {
        Self {
            now: Mutex::new(Instant::now()),
            system_time: Mutex::new(SystemTime::now()),
        }
    }

    pub fn advance(&self, duration: Duration) {
        let mut now = self.now.lock().unwrap();
        *now += duration;
        let mut sys = self.system_time.lock().unwrap();
        *sys += duration;
    }

    pub fn set_now(&self, instant: Instant) {
        *self.now.lock().unwrap() = instant;
    }

    pub fn set_system_time(&self, time: SystemTime) {
        *self.system_time.lock().unwrap() = time;
    }
}

impl Clock for MockClock {
    fn now(&self) -> Instant {
        *self.now.lock().unwrap()
    }

    fn system_time(&self) -> SystemTime {
        *self.system_time.lock().unwrap()
    }
}

// ============================================================================
// 5. GpuBackend
// ============================================================================

/// Consolidated GPU telemetry status.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GpuTelemetry {
    pub temperature_c: i32,
    pub fan_speed_percent: Option<i32>,
    pub power_usage_mw: Option<u32>,
    pub memory_used_bytes: Option<u64>,
    pub memory_total_bytes: Option<u64>,
    pub gpu_utilization_percent: Option<u32>,
    pub memory_utilization_percent: Option<u32>,
    pub is_suspended: bool,
}

/// Recorded GPU control mutations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuWrite {
    SetPowerLimit {
        watts: u32,
        timestamp: Instant,
    },
    SetPowerOffset {
        offset_watts: i32,
        timestamp: Instant,
    },
}

/// Abstract trait for GPU status monitoring and control.
pub trait GpuBackend: Send + Sync {
    fn get_telemetry(&self) -> Result<GpuTelemetry, NvmlError>;
    fn is_suspended(&self) -> bool;
    fn set_power_limit(&self, watts: u32) -> Result<(), NvmlError>;
    fn set_power_offset(&self, offset_watts: i32) -> Result<(), NvmlError>;
}

/// In-memory mock GPU backend with a write journal.
#[derive(Debug, Default)]
pub struct MockGpuBackend {
    pub telemetry: Mutex<GpuTelemetry>,
    pub suspended: AtomicBool,
    pub fail_telemetry: AtomicBool,
    pub fail_power: AtomicBool,
    pub journal: Arc<Mutex<Vec<GpuWrite>>>,
}

impl MockGpuBackend {
    pub fn new() -> Self {
        Self {
            telemetry: Mutex::new(GpuTelemetry::default()),
            suspended: AtomicBool::new(false),
            fail_telemetry: AtomicBool::new(false),
            fail_power: AtomicBool::new(false),
            journal: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn set_telemetry(&self, telemetry: GpuTelemetry) {
        *self.telemetry.lock().unwrap() = telemetry;
    }

    pub fn set_suspended(&self, is_susp: bool) {
        self.suspended.store(is_susp, Ordering::SeqCst);
    }

    pub fn journal(&self) -> Vec<GpuWrite> {
        self.journal.lock().unwrap().clone()
    }

    pub fn journal_ref(&self) -> Arc<Mutex<Vec<GpuWrite>>> {
        Arc::clone(&self.journal)
    }

    pub fn clear_journal(&self) {
        self.journal.lock().unwrap().clear();
    }
}

impl GpuBackend for MockGpuBackend {
    fn get_telemetry(&self) -> Result<GpuTelemetry, NvmlError> {
        if self.is_suspended() {
            return Err(NvmlError::GpuSuspended);
        }
        if self.fail_telemetry.load(Ordering::SeqCst) {
            return Err(NvmlError::NvmlCall(1));
        }
        Ok(self.telemetry.lock().unwrap().clone())
    }

    fn is_suspended(&self) -> bool {
        self.suspended.load(Ordering::SeqCst)
    }

    fn set_power_limit(&self, watts: u32) -> Result<(), NvmlError> {
        if self.is_suspended() {
            return Err(NvmlError::GpuSuspended);
        }
        if self.fail_power.load(Ordering::SeqCst) {
            return Err(NvmlError::NvmlCall(1));
        }
        self.journal.lock().unwrap().push(GpuWrite::SetPowerLimit {
            watts,
            timestamp: Instant::now(),
        });
        Ok(())
    }

    fn set_power_offset(&self, offset_watts: i32) -> Result<(), NvmlError> {
        if self.is_suspended() {
            return Err(NvmlError::GpuSuspended);
        }
        if self.fail_power.load(Ordering::SeqCst) {
            return Err(NvmlError::NvmlCall(1));
        }
        self.journal.lock().unwrap().push(GpuWrite::SetPowerOffset {
            offset_watts,
            timestamp: Instant::now(),
        });
        Ok(())
    }
}
