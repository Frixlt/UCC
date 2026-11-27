//! Deterministic property-based tests for CPU limits, fan curves, pump voltage
//! validation, and `FanControlLogic` cooling behavior in `ucc-core`.
//!
//! Uses a self-contained SplitMix64 PRNG with fixed seeds (zero external dev-dependencies)
//! to verify:
//! - CPU core masks, sparse core IDs (`enabled_cores` / `online_cores`), mandatory core 0
//!   preservation, and frequency/TDP bounds and sentinel round-trips.
//! - Monotonic and non-monotonic fan/pump curves, interpolation accuracy, half-integer `.5`
//!   rounding (round-half-away-from-zero), and out-of-range clamping.
//! - Pump step selection, permutation invariance, legacy `V12` clamping, and strict
//!   rejection of forbidden pump voltage code `1` (`V12`).
//! - `FanControlLogic` hardware min-speed limits, asymmetric temperature filtering,
//!   and critical/overheat safety overrides.

use std::collections::BTreeSet;
use ucc_core::{
    get_pump_speed_for_temp, pump_speed_to_voltage, voltage_to_pump_speed, CpuLimits,
    FanControlLogic, FanCurve, FanLogicType, FanPoint, FanProfile, LegacyCpuDto,
    LegacyOdmPowerLimitsDto, LegacyProfileDto, ProfileCpu, ProfileFanControl, PumpPoint,
    PumpVoltage, TemperatureFilter, TemporaryFanCurves, UccProfile, ValidationError,
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

    fn gen_range_u32(&mut self, low: u32, high: u32) -> u32 {
        assert!(low <= high);
        let span = u64::from(high) - u64::from(low) + 1;
        low + (self.next_u64() % span) as u32
    }

    fn gen_range_i32(&mut self, low: i32, high: i32) -> i32 {
        assert!(low <= high);
        let span = (i64::from(high) - i64::from(low) + 1) as u64;
        (i64::from(low) + (self.next_u64() % span) as i64) as i32
    }

    fn gen_range_u8(&mut self, low: u8, high: u8) -> u8 {
        self.gen_range_u32(u32::from(low), u32::from(high)) as u8
    }

    fn next_bool(&mut self) -> bool {
        (self.next_u64() & 1) == 1
    }
}

// ========================================================================
// 1. CPU Masks, Sparse Core IDs, Mandatory Core 0, Frequency & TDP Bounds
// ========================================================================

#[test]
fn prop_cpu_sparse_core_masks_and_mandatory_core_zero() {
    let mut rng = SplitMix64::new(0x4350_5543_4F52_0001);

    for _ in 0..500 {
        let count = rng.gen_range_u32(1, 24) as usize;
        let mut set = BTreeSet::new();
        set.insert(0u32);
        while set.len() < count {
            // Mix dense low core IDs and sparse high core IDs up to max valid 1_048_575.
            let id = if rng.next_bool() {
                rng.gen_range_u32(1, 128)
            } else {
                rng.gen_range_u32(1, 1_048_575)
            };
            set.insert(id);
        }

        // Shuffle core IDs to verify validation accepts arbitrary orderings.
        let mut cores: Vec<u32> = set.into_iter().collect();
        for i in (1..cores.len()).rev() {
            let j = rng.gen_range_u32(0, i as u32) as usize;
            cores.swap(i, j);
        }

        let online = rng.gen_range_u32(1, 4096);
        let limits = CpuLimits {
            online_cores: Some(online),
            enabled_cores: Some(cores.clone()),
            ..CpuLimits::new()
        };
        assert!(
            limits.validate().is_ok(),
            "valid sparse core set with core 0 must pass: {cores:?}"
        );

        // Removing mandatory core 0 must always be rejected.
        let without_zero: Vec<u32> = cores.iter().copied().filter(|&c| c != 0).collect();
        let no_zero_limits = CpuLimits {
            enabled_cores: Some(without_zero),
            ..CpuLimits::new()
        };
        assert!(
            matches!(
                no_zero_limits.validate(),
                Err(ValidationError::InvalidCoreSelection(_))
            ),
            "core selection without mandatory core 0 must fail"
        );

        // Duplicating any existing core ID must always be rejected.
        let mut with_dup = cores.clone();
        let dup_val = cores[rng.gen_range_u32(0, (cores.len() - 1) as u32) as usize];
        with_dup.push(dup_val);
        let dup_limits = CpuLimits {
            enabled_cores: Some(with_dup),
            ..CpuLimits::new()
        };
        assert!(
            matches!(
                dup_limits.validate(),
                Err(ValidationError::InvalidCoreSelection(_))
            ),
            "duplicate core ID {dup_val} must be rejected"
        );

        // Adding out-of-range core ID (> 1_048_575) must always be rejected.
        let mut with_oob = cores.clone();
        with_oob.push(1_048_576 + rng.gen_range_u32(0, 10_000));
        let oob_limits = CpuLimits {
            enabled_cores: Some(with_oob),
            ..CpuLimits::new()
        };
        assert!(
            matches!(
                oob_limits.validate(),
                Err(ValidationError::InvalidCoreSelection(_))
            ),
            "core ID > 1_048_575 must be rejected"
        );
    }

    // Explicit boundary: max supported 4096 cores passes; 4097 cores fails.
    let max_cores: Vec<u32> = (0..4096).collect();
    let max_limits = CpuLimits {
        enabled_cores: Some(max_cores),
        ..CpuLimits::new()
    };
    assert!(max_limits.validate().is_ok());

    let too_many_cores: Vec<u32> = (0..4097).collect();
    let too_many_limits = CpuLimits {
        enabled_cores: Some(too_many_cores),
        ..CpuLimits::new()
    };
    assert!(matches!(
        too_many_limits.validate(),
        Err(ValidationError::InvalidCoreSelection(_))
    ));

    // Boundary: online_cores == Some(0) is rejected, while LegacyProfileDto rejects negative IDs.
    let zero_online = CpuLimits {
        online_cores: Some(0),
        ..CpuLimits::new()
    };
    assert!(matches!(
        zero_online.validate(),
        Err(ValidationError::InvalidCoreSelection(_))
    ));

    let neg_dto = LegacyProfileDto {
        id: "neg".to_string(),
        name: "Neg".to_string(),
        cpu: Some(LegacyCpuDto {
            enabled_cores: Some(vec![0, -1, 2]),
            ..LegacyCpuDto::default()
        }),
        ..LegacyProfileDto::default()
    };
    assert!(matches!(
        neg_dto.into_profile(),
        Err(ValidationError::InvalidCoreSelection(_))
    ));

    // Sparse core IDs round-trip cleanly through LegacyProfileDto.
    let sparse_dto = LegacyProfileDto {
        id: "sparse".to_string(),
        name: "Sparse".to_string(),
        cpu: Some(LegacyCpuDto {
            online_cores: Some(4),
            enabled_cores: Some(vec![0, 4, 8, 12]),
            ..LegacyCpuDto::default()
        }),
        ..LegacyProfileDto::default()
    };
    let profile = sparse_dto
        .into_profile()
        .expect("sparse DTO must validate");
    assert_eq!(profile.cpu.enabled_cores, Some(vec![0, 4, 8, 12]));
    let roundtrip = LegacyProfileDto::from_profile(&profile);
    assert_eq!(
        roundtrip.cpu.expect("cpu present").enabled_cores,
        Some(vec![0, 4, 8, 12])
    );
}

#[test]
fn prop_cpu_frequency_tdp_bounds_and_sentinels() {
    let mut rng = SplitMix64::new(0x4350_5543_4F52_0002);

    // 500 randomized frequency bound checks.
    for _ in 0..500 {
        let min = if rng.next_bool() {
            Some(if rng.next_bool() {
                rng.gen_range_u32(100_000, 10_000_000)
            } else {
                rng.gen_range_u32(0, 12_000_000)
            })
        } else {
            None
        };
        let max = if rng.next_bool() {
            Some(if rng.next_bool() {
                rng.gen_range_u32(100_000, 10_000_000)
            } else {
                rng.gen_range_u32(0, 12_000_000)
            })
        } else {
            None
        };

        let limits = CpuLimits {
            min_freq_khz: min,
            max_freq_khz: max,
            ..CpuLimits::new()
        };

        let expected = match (min, max) {
            (Some(lo), Some(hi)) if lo > hi => {
                Err(ValidationError::InvalidFrequencyRange { min: lo, max: hi })
            }
            (Some(lo), _) if !(100_000..=10_000_000).contains(&lo) => {
                Err(ValidationError::InvalidFrequency(lo))
            }
            (_, Some(hi)) if !(100_000..=10_000_000).contains(&hi) => {
                Err(ValidationError::InvalidFrequency(hi))
            }
            _ => Ok(()),
        };
        assert_eq!(limits.validate(), expected, "min={min:?}, max={max:?}");
    }

    // 500 randomized TDP validation checks (valid monotonic, out-of-range, non-monotonic).
    for _ in 0..500 {
        let len = rng.gen_range_u32(0, 6) as usize;
        let mut tdps = Vec::with_capacity(len);
        for _ in 0..len {
            tdps.push(rng.gen_range_u32(0, 520));
        }

        let limits = CpuLimits {
            tdp_values_w: tdps.clone(),
            ..CpuLimits::new()
        };

        let mut expected = Ok(());
        for &w in &tdps {
            if w == 0 || w > 500 {
                expected = Err(ValidationError::InvalidTdp(w));
                break;
            }
        }
        if expected.is_ok() {
            for (idx, pair) in tdps.windows(2).enumerate() {
                if pair[1] < pair[0] {
                    expected = Err(ValidationError::NonMonotonicTdp {
                        index: idx + 1,
                        prev: pair[0],
                        curr: pair[1],
                    });
                    break;
                }
            }
        }
        assert_eq!(limits.validate(), expected, "tdps={tdps:?}");
    }

    // ProfileCpu JSON sentinel handling: negative -> sentinel, 0 -> None, positive -> kHz.
    let raw_json = r#"{
        "onlineCores": -1,
        "scalingMinFrequency": -2,
        "scalingMaxFrequency": 0,
        "governor": "powersave"
    }"#;
    let parsed: ProfileCpu = serde_json::from_str(raw_json).expect("deserialize sentinel cpu");
    assert_eq!(parsed.online_cores, None);
    assert_eq!(parsed.min_freq_khz, None);
    assert_eq!(parsed.min_freq_sentinel, Some(-2));
    assert_eq!(parsed.max_freq_khz, None);
    assert_eq!(parsed.max_freq_sentinel, None);
    assert!(parsed.validate(&[25, 45, 65]).is_ok());

    // LegacyProfileDto clamps negative TDP values to 0, failing validation as InvalidTdp(0).
    let neg_tdp_dto = LegacyProfileDto {
        id: "neg-tdp".to_string(),
        name: "Neg TDP".to_string(),
        odm_power_limits: Some(LegacyOdmPowerLimitsDto {
            tdp_values: vec![-10, 45],
        }),
        ..LegacyProfileDto::default()
    };
    assert_eq!(
        neg_tdp_dto.into_profile(),
        Err(ValidationError::InvalidTdp(0))
    );
}

// ========================================================================
// 2. Fan Curves: Monotonicity, Interpolation, Rounding (.5), Clamping
// ========================================================================

#[test]
fn prop_fan_curve_monotonic_and_non_monotonic_validation() {
    let mut rng = SplitMix64::new(0x4641_4E43_5552_0001);

    for _ in 0..500 {
        let len = rng.gen_range_u32(1, 10) as usize;
        let mut temps = BTreeSet::new();
        while temps.len() < len {
            temps.insert(rng.gen_range_i32(-20, 150));
        }
        let temp_vec: Vec<i32> = temps.into_iter().collect();

        let mut speeds: Vec<u8> = (0..len).map(|_| rng.gen_range_u8(0, 100)).collect();
        speeds.sort();

        let points: Vec<FanPoint> = temp_vec
            .iter()
            .zip(speeds.iter())
            .map(|(&t, &s)| FanPoint::new(t, s))
            .collect();

        // Valid monotonic curve passes.
        assert!(FanCurve::new(points.clone()).is_ok());

        // Inject out-of-range speed (> 100) -> InvalidFanSpeed.
        let mut bad_speed = points.clone();
        let idx = rng.gen_range_u32(0, (len - 1) as u32) as usize;
        let invalid_s = rng.gen_range_u8(101, 255);
        bad_speed[idx].speed = invalid_s;
        assert_eq!(
            FanCurve::new(bad_speed),
            Err(ValidationError::InvalidFanSpeed { speed: invalid_s })
        );

        // Inject out-of-range temperature (< -20 or > 150) -> InvalidTemperature.
        let mut bad_temp = points.clone();
        let invalid_t = if rng.next_bool() {
            rng.gen_range_i32(-500, -21)
        } else {
            rng.gen_range_i32(151, 500)
        };
        bad_temp[idx].temp = invalid_t;
        assert_eq!(
            FanCurve::new(bad_temp),
            Err(ValidationError::InvalidTemperature { temp: invalid_t })
        );

        if len >= 2 {
            let p_idx = rng.gen_range_u32(1, (len - 1) as u32) as usize;

            // Decreasing or duplicate temperature -> NonMonotonicTemperature.
            let mut non_mono_t = points.clone();
            let prev_t = non_mono_t[p_idx - 1].temp;
            let drop_t = if prev_t > -20 && rng.next_bool() {
                prev_t - 1
            } else {
                prev_t
            };
            non_mono_t[p_idx].temp = drop_t;
            assert_eq!(
                FanCurve::new(non_mono_t),
                Err(ValidationError::NonMonotonicTemperature {
                    index: p_idx,
                    prev: prev_t,
                    curr: drop_t,
                })
            );

            // Decreasing speed -> NonMonotonicSpeed.
            if points[p_idx - 1].speed > 0 {
                let mut non_mono_s = points.clone();
                let prev_s = non_mono_s[p_idx - 1].speed;
                let curr_s = prev_s - 1;
                non_mono_s[p_idx].speed = curr_s;
                assert_eq!(
                    FanCurve::new(non_mono_s.clone()),
                    Err(ValidationError::NonMonotonicSpeed {
                        index: p_idx,
                        prev: prev_s,
                        curr: curr_s,
                    })
                );

                // Unvalidated non-monotonic FanCurve still evaluates deterministically in 0..=100.
                let unvalidated = FanCurve { points: non_mono_s };
                for sample_t in [-30, points[0].temp, points[p_idx].temp, 160] {
                    let spd = unvalidated.get_speed_for_temp(sample_t);
                    assert!((0..=100).contains(&spd));
                }
            }
        }
    }

    // Empty curve validation and lookup behavior.
    assert_eq!(FanCurve::new(vec![]), Err(ValidationError::EmptyFanCurve));
    assert_eq!(FanCurve::default().get_speed_for_temp(50), -1);
}

#[test]
fn prop_fan_curve_interpolation_rounding_and_clamping() {
    // Explicit half-integer (.5) rounding tests confirming round-half-away-from-zero
    // (both even and odd integer parts, matching C++ std::lround).
    let half_cases = [
        (0u8, 1u8, 1i32),    // 0.5 -> 1 (even base)
        (1u8, 2u8, 2i32),    // 1.5 -> 2 (odd base)
        (2u8, 3u8, 3i32),    // 2.5 -> 3 (even base)
        (10u8, 25u8, 18i32), // 17.5 -> 18
        (75u8, 90u8, 83i32), // 82.5 -> 83
        (99u8, 100u8, 100),  // 99.5 -> 100
    ];
    for (s0, s1, expected_mid) in half_cases {
        let curve =
            FanCurve::new(vec![FanPoint::new(40, s0), FanPoint::new(50, s1)]).expect("valid");
        assert_eq!(
            curve.get_speed_for_temp(45),
            expected_mid,
            "midpoint of ({s0}, {s1}) must round half away from zero to {expected_mid}"
        );
    }

    // Randomized interpolation accuracy, monotonicity, and extreme clamping across 400 curves.
    let mut rng = SplitMix64::new(0x4641_4E43_5552_0002);
    for _ in 0..400 {
        let len = rng.gen_range_u32(1, 10) as usize;
        let mut temps = BTreeSet::new();
        while temps.len() < len {
            temps.insert(rng.gen_range_i32(-20, 150));
        }
        let temp_vec: Vec<i32> = temps.into_iter().collect();
        let mut speeds: Vec<u8> = (0..len).map(|_| rng.gen_range_u8(0, 100)).collect();
        speeds.sort();

        let points: Vec<FanPoint> = temp_vec
            .iter()
            .zip(speeds.iter())
            .map(|(&t, &s)| FanPoint::new(t, s))
            .collect();
        let curve = FanCurve::new(points.clone()).expect("valid curve");

        let first = points[0];
        let last = points[len - 1];

        // Out-of-range clamping including extreme i32 boundaries.
        for low_t in [i32::MIN, -1000, first.temp - 1, first.temp] {
            assert_eq!(curve.get_speed_for_temp(low_t), i32::from(first.speed));
        }
        for high_t in [last.temp, last.temp + 1, 1000, i32::MAX] {
            assert_eq!(curve.get_speed_for_temp(high_t), i32::from(last.speed));
        }

        // Exact interpolation check and monotonicity across the full spanned range.
        let mut prev_out = -1;
        for t in (first.temp - 5)..=(last.temp + 5) {
            let out = curve.get_speed_for_temp(t);
            assert!((i32::from(first.speed)..=i32::from(last.speed)).contains(&out));
            assert!(
                out >= prev_out,
                "interpolated output must be monotonic at t={t}: {out} < {prev_out}"
            );
            prev_out = out;

            // Compare directly against independent linear formula on active segment.
            if t > first.temp && t < last.temp {
                let seg = points
                    .windows(2)
                    .find(|w| (w[0].temp..=w[1].temp).contains(&t))
                    .expect("segment covers t");
                let frac = f64::from(t - seg[0].temp) / f64::from(seg[1].temp - seg[0].temp);
                let expected = (f64::from(seg[0].speed)
                    + frac * f64::from(seg[1].speed - seg[0].speed))
                .round() as i32;
                assert_eq!(out, expected);
            }
        }
    }

    // FanProfile water cooler fan fallback: max(cpu, gpu) when table_water_cooler_fan is empty.
    let mut fp = FanProfile::new("wc-fallback", "WC Fallback");
    fp.table_cpu = vec![FanPoint::new(40, 20), FanPoint::new(80, 70)];
    fp.table_gpu = vec![FanPoint::new(40, 35), FanPoint::new(80, 60)];
    for t in [30, 40, 55, 65, 80, 95] {
        let expected = fp.get_speed_for_temp(t, true).max(fp.get_speed_for_temp(t, false));
        assert_eq!(fp.get_water_cooler_fan_speed_for_temp(t), expected);
    }

    // Once an explicit water cooler fan curve is set, it overrides the fallback.
    fp.table_water_cooler_fan = vec![FanPoint::new(40, 10), FanPoint::new(80, 50)];
    assert_eq!(fp.get_water_cooler_fan_speed_for_temp(60), 30);

    // Regression: extreme unvalidated knot span (i32::MIN..=i32::MAX) must not overflow.
    let extreme_curve = FanCurve {
        points: vec![FanPoint::new(i32::MIN, 0), FanPoint::new(i32::MAX, 100)],
    };
    let mid_speed = extreme_curve.get_speed_for_temp(0);
    assert!(
        (0..=100).contains(&mid_speed),
        "extreme knot span interpolation out of bounds: {mid_speed}"
    );
}

// ========================================================================
// 3. Pump Step Selection & Forbidden Code 1 (V12) Rejection
// ========================================================================

#[test]
fn prop_pump_step_selection_and_forbidden_v12_rejection() {
    // 1. Exhaustive sweep of discrete pump voltage codes: ONLY {0, 2, 3, 4} are valid;
    //    code 1 (V12) is strictly forbidden.
    for code in -1000..=1000 {
        let res = PumpVoltage::from_discrete_code(code);
        match code {
            0 => {
                assert_eq!(res, Ok(PumpVoltage::V11));
                assert!(PumpVoltage::is_valid_manual_code(code));
                assert_eq!(PumpVoltage::V11.to_discrete_code(), 0);
            }
            2 => {
                assert_eq!(res, Ok(PumpVoltage::V7));
                assert!(PumpVoltage::is_valid_manual_code(code));
                assert_eq!(PumpVoltage::V7.to_discrete_code(), 2);
            }
            3 => {
                assert_eq!(res, Ok(PumpVoltage::V8));
                assert!(PumpVoltage::is_valid_manual_code(code));
                assert_eq!(PumpVoltage::V8.to_discrete_code(), 3);
            }
            4 => {
                assert_eq!(res, Ok(PumpVoltage::Off));
                assert!(PumpVoltage::is_valid_manual_code(code));
                assert_eq!(PumpVoltage::Off.to_discrete_code(), 4);
            }
            _ => {
                assert_eq!(res, Err(ValidationError::InvalidPumpVoltage(code)));
                assert!(!PumpVoltage::is_valid_manual_code(code));
            }
        }
    }
    assert_eq!(
        PumpVoltage::from_discrete_code(i32::MIN),
        Err(ValidationError::InvalidPumpVoltage(i32::MIN))
    );
    assert_eq!(
        PumpVoltage::from_discrete_code(i32::MAX),
        Err(ValidationError::InvalidPumpVoltage(i32::MAX))
    );

    // ProfileFanControl and UccProfile must strictly reject manual_pump_voltage = Some(1) (V12).
    let bad_fan = ProfileFanControl {
        manual_pump_voltage: Some(1),
        ..ProfileFanControl::default()
    };
    assert_eq!(
        bad_fan.validate(),
        Err(ValidationError::InvalidPumpVoltage(1))
    );

    let mut bad_profile = UccProfile::new("p1", "Forbidden V12");
    bad_profile.fan.manual_pump_voltage = Some(1);
    assert_eq!(
        bad_profile.validate(),
        Err(ValidationError::InvalidPumpVoltage(1))
    );

    // Speed index <-> PumpVoltage bijection on 0..=4 and rejection of 5..=255.
    for idx in 0u8..=4 {
        let voltage = pump_speed_to_voltage(idx).expect("valid pump speed index");
        assert_eq!(voltage_to_pump_speed(voltage), idx);
    }
    for idx in 5u8..=255 {
        assert_eq!(
            pump_speed_to_voltage(idx),
            Err(ValidationError::InvalidPumpSpeedLevel(idx))
        );
    }

    // 2. Randomized pump curve step-wise lookup, permutation invariance, and V12 clamping.
    let mut rng = SplitMix64::new(0x5055_4D50_5354_0003);
    for _ in 0..500 {
        let len = rng.gen_range_u32(1, 8) as usize;
        let mut temps = BTreeSet::new();
        while temps.len() < len {
            temps.insert(rng.gen_range_i32(-20, 120));
        }

        let sorted_table: Vec<PumpPoint> = temps
            .into_iter()
            .map(|t| PumpPoint::new(t, rng.gen_range_u8(0, 4)))
            .collect();

        // Shuffle a copy to verify get_pump_speed_for_temp is permutation-invariant.
        let mut shuffled_table = sorted_table.clone();
        for i in (1..shuffled_table.len()).rev() {
            let j = rng.gen_range_u32(0, i as u32) as usize;
            shuffled_table.swap(i, j);
        }

        let min_t = sorted_table[0].temp;
        let max_t = sorted_table[len - 1].temp;

        for t in [
            i32::MIN,
            min_t - 10,
            min_t - 1,
            min_t,
            (min_t + max_t) / 2,
            max_t,
            max_t + 10,
            i32::MAX,
        ] {
            let v_sorted = get_pump_speed_for_temp(&sorted_table, t);
            let v_shuffled = get_pump_speed_for_temp(&shuffled_table, t);
            assert_eq!(v_sorted, v_shuffled, "pump lookup must ignore input order");

            // Hardware safety invariant: get_pump_speed_for_temp NEVER returns forbidden V12.
            assert_ne!(
                v_sorted,
                PumpVoltage::V12,
                "legacy speed level 4 must be clamped to V11"
            );

            // Reference oracle: floor step lookup with speed clamped to 3.
            let expected = sorted_table
                .iter()
                .rev()
                .find(|p| t >= p.temp)
                .map_or(PumpVoltage::Off, |p| {
                    pump_speed_to_voltage(p.speed.min(3)).expect("clamped 0..=3 is valid")
                });
            assert_eq!(v_sorted, expected);
        }
    }

    // Empty pump table always returns Off.
    assert_eq!(get_pump_speed_for_temp(&[], 75), PumpVoltage::Off);
}

// ========================================================================
// 4. FanControlLogic: HW Limits, Hysteresis, Filtering & Critical Safety
// ========================================================================

#[test]
fn prop_cooling_controller_hw_limits_filtering_and_critical_override() {
    let mut rng = SplitMix64::new(0x434F_4F4C_494E_0004);

    // Asymmetric TemperatureFilter: rising responds faster than falling for equal delta.
    for _ in 0..200 {
        let base = rng.gen_range_i32(20, 70);
        let delta = rng.gen_range_i32(6, 30);

        let mut rising = TemperatureFilter::default();
        rising.add_value(base);
        rising.add_value(base + delta);
        let rise_step = rising.filtered_value() - base;

        let mut falling = TemperatureFilter::default();
        falling.add_value(base + delta);
        falling.add_value(base);
        let fall_step = (base + delta) - falling.filtered_value();

        assert!(
            rise_step > fall_step,
            "rising step ({rise_step}) must exceed falling step ({fall_step}) for delta={delta}"
        );
    }

    // Randomized FanControlLogic traces verifying bounds, hw min-speed, and thermal overrides.
    for _ in 0..300 {
        let mut fp = FanProfile::new("ctrl", "Controller");
        fp.table_cpu = vec![
            FanPoint::new(30, rng.gen_range_u8(0, 20)),
            FanPoint::new(60, rng.gen_range_u8(20, 60)),
            FanPoint::new(90, rng.gen_range_u8(60, 100)),
        ];
        fp.table_gpu = fp.table_cpu.clone();

        let logic_type = if rng.next_bool() {
            FanLogicType::Cpu
        } else {
            FanLogicType::Gpu
        };
        let mut ctrl = FanControlLogic::new(fp, logic_type);

        // Pass arbitrary i32 hardware limit to verify internal 0..=100 clamping.
        let raw_min_hw = rng.gen_range_i32(-20, 120);
        let clamped_min_hw = raw_min_hw.clamp(0, 100) as u8;
        let fans_off_available = rng.next_bool();
        ctrl.set_fans_min_speed_hw_limit(raw_min_hw);
        ctrl.set_fans_off_available(fans_off_available);

        for _step in 0..25 {
            let sample = rng.gen_range_i32(15, 105);
            let spd = ctrl
                .report_temperature_checked(sample)
                .expect("non-empty curve returns Some");
            assert!(spd <= 100);
            assert_eq!(ctrl.speed_percent(), spd);

            if !fans_off_available {
                assert!(
                    spd >= clamped_min_hw,
                    "when fans_off_available=false, speed {spd} must be >= {clamped_min_hw}"
                );
            }

            let filtered = ctrl.filtered_temperature();
            if filtered >= FanControlLogic::OVERHEAT_TEMPERATURE_C {
                assert_eq!(spd, 100, "overheat (>= 90C) must force 100% fan speed");
            } else if filtered >= FanControlLogic::CRITICAL_TEMPERATURE_C {
                assert!(
                    spd >= 80,
                    "critical temp (>= 85C) must force at least 80% fan speed, got {spd}"
                );
            }
        }
    }

    // Empty curve returns None from report_temperature_checked and 0 from report_temperature.
    let mut empty_ctrl = FanControlLogic::new(FanProfile::new("empty", "Empty"), FanLogicType::Cpu);
    assert_eq!(empty_ctrl.report_temperature_checked(75), None);
    assert_eq!(empty_ctrl.report_temperature(75), 0);

    // Regression: falling filtered temperature near i32::MAX must not overflow hysteresis.
    let mut extreme_ctrl = FanControlLogic::new(FanCurve {
        points: vec![FanPoint::new(20, 20), FanPoint::new(90, 100)],
    });
    assert_eq!(extreme_ctrl.report_temperature_checked(i32::MAX), Some(100));
    assert_eq!(
        extreme_ctrl.report_temperature_checked(i32::MAX - 1),
        Some(100)
    );
    assert_eq!(
        extreme_ctrl.report_temperature_checked(i32::MAX - 4),
        Some(100)
    );

    // TemporaryFanCurves overrides only the targeted fan index and non-empty tables.
    let mut base = FanProfile::new("base", "Base");
    base.table_cpu = vec![FanPoint::new(40, 20), FanPoint::new(80, 80)];
    base.table_gpu = vec![FanPoint::new(40, 25), FanPoint::new(80, 85)];
    let temp = TemporaryFanCurves {
        cpu: vec![FanPoint::new(40, 50), FanPoint::new(80, 100)],
        gpu: vec![FanPoint::new(40, 60), FanPoint::new(80, 95)],
        water_cooler_fan: vec![],
        pump: vec![PumpPoint::new(50, 2)],
    };
    let merged_cpu = temp.apply_to_profile(&base, 0);
    assert_eq!(merged_cpu.table_cpu, temp.cpu);
    assert_eq!(merged_cpu.table_gpu, base.table_gpu);
    assert_eq!(merged_cpu.table_pump, temp.pump);

    let merged_gpu = temp.apply_to_profile(&base, 1);
    assert_eq!(merged_gpu.table_cpu, base.table_cpu);
    assert_eq!(merged_gpu.table_gpu, temp.gpu);
}
