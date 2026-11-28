// SPDX-License-Identifier: GPL-3.0-or-later

//! Unit tests for `DriverAlertState` transitions and `PowerSourceAlertState::reset()`.

use serde_json::json;
use ucc_session_agent::{
    DriverAlertAction, DriverAlertState, PowerSourceAlertState, PowerSourceNotice,
};

#[test]
fn driver_alert_state_transitions_new_replace_and_clear_on_healthy() {
    let mut st = DriverAlertState::default();

    // 1. Healthy baseline produces no action.
    let (_, act0) = st.evaluate(&json!({"healthy": true}));
    assert_eq!(act0, DriverAlertAction::None);

    // 2. New error -> Notify; duplicate evaluation while pending -> None.
    let (g1, act1) = st.evaluate(&json!({"healthy": false, "message": "tuxedo_io missing"}));
    assert_eq!(act1, DriverAlertAction::Notify("tuxedo_io missing".into()));
    let (g1_dup, act1_dup) =
        st.evaluate(&json!({"healthy": false, "message": "tuxedo_io missing"}));
    assert_eq!(g1_dup, g1);
    assert_eq!(act1_dup, DriverAlertAction::None);

    // Complete delivery with notification ID 101.
    assert_eq!(st.on_notify_finished(g1, Some(101)), None);

    // Same error after delivery -> None.
    let (_, act1_after) = st.evaluate(&json!({"healthy": false, "message": "tuxedo_io missing"}));
    assert_eq!(act1_after, DriverAlertAction::None);

    // 3. Error text change -> Replace closing old ID 101 and advancing generation.
    let (g2, act2) = st.evaluate(&json!({"healthy": false, "message": "EC firmware timeout"}));
    assert!(g2 > g1);
    assert_eq!(
        act2,
        DriverAlertAction::Replace {
            close_id: 101,
            problem: "EC firmware timeout".into(),
        }
    );

    // Stale completion from g1 returns the stale notification ID for cleanup without clearing g2.
    assert_eq!(st.on_notify_finished(g1, Some(999)), Some(999));

    // Complete g2 delivery with notification ID 202.
    assert_eq!(st.on_notify_finished(g2, Some(202)), None);

    // 4. Recovery to healthy -> Close(202); subsequent healthy check -> None.
    let (g3, act3) = st.evaluate(&json!({"healthy": true}));
    assert!(g3 > g2);
    assert_eq!(act3, DriverAlertAction::Close(202));

    let (_, act4) = st.evaluate(&json!({"healthy": true}));
    assert_eq!(act4, DriverAlertAction::None);
}

#[test]
fn driver_alert_state_default_message_and_failed_notify_retry() {
    let mut st = DriverAlertState::default();

    // Missing "message" falls back to "Kernel driver unavailable".
    let (g1, act1) = st.evaluate(&json!({"healthy": false}));
    assert_eq!(
        act1,
        DriverAlertAction::Notify("Kernel driver unavailable".into())
    );

    // Failed notify (None) clears pending so the next evaluate retries Notify.
    assert_eq!(st.on_notify_finished(g1, None), None);
    let (g1_retry, act_retry) = st.evaluate(&json!({"healthy": false}));
    assert_eq!(g1_retry, g1);
    assert_eq!(
        act_retry,
        DriverAlertAction::Notify("Kernel driver unavailable".into())
    );
}

#[test]
fn power_source_alert_state_reset_clears_state_and_re_seeds_next_source() {
    let mut st = PowerSourceAlertState::default();

    // Initial seed ("ac") does not schedule a notification.
    assert!(!st.on_policy_json(r#"{"real_power_source":"ac"}"#));
    assert_eq!(st.take_pending_notice(false), None);

    // Transition to "bat" schedules a pending notice; preview mode leaves it pending.
    assert!(st.on_policy_json(r#"{"real_power_source":"bat"}"#));
    assert_eq!(st.take_pending_notice(true), None);
    assert_eq!(
        st.take_pending_notice(false),
        Some(PowerSourceNotice {
            icon: "battery",
            title: "Питание от батареи",
            body: "Ноутбук работает от батареи.",
        })
    );
    assert_eq!(st.take_pending_notice(false), None);

    // Queue another transition, then call reset() before taking it -> pending notice is cleared.
    assert!(st.on_policy_json(r#"{"real_power_source":"ac"}"#));
    st.reset();
    assert_eq!(st.take_pending_notice(false), None);

    // First policy update after reset() acts as initial seed again (returns false, no notice).
    assert!(!st.on_policy_json(r#"{"real_power_source":"bat"}"#));
    assert_eq!(st.take_pending_notice(false), None);

    // Subsequent transition to "type_c" emits USB-C notice.
    assert!(st.on_policy_json(r#"{"real_power_source":"type_c"}"#));
    assert_eq!(
        st.take_pending_notice(false),
        Some(PowerSourceNotice {
            icon: "ac-adapter",
            title: "Питание подключено",
            body: "Подключено питание USB-C.",
        })
    );
}
