// SPDX-License-Identifier: GPL-3.0-or-later

//! Race-free GPU process terminator using Linux `pidfd_open` + `startTime` verification.

use crate::telemetry::{SessionPaths, SessionReader};
use std::collections::HashSet;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminationTarget {
    pub pid: i64,
    pub start_time: u64,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TerminationReport {
    pub terminated: usize,
    pub killed: usize,
    pub errors: Vec<String>,
}

impl TerminationReport {
    pub fn summary_text(&self) -> String {
        let mut text = format!("Exited: {} · Forced: {}", self.terminated, self.killed);
        if !self.errors.is_empty() {
            text.push('\n');
            text.push_str(&self.errors.join("\n"));
        }
        text
    }
}

struct OpenTarget {
    pid: i64,
    fd: OwnedFd,
    killed: bool,
    done: bool,
}

pub struct ProcessTerminator {
    reader: SessionReader,
    escalation: Duration,
    deadline: Duration,
}

impl ProcessTerminator {
    pub fn new(paths: SessionPaths) -> Self {
        Self {
            reader: SessionReader::new(paths),
            escalation: Duration::from_secs(3),
            deadline: Duration::from_secs(2),
        }
    }

    pub fn with_timeouts(mut self, escalation: Duration, deadline: Duration) -> Self {
        self.escalation = escalation;
        self.deadline = deadline;
        self
    }

    fn pidfd_open(pid: i64) -> Result<OwnedFd, String> {
        let Ok(raw_pid) = nix::libc::pid_t::try_from(pid) else {
            return Err(format!("PID {pid}: invalid PID"));
        };
        let raw = unsafe {
            nix::libc::syscall(nix::libc::SYS_pidfd_open, raw_pid, 0 as nix::libc::c_uint)
        };
        if raw < 0 {
            let err = std::io::Error::last_os_error();
            return Err(format!("PID {pid}: {err}"));
        }
        // SAFETY: syscall returned a fresh valid file descriptor owned by this process.
        Ok(unsafe { OwnedFd::from_raw_fd(raw as i32) })
    }

    fn pidfd_send_signal(fd: &OwnedFd, sig: i32) -> std::io::Result<()> {
        let rc = unsafe {
            nix::libc::syscall(
                nix::libc::SYS_pidfd_send_signal,
                fd.as_raw_fd(),
                sig,
                std::ptr::null::<nix::libc::siginfo_t>(),
                0 as nix::libc::c_uint,
            )
        };
        if rc == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    fn pidfd_exited(fd: &OwnedFd) -> bool {
        let mut pfd = nix::libc::pollfd {
            fd: fd.as_raw_fd(),
            events: nix::libc::POLLIN,
            revents: 0,
        };
        let rc = unsafe { nix::libc::poll(&mut pfd, 1, 0) };
        rc > 0 && (pfd.revents & nix::libc::POLLIN) != 0
    }

    pub async fn terminate(&self, targets: &[TerminationTarget]) -> TerminationReport {
        let mut report = TerminationReport::default();
        if targets.is_empty() {
            return report;
        }
        if unsafe { nix::libc::geteuid() } == 0 {
            report
                .errors
                .push("Run process management as your desktop user".to_string());
            return report;
        }
        if ucc_config::is_read_only_preview() {
            report
                .errors
                .push("Process termination is disabled in read-only preview".to_string());
            return report;
        }

        let mut seen = HashSet::new();
        let mut open_targets = Vec::new();

        for target in targets {
            if !seen.insert(target.pid) {
                continue;
            }
            let Some(before) = self.reader.process_identity(target.pid) else {
                report.errors.push(format!(
                    "PID {} ended or no longer matches the selected GPU process",
                    target.pid
                ));
                continue;
            };
            if before.start_time != target.start_time
                || !self.reader.holds_nvidia_device(target.pid)
            {
                report.errors.push(format!(
                    "PID {} ended or no longer matches the selected GPU process",
                    target.pid
                ));
                continue;
            }

            let fd = match Self::pidfd_open(target.pid) {
                Ok(fd) => fd,
                Err(msg) => {
                    report.errors.push(msg);
                    continue;
                }
            };

            // Re-verify identity AFTER opening pidfd so reused PIDs cannot be signaled.
            let Some(after) = self.reader.process_identity(target.pid) else {
                report
                    .errors
                    .push(format!("PID {} changed before termination", target.pid));
                continue;
            };
            if after.start_time != before.start_time || !self.reader.holds_nvidia_device(target.pid)
            {
                report
                    .errors
                    .push(format!("PID {} changed before termination", target.pid));
                continue;
            }

            if let Err(err) = Self::pidfd_send_signal(&fd, nix::libc::SIGTERM) {
                if err.raw_os_error() == Some(nix::libc::ESRCH) {
                    report.terminated += 1;
                } else {
                    report.errors.push(format!("PID {}: {err}", target.pid));
                }
                continue;
            }

            open_targets.push(OpenTarget {
                pid: target.pid,
                fd,
                killed: false,
                done: false,
            });
        }

        if open_targets.is_empty() {
            return report;
        }

        let term_deadline = Instant::now() + self.escalation;
        while Instant::now() < term_deadline {
            for t in &mut open_targets {
                if !t.done && Self::pidfd_exited(&t.fd) {
                    t.done = true;
                    report.terminated += 1;
                }
            }
            if open_targets.iter().all(|t| t.done) {
                return report;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        for t in &mut open_targets {
            if t.done {
                continue;
            }
            if Self::pidfd_exited(&t.fd) {
                t.done = true;
                report.terminated += 1;
                continue;
            }
            match Self::pidfd_send_signal(&t.fd, nix::libc::SIGKILL) {
                Ok(()) => t.killed = true,
                Err(err) if err.raw_os_error() == Some(nix::libc::ESRCH) => {
                    t.done = true;
                    report.terminated += 1;
                }
                Err(err) => {
                    t.done = true;
                    report.errors.push(format!("PID {}: {err}", t.pid));
                }
            }
        }

        let kill_deadline = Instant::now() + self.deadline;
        while Instant::now() < kill_deadline {
            for t in &mut open_targets {
                if !t.done && Self::pidfd_exited(&t.fd) {
                    t.done = true;
                    if t.killed {
                        report.killed += 1;
                    } else {
                        report.terminated += 1;
                    }
                }
            }
            if open_targets.iter().all(|t| t.done) {
                return report;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        for t in &mut open_targets {
            if !t.done && Self::pidfd_exited(&t.fd) {
                t.done = true;
                if t.killed {
                    report.killed += 1;
                } else {
                    report.terminated += 1;
                }
            }
        }

        for t in &open_targets {
            if !t.done {
                report.errors.push(format!("PID {} has not exited", t.pid));
            }
        }

        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn refuses_stale_or_nonexistent_pid_without_signaling() {
        let terminator = ProcessTerminator::new(SessionPaths::default())
            .with_timeouts(Duration::from_millis(50), Duration::from_millis(50));
        let report = terminator
            .terminate(&[
                TerminationTarget {
                    pid: 1,
                    start_time: 1,
                },
                TerminationTarget {
                    pid: std::process::id() as i64,
                    start_time: 999999,
                },
            ])
            .await;
        assert_eq!(report.terminated, 0);
        assert_eq!(report.killed, 0);
        assert_eq!(report.errors.len(), 2);
    }
}
