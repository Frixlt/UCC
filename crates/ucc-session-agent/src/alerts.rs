// SPDX-License-Identifier: GPL-3.0-or-later

//! Stateful decision logic for driver health and power-source notifications.

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverAlertAction {
    None,
    Close(u32),
    Notify(String),
    Replace { close_id: u32, problem: String },
}

#[derive(Debug, Default, Clone)]
pub struct DriverAlertState {
    current_problem: String,
    notified_problem: String,
    generation: u64,
    active_id: u32,
    pending: bool,
}

impl DriverAlertState {
    pub fn evaluate(&mut self, status: &Value) -> (u64, DriverAlertAction) {
        let healthy = status
            .get("healthy")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let problem = if healthy {
            String::new()
        } else {
            status
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Kernel driver unavailable")
                .to_string()
        };

        let mut close_id = None;
        if problem != self.current_problem {
            self.current_problem = problem.clone();
            self.notified_problem.clear();
            self.generation += 1;
            self.pending = false;
            if self.active_id != 0 {
                close_id = Some(self.active_id);
                self.active_id = 0;
            }
        }

        if problem.is_empty() || self.pending || self.notified_problem == problem {
            let act = match close_id {
                Some(id) => DriverAlertAction::Close(id),
                None => DriverAlertAction::None,
            };
            return (self.generation, act);
        }

        self.pending = true;
        let act = match close_id {
            Some(id) => DriverAlertAction::Replace {
                close_id: id,
                problem,
            },
            None => DriverAlertAction::Notify(problem),
        };
        (self.generation, act)
    }

    pub fn on_notify_finished(&mut self, generation: u64, notification_id: Option<u32>) -> Option<u32> {
        if generation != self.generation {
            return notification_id;
        }
        self.pending = false;
        if let Some(id) = notification_id {
            self.active_id = id;
            self.notified_problem = self.current_problem.clone();
        }
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerSourceNotice {
    pub icon: &'static str,
    pub title: &'static str,
    pub body: &'static str,
}

#[derive(Debug, Default, Clone)]
pub struct PowerSourceAlertState {
    source: String,
    announced: String,
}

impl PowerSourceAlertState {
    pub fn reset(&mut self) {
        self.source.clear();
        self.announced.clear();
    }

    /// Returns `true` if the transition should schedule the debounce timer.
    pub fn on_policy_json(&mut self, json: &str) -> bool {
        let Ok(val) = serde_json::from_str::<Value>(json) else {
            return false;
        };
        let Some(src) = val.get("real_power_source").and_then(|v| v.as_str()) else {
            return false;
        };
        if !matches!(src, "ac" | "bat" | "type_c") {
            return false;
        }
        if self.source.is_empty() {
            self.source = src.to_string();
            self.announced = src.to_string();
            return false;
        }
        if self.source == src {
            return false;
        }
        self.source = src.to_string();
        true
    }

    pub fn take_pending_notice(&mut self, read_only_preview: bool) -> Option<PowerSourceNotice> {
        if read_only_preview || self.source == self.announced || self.source.is_empty() {
            return None;
        }
        self.announced = self.source.clone();
        Some(match self.source.as_str() {
            "bat" => PowerSourceNotice {
                icon: "battery",
                title: "Питание от батареи",
                body: "Ноутбук работает от батареи.",
            },
            "type_c" => PowerSourceNotice {
                icon: "ac-adapter",
                title: "Питание подключено",
                body: "Подключено питание USB-C.",
            },
            _ => PowerSourceNotice {
                icon: "ac-adapter",
                title: "Питание подключено",
                body: "Подключено питание от сети.",
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn driver_alert_opens_and_closes_once_per_problem() {
        let mut st = DriverAlertState::default();
        let (g1, act1) = st.evaluate(&json!({"healthy": false, "message": "tuxedo_io missing"}));
        assert_eq!(act1, DriverAlertAction::Notify("tuxedo_io missing".into()));
        assert_eq!(st.on_notify_finished(g1, Some(42)), None);

        let (_, act2) = st.evaluate(&json!({"healthy": false, "message": "tuxedo_io missing"}));
        assert_eq!(act2, DriverAlertAction::None);

        let (_, act3) = st.evaluate(&json!({"healthy": true}));
        assert_eq!(act3, DriverAlertAction::Close(42));
    }

    #[test]
    fn power_source_alert_skips_initial_state_and_emits_on_change() {
        let mut st = PowerSourceAlertState::default();
        assert!(!st.on_policy_json(r#"{"real_power_source":"ac"}"#));
        assert_eq!(st.take_pending_notice(false), None);

        assert!(st.on_policy_json(r#"{"real_power_source":"bat"}"#));
        let notice = st.take_pending_notice(false).unwrap();
        assert_eq!(notice.icon, "battery");
        assert_eq!(st.take_pending_notice(false), None);
    }
}
