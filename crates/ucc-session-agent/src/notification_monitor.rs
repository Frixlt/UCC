// SPDX-License-Identifier: GPL-3.0-or-later

//! Streaming `dbus-monitor` parser for external `org.freedesktop.Notifications.Notify` calls.
//! Filters self-emitted UCC alerts (`app_name` / sender) and discards body/summary immediately.

use std::time::{Duration, Instant};

const MAX_LINE_BYTES: usize = 4096;
const DEBOUNCE_WINDOW: Duration = Duration::from_millis(100);
const SELF_APP_NAMES: [&str; 3] = ["UCC", "Frixit Control Center", "Uniwill Control Center"];

pub struct NotificationMonitorParser {
    pending: Vec<u8>,
    dropping: bool,
    awaiting_app_name: bool,
    own_bus_name: Option<String>,
    last_emitted: Option<Instant>,
}

impl Default for NotificationMonitorParser {
    fn default() -> Self {
        Self::new(None)
    }
}

impl NotificationMonitorParser {
    pub fn new(own_bus_name: Option<String>) -> Self {
        Self {
            pending: Vec::with_capacity(256),
            dropping: false,
            awaiting_app_name: false,
            own_bus_name,
            last_emitted: None,
        }
    }

    pub fn is_notify_header(line: &str) -> bool {
        let trimmed = line.trim();
        trimmed.starts_with("method call ")
            && trimmed.contains("interface=org.freedesktop.Notifications;")
            && trimmed.ends_with("member=Notify")
    }

    fn header_from_self(&self, line: &str) -> bool {
        let Some(ref own) = self.own_bus_name else {
            return false;
        };
        let needle = format!("sender={own}");
        line.contains(&needle)
    }

    fn parse_string_arg(line: &str) -> Option<&str> {
        let trimmed = line.trim();
        let rest = trimmed.strip_prefix("string \"")?;
        rest.strip_suffix('"')
    }

    fn try_emit(&mut self, now: Instant) -> bool {
        if let Some(prev) = self.last_emitted {
            if now.duration_since(prev) < DEBOUNCE_WINDOW {
                return false;
            }
        }
        self.last_emitted = Some(now);
        true
    }

    /// Feeds raw stdout bytes from `dbus-monitor` and returns the count of external Notify events triggered.
    pub fn feed_bytes(&mut self, bytes: &[u8], now: Instant) -> usize {
        let mut count = 0usize;
        for &b in bytes {
            if b == b'\n' {
                if !self.dropping {
                    if let Ok(line) = std::str::from_utf8(&self.pending) {
                        if self.awaiting_app_name {
                            self.awaiting_app_name = false;
                            if let Some(app_name) = Self::parse_string_arg(line) {
                                if !SELF_APP_NAMES.contains(&app_name) && self.try_emit(now) {
                                    count += 1;
                                }
                            } else if Self::is_notify_header(line) && !self.header_from_self(line)
                            {
                                // Back-to-back Notify header without visible string line
                                if self.try_emit(now) {
                                    count += 1;
                                }
                                self.awaiting_app_name = true;
                            } else if self.try_emit(now) {
                                // Emit conservatively if dbus-monitor omitted arg details
                                count += 1;
                            }
                        } else if Self::is_notify_header(line) && !self.header_from_self(line) {
                            self.awaiting_app_name = true;
                        }
                    }
                }
                self.pending.clear();
                self.dropping = false;
            } else if !self.dropping {
                if self.pending.len() < MAX_LINE_BYTES {
                    self.pending.push(b);
                } else {
                    self.pending.clear();
                    self.dropping = true;
                }
            }
        }
        count
    }

    /// Flushes a trailing header line if the stream ended right after a `member=Notify` header.
    pub fn reset(&mut self) {
        self.pending.clear();
        self.dropping = false;
        self.awaiting_app_name = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_self_notifications_and_debounces_external_ones() {
        let mut parser = NotificationMonitorParser::new(Some(":1.77".to_string()));
        let t0 = Instant::now();

        let self_app = b"method call time=1.0 sender=:1.42 -> destination=:1.5 serial=9 path=/org/freedesktop/Notifications; interface=org.freedesktop.Notifications; member=Notify\n   string \"Frixit Control Center\"\n   uint32 0\n";
        assert_eq!(parser.feed_bytes(self_app, t0), 0);

        let self_sender = b"method call time=1.1 sender=:1.77 -> destination=:1.5 serial=10 path=/org/freedesktop/Notifications; interface=org.freedesktop.Notifications; member=Notify\n   string \"Other\"\n";
        assert_eq!(parser.feed_bytes(self_sender, t0 + Duration::from_millis(200)), 0);

        let external = b"method call time=1.2 sender=:1.88 -> destination=:1.5 serial=11 path=/org/freedesktop/Notifications; interface=org.freedesktop.Notifications; member=Notify\n   string \"Firefox\"\n";
        assert_eq!(parser.feed_bytes(external, t0 + Duration::from_millis(400)), 1);
        // Within 100ms debounce -> suppressed
        assert_eq!(parser.feed_bytes(external, t0 + Duration::from_millis(450)), 0);
        // After 100ms -> allowed again
        assert_eq!(parser.feed_bytes(external, t0 + Duration::from_millis(550)), 1);
    }
}
