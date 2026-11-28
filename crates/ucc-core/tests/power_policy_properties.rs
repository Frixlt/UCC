//! Deterministic property-based tests for `ucc_core::power` (`PowerPolicy` matrix).
//!
//! Uses a self-contained SplitMix64 PRNG with fixed seeds (zero external dev-dependencies)
//! to verify:
//! - Full Cartesian matrix of physical power sources (`Ac`, `TypeC`, `Bat`, `Wc`) ×
//!   manual overrides × app power requests × water-cooler connection state.
//! - Priority invariants: manual override > app requests > physical source; app requests
//!   only elevate and never lower; physical source stays distinct from effective mode.
//! - Owner disconnect (`remove_owner`) isolation and idempotence.
//! - Request count caps (256 total, 32 per owner) and name/reason/mode validation bounds.
//! - Strict parsing and serialization round-trips for power modes and profile states.

use std::collections::BTreeMap;
use ucc_core::{
    LegacyPowerPolicyStatusDto, ManualPowerMode, PolicyDecision, PowerMode, PowerPolicy,
    ProfileState, ValidationError,
};

/// Self-contained deterministic SplitMix64 PRNG for offline property-based testing.
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn next_usize(&mut self, upper_exclusive: usize) -> usize {
        assert!(upper_exclusive > 0);
        (self.next_u64() % (upper_exclusive as u64)) as usize
    }

    fn next_bool(&mut self) -> bool {
        (self.next_u64() & 1) == 1
    }

    fn choose<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        let idx = self.next_usize(items.len());
        &items[idx]
    }
}

const ALL_SOURCES: [ProfileState; 4] = [
    ProfileState::Ac,
    ProfileState::TypeC,
    ProfileState::Bat,
    ProfileState::Wc,
];

const ALL_MANUAL_MODES: [ManualPowerMode; 4] = [
    ManualPowerMode::Auto,
    ManualPowerMode::Ac,
    ManualPowerMode::TypeC,
    ManualPowerMode::Bat,
];

const VALID_APP_MODES: [&str; 4] = ["", "bat", "type_c", "ac"];

fn base_effective_for_source(source: ProfileState) -> ProfileState {
    match source {
        ProfileState::Wc => ProfileState::Ac,
        other => other,
    }
}

fn manual_to_profile_state(mode: ManualPowerMode) -> Option<ProfileState> {
    match mode {
        ManualPowerMode::Auto => None,
        ManualPowerMode::Ac => Some(ProfileState::Ac),
        ManualPowerMode::TypeC => Some(ProfileState::TypeC),
        ManualPowerMode::Bat => Some(ProfileState::Bat),
    }
}

fn rank_to_profile_state(rank: u8) -> ProfileState {
    match rank {
        3 => ProfileState::Ac,
        2 => ProfileState::TypeC,
        _ => ProfileState::Bat,
    }
}

/// Independent reference oracle asserting all `PowerPolicy::evaluate` invariants.
fn assert_policy_invariants(
    policy: &PowerPolicy,
    source: ProfileState,
    water_cooler_connected: bool,
    decision: &PolicyDecision,
) {
    // 1. Physical source is always preserved separately from effective operational mode.
    assert_eq!(decision.source, source);
    assert_eq!(
        decision.status["real_power_source"].as_str(),
        Some(source.mode_name())
    );
    assert_eq!(
        decision.status["manual_mode"].as_str(),
        Some(policy.manual_mode().as_str())
    );
    assert_eq!(
        decision.status["water_cooler_connected"].as_bool(),
        Some(water_cooler_connected)
    );
    assert_eq!(
        decision.status["revision"].as_u64(),
        Some(policy.revision())
    );

    // 2. Compute highest requested app rank.
    let max_app_rank = policy
        .requests()
        .values()
        .filter_map(|req| {
            if req.mode.is_empty() {
                None
            } else {
                req.mode.parse::<PowerMode>().ok().map(PowerMode::rank)
            }
        })
        .max()
        .unwrap_or(0);

    let base_effective = base_effective_for_source(source);

    // 3. Priority hierarchy: manual override > app requests (elevate only) > physical source.
    let (expected_effective, expected_overridden) =
        if let Some(forced) = manual_to_profile_state(policy.manual_mode()) {
            (forced, true)
        } else if max_app_rank > base_effective.rank() {
            (rank_to_profile_state(max_app_rank), true)
        } else {
            (base_effective, false)
        };

    assert_eq!(decision.effective, expected_effective);
    assert_ne!(
        decision.effective,
        ProfileState::Wc,
        "effective mode must always be Ac, TypeC, or Bat"
    );

    // When in Auto mode, app requests may only elevate, never lower below physical source.
    if policy.manual_mode() == ManualPowerMode::Auto {
        assert!(
            decision.effective.rank() >= base_effective.rank(),
            "auto mode must never lower effective rank below physical source"
        );
        if max_app_rank > 0 {
            assert!(
                decision.effective.rank() >= max_app_rank,
                "auto mode must satisfy highest app request rank"
            );
        }
    }

    // 4. Water cooler profile state (`power_wc`) is selected iff connected and not overridden.
    let expected_profile_state = if !expected_overridden && water_cooler_connected {
        ProfileState::Wc
    } else {
        expected_effective
    };
    assert_eq!(decision.profile_state, expected_profile_state);

    // 5. Verify status JSON and LegacyPowerPolicyStatusDto parity.
    assert_eq!(
        decision.status["current_mode"].as_str(),
        Some(decision.effective.mode_name())
    );
    assert_eq!(
        decision.status["profile_state"].as_str(),
        Some(decision.profile_state.as_str())
    );

    let dto = LegacyPowerPolicyStatusDto::from(decision);
    assert_eq!(dto.real_power_source, source.mode_name());
    assert_eq!(dto.current_mode, decision.effective.mode_name());
    assert_eq!(dto.manual_mode, policy.manual_mode().as_str());
    assert_eq!(dto.profile_state, decision.profile_state.as_str());
    assert_eq!(dto.water_cooler_connected, water_cooler_connected);
    assert_eq!(dto.revision, policy.revision());
    assert_eq!(dto.registered_apps.len(), policy.requests().len());

    // BTreeMap ordering guarantees deterministic registered_apps ordering by (owner, name).
    for (dto_app, ((owner, name), req)) in dto.registered_apps.iter().zip(policy.requests().iter())
    {
        assert_eq!(&dto_app.owner, owner);
        assert_eq!(&dto_app.name, name);
        assert_eq!(&dto_app.reason, &req.reason);
        assert_eq!(&dto_app.force_mode, &req.mode);
    }
}

#[test]
fn prop_exhaustive_power_policy_matrix() {
    // Request portfolios covering empty, informational-only, single-mode, and mixed multi-owner.
    let portfolios: [&[(&str, &str, &str, &str)]; 7] = [
        &[],
        &[(":1.1", "monitor", "telemetry", "")],
        &[(":1.1", "saver", "idle", "bat")],
        &[(":1.1", "compiler", "cargo build", "type_c")],
        &[(":1.1", "game", "3d render", "ac")],
        &[
            (":1.1", "saver", "idle", "bat"),
            (":1.2", "compiler", "build", "type_c"),
        ],
        &[
            (":1.1", "info", "logging", ""),
            (":1.2", "saver", "idle", "bat"),
            (":1.3", "game", "heavy load", "ac"),
        ],
    ];

    for &manual in &ALL_MANUAL_MODES {
        for portfolio in portfolios {
            let mut policy = PowerPolicy::new();
            assert!(policy.set_manual_mode(manual.as_str()));
            for &(owner, name, reason, mode) in portfolio {
                assert!(policy.register_request(owner, name, reason, mode));
            }

            for &source in &ALL_SOURCES {
                for wc_connected in [false, true] {
                    let decision = policy.evaluate(source, wc_connected);
                    assert_policy_invariants(&policy, source, wc_connected, &decision);
                }
            }
        }
    }
}

#[test]
fn prop_random_state_machine_operations() {
    let mut rng = SplitMix64::new(0x504F_5745_5250_0001);
    let owners = [":1.10", ":1.11", ":1.12", ":1.13", ":1.14", ":1.15"];
    let app_names = ["blender", "steam", "rustc", "obs", "ffmpeg", "idle_watch"];
    let invalid_modes = ["power_wc", "wc", "power_ac", "unknown", "absent", "AC", " "];

    for _trial in 0..300 {
        let mut policy = PowerPolicy::new();
        let mut expected_requests: BTreeMap<(String, String), (String, String)> = BTreeMap::new();
        let mut expected_manual = ManualPowerMode::Auto;
        let mut expected_revision: u64 = 0;

        let steps = 5 + rng.next_usize(15);
        for step in 0..steps {
            match rng.next_usize(5) {
                // Set valid manual mode
                0 => {
                    let next_mode = *rng.choose(&ALL_MANUAL_MODES);
                    assert!(policy.set_manual_mode(next_mode.as_str()));
                    if next_mode != expected_manual {
                        expected_manual = next_mode;
                        expected_revision = expected_revision.wrapping_add(1);
                    }
                    assert_eq!(policy.manual_mode(), expected_manual);
                    assert_eq!(policy.revision(), expected_revision);
                }
                // Attempt invalid manual mode -> must fail without changing state or revision
                1 => {
                    let bad = *rng.choose(&invalid_modes);
                    assert!(!policy.set_manual_mode(bad));
                    assert_eq!(policy.manual_mode(), expected_manual);
                    assert_eq!(policy.revision(), expected_revision);
                }
                // Register or update an app request (valid or invalid mode)
                2 => {
                    let owner = *rng.choose(&owners);
                    let name = *rng.choose(&app_names);
                    let reason = format!("step-{step}");
                    if rng.next_bool() {
                        let mode = *rng.choose(&VALID_APP_MODES);
                        assert!(policy.register_request(owner, name, &reason, mode));
                        expected_requests.insert(
                            (owner.to_string(), name.to_string()),
                            (reason, mode.to_string()),
                        );
                    } else {
                        let bad_mode = *rng.choose(&invalid_modes);
                        assert!(!policy.register_request(owner, name, &reason, bad_mode));
                    }
                }
                // Unregister a request
                3 => {
                    let owner = *rng.choose(&owners);
                    let name = *rng.choose(&app_names);
                    let existed = expected_requests
                        .remove(&(owner.to_string(), name.to_string()))
                        .is_some();
                    assert_eq!(policy.unregister_request(owner, name), existed);
                }
                // Owner disconnect cleanup
                _ => {
                    let owner = *rng.choose(&owners);
                    policy.remove_owner(owner);
                    expected_requests.retain(|k, _| k.0 != owner);
                }
            }

            // Shadow model must match policy internal map exactly.
            assert_eq!(policy.requests().len(), expected_requests.len());
            for (k, (reason, mode)) in &expected_requests {
                let actual = policy.requests().get(k).expect("expected request present");
                assert_eq!(&actual.reason, reason);
                assert_eq!(&actual.mode, mode);
            }

            // Evaluate across a random source & both WC states at every step.
            let source = *rng.choose(&ALL_SOURCES);
            for wc_connected in [false, true] {
                let decision = policy.evaluate(source, wc_connected);
                assert_policy_invariants(&policy, source, wc_connected, &decision);
            }
        }
    }
}

#[test]
fn prop_owner_disconnect_isolation() {
    let mut rng = SplitMix64::new(0x504F_5745_5250_0002);

    for _trial in 0..300 {
        let mut policy = PowerPolicy::new();
        let owner_count = 2 + rng.next_usize(7);
        let owners: Vec<String> = (0..owner_count).map(|i| format!(":1.{}", 100 + i)).collect();

        for owner in &owners {
            let req_count = 1 + rng.next_usize(6);
            for j in 0..req_count {
                let name = format!("app-{j}");
                let reason = format!("reason-{owner}-{j}");
                let mode = *rng.choose(&VALID_APP_MODES);
                assert!(policy.register_request(owner, &name, &reason, mode));
            }
        }

        let victim_idx = rng.next_usize(owners.len());
        let victim = owners[victim_idx].as_str();

        let expected_remaining: BTreeMap<(String, String), (String, String)> = policy
            .requests()
            .iter()
            .filter(|((o, _), _)| o != victim)
            .map(|(k, v)| (k.clone(), (v.reason.clone(), v.mode.clone())))
            .collect();

        policy.remove_owner(victim);

        // Victim has zero remaining requests.
        assert!(policy.requests().keys().all(|(o, _)| o != victim));

        // All other owners' requests are completely intact.
        assert_eq!(policy.requests().len(), expected_remaining.len());
        for (k, (reason, mode)) in &expected_remaining {
            let req = policy
                .requests()
                .get(k)
                .expect("non-victim request must remain");
            assert_eq!(&req.reason, reason);
            assert_eq!(&req.mode, mode);
        }

        // Second remove_owner on the same owner is a no-op (idempotent).
        let snapshot = policy.clone();
        policy.remove_owner(victim);
        assert_eq!(policy, snapshot);

        for &source in &ALL_SOURCES {
            for wc_connected in [false, true] {
                let decision = policy.evaluate(source, wc_connected);
                assert_policy_invariants(&policy, source, wc_connected, &decision);
            }
        }
    }

    // Explicit elevation rollback + water-cooler restoration on disconnect.
    let mut policy = PowerPolicy::new();
    assert!(policy.register_request(":1.high", "render", "gpu", "ac"));
    assert!(policy.register_request(":1.mid", "compile", "cpu", "type_c"));
    let d1 = policy.evaluate(ProfileState::Bat, true);
    assert_eq!(d1.effective, ProfileState::Ac);
    assert_eq!(d1.profile_state, ProfileState::Ac);

    policy.remove_owner(":1.high");
    let d2 = policy.evaluate(ProfileState::Bat, true);
    assert_eq!(d2.effective, ProfileState::TypeC);
    assert_eq!(d2.profile_state, ProfileState::TypeC);

    policy.remove_owner(":1.mid");
    let d3 = policy.evaluate(ProfileState::Bat, true);
    assert_eq!(d3.effective, ProfileState::Bat);
    assert_eq!(d3.profile_state, ProfileState::Wc);
}

#[test]
fn prop_request_caps_and_length_bounds() {
    let mut rng = SplitMix64::new(0x504F_5745_5250_0003);

    // 1. Explicit owner / name / reason / mode boundary checks.
    let mut policy = PowerPolicy::new();
    assert!(!policy.register_request("", "app", "reason", "ac"));
    assert!(!policy.register_request(":1.1", "", "reason", "ac"));

    let name_128 = "n".repeat(128);
    let name_129 = "n".repeat(129);
    let reason_512 = "r".repeat(512);
    let reason_513 = "r".repeat(513);

    assert!(policy.register_request(":1.1", &name_128, "", ""));
    assert!(policy.register_request(":1.1", "a", &reason_512, "bat"));
    assert!(!policy.register_request(":1.1", &name_129, "ok", "ac"));
    assert!(!policy.register_request(":1.1", "ok", &reason_513, "ac"));
    assert!(!policy.register_request(":1.1", "ok", "ok", "auto"));
    assert!(!policy.register_request(":1.1", "ok", "ok", "wc"));
    assert!(!policy.register_request(":1.1", "ok", "ok", "power_ac"));

    // 2. Randomized length boundary property across 300 cases.
    for _ in 0..300 {
        let mut p = PowerPolicy::new();
        let owner_len = rng.next_usize(12);
        let name_len = rng.next_usize(160);
        let reason_len = rng.next_usize(560);
        let owner = "o".repeat(owner_len);
        let name = "a".repeat(name_len);
        let reason = "x".repeat(reason_len);
        let mode = *rng.choose(&VALID_APP_MODES);

        let expected_ok = owner_len >= 1 && (1..=128).contains(&name_len) && reason_len <= 512;
        assert_eq!(
            p.register_request(&owner, &name, &reason, mode),
            expected_ok,
            "owner_len={owner_len}, name_len={name_len}, reason_len={reason_len}"
        );
        assert_eq!(p.requests().len(), usize::from(expected_ok));
    }

    // 3. Per-owner cap (32) and total cap (256), including in-place updates at cap.
    let mut cap_policy = PowerPolicy::new();
    for owner_idx in 0..8 {
        let owner = format!(":1.{owner_idx}");
        for app_idx in 0..32 {
            let name = format!("app-{app_idx}");
            assert!(cap_policy.register_request(&owner, &name, "init", "bat"));
        }
        // 33rd distinct request for this owner must be rejected.
        assert!(!cap_policy.register_request(&owner, "app-32", "overflow", "ac"));
        // Updating an existing request at per-owner cap must still succeed.
        assert!(cap_policy.register_request(&owner, "app-0", "updated", "type_c"));
    }

    assert_eq!(cap_policy.requests().len(), 256);

    // 257th total request from a brand-new owner must be rejected.
    assert!(!cap_policy.register_request(":1.overflow", "app-0", "cap", "ac"));
    // Updating an existing request at the 256 total cap must still succeed.
    assert!(cap_policy.register_request(":1.0", "app-1", "updated-at-global-cap", "ac"));
    assert_eq!(cap_policy.requests().len(), 256);

    // Unregistering one request frees exactly one slot.
    assert!(cap_policy.unregister_request(":1.0", "app-1"));
    assert_eq!(cap_policy.requests().len(), 255);
    assert!(cap_policy.register_request(":1.overflow", "app-0", "now-fits", "ac"));
    assert_eq!(cap_policy.requests().len(), 256);

    // Removing an entire owner frees all 32 of its slots.
    cap_policy.remove_owner(":1.7");
    assert_eq!(cap_policy.requests().len(), 224);
    assert!(cap_policy.register_request(":1.overflow", "app-1", "fits-again", "type_c"));
}

#[test]
fn prop_power_enum_parsing_and_serde_invariants() {
    // ProfileState canonical wire & short-name parsing + serde round-trip.
    for &state in &ALL_SOURCES {
        assert_eq!(state.as_str().parse::<ProfileState>(), Ok(state));
        assert_eq!(state.mode_name().parse::<ProfileState>(), Ok(state));
        assert_eq!(state.to_string(), state.as_str());

        let json = serde_json::to_string(&state).expect("serialize ProfileState");
        let parsed: ProfileState = serde_json::from_str(&json).expect("deserialize ProfileState");
        assert_eq!(parsed, state);
    }

    // PowerMode parsing + rank + profile_state mapping + serde round-trip.
    for mode in [PowerMode::Ac, PowerMode::TypeC, PowerMode::Bat] {
        assert_eq!(mode.as_str().parse::<PowerMode>(), Ok(mode));
        assert_eq!(mode.to_string(), mode.as_str());
        assert_eq!(mode.rank(), mode.to_profile_state().rank());

        let json = serde_json::to_string(&mode).expect("serialize PowerMode");
        let parsed: PowerMode = serde_json::from_str(&json).expect("deserialize PowerMode");
        assert_eq!(parsed, mode);
    }

    // ManualPowerMode parsing + serde round-trip.
    for &manual in &ALL_MANUAL_MODES {
        assert_eq!(manual.as_str().parse::<ManualPowerMode>(), Ok(manual));
        assert_eq!(manual.to_string(), manual.as_str());

        let json = serde_json::to_string(&manual).expect("serialize ManualPowerMode");
        let parsed: ManualPowerMode =
            serde_json::from_str(&json).expect("deserialize ManualPowerMode");
        assert_eq!(parsed, manual);
    }

    // Unknown / absent / forbidden strings must fail cleanly.
    for bad in ["", "unknown", "absent", "none", "AC", "BAT", "power_unknown"] {
        assert_eq!(
            bad.parse::<ProfileState>(),
            Err(ValidationError::InvalidProfileState(bad.to_string()))
        );
        assert_eq!(
            bad.parse::<PowerMode>(),
            Err(ValidationError::InvalidPowerMode(bad.to_string()))
        );
        assert_eq!(
            bad.parse::<ManualPowerMode>(),
            Err(ValidationError::InvalidPowerMode(bad.to_string()))
        );
    }

    // Water cooler state cannot be parsed as a PowerMode or ManualPowerMode.
    for wc_str in ["wc", "power_wc"] {
        assert_eq!(
            wc_str.parse::<PowerMode>(),
            Err(ValidationError::InvalidPowerMode(wc_str.to_string()))
        );
        assert_eq!(
            wc_str.parse::<ManualPowerMode>(),
            Err(ValidationError::InvalidPowerMode(wc_str.to_string()))
        );
    }
}
