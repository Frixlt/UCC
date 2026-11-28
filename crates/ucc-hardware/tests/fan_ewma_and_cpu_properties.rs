// SPDX-License-Identifier: GPL-3.0-or-later
//! Deterministic property-based tests for fan curve / EWMA / hysteresis / fail-safe
//! cooling logic and CPU topology / selection / frequency clamping.

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use tempfile::TempDir;
use ucc_core::{
    FanControlLogic, FanCurve, FanLogicType, FanPoint, FanProfile, TemperatureFilter,
    TemporaryFanCurves, ValidationError,
};
use ucc_hardware::{
    find_closest_value, format_cpu_range, parse_cpu_range, parse_cpu_range_list, CpuController,
    CpuTopology, DeviceCapabilities, DeviceVendor, FanController, MockPlatformBackend,
    PlatformBackendKind, PlatformCapabilities, PlatformWrite,
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
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn gen_bool(&mut self) -> bool {
        (self.next_u64() & 1) == 1
    }

    fn gen_usize_range(&mut self, low: usize, high_inclusive: usize) -> usize {
        debug_assert!(low <= high_inclusive);
        let span = (high_inclusive - low) as u64 + 1;
        low + (self.next_u64() % span) as usize
    }

    fn gen_u8_range(&mut self, low: u8, high_inclusive: u8) -> u8 {
        debug_assert!(low <= high_inclusive);
        let span = u64::from(high_inclusive - low) + 1;
        low + (self.next_u64() % span) as u8
    }

    fn gen_u32_range(&mut self, low: u32, high_inclusive: u32) -> u32 {
        debug_assert!(low <= high_inclusive);
        let span = u64::from(high_inclusive - low) + 1;
        low + (self.next_u64() % span) as u32
    }

    fn gen_i32_range(&mut self, low: i32, high_inclusive: i32) -> i32 {
        debug_assert!(low <= high_inclusive);
        let span = (i64::from(high_inclusive) - i64::from(low) + 1) as u64;
        (i64::from(low) + (self.next_u64() % span) as i64) as i32
    }
}

fn gen_monotonic_points(rng: &mut SplitMix64, min_pts: usize, max_pts: usize) -> Vec<FanPoint> {
    let count = rng.gen_usize_range(min_pts, max_pts);
    let mut temps = Vec::with_capacity(count);
    let mut current_temp = rng.gen_i32_range(-20, 40);
    for _ in 0..count {
        temps.push(current_temp);
        let step = rng.gen_i32_range(1, 15);
        current_temp = (current_temp + step).min(150);
        if current_temp == *temps.last().unwrap() {
            break;
        }
    }

    let mut speeds = Vec::with_capacity(temps.len());
    let mut current_speed = rng.gen_u8_range(0, 35);
    for _ in 0..temps.len() {
        speeds.push(current_speed);
        let step = rng.gen_u8_range(0, 25);
        current_speed = current_speed.saturating_add(step).min(100);
    }

    temps
        .into_iter()
        .zip(speeds)
        .map(|(t, s)| FanPoint::new(t, s))
        .collect()
}

fn make_profile(cpu: Vec<FanPoint>, gpu: Vec<FanPoint>) -> FanProfile {
    FanProfile {
        id: "prop-test".to_string(),
        name: "Prop Test".to_string(),
        table_cpu: cpu,
        table_gpu: gpu,
        ..Default::default()
    }
}

fn make_mock_backend(
    fan_count: usize,
    fans_min_speed: i32,
    fans_off_available: bool,
) -> MockPlatformBackend {
    let caps = PlatformCapabilities {
        backend: PlatformBackendKind::Mock,
        device: DeviceCapabilities {
            vendor: DeviceVendor::Uniwill,
            number_of_fans: fan_count,
            fans_min_speed,
            fans_off_available,
            fan_control: true,
            ..DeviceCapabilities::default()
        },
        tdp_channels: 0,
        perf_profiles_available: 3,
    };
    let backend = MockPlatformBackend::new(caps);
    for idx in 0..fan_count {
        backend.set_fan_temp(idx, 45);
        backend.set_fan_speed(idx, 30);
    }
    backend
}

#[test]
fn prop_monotonic_fan_curve_interpolation_clamping_and_rounding() {
    let mut rng = SplitMix64::new(0xFA11_0001_2026_0001);

    for case in 0..300 {
        let points = gen_monotonic_points(&mut rng, 1, 10);
        let curve = FanCurve::new(points.clone()).expect("generated monotonic curve must validate");

        // Clamping at and beyond boundaries
        let first = points[0];
        let last = *points.last().unwrap();
        assert_eq!(
            curve.get_speed_for_temp(first.temp - 25),
            i32::from(first.speed),
            "case {case}: below-min temp must clamp to first knot speed"
        );
        assert_eq!(
            curve.get_speed_for_temp(last.temp + 25),
            i32::from(last.speed),
            "case {case}: above-max temp must clamp to last knot speed"
        );

        // Exact knot values
        for pt in &points {
            assert_eq!(
                curve.get_speed_for_temp(pt.temp),
                i32::from(pt.speed),
                "case {case}: exact knot {}C must return exact speed {}",
                pt.temp,
                pt.speed
            );
        }

        // Global monotonicity across sampled temperatures
        let mut prev_speed = curve.get_speed_for_temp(-30);
        assert!((0..=100).contains(&prev_speed));
        for temp in (-29..=160).step_by(3) {
            let spd = curve.get_speed_for_temp(temp);
            assert!(
                (0..=100).contains(&spd),
                "case {case}: speed {spd} out of 0..=100 at {temp}C"
            );
            assert!(
                spd >= prev_speed,
                "case {case}: monotonic curve decreased from {prev_speed} to {spd} at {temp}C"
            );
            prev_speed = spd;
        }

        // Exact linear interpolation + round-half-away-from-zero check on interior segments
        for pair in points.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if b.temp - a.temp > 1 {
                let sample_temp = rng.gen_i32_range(a.temp + 1, b.temp - 1);
                let frac = f64::from(sample_temp - a.temp) / f64::from(b.temp - a.temp);
                let expected = (f64::from(a.speed)
                    + frac * (f64::from(b.speed) - f64::from(a.speed)))
                .round() as i32;
                assert_eq!(
                    curve.get_speed_for_temp(sample_temp),
                    expected,
                    "case {case}: interpolation mismatch at {sample_temp}C"
                );
            }
        }
    }

    // Explicit rounding half-way edge cases (round-half-away-from-zero matches C++ std::lround)
    let half_curve = FanCurve::new(vec![FanPoint::new(40, 20), FanPoint::new(42, 21)]).unwrap();
    assert_eq!(half_curve.get_speed_for_temp(41), 21);

    let even_half_curve =
        FanCurve::new(vec![FanPoint::new(40, 21), FanPoint::new(42, 22)]).unwrap();
    assert_eq!(even_half_curve.get_speed_for_temp(41), 22);
}

#[test]
fn prop_non_monotonic_curves_rejected_by_validate_and_safe_to_evaluate() {
    let mut rng = SplitMix64::new(0xFA11_0002_2026_0002);

    // Empty curve must be rejected and return -1 / None
    let empty = FanCurve { points: Vec::new() };
    assert_eq!(empty.validate(), Err(ValidationError::EmptyFanCurve));
    assert_eq!(empty.get_speed_for_temp(50), -1);
    let mut empty_logic = FanControlLogic::new(make_profile(vec![], vec![]), FanLogicType::Cpu);
    assert_eq!(empty_logic.report_temperature_checked(50), None);

    for case in 0..250 {
        let len = rng.gen_usize_range(2, 8);
        let mut temps = Vec::with_capacity(len);
        let mut t = rng.gen_i32_range(0, 30);
        for _ in 0..len {
            temps.push(t);
            t += rng.gen_i32_range(2, 12);
        }
        let mut speeds: Vec<u8> = (0..len).map(|_| rng.gen_u8_range(0, 100)).collect();

        // Force at least one speed inversion so the curve is non-monotonic
        let inv_idx = rng.gen_usize_range(1, len - 1);
        if speeds[inv_idx] >= speeds[inv_idx - 1] {
            speeds[inv_idx - 1] = rng.gen_u8_range(1, 100);
            speeds[inv_idx] = rng.gen_u8_range(0, speeds[inv_idx - 1] - 1);
        }

        let points: Vec<FanPoint> = temps
            .into_iter()
            .zip(speeds)
            .map(|(temp, speed)| FanPoint::new(temp, speed))
            .collect();
        let curve = FanCurve {
            points: points.clone(),
        };

        assert!(
            matches!(curve.validate(), Err(ValidationError::NonMonotonicSpeed { .. })),
            "case {case}: non-monotonic speed curve must fail validate()"
        );

        // Direct evaluation must never panic and keeps interpolated values within segment bounds
        for pair in points.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let mid = a.temp + (b.temp - a.temp) / 2;
            let spd = curve.get_speed_for_temp(mid);
            let lo = i32::from(a.speed.min(b.speed));
            let hi = i32::from(a.speed.max(b.speed));
            assert!(
                (lo..=hi).contains(&spd),
                "case {case}: speed {spd} outside segment [{lo}, {hi}] at {mid}C"
            );
        }

        // FanControlLogic over a non-monotonic profile stays within 0..=100
        let mut logic = FanControlLogic::new(
            make_profile(points.clone(), points),
            FanLogicType::Cpu,
        );
        for _ in 0..8 {
            let sample = rng.gen_i32_range(0, 110);
            let out = logic.report_temperature_checked(sample);
            let spd = out.expect("non-empty curve must return Some");
            assert!(spd <= 100, "case {case}: speed {spd} > 100");
        }
    }

    // Non-monotonic (duplicate or decreasing) temperatures must fail validate() and not panic
    for case in 0..100 {
        let t0 = rng.gen_i32_range(20, 80);
        let t1 = rng.gen_i32_range(0, t0);
        let curve = FanCurve {
            points: vec![FanPoint::new(t0, 30), FanPoint::new(t1, 60)],
        };
        assert!(
            matches!(
                curve.validate(),
                Err(ValidationError::NonMonotonicTemperature { .. })
            ),
            "case {case}: non-increasing temperatures ({t0}, {t1}) must fail validate()"
        );
        for query in [-10, 0, t1, t0, 100] {
            let spd = curve.get_speed_for_temp(query);
            assert!((0..=100).contains(&spd));
        }
    }
}

#[test]
fn prop_asymmetric_ewma_temperature_and_speed_smoothing() {
    let mut rng = SplitMix64::new(0xFA11_0003_2026_0003);

    // 1. TemperatureFilter asymmetric EWMA (alpha_rising = 0.5 vs alpha_falling = 0.15)
    for case in 0..300 {
        let low = rng.gen_i32_range(0, 80);
        let delta = rng.gen_i32_range(4, 60);
        let high = low + delta;

        let mut rising = TemperatureFilter::default();
        assert!(!rising.is_initialized());
        assert_eq!(rising.filtered_value(), 0);
        rising.add_value(low);
        assert!(rising.is_initialized());
        assert_eq!(rising.filtered_value(), low);
        rising.add_value(high);

        let mut falling = TemperatureFilter::default();
        falling.add_value(high);
        assert_eq!(falling.filtered_value(), high);
        falling.add_value(low);

        let rise_step = rising.filtered_value() - low;
        let fall_step = high - falling.filtered_value();
        assert!(
            rise_step > fall_step,
            "case {case}: rising ({rise_step}) must exceed falling ({fall_step}), delta={delta}"
        );

        let expected_rise = (f64::from(low) + 0.5 * f64::from(delta)).round() as i32;
        let expected_fall = (f64::from(high) + 0.15 * (f64::from(low) - f64::from(high)))
            .round() as i32;
        assert_eq!(rising.filtered_value(), expected_rise);
        assert_eq!(falling.filtered_value(), expected_fall);

        // Multi-step trajectory exact recurrence and convex-hull boundedness
        let mut filter = TemperatureFilter::default();
        let mut ref_val: Option<f64> = None;
        let mut min_seen = i32::MAX;
        let mut max_seen = i32::MIN;
        for _ in 0..20 {
            let sample = rng.gen_i32_range(0, 120);
            min_seen = min_seen.min(sample);
            max_seen = max_seen.max(sample);
            filter.add_value(sample);
            let raw = f64::from(sample);
            ref_val = Some(match ref_val {
                None => raw,
                Some(prev) => {
                    let alpha = if raw > prev { 0.5 } else { 0.15 };
                    prev + alpha * (raw - prev)
                }
            });
            let filtered = filter.filtered_value();
            assert_eq!(filtered, ref_val.unwrap().round() as i32);
            assert!(
                (min_seen..=max_seen).contains(&filtered),
                "case {case}: filtered {filtered} escaped [{min_seen}, {max_seen}]"
            );
        }
    }

    // 2. FanControlLogic asymmetric speed EWMA (SPEED_ALPHA_UP = 0.4 vs SPEED_ALPHA_DOWN = 0.08)
    // Sharp step at 67C->68C below critical (85C) so one step crosses cleanly in both directions.
    for case in 0..250 {
        let s_low = rng.gen_u8_range(0, 60);
        let s_delta = rng.gen_u8_range(10, 100 - s_low);
        let s_high = s_low + s_delta;

        let step_curve = vec![
            FanPoint::new(0, s_low),
            FanPoint::new(67, s_low),
            FanPoint::new(68, s_high),
            FanPoint::new(100, s_high),
        ];
        let profile = make_profile(step_curve.clone(), step_curve);

        // Rising: 60C -> 80C yields filtered=70C (<85C, >=68C) -> curve=s_high
        let mut up_logic = FanControlLogic::new(profile.clone(), FanLogicType::Cpu);
        assert_eq!(up_logic.report_temperature(60), s_low);
        let up_speed = up_logic.report_temperature(80);
        let expected_up = (f64::from(s_low)
            + FanControlLogic::SPEED_ALPHA_UP * f64::from(s_delta))
        .round() as u8;
        assert_eq!(
            up_speed, expected_up,
            "case {case}: rising speed EWMA mismatch for {s_low}->{s_high}"
        );

        // Falling: 75C -> 0C yields filtered=64C, effective=min(75, 64+3)=67C -> curve=s_low
        let mut down_logic = FanControlLogic::new(profile, FanLogicType::Cpu);
        assert_eq!(down_logic.report_temperature(75), s_high);
        let down_speed = down_logic.report_temperature(0);
        let expected_down = (f64::from(s_high)
            + FanControlLogic::SPEED_ALPHA_DOWN * (f64::from(s_low) - f64::from(s_high)))
        .round() as u8;
        assert_eq!(
            down_speed, expected_down,
            "case {case}: falling speed EWMA mismatch for {s_high}->{s_low}"
        );

        assert!(
            up_speed - s_low > s_high - down_speed,
            "case {case}: speed ramp-up ({}) must exceed ramp-down ({})",
            up_speed - s_low,
            s_high - down_speed
        );
    }
}

#[test]
fn prop_hysteresis_band_holds_speed_within_3c_and_tracks_outside() {
    let mut rng = SplitMix64::new(0xFA11_0004_2026_0004);
    let linear_curve = vec![FanPoint::new(0, 0), FanPoint::new(100, 100)];
    let profile = make_profile(linear_curve.clone(), linear_curve);

    for case in 0..250 {
        let base_temp = rng.gen_i32_range(35, 78);
        let mut logic = FanControlLogic::new(profile.clone(), FanLogicType::Cpu);
        let initial = logic.report_temperature(base_temp);
        assert_eq!(initial, base_temp as u8);

        // Small drops keeping filtered_temp within [base_temp - 3, base_temp] must not drop speed.
        for drop in 1..=FanControlLogic::HYSTERESIS_DEG {
            let mut probe = logic.clone();
            // Feed raw temp slightly below base_temp; EWMA moves by 0.15 * drop <= 3C
            let next_speed = probe.report_temperature(base_temp - drop);
            assert_eq!(
                next_speed, initial,
                "case {case}: drop of {drop}C from {base_temp}C must stay inside hysteresis band"
            );
        }

        // Feed raw temp chosen so filtered_temp drops by exactly 3C on first step:
        // 0.15 * 20 = 3.0 -> filtered_temp = base_temp - 3 -> still inside HYSTERESIS_DEG = 3!
        let mut exact_band = logic.clone();
        let band_speed = exact_band.report_temperature(base_temp - 20);
        assert_eq!(exact_band.filtered_temperature(), base_temp - 3);
        assert_eq!(
            band_speed, initial,
            "case {case}: exact 3C filtered drop must remain pinned by hysteresis"
        );

        // Sustained drops well beyond the 3C hysteresis band must eventually reduce speed,
        // while never dropping below the filtered temperature itself.
        let mut cooling = logic.clone();
        let mut prev_speed = initial;
        for _ in 0..10 {
            let spd = cooling.report_temperature(0);
            assert!(spd <= prev_speed, "case {case}: cooling speed must not increase");
            assert!(
                i32::from(spd) >= cooling.filtered_temperature(),
                "case {case}: speed {spd} dropped below filtered temp {}",
                cooling.filtered_temperature()
            );
            prev_speed = spd;
        }
        assert!(
            prev_speed < initial,
            "case {case}: sustained drop below hysteresis band must eventually lower speed"
        );
    }
}

#[test]
fn prop_critical_temperature_overrides_and_hw_limits() {
    let mut rng = SplitMix64::new(0xFA11_0005_2026_0005);

    for case in 0..250 {
        // Intentionally low fan curves (0..=40%) to prove critical overrides supersede any curve
        let max_curve_spd = rng.gen_u8_range(0, 40);
        let low_curve = vec![
            FanPoint::new(20, 0),
            FanPoint::new(60, max_curve_spd / 2),
            FanPoint::new(100, max_curve_spd),
        ];
        let profile = make_profile(low_curve.clone(), low_curve);
        let hw_min = rng.gen_i32_range(0, 45);
        let off_avail = rng.gen_bool();

        let mut logic = FanControlLogic::new(profile, FanLogicType::Cpu);
        logic.set_fans_min_speed_hw_limit(hw_min);
        logic.set_fans_off_available(off_avail);

        for step in 0..20 {
            let temp = rng.gen_i32_range(20, 115);
            let spd = logic.report_temperature(temp);
            let filtered = logic.filtered_temperature();

            assert!(spd <= 100);
            if filtered >= FanControlLogic::OVERHEAT_TEMPERATURE_C {
                assert_eq!(
                    spd, 100,
                    "case {case} step {step}: filtered {filtered}C >= 90C must be 100%, got {spd}%"
                );
            } else if filtered >= FanControlLogic::CRITICAL_TEMPERATURE_C {
                assert!(
                    spd >= 80,
                    "case {case} step {step}: filtered {filtered}C >= 85C must be >=80%, got {spd}%"
                );
            } else if !off_avail && hw_min > 0 {
                assert!(
                    i32::from(spd) >= hw_min,
                    "case {case} step {step}: off=false requires speed {spd} >= hw_min {hw_min}"
                );
            } else if off_avail && hw_min > 0 {
                assert!(
                    spd == 0 || i32::from(spd) >= hw_min,
                    "case {case} step {step}: speed {spd} violated hw_min {hw_min} deadband"
                );
            }
        }
    }

    // Explicit boundary checks at 84C, 85C, 89C, 90C, 150C
    let flat_zero = make_profile(
        vec![FanPoint::new(0, 0), FanPoint::new(150, 0)],
        vec![FanPoint::new(0, 0), FanPoint::new(150, 0)],
    );
    for (temp, expected_min, expected_exact) in [
        (84, 0u8, Some(0u8)),
        (85, 80, Some(80)),
        (89, 80, Some(80)),
        (90, 100, Some(100)),
        (150, 100, Some(100)),
    ] {
        let mut logic = FanControlLogic::new(flat_zero.clone(), FanLogicType::Cpu);
        let spd = logic.report_temperature(temp);
        assert!(spd >= expected_min);
        if let Some(exact) = expected_exact {
            assert_eq!(spd, exact, "boundary {temp}C expected {exact}%, got {spd}%");
        }
    }
}

#[test]
fn prop_fan_controller_same_speed_and_sensor_failsafe() {
    let mut rng = SplitMix64::new(0xFA11_0006_2026_0006);

    for case in 0..200 {
        let fan_count = rng.gen_usize_range(2, 4);
        let hw_min = rng.gen_i32_range(0, 25);
        let off_avail = rng.gen_bool();
        let backend = make_mock_backend(fan_count, hw_min, off_avail);

        let cpu_pts = gen_monotonic_points(&mut rng, 2, 6);
        let gpu_pts = gen_monotonic_points(&mut rng, 2, 6);
        let profile = make_profile(cpu_pts, gpu_pts);

        let same_speed = rng.gen_bool();
        let mut controller = FanController::new(&backend, profile.clone(), same_speed);
        assert_eq!(controller.fan_count(), fan_count);
        assert_eq!(controller.same_speed(), same_speed);

        for idx in 0..fan_count {
            backend.set_fan_temp(idx, rng.gen_i32_range(25, 95));
        }

        // Normal tick: verify same_speed synchronization vs independent speeds
        backend.clear_journal();
        let snap = controller.tick(&backend, true, 1000 + case as i64);
        assert!(!snap.fallback_auto);
        assert_eq!(snap.speeds_percent.len(), fan_count);
        assert_eq!(snap.calculated_speeds.len(), fan_count);

        let applied: Vec<u8> = snap
            .speeds_percent
            .iter()
            .map(|s| s.expect("healthy tick must report confirmed speed"))
            .collect();
        if same_speed {
            let first = applied[0];
            assert!(
                applied.iter().all(|&s| s == first),
                "case {case}: same_speed=true must synchronize all fans, got {applied:?}"
            );
        }

        // Journal must contain exactly one SetFanSpeed per fan matching snapshot
        let writes = backend.journal();
        assert_eq!(writes.len(), fan_count);
        for (idx, w) in writes.iter().enumerate() {
            assert_eq!(
                *w,
                PlatformWrite::SetFanSpeed {
                    fan_idx: idx,
                    percent: applied[idx],
                }
            );
        }

        // Toggle same_speed and verify sync invariant on next tick
        controller.set_same_speed(true);
        backend.clear_journal();
        let snap_sync = controller.tick(&backend, true, 2000 + case as i64);
        assert!(!snap_sync.fallback_auto);
        let synced: Vec<u8> = snap_sync.speeds_percent.iter().map(|s| s.unwrap()).collect();
        assert!(synced.iter().all(|&s| s == synced[0]));

        // Inject sensor failure on random non-empty subset (missing sensor or out-of-range temp)
        backend.clear_journal();
        let failed_idx = rng.gen_usize_range(0, fan_count - 1);
        for idx in 0..fan_count {
            if idx == failed_idx || rng.gen_bool() {
                match rng.gen_usize_range(0, 3) {
                    0 => backend.set_fan_temp_error(idx),
                    1 => backend.set_fan_temp(idx, -rng.gen_i32_range(1, 50)),
                    2 => backend.set_fan_temp(idx, rng.gen_i32_range(151, 500)),
                    _ => backend.set_fan_temp(idx, i32::MAX),
                }
            }
        }

        let snap_fail = controller.tick(&backend, true, 3000 + case as i64);
        assert!(
            snap_fail.fallback_auto,
            "case {case}: sensor failure must engage fallback_auto"
        );
        assert!(
            snap_fail.speeds_percent.iter().all(Option::is_none),
            "case {case}: fallback_auto must not report manual applied speeds"
        );
        let fail_journal = backend.journal();
        assert_eq!(
            fail_journal,
            vec![PlatformWrite::SetFansAuto],
            "case {case}: sensor failure must call SetFansAuto and never write SetFanSpeed"
        );
        assert!(!fail_journal.iter().any(|w| matches!(
            w,
            PlatformWrite::SetFanSpeed { percent: 0, .. }
        )));

        // Recovery: restore valid temps on all channels -> resumes active curve control
        for idx in 0..fan_count {
            backend.set_fan_temp(idx, rng.gen_i32_range(35, 80));
        }
        backend.clear_journal();
        let snap_rec = controller.tick(&backend, true, 4000 + case as i64);
        assert!(!snap_rec.fallback_auto);
        assert!(snap_rec.speeds_percent.iter().all(Option::is_some));
    }

    // Edge case: empty curve triggers failsafe auto (never 0%), and cancellation writes nothing.
    let backend = make_mock_backend(2, 20, true);
    let mut ctrl = FanController::new(&backend, make_profile(vec![], vec![]), false);
    backend.clear_journal();
    let snap = ctrl.tick(&backend, true, 100);
    assert!(snap.fallback_auto);
    assert_eq!(backend.journal(), vec![PlatformWrite::SetFansAuto]);

    // Temporary curve restores valid curve, then consecutive write failures trigger write failsafe
    ctrl.apply_temporary_curves(TemporaryFanCurves {
        cpu: vec![FanPoint::new(40, 50), FanPoint::new(80, 90)],
        gpu: vec![FanPoint::new(40, 45), FanPoint::new(80, 85)],
        water_cooler_fan: vec![],
        pump: vec![],
    });
    backend.clear_journal();
    let cancelled = ctrl.tick_cancellable(&backend, true, 200, || true);
    assert!(cancelled.speeds_percent.iter().all(Option::is_none));
    assert!(backend.journal().is_empty());

    backend.set_fail_writes(true);
    let fail1 = ctrl.tick(&backend, true, 300);
    assert!(fail1.speeds_percent.iter().all(Option::is_none));
    let fail2 = ctrl.tick(&backend, true, 400);
    assert!(fail2.speeds_percent.iter().all(Option::is_none));
    assert!(backend.journal().is_empty());
}

fn write_sparse_cpu_fixture(
    root: &Path,
    present_ids: &[u32],
    mandatory_ids: &HashSet<u32>,
    initial_online: &HashSet<u32>,
) {
    fs::create_dir_all(root).unwrap();
    let present_i32: Vec<i32> = present_ids.iter().map(|&id| id as i32).collect();
    let range_str = format_cpu_range(&present_i32);
    fs::write(root.join("present"), &range_str).unwrap();
    fs::write(root.join("possible"), &range_str).unwrap();

    for &id in present_ids {
        let cpu_dir = root.join(format!("cpu{id}"));
        fs::create_dir_all(&cpu_dir).unwrap();
        if !mandatory_ids.contains(&id) {
            let val = if initial_online.contains(&id) { "1" } else { "0" };
            fs::write(cpu_dir.join("online"), val).unwrap();
        }
        let topo_dir = cpu_dir.join("topology");
        fs::create_dir_all(&topo_dir).unwrap();
        fs::write(topo_dir.join("core_id"), (id / 2).to_string()).unwrap();
        fs::write(topo_dir.join("thread_siblings_list"), id.to_string()).unwrap();
    }
}

#[test]
fn prop_sparse_cpu_range_roundtrip_and_topology_discovery() {
    let mut rng = SplitMix64::new(0xC900_0001_2026_0001);

    for case in 0..250 {
        let count = rng.gen_usize_range(1, 16);
        let mut id_set = HashSet::new();
        id_set.insert(0u32);
        while id_set.len() < count {
            id_set.insert(rng.gen_u32_range(0, 127));
        }
        let mut ids: Vec<u32> = id_set.into_iter().collect();
        ids.sort_unstable();

        let ids_i32: Vec<i32> = ids.iter().map(|&x| x as i32).collect();
        let formatted = format_cpu_range(&ids_i32);
        let parsed_i32 = parse_cpu_range(&formatted).expect("formatted range must parse");
        assert_eq!(parsed_i32, ids_i32, "case {case}: parse_cpu_range mismatch");
        assert_eq!(
            parse_cpu_range_list(&formatted),
            ids,
            "case {case}: parse_cpu_range_list mismatch"
        );

        // Verify TempDir sparse CpuTopology discovery never modifies offline cores
        if case < 60 {
            let temp = TempDir::new().unwrap();
            let mut mandatory = HashSet::new();
            mandatory.insert(0u32);
            let mut online = HashSet::new();
            online.insert(0u32);
            for &id in &ids {
                if id != 0 && rng.gen_bool() {
                    online.insert(id);
                }
            }
            write_sparse_cpu_fixture(temp.path(), &ids, &mandatory, &online);

            let mut topo = CpuTopology::new(temp.path());
            let discovered = topo.read();
            let discovered_ids: Vec<u32> = discovered.iter().map(|c| c.id).collect();
            assert_eq!(discovered_ids, ids, "case {case}: discovered sparse IDs mismatch");

            for info in &discovered {
                let is_mandatory = mandatory.contains(&info.id);
                assert_eq!(info.hotpluggable, !is_mandatory);
                let expected_online = is_mandatory || online.contains(&info.id);
                assert_eq!(info.online, expected_online);
                if !is_mandatory {
                    let disk_val = fs::read_to_string(
                        temp.path().join(format!("cpu{}/online", info.id)),
                    )
                    .unwrap();
                    assert_eq!(disk_val.trim(), if expected_online { "1" } else { "0" });
                }
            }
        }
    }
}

#[test]
fn prop_cpu_mandatory_preservation_and_enable_before_disable_ordering() {
    let mut rng = SplitMix64::new(0xC900_0002_2026_0002);

    for case in 0..120 {
        let count = rng.gen_usize_range(2, 12);
        let mut id_set = HashSet::new();
        id_set.insert(0u32);
        while id_set.len() < count {
            id_set.insert(rng.gen_u32_range(1, 63));
        }
        let mut ids: Vec<u32> = id_set.into_iter().collect();
        ids.sort_unstable();

        // Core 0 is always mandatory; occasionally make a second core mandatory too
        let mut mandatory = HashSet::new();
        mandatory.insert(0u32);
        if count >= 4 && rng.gen_bool() {
            mandatory.insert(ids[1]);
        }

        let mut initial_online = mandatory.clone();
        for &id in &ids {
            if !mandatory.contains(&id) && rng.gen_bool() {
                initial_online.insert(id);
            }
        }

        let temp = TempDir::new().unwrap();
        write_sparse_cpu_fixture(temp.path(), &ids, &mandatory, &initial_online);
        let controller = CpuController::new(temp.path());
        assert_eq!(controller.cores.len(), ids.len());

        // Validation: omitting any mandatory core must fail
        for &mand_id in &mandatory {
            let without_mand: Vec<u32> = ids.iter().copied().filter(|&x| x != mand_id).collect();
            if !without_mand.is_empty() {
                let err = controller
                    .validate_enabled_cores(&without_mand)
                    .expect_err("omitting mandatory core must fail");
                assert!(
                    err.contains("cannot be disabled"),
                    "case {case}: unexpected error '{err}'"
                );
            }
        }

        // Validation: empty, duplicate, and unknown IDs must fail
        assert!(controller.validate_enabled_cores(&[]).is_err());
        assert!(controller.validate_enabled_cores(&[0, 0]).is_err());
        assert!(controller.validate_enabled_cores(&[0, 999]).is_err());

        // Construct random valid target selection containing all mandatory cores
        let mut target: Vec<u32> = mandatory.iter().copied().collect();
        for &id in &ids {
            if !mandatory.contains(&id) && rng.gen_bool() {
                target.push(id);
            }
        }
        target.sort_unstable();
        assert!(controller.validate_enabled_cores(&target).is_ok());

        // Execute set_enabled_cores and verify strict enable-before-disable ordering
        let mut events: Vec<(u32, bool)> = Vec::new();
        let res = controller.set_enabled_cores(&target, |core, enable| {
            events.push((core.core_index, enable));
            core.write_online(enable).is_ok()
        });
        assert!(res.is_ok(), "case {case}: set_enabled_cores failed: {res:?}");

        let mut seen_disable = false;
        for &(core_id, enable) in &events {
            assert!(
                !mandatory.contains(&core_id),
                "case {case}: mandatory core {core_id} must never be toggled"
            );
            if enable {
                assert!(
                    !seen_disable,
                    "case {case}: enable({core_id}) occurred after a disable in {events:?}"
                );
            } else {
                seen_disable = true;
            }
        }

        // Verify final disk state matches target selection
        let target_set: HashSet<u32> = target.iter().copied().collect();
        for core in &controller.cores {
            assert_eq!(
                core.read_online(),
                Some(target_set.contains(&core.core_index)),
                "case {case}: final online state mismatch on core {}",
                core.core_index
            );
        }

        // Verify failed enable aborts immediately with ZERO disables
        let offline_hotplug: Vec<u32> = controller
            .cores
            .iter()
            .filter(|c| c.is_online_controllable() && !c.read_online().unwrap_or(false))
            .map(|c| c.core_index)
            .collect();
        if let Some(&to_enable) = offline_hotplug.first() {
            let mut fail_target: Vec<u32> = mandatory.iter().copied().collect();
            fail_target.push(to_enable);
            fail_target.sort_unstable();
            let mut fail_events = Vec::new();
            let err = controller.set_enabled_cores(&fail_target, |core, enable| {
                fail_events.push((core.core_index, enable));
                false
            });
            assert!(err.is_err());
            assert!(
                fail_events.iter().all(|&(_, enable)| enable),
                "case {case}: failed enable must never proceed to disable: {fail_events:?}"
            );
        }

        // use_cores(Some(0)) and use_cores(Some(999)) preserve all mandatory cores
        assert!(controller.use_cores(Some(0)).is_ok());
        for core in &controller.cores {
            if mandatory.contains(&core.core_index) {
                assert_eq!(core.read_online(), Some(true));
            }
        }
        assert!(controller.use_cores(Some(999)).is_ok());
        assert!(controller
            .cores
            .iter()
            .all(|c| c.read_online() == Some(true)));
    }
}

#[test]
fn prop_cpu_frequency_ordering_clamping_and_snapping() {
    let mut rng = SplitMix64::new(0xC900_0003_2026_0003);

    // 1. find_closest_value optimality property
    for case in 0..300 {
        let len = rng.gen_usize_range(0, 12);
        let values: Vec<u32> = (0..len)
            .map(|_| rng.gen_u32_range(400_000, 6_000_000))
            .collect();
        let target = rng.gen_u32_range(0, 7_000_000);
        let chosen = find_closest_value(target, &values);
        if values.is_empty() {
            assert_eq!(chosen, target);
        } else {
            assert!(values.contains(&chosen), "case {case}: chosen not in slice");
            let best_diff = target.abs_diff(chosen);
            for &v in &values {
                assert!(
                    best_diff <= target.abs_diff(v),
                    "case {case}: {v} is closer to {target} than {chosen}"
                );
            }
        }
    }

    // 2. Per-core frequency min/max clamping, no_turbo base_frequency limit, and snapping
    for case in 0..120 {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("intel_pstate")).unwrap();
        let no_turbo = rng.gen_bool();
        fs::write(
            root.join("intel_pstate/no_turbo"),
            if no_turbo { "1" } else { "0" },
        )
        .unwrap();
        fs::write(root.join("present"), "0,4").unwrap();
        fs::write(root.join("possible"), "0,4").unwrap();

        let mut expected_hw_max = Vec::new();
        for &id in &[0u32, 4u32] {
            let cpu_dir = root.join(format!("cpu{id}"));
            let cpufreq = cpu_dir.join("cpufreq");
            fs::create_dir_all(&cpufreq).unwrap();
            if id != 0 {
                fs::write(cpu_dir.join("online"), "1").unwrap();
            }

            let hw_min = rng.gen_u32_range(400_000, 1_200_000);
            let base = rng.gen_u32_range(hw_min + 200_000, 3_000_000);
            let hw_max = rng.gen_u32_range(base + 100_000, 5_800_000);
            let eff_max = if no_turbo { hw_max.min(base) } else { hw_max };
            expected_hw_max.push((hw_min, eff_max));

            fs::write(cpufreq.join("cpuinfo_min_freq"), hw_min.to_string()).unwrap();
            fs::write(cpufreq.join("cpuinfo_max_freq"), hw_max.to_string()).unwrap();
            fs::write(cpufreq.join("base_frequency"), base.to_string()).unwrap();
            fs::write(cpufreq.join("scaling_min_freq"), hw_min.to_string()).unwrap();
            fs::write(cpufreq.join("scaling_max_freq"), hw_max.to_string()).unwrap();
            fs::write(cpufreq.join("scaling_driver"), "intel_pstate").unwrap();

            if rng.gen_bool() {
                let mut avail = vec![hw_min, base, eff_max, hw_max];
                for _ in 0..4 {
                    avail.push(rng.gen_u32_range(hw_min, hw_max));
                }
                avail.sort_unstable();
                avail.dedup();
                let s: Vec<String> = avail.iter().map(u32::to_string).collect();
                fs::write(cpufreq.join("scaling_available_frequencies"), s.join(" ")).unwrap();
            }
        }

        let controller = CpuController::new(root);
        assert_eq!(controller.cores.len(), 2);

        for (idx, core) in controller.cores.iter().enumerate() {
            let (hw_min, eff_max) = expected_hw_max[idx];
            assert_eq!(
                controller.effective_hardware_max_freq(core),
                Some(eff_max),
                "case {case} core {}: effective_hardware_max_freq mismatch",
                core.core_index
            );

            // Random target_max values including sentinel -1, None, 0, i64::MIN, i64::MAX
            let target_max = match rng.gen_usize_range(0, 5) {
                0 => None,
                1 => Some(-1),
                2 => Some(0),
                3 => Some(i64::MAX),
                4 => Some(i64::MIN),
                _ => Some(i64::from(rng.gen_u32_range(200_000, 7_000_000))),
            };
            let computed_max = controller
                .compute_effective_max_freq(core, target_max, false)
                .expect("valid cpufreq nodes must produce Some max");
            let cur_min = core.read_scaling_min_freq().unwrap().min(eff_max);
            assert!(
                (cur_min..=eff_max).contains(&computed_max),
                "case {case}: computed_max {computed_max} outside [{cur_min}, {eff_max}]"
            );

            // Random target_min values including sentinel -2, None, 0, i64::MAX
            let target_min = match rng.gen_usize_range(0, 4) {
                0 => None,
                1 => Some(-2),
                2 => Some(0),
                3 => Some(i64::MAX),
                _ => Some(i64::from(rng.gen_u32_range(200_000, 7_000_000))),
            };
            let computed_min = controller
                .compute_effective_min_freq(core, target_min)
                .expect("valid cpufreq nodes must produce Some min");
            let cur_max = core.read_scaling_max_freq().unwrap().min(eff_max);
            assert!(
                (hw_min..=cur_max).contains(&computed_min),
                "case {case}: computed_min {computed_min} outside [{hw_min}, {cur_max}]"
            );
        }

        // Apply arbitrary requested min/max pair and confirm scaling_min <= scaling_max <= eff_max
        let req_min = Some(i64::from(rng.gen_u32_range(200_000, 6_500_000)));
        let req_max = Some(i64::from(rng.gen_u32_range(200_000, 6_500_000)));
        controller.set_governor_scaling_min_frequency(None).unwrap();
        controller.set_governor_scaling_max_frequency(None).unwrap();
        controller
            .set_governor_scaling_min_frequency(req_min)
            .unwrap();
        controller
            .set_governor_scaling_max_frequency(req_max)
            .unwrap();

        for (idx, core) in controller.cores.iter().enumerate() {
            let (hw_min, eff_max) = expected_hw_max[idx];
            let smin = core.read_scaling_min_freq().unwrap();
            let smax = core.read_scaling_max_freq().unwrap();
            assert!(
                (hw_min..=smax).contains(&smin) && smax <= eff_max,
                "case {case}: bad freq order hw_min={hw_min} smin={smin} smax={smax} max={eff_max}"
            );
        }
    }

    // Regression: high u32 cpuinfo_min_freq + cpuinfo_max_freq must not overflow midpoint.
    let overflow_dir = tempfile::tempdir().unwrap();
    fs::write(overflow_dir.path().join("present"), "0").unwrap();
    fs::write(overflow_dir.path().join("possible"), "0").unwrap();
    let cpufreq = overflow_dir.path().join("cpu0/cpufreq");
    fs::create_dir_all(&cpufreq).unwrap();
    fs::write(
        cpufreq.join("cpuinfo_min_freq"),
        (u32::MAX - 10).to_string(),
    )
    .unwrap();
    fs::write(cpufreq.join("cpuinfo_max_freq"), u32::MAX.to_string()).unwrap();
    let overflow_ctrl = CpuController::new(overflow_dir.path().to_path_buf());
    assert_eq!(
        overflow_ctrl.cores[0].get_reduced_available_freq(),
        Some(u32::MAX - 5)
    );
}
