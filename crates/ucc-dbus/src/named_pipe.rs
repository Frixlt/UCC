// SPDX-License-Identifier: GPL-3.0-or-later

//! Windows Named Pipes IPC transport with SDDL ACL/SID enforcement (`M15.1`, `ADR-06`).
//!
//! Provides:
//! - [`WINDOWS_PIPE_SDDL`] and [`PipeAclPolicy`] parser/validator enforcing local-only SID
//!   restrictions (`SY`, `BA`, `IU`; denying `WD`/`Everyone`, `AN`/`Anonymous`, `NU`/`Network`).
//! - Portable [`NamedPipeTransport`], [`NamedPipeConnection`], [`MockNamedPipeTransport`], and
//!   [`NamedPipeServer`] serving framed JSON-RPC requests via [`dispatch_json_rpc_with_identity`]
//!   with [`MAX_IPC_FRAME_BYTES`] enforcement and clean `AtomicBool` shutdown.
//! - Real `#[cfg(windows)]` [`Win32NamedPipeTransport`] and [`Win32NamedPipeClient`] bound directly
//!   to `kernel32` and `advapi32` with zero additional Cargo dependencies.

use crate::backend::UccdBackend;
use crate::ipc::{
    decode_frame_bounded, dispatch_json_rpc_with_identity, encode_frame, CallerIdentity,
    IpcRequest, IpcResponse, MAX_IPC_FRAME_BYTES, WINDOWS_NAMED_PIPE_PATH,
};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use thiserror::Error;

/// Default SDDL DACL for `\\.\pipe\ucc-rusted`:
/// - `D:P` — Protected DACL (no inherited ACEs)
/// - `(A;;GA;;;SY)` — LocalSystem: Generic All
/// - `(A;;GA;;;BA)` — Local Administrators: Generic All
/// - `(A;;GRGW;;;IU)` — Interactive local users: Generic Read + Generic Write
///
/// Explicitly omits `WD` (Everyone), `AN` (Anonymous), `NU` (Network), `AU` (Authenticated Users),
/// and `BG` (Built-in Guests).
pub const WINDOWS_PIPE_SDDL: &str = "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)";

const FORBIDDEN_ALLOW_SIDS: &[&str] = &["WD", "AN", "NU", "AU", "BG"];
const REQUIRED_ALLOW_SIDS: &[&str] = &["SY", "BA", "IU"];

/// Validation errors returned by [`PipeAclPolicy::parse_and_validate`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PipeAclError {
    #[error("SDDL must start with a protected DACL ('D:P')")]
    MissingProtectedDacl,
    #[error("SDDL contains malformed ACE syntax: {0}")]
    MalformedAce(String),
    #[error("SDDL allows forbidden principal SID '{0}'")]
    ForbiddenAllowSid(String),
    #[error("SDDL interactive user ('IU') ACE grants overly broad rights '{0}'")]
    ExcessiveInteractiveRights(String),
    #[error("SDDL is missing required allow ACE for principal SID '{0}'")]
    MissingRequiredSid(String),
}

/// Single parsed SDDL Access Control Entry `(ace_type;ace_flags;rights;obj;inherit_obj;sid)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipeAce {
    pub ace_type: String,
    pub ace_flags: String,
    pub rights: String,
    pub object_guid: String,
    pub inherit_object_guid: String,
    pub account_sid: String,
}

/// Validated SDDL ACL policy for the UCC Windows Named Pipe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipeAclPolicy {
    sddl: String,
    aces: Vec<PipeAce>,
}

impl PipeAclPolicy {
    /// Parses and validates an SDDL string against UCC pipe security invariants:
    /// - Must start with protected DACL prefix `D:P`
    /// - Must not allow `WD` (Everyone), `AN` (Anonymous), `NU` (Network), `AU`, or `BG`
    /// - Must allow `SY` (LocalSystem), `BA` (Administrators), and `IU` (Interactive Users)
    /// - Must not grant `GA` (Generic All), `WD` (Write DAC), or `WO` (Write Owner) to `IU`
    pub fn parse_and_validate(sddl: &str) -> Result<Self, PipeAclError> {
        let trimmed = sddl.trim();
        let Some(rest) = trimmed.strip_prefix("D:P") else {
            return Err(PipeAclError::MissingProtectedDacl);
        };

        let mut aces = Vec::new();
        let mut remaining = rest;
        while !remaining.is_empty() {
            let Some(after_open) = remaining.strip_prefix('(') else {
                return Err(PipeAclError::MalformedAce(remaining.to_string()));
            };
            let Some(close_idx) = after_open.find(')') else {
                return Err(PipeAclError::MalformedAce(remaining.to_string()));
            };
            let body = &after_open[..close_idx];
            let fields: Vec<&str> = body.split(';').collect();
            if fields.len() != 6 {
                return Err(PipeAclError::MalformedAce(format!("({body})")));
            }
            let ace = PipeAce {
                ace_type: fields[0].trim().to_string(),
                ace_flags: fields[1].trim().to_string(),
                rights: fields[2].trim().to_string(),
                object_guid: fields[3].trim().to_string(),
                inherit_object_guid: fields[4].trim().to_string(),
                account_sid: fields[5].trim().to_string(),
            };
            if ace.ace_type.is_empty() || ace.rights.is_empty() || ace.account_sid.is_empty() {
                return Err(PipeAclError::MalformedAce(format!("({body})")));
            }
            aces.push(ace);
            remaining = &after_open[close_idx + 1..];
        }

        if aces.is_empty() {
            return Err(PipeAclError::MalformedAce("no ACE entries present".into()));
        }

        for ace in &aces {
            if ace.ace_type == "A" {
                if FORBIDDEN_ALLOW_SIDS.contains(&ace.account_sid.as_str()) {
                    return Err(PipeAclError::ForbiddenAllowSid(ace.account_sid.clone()));
                }
                if ace.account_sid == "IU"
                    && (ace.rights.contains("GA")
                        || ace.rights.contains("WD")
                        || ace.rights.contains("WO"))
                {
                    return Err(PipeAclError::ExcessiveInteractiveRights(ace.rights.clone()));
                }
            }
        }

        for required_sid in REQUIRED_ALLOW_SIDS {
            let has_allow = aces
                .iter()
                .any(|ace| ace.ace_type == "A" && ace.account_sid == *required_sid);
            if !has_allow {
                return Err(PipeAclError::MissingRequiredSid(
                    (*required_sid).to_string(),
                ));
            }
        }

        Ok(Self {
            sddl: trimmed.to_string(),
            aces,
        })
    }

    /// Returns the default hardened SDDL policy ([`WINDOWS_PIPE_SDDL`]).
    pub fn default_policy() -> Self {
        Self {
            sddl: WINDOWS_PIPE_SDDL.to_string(),
            aces: vec![
                PipeAce {
                    ace_type: "A".to_string(),
                    ace_flags: String::new(),
                    rights: "GA".to_string(),
                    object_guid: String::new(),
                    inherit_object_guid: String::new(),
                    account_sid: "SY".to_string(),
                },
                PipeAce {
                    ace_type: "A".to_string(),
                    ace_flags: String::new(),
                    rights: "GA".to_string(),
                    object_guid: String::new(),
                    inherit_object_guid: String::new(),
                    account_sid: "BA".to_string(),
                },
                PipeAce {
                    ace_type: "A".to_string(),
                    ace_flags: String::new(),
                    rights: "GRGW".to_string(),
                    object_guid: String::new(),
                    inherit_object_guid: String::new(),
                    account_sid: "IU".to_string(),
                },
            ],
        }
    }

    /// Returns the validated SDDL string.
    pub fn sddl(&self) -> &str {
        &self.sddl
    }

    /// Returns the parsed ACE list.
    pub fn aces(&self) -> &[PipeAce] {
        &self.aces
    }

    /// Returns `true` if the policy contains an Allow ACE for `sid` (and no Deny ACE).
    pub fn allows_sid(&self, sid: &str) -> bool {
        let denied = self
            .aces
            .iter()
            .any(|ace| ace.ace_type == "D" && ace.account_sid == sid);
        if denied {
            return false;
        }
        self.aces
            .iter()
            .any(|ace| ace.ace_type == "A" && ace.account_sid == sid)
    }

    /// Returns `true` if `sid` is explicitly denied or absent from Allow ACEs.
    pub fn denies_or_omits_sid(&self, sid: &str) -> bool {
        !self.allows_sid(sid)
    }

    /// Returns `true` when `sid` represents a privileged Windows principal (`SY` or `BA`).
    pub fn is_privileged_sid(sid: &str) -> bool {
        matches!(sid, "SY" | "BA")
    }
}

/// Errors returned by Named Pipe transports and server operations.
#[derive(Debug, Error)]
pub enum NamedPipeError {
    #[error("pipe ACL policy error: {0}")]
    InvalidAcl(#[from] PipeAclError),
    #[error("IPC frame exceeds maximum size of {max} bytes (got {actual} bytes)")]
    FrameTooLarge { actual: usize, max: usize },
    #[error("invalid UTF-8 or JSON frame: {0}")]
    InvalidFrame(String),
    #[error("named pipe I/O error: {0}")]
    Io(String),
    #[error("client disconnected")]
    Disconnected,
    #[error("named pipe server shutting down")]
    Shutdown,
}

/// A single connected client session over a Named Pipe transport.
pub trait NamedPipeConnection: Send {
    /// Returns the verified caller identity for this client connection.
    fn caller_identity(&self) -> CallerIdentity;

    /// Reads one newline-delimited UTF-8 frame bounded by `max_bytes`.
    /// Returns `Ok(None)` on clean EOF.
    fn read_frame(&mut self, max_bytes: usize) -> Result<Option<String>, NamedPipeError>;

    /// Writes one newline-delimited UTF-8 response frame.
    fn write_frame(&mut self, frame: &str) -> Result<(), NamedPipeError>;
}

/// Platform-abstracted Named Pipe server transport.
pub trait NamedPipeTransport: Send + Sync {
    type Connection: NamedPipeConnection;

    /// Named pipe path (e.g. `\\.\pipe\ucc-rusted`).
    fn pipe_path(&self) -> &str;

    /// Validated SDDL ACL policy applied to the pipe.
    fn acl_policy(&self) -> &PipeAclPolicy;

    /// Accepts the next client connection, or returns `Ok(None)` when no further connections
    /// are pending (e.g. mock queue drained or shutdown).
    fn accept(&self) -> Result<Option<Self::Connection>, NamedPipeError>;
}

/// In-memory mock connection for cross-platform testing of Named Pipe IPC.
#[derive(Debug)]
pub struct MockNamedPipeConnection {
    caller: CallerIdentity,
    incoming: VecDeque<String>,
    outgoing: Arc<Mutex<Vec<String>>>,
}

impl MockNamedPipeConnection {
    /// Creates a new mock client connection with pre-queued request frames.
    pub fn new(caller: CallerIdentity, frames: Vec<String>) -> (Self, Arc<Mutex<Vec<String>>>) {
        let outgoing = Arc::new(Mutex::new(Vec::new()));
        let conn = Self {
            caller,
            incoming: VecDeque::from(frames),
            outgoing: Arc::clone(&outgoing),
        };
        (conn, outgoing)
    }
}

impl NamedPipeConnection for MockNamedPipeConnection {
    fn caller_identity(&self) -> CallerIdentity {
        self.caller.clone()
    }

    fn read_frame(&mut self, max_bytes: usize) -> Result<Option<String>, NamedPipeError> {
        let Some(frame) = self.incoming.pop_front() else {
            return Ok(None);
        };
        if frame.len() > max_bytes {
            return Err(NamedPipeError::FrameTooLarge {
                actual: frame.len(),
                max: max_bytes,
            });
        }
        Ok(Some(frame))
    }

    fn write_frame(&mut self, frame: &str) -> Result<(), NamedPipeError> {
        let mut out = self.outgoing.lock().unwrap_or_else(|e| e.into_inner());
        out.push(frame.to_string());
        Ok(())
    }
}

/// In-memory mock Named Pipe transport supporting SDDL validation and queued client sessions.
#[derive(Debug)]
pub struct MockNamedPipeTransport {
    pipe_path: String,
    acl_policy: PipeAclPolicy,
    pending_connections: Mutex<VecDeque<MockNamedPipeConnection>>,
}

impl MockNamedPipeTransport {
    /// Creates a new mock transport after validating `sddl` via [`PipeAclPolicy`].
    pub fn new(pipe_path: impl Into<String>, sddl: &str) -> Result<Self, NamedPipeError> {
        let acl_policy = PipeAclPolicy::parse_and_validate(sddl)?;
        Ok(Self {
            pipe_path: pipe_path.into(),
            acl_policy,
            pending_connections: Mutex::new(VecDeque::new()),
        })
    }

    /// Creates a mock transport configured with [`WINDOWS_NAMED_PIPE_PATH`] and
    /// [`WINDOWS_PIPE_SDDL`].
    pub fn default_secure() -> Self {
        Self {
            pipe_path: WINDOWS_NAMED_PIPE_PATH.to_string(),
            acl_policy: PipeAclPolicy::default_policy(),
            pending_connections: Mutex::new(VecDeque::new()),
        }
    }

    /// Enqueues a mock client session and returns a handle to capture response frames written
    /// back to that client.
    pub fn enqueue_session(
        &self,
        caller: CallerIdentity,
        frames: Vec<String>,
    ) -> Arc<Mutex<Vec<String>>> {
        let (conn, outgoing) = MockNamedPipeConnection::new(caller, frames);
        let mut queue = self
            .pending_connections
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        queue.push_back(conn);
        outgoing
    }
}

impl NamedPipeTransport for MockNamedPipeTransport {
    type Connection = MockNamedPipeConnection;

    fn pipe_path(&self) -> &str {
        &self.pipe_path
    }

    fn acl_policy(&self) -> &PipeAclPolicy {
        &self.acl_policy
    }

    fn accept(&self) -> Result<Option<Self::Connection>, NamedPipeError> {
        let mut queue = self
            .pending_connections
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        Ok(queue.pop_front())
    }
}

/// Summary counters returned by [`NamedPipeServer::run_until_shutdown`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ServerStats {
    pub connections_served: usize,
    pub requests_served: usize,
    pub rejected_frames: usize,
}

/// Framed JSON-RPC server running over any [`NamedPipeTransport`].
pub struct NamedPipeServer<T: NamedPipeTransport> {
    transport: T,
    backend: Arc<dyn UccdBackend>,
    shutdown: Arc<AtomicBool>,
    max_frame_bytes: usize,
}

impl<T: NamedPipeTransport> NamedPipeServer<T> {
    /// Creates a new server bound to `transport` and `backend` using [`MAX_IPC_FRAME_BYTES`].
    pub fn new(transport: T, backend: Arc<dyn UccdBackend>) -> Self {
        Self::with_shutdown_flag(transport, backend, Arc::new(AtomicBool::new(false)))
    }

    /// Creates a new server sharing an externally controlled `shutdown` flag.
    pub fn with_shutdown_flag(
        transport: T,
        backend: Arc<dyn UccdBackend>,
        shutdown: Arc<AtomicBool>,
    ) -> Self {
        Self {
            transport,
            backend,
            shutdown,
            max_frame_bytes: MAX_IPC_FRAME_BYTES,
        }
    }

    /// Returns a cloned handle to the server's cooperative shutdown flag.
    pub fn shutdown_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.shutdown)
    }

    /// Signals the server loop to stop accepting and processing further requests.
    pub fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }

    /// Returns `true` if shutdown has been requested.
    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    /// Returns a reference to the underlying transport.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Serves all frames on a single client connection until EOF, shutdown, or oversized frame.
    ///
    /// Returns `(requests_served, rejected_frames)`.
    pub fn serve_connection(
        &self,
        conn: &mut T::Connection,
    ) -> Result<(usize, usize), NamedPipeError> {
        let caller = conn.caller_identity();
        let mut requests_served = 0usize;
        let mut rejected_frames = 0usize;

        while !self.is_shutdown() {
            match conn.read_frame(self.max_frame_bytes) {
                Ok(None) => break,
                Ok(Some(raw_frame)) => {
                    let resp = match decode_frame_bounded::<IpcRequest>(
                        &raw_frame,
                        self.max_frame_bytes,
                    ) {
                        Ok(req) => {
                            requests_served += 1;
                            dispatch_json_rpc_with_identity(self.backend.as_ref(), &req, &caller)
                        }
                        Err(err) => {
                            rejected_frames += 1;
                            IpcResponse::failure(0, format!("invalid IPC frame: {err}"))
                        }
                    };
                    let wire = encode_frame(&resp)
                        .map_err(|e| NamedPipeError::InvalidFrame(e.to_string()))?;
                    conn.write_frame(&wire)?;
                }
                Err(NamedPipeError::FrameTooLarge { actual, max }) => {
                    rejected_frames += 1;
                    let resp = IpcResponse::failure(
                        0,
                        format!(
                            "IPC frame exceeds maximum size of {max} bytes (got {actual} bytes)"
                        ),
                    );
                    let wire = encode_frame(&resp)
                        .map_err(|e| NamedPipeError::InvalidFrame(e.to_string()))?;
                    let _ = conn.write_frame(&wire);
                    break;
                }
                Err(NamedPipeError::Disconnected) => break,
                Err(err) => return Err(err),
            }
        }

        Ok((requests_served, rejected_frames))
    }

    /// Accepts and serves connections until shutdown is requested or the transport drains.
    pub fn run_until_shutdown(&self) -> Result<ServerStats, NamedPipeError> {
        let mut stats = ServerStats::default();
        while !self.is_shutdown() {
            let Some(mut conn) = self.transport.accept()? else {
                break;
            };
            stats.connections_served += 1;
            let (reqs, rejected) = self.serve_connection(&mut conn)?;
            stats.requests_served += reqs;
            stats.rejected_frames += rejected;
        }
        Ok(stats)
    }
}

#[cfg(windows)]
mod win32 {
    use super::*;
    use std::ffi::{c_void, OsStr};
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    const PIPE_ACCESS_DUPLEX: u32 = 0x0000_0003;
    const FILE_FLAG_FIRST_PIPE_INSTANCE: u32 = 0x0008_0000;
    const PIPE_TYPE_BYTE: u32 = 0x0000_0000;
    const PIPE_READMODE_BYTE: u32 = 0x0000_0000;
    const PIPE_WAIT: u32 = 0x0000_0000;
    const PIPE_REJECT_REMOTE_CLIENTS: u32 = 0x0000_0008;
    const PIPE_UNLIMITED_INSTANCES: u32 = 255;
    const SDDL_REVISION_1: u32 = 1;
    const ERROR_PIPE_CONNECTED: u32 = 535;
    const ERROR_BROKEN_PIPE: u32 = 109;
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const OPEN_EXISTING: u32 = 3;
    const WIN_BUILTIN_ADMINISTRATORS_SID: u32 = 26;
    const WIN_LOCAL_SYSTEM_SID: u32 = 22;

    fn invalid_handle_value() -> *mut c_void {
        (-1isize) as *mut c_void
    }

    #[repr(C)]
    struct SecurityAttributes {
        n_length: u32,
        lp_security_descriptor: *mut c_void,
        b_inherit_handle: i32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateNamedPipeW(
            lp_name: *const u16,
            dw_open_mode: u32,
            dw_pipe_mode: u32,
            n_max_instances: u32,
            n_out_buffer_size: u32,
            n_in_buffer_size: u32,
            n_default_time_out: u32,
            lp_security_attributes: *const SecurityAttributes,
        ) -> *mut c_void;
        fn ConnectNamedPipe(h_named_pipe: *mut c_void, lp_overlapped: *mut c_void) -> i32;
        fn DisconnectNamedPipe(h_named_pipe: *mut c_void) -> i32;
        fn ReadFile(
            h_file: *mut c_void,
            lp_buffer: *mut c_void,
            n_number_of_bytes_to_read: u32,
            lp_number_of_bytes_read: *mut u32,
            lp_overlapped: *mut c_void,
        ) -> i32;
        fn WriteFile(
            h_file: *mut c_void,
            lp_buffer: *const c_void,
            n_number_of_bytes_to_write: u32,
            lp_number_of_bytes_written: *mut u32,
            lp_overlapped: *mut c_void,
        ) -> i32;
        fn FlushFileBuffers(h_file: *mut c_void) -> i32;
        fn CloseHandle(h_object: *mut c_void) -> i32;
        fn CreateFileW(
            lp_file_name: *const u16,
            dw_desired_access: u32,
            dw_share_mode: u32,
            lp_security_attributes: *const SecurityAttributes,
            dw_creation_disposition: u32,
            dw_flags_and_attributes: u32,
            h_template_file: *mut c_void,
        ) -> *mut c_void;
        fn LocalFree(h_mem: *mut c_void) -> *mut c_void;
        fn GetLastError() -> u32;
    }

    #[link(name = "advapi32")]
    extern "system" {
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            string_security_descriptor: *const u16,
            string_sd_revision: u32,
            security_descriptor: *mut *mut c_void,
            security_descriptor_size: *mut u32,
        ) -> i32;
        fn ImpersonateNamedPipeClient(h_named_pipe: *mut c_void) -> i32;
        fn RevertToSelf() -> i32;
        fn CreateWellKnownSid(
            well_known_sid_type: u32,
            domain_sid: *const c_void,
            p_sid: *mut c_void,
            cb_sid: *mut u32,
        ) -> i32;
        fn CheckTokenMembership(
            token_handle: *mut c_void,
            sid_to_check: *const c_void,
            is_member: *mut i32,
        ) -> i32;
    }

    fn encode_wide(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(Some(0)).collect()
    }

    /// RAII wrapper ensuring Win32 `HANDLE` values are always closed on drop.
    struct Win32Handle(*mut c_void);

    // SAFETY: Win32 pipe handles are kernel objects safe to transfer between threads.
    unsafe impl Send for Win32Handle {}

    impl Win32Handle {
        fn new(raw: *mut c_void) -> Option<Self> {
            if raw.is_null() || raw == invalid_handle_value() {
                None
            } else {
                Some(Self(raw))
            }
        }

        fn as_raw(&self) -> *mut c_void {
            self.0
        }
    }

    impl Drop for Win32Handle {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != invalid_handle_value() {
                // SAFETY: `self.0` is a valid owned handle acquired from Win32 APIs.
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }

    /// RAII wrapper ensuring `SECURITY_DESCRIPTOR` memory allocated by
    /// `ConvertStringSecurityDescriptorToSecurityDescriptorW` is freed via `LocalFree`.
    struct SecurityDescriptorGuard(*mut c_void);

    impl SecurityDescriptorGuard {
        fn from_sddl(sddl: &str) -> Result<Self, NamedPipeError> {
            let wide = encode_wide(sddl);
            let mut sd: *mut c_void = ptr::null_mut();
            let mut sd_size: u32 = 0;
            // SAFETY: `wide` is null-terminated UTF-16; output pointers are valid stack locals.
            let ok = unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    wide.as_ptr(),
                    SDDL_REVISION_1,
                    &mut sd,
                    &mut sd_size,
                )
            };
            if ok == 0 || sd.is_null() {
                let err = unsafe { GetLastError() };
                return Err(NamedPipeError::Io(format!(
                    "SDDL conversion failed (Win32 error {err})"
                )));
            }
            Ok(Self(sd))
        }

        fn as_ptr(&self) -> *mut c_void {
            self.0
        }
    }

    impl Drop for SecurityDescriptorGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: `self.0` was allocated by SDDL conversion; free via LocalFree.
                unsafe {
                    LocalFree(self.0);
                }
            }
        }
    }

    fn is_impersonated_member_of(well_known_sid: u32) -> bool {
        let mut sid_buf = [0u8; 68];
        let mut sid_len = sid_buf.len() as u32;
        // SAFETY: `sid_buf` is 68 bytes (SECURITY_MAX_SID_SIZE).
        let created = unsafe {
            CreateWellKnownSid(
                well_known_sid,
                ptr::null(),
                sid_buf.as_mut_ptr().cast::<c_void>(),
                &mut sid_len,
            )
        };
        if created == 0 {
            return false;
        }
        let mut is_member: i32 = 0;
        // SAFETY: Passing NULL token checks the current thread's impersonation token.
        let checked = unsafe {
            CheckTokenMembership(
                ptr::null_mut(),
                sid_buf.as_ptr().cast::<c_void>(),
                &mut is_member,
            )
        };
        checked != 0 && is_member != 0
    }

    fn inspect_pipe_client_identity(handle: *mut c_void) -> CallerIdentity {
        // SAFETY: `handle` is an active connected named pipe handle.
        if unsafe { ImpersonateNamedPipeClient(handle) } == 0 {
            return CallerIdentity::interactive("windows-pipe-client");
        }
        let is_system = is_impersonated_member_of(WIN_LOCAL_SYSTEM_SID);
        let is_admin = !is_system && is_impersonated_member_of(WIN_BUILTIN_ADMINISTRATORS_SID);
        // SAFETY: Always revert impersonation before returning to daemon context.
        unsafe {
            RevertToSelf();
        }
        if is_system {
            CallerIdentity::privileged("SY")
        } else if is_admin {
            CallerIdentity::privileged("BA")
        } else {
            CallerIdentity::interactive("IU")
        }
    }

    fn read_framed_line(
        handle: *mut c_void,
        pending: &mut Vec<u8>,
        max_bytes: usize,
    ) -> Result<Option<String>, NamedPipeError> {
        loop {
            if let Some(pos) = pending.iter().position(|&b| b == b'\n') {
                let line_bytes: Vec<u8> = pending.drain(..=pos).collect();
                if line_bytes.len() > max_bytes {
                    return Err(NamedPipeError::FrameTooLarge {
                        actual: line_bytes.len(),
                        max: max_bytes,
                    });
                }
                let line = String::from_utf8(line_bytes)
                    .map_err(|e| NamedPipeError::InvalidFrame(e.to_string()))?;
                return Ok(Some(line));
            }

            if pending.len() > max_bytes {
                let actual = pending.len();
                pending.clear();
                return Err(NamedPipeError::FrameTooLarge {
                    actual,
                    max: max_bytes,
                });
            }

            let mut chunk = [0u8; 4096];
            let mut bytes_read: u32 = 0;
            // SAFETY: `chunk` and `bytes_read` are valid writable buffers.
            let ok = unsafe {
                ReadFile(
                    handle,
                    chunk.as_mut_ptr().cast::<c_void>(),
                    chunk.len() as u32,
                    &mut bytes_read,
                    ptr::null_mut(),
                )
            };
            if ok == 0 {
                let err = unsafe { GetLastError() };
                if err == ERROR_BROKEN_PIPE {
                    if pending.is_empty() {
                        return Ok(None);
                    }
                    let tail = std::mem::take(pending);
                    let line = String::from_utf8(tail)
                        .map_err(|e| NamedPipeError::InvalidFrame(e.to_string()))?;
                    return Ok(Some(line));
                }
                return Err(NamedPipeError::Io(format!(
                    "ReadFile failed (Win32 error {err})"
                )));
            }
            if bytes_read == 0 {
                return Ok(None);
            }
            pending.extend_from_slice(&chunk[..bytes_read as usize]);
        }
    }

    fn write_all_and_flush(handle: *mut c_void, frame: &str) -> Result<(), NamedPipeError> {
        let bytes = frame.as_bytes();
        let mut offset = 0usize;
        while offset < bytes.len() {
            let remaining = &bytes[offset..];
            let to_write = u32::try_from(remaining.len()).unwrap_or(u32::MAX);
            let mut written: u32 = 0;
            // SAFETY: `remaining` is valid for `to_write` bytes.
            let ok = unsafe {
                WriteFile(
                    handle,
                    remaining.as_ptr().cast::<c_void>(),
                    to_write,
                    &mut written,
                    ptr::null_mut(),
                )
            };
            if ok == 0 {
                let err = unsafe { GetLastError() };
                return Err(NamedPipeError::Io(format!(
                    "WriteFile failed (Win32 error {err})"
                )));
            }
            if written == 0 {
                return Err(NamedPipeError::Disconnected);
            }
            offset += written as usize;
        }
        // SAFETY: `handle` is an active pipe handle.
        let _ = unsafe { FlushFileBuffers(handle) };
        Ok(())
    }

    /// Connected Win32 Named Pipe server instance with automatic disconnect and handle cleanup.
    pub struct Win32NamedPipeConnection {
        handle: Win32Handle,
        caller: CallerIdentity,
        pending: Vec<u8>,
    }

    impl Drop for Win32NamedPipeConnection {
        fn drop(&mut self) {
            // SAFETY: Flushing and disconnecting an owned server pipe handle before close.
            unsafe {
                FlushFileBuffers(self.handle.as_raw());
                DisconnectNamedPipe(self.handle.as_raw());
            }
        }
    }

    impl NamedPipeConnection for Win32NamedPipeConnection {
        fn caller_identity(&self) -> CallerIdentity {
            self.caller.clone()
        }

        fn read_frame(&mut self, max_bytes: usize) -> Result<Option<String>, NamedPipeError> {
            read_framed_line(self.handle.as_raw(), &mut self.pending, max_bytes)
        }

        fn write_frame(&mut self, frame: &str) -> Result<(), NamedPipeError> {
            write_all_and_flush(self.handle.as_raw(), frame)
        }
    }

    /// Real Windows Named Pipe server transport backed by `kernel32` and `advapi32`.
    pub struct Win32NamedPipeTransport {
        pipe_path: String,
        acl_policy: PipeAclPolicy,
        first_instance: AtomicBool,
    }

    impl Win32NamedPipeTransport {
        /// Creates a new Win32 Named Pipe transport after validating `sddl`.
        pub fn new(pipe_path: impl Into<String>, sddl: &str) -> Result<Self, NamedPipeError> {
            let acl_policy = PipeAclPolicy::parse_and_validate(sddl)?;
            Ok(Self {
                pipe_path: pipe_path.into(),
                acl_policy,
                first_instance: AtomicBool::new(true),
            })
        }

        /// Creates a transport for [`WINDOWS_NAMED_PIPE_PATH`] with [`WINDOWS_PIPE_SDDL`].
        pub fn default_secure() -> Self {
            Self {
                pipe_path: WINDOWS_NAMED_PIPE_PATH.to_string(),
                acl_policy: PipeAclPolicy::default_policy(),
                first_instance: AtomicBool::new(true),
            }
        }
    }

    impl NamedPipeTransport for Win32NamedPipeTransport {
        type Connection = Win32NamedPipeConnection;

        fn pipe_path(&self) -> &str {
            &self.pipe_path
        }

        fn acl_policy(&self) -> &PipeAclPolicy {
            &self.acl_policy
        }

        fn accept(&self) -> Result<Option<Self::Connection>, NamedPipeError> {
            let sd_guard = SecurityDescriptorGuard::from_sddl(self.acl_policy.sddl())?;
            let sa = SecurityAttributes {
                n_length: std::mem::size_of::<SecurityAttributes>() as u32,
                lp_security_descriptor: sd_guard.as_ptr(),
                b_inherit_handle: 0,
            };

            let mut open_mode = PIPE_ACCESS_DUPLEX;
            if self.first_instance.swap(false, Ordering::SeqCst) {
                open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
            }
            let pipe_mode =
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS;
            let wide_name = encode_wide(&self.pipe_path);
            let buf_size = MAX_IPC_FRAME_BYTES as u32;

            // SAFETY: `wide_name` is null-terminated UTF-16 and `sa` points to a valid SD
            // kept alive by `sd_guard` for the duration of `CreateNamedPipeW`.
            let raw_handle = unsafe {
                CreateNamedPipeW(
                    wide_name.as_ptr(),
                    open_mode,
                    pipe_mode,
                    PIPE_UNLIMITED_INSTANCES,
                    buf_size,
                    buf_size,
                    0,
                    &sa,
                )
            };
            drop(sd_guard);

            let Some(handle) = Win32Handle::new(raw_handle) else {
                let err = unsafe { GetLastError() };
                return Err(NamedPipeError::Io(format!(
                    "CreateNamedPipeW failed for '{}' (Win32 error {err})",
                    self.pipe_path
                )));
            };

            // SAFETY: `handle` is a valid pipe instance created above.
            let connected = unsafe { ConnectNamedPipe(handle.as_raw(), ptr::null_mut()) };
            if connected == 0 {
                let err = unsafe { GetLastError() };
                if err != ERROR_PIPE_CONNECTED {
                    return Err(NamedPipeError::Io(format!(
                        "ConnectNamedPipe failed (Win32 error {err})"
                    )));
                }
            }

            let caller = inspect_pipe_client_identity(handle.as_raw());
            Ok(Some(Win32NamedPipeConnection {
                handle,
                caller,
                pending: Vec::new(),
            }))
        }
    }

    /// Win32 Named Pipe client backed by `CreateFileW` / `ReadFile` / `WriteFile`.
    pub struct Win32NamedPipeClient {
        handle: Win32Handle,
        pending: Vec<u8>,
    }

    impl Win32NamedPipeClient {
        /// Connects to a local Windows Named Pipe endpoint (e.g. `\\.\pipe\ucc-rusted`).
        pub fn connect(pipe_path: &str) -> Result<Self, NamedPipeError> {
            let wide_name = encode_wide(pipe_path);
            // SAFETY: `wide_name` is null-terminated UTF-16.
            let raw_handle = unsafe {
                CreateFileW(
                    wide_name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    ptr::null(),
                    OPEN_EXISTING,
                    0,
                    ptr::null_mut(),
                )
            };
            let Some(handle) = Win32Handle::new(raw_handle) else {
                let err = unsafe { GetLastError() };
                return Err(NamedPipeError::Io(format!(
                    "CreateFileW failed for '{}' (Win32 error {err})",
                    pipe_path
                )));
            };
            Ok(Self {
                handle,
                pending: Vec::new(),
            })
        }

        /// Sends one framed [`IpcRequest`] and reads the framed [`IpcResponse`].
        pub fn call(&mut self, req: &IpcRequest) -> Result<IpcResponse, NamedPipeError> {
            let wire =
                encode_frame(req).map_err(|e| NamedPipeError::InvalidFrame(e.to_string()))?;
            write_all_and_flush(self.handle.as_raw(), &wire)?;
            let Some(raw_resp) =
                read_framed_line(self.handle.as_raw(), &mut self.pending, MAX_IPC_FRAME_BYTES)?
            else {
                return Err(NamedPipeError::Disconnected);
            };
            decode_frame_bounded::<IpcResponse>(&raw_resp, MAX_IPC_FRAME_BYTES)
                .map_err(|e| NamedPipeError::InvalidFrame(e.to_string()))
        }
    }
}

#[cfg(windows)]
pub use win32::{Win32NamedPipeClient, Win32NamedPipeConnection, Win32NamedPipeTransport};
