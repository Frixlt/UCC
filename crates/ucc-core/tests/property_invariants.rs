//! Property-based and edge-case tests of domain model invariants.
//!
//! Covers:
//! - Boundary values for TDP, CPU frequencies, and core selection.
//! - Monotonicity and fuzzed perturbations of arbitrary fan curves.
//! - Discrete voltage codes for LCT water cooler pump (0, 2, 3, 4 accepted, 1 rejected).
//! - Step-wise pump lookups and linear fan interpolation.
//!
//! Addresses M2.6, M2.7, and M2.8.

use std::collections::BTreeSet;
use ucc_core::{
    get_pump_speed_for_temp, CpuLimits, FanCurve, FanPoint, PumpPoint, PumpVoltage,
    ValidationError,
};

/// Deterministic, zero-dependency linear congruential pseudo-random generator
/// for repeatable fuzz testing across CI environments.
struct FuzzRng {
    state: u64,
}

impl FuzzRng {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u32(&mut self) -> u32 {
        self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.state >> 32) as u32
    }

    fn gen_range_u32(&mut self, low: u32, high: u32) -> u32 {
        assert!(low <= high);
        if low == high {
            return low;
        }
        low + (self.next_u32() % (high - low + 1))
    }

    fn gen_range_i32(&mut self, low: i32, high: i32) -> i32 {
        assert!(low <= high);
        let span = (high - low) as u32;
        low + (self.next_u32() % (span + 1)) as i32
    }

    fn gen_range_u8(&mut self, low: u8, high: u8) -> u8 {
        self.gen_range_u32(low as u32, high as u32) as u8
    }
}

// ========================================================================
// 1. CPU Frequency Boundary and Range Invariant Tests
// ========================================================================

#[test]
fn test_cpu_frequency_boundary_values() {
    let mut limits = CpuLimits::new();

    // Exact min boundary: 100_000 kHz
    limits.min_freq_khz = Some(100_000);
    limits.max_freq_khz = Some(100_000);
    assert!(limits.validate().is_ok(), "100_000 kHz is valid exact lower bound");

    // Off-by-one below minimum: 99_999 kHz
    limits.min_freq_khz = Some(99_999);
    assert_eq!(
        limits.validate(),
        Err(ValidationError::InvalidFrequency(99_999)),
        "99_999 kHz is below minimum 100_000 kHz"
    );

    // Exact max boundary: 10_000_000 kHz (10 GHz)
    limits.min_freq_khz = Some(100_000);
    limits.max_freq_khz = Some(10_000_000);
    assert!(limits.validate().is_ok(), "10_000_000 kHz is valid exact upper bound");

    // Off-by-one above maximum: 10_000_001 kHz
    limits.max_freq_khz = Some(10_000_001);
    assert_eq!(
        limits.validate(),
        Err(ValidationError::InvalidFrequency(10_000_001)),
        "10_000_001 kHz exceeds maximum 10_000_000 kHz"
    );

    // Extreme zero frequency
    limits.min_freq_khz = Some(0);
    limits.max_freq_khz = Some(3_000_000);
    assert_eq!(
        limits.validate(),
        Err(ValidationError::InvalidFrequency(0))
    );

    // Extreme u32::MAX
    limits.min_freq_khz = Some(100_000);
    limits.max_freq_khz = Some(u32::MAX);
    assert_eq!(
        limits.validate(),
        Err(ValidationError::InvalidFrequency(u32::MAX))
    );
}

#[test]
fn test_cpu_frequency_range_invariants() {
    let mut limits = CpuLimits::new();

    // min < max: valid standard range
    limits.min_freq_khz = Some(800_000);
    limits.max_freq_khz = Some(4_800_000);
    assert!(limits.validate().is_ok());

    // min == max: valid fixed clock
    limits.min_freq_khz = Some(2_400_000);
    limits.max_freq_khz = Some(2_400_000);
    assert!(limits.validate().is_ok());

    // min > max: strictly invalid range
    limits.min_freq_khz = Some(3_500_000);
    limits.max_freq_khz = Some(2_000_000);
    assert_eq!(
        limits.validate(),
        Err(ValidationError::InvalidFrequencyRange {
            min: 3_500_000,
            max: 2_000_000
        })
    );

    // Only min specified: valid
    limits.min_freq_khz = Some(1_200_000);
    limits.max_freq_khz = None;
    assert!(limits.validate().is_ok());

    // Only max specified: valid
    limits.min_freq_khz = None;
    limits.max_freq_khz = Some(4_200_000);
    assert!(limits.validate().is_ok());

    // Neither specified: valid
    limits.min_freq_khz = None;
    limits.max_freq_khz = None;
    assert!(limits.validate().is_ok());
}

// ========================================================================
// 2. TDP Boundary and Monotonicity Tests
// ========================================================================

#[test]
fn test_tdp_boundary_values() {
    let mut limits = CpuLimits::new();

    // 0 W is strictly invalid (minimum 1W)
    limits.tdp_values_w = vec![0];
    assert_eq!(limits.validate(), Err(ValidationError::InvalidTdp(0)));

    // Exact lower bound: 1 W
    limits.tdp_values_w = vec![1];
    assert!(limits.validate().is_ok());

    // Exact upper bound: 500 W
    limits.tdp_values_w = vec![500];
    assert!(limits.validate().is_ok());

    // Off-by-one above upper bound: 501 W
    limits.tdp_values_w = vec![501];
    assert_eq!(limits.validate(), Err(ValidationError::InvalidTdp(501)));

    // Extreme u32::MAX
    limits.tdp_values_w = vec![u32::MAX];
    assert_eq!(limits.validate(), Err(ValidationError::InvalidTdp(u32::MAX)));
}

#[test]
fn test_tdp_monotonicity_invariants() {
    let mut limits = CpuLimits::new();

    // Empty TDP values: valid (unconfigured)
    limits.tdp_values_w = vec![];
    assert!(limits.validate().is_ok());

    // Single value: valid
    limits.tdp_values_w = vec![45];
    assert!(limits.validate().is_ok());

    // Strictly increasing: valid [PL1, PL2, PL3]
    limits.tdp_values_w = vec![35, 65, 95];
    assert!(limits.validate().is_ok());

    // Non-decreasing with equal stages: valid [PL1, PL2, PL3]
    limits.tdp_values_w = vec![65, 65, 65];
    assert!(limits.validate().is_ok());

    // Non-monotonic at index 1: PL1 > PL2
    limits.tdp_values_w = vec![95, 65, 110];
    assert_eq!(
        limits.validate(),
        Err(ValidationError::NonMonotonicTdp {
            index: 1,
            prev: 95,
            curr: 65
        })
    );

    // Non-monotonic at index 2: PL2 > PL3
    limits.tdp_values_w = vec![45, 95, 80];
    assert_eq!(
        limits.validate(),
        Err(ValidationError::NonMonotonicTdp {
            index: 2,
            prev: 95,
            curr: 80
        })
    );

    // Non-monotonic at index 3: PL3 > PL4
    limits.tdp_values_w = vec![25, 45, 90, 85];
    assert_eq!(
        limits.validate(),
        Err(ValidationError::NonMonotonicTdp {
            index: 3,
            prev: 90,
            curr: 85
        })
    );
}

#[test]
fn test_tdp_property_fuzzing() {
    let mut rng = FuzzRng::new(0xDEAD_BEEF_CAFE_0001);

    for _ in 0..1000 {
        let count = rng.gen_range_u32(1, 8) as usize;
        let mut vals = Vec::with_capacity(count);
        for _ in 0..count {
            vals.push(rng.gen_range_u32(1, 500));
        }

        // Sort to ensure monotonic non-decreasing order
        vals.sort();

        let mut limits = CpuLimits::new();
        limits.tdp_values_w = vals.clone();
        assert!(
            limits.validate().is_ok(),
            "Sorted valid TDP sequence must pass: {:?}",
            vals
        );

        // If count >= 2, inject a non-monotonic perturbation and assert rejection
        if count >= 2 {
            let idx = rng.gen_range_u32(1, (count - 1) as u32) as usize;
            let mut bad_vals = vals.clone();
            if bad_vals[idx - 1] > 1 {
                bad_vals[idx] = bad_vals[idx - 1] - 1;
                let mut bad_limits = CpuLimits::new();
                bad_limits.tdp_values_w = bad_vals.clone();
                let err = bad_limits.validate();
                assert!(
                    matches!(err, Err(ValidationError::NonMonotonicTdp { .. })),
                    "Perturbed non-monotonic TDP {:?} must be rejected, got {:?}",
                    bad_vals,
                    err
                );
            }
        }
    }
}

// ========================================================================
// 3. CPU Core Selection Invariants
// ========================================================================

#[test]
fn test_core_selection_mandatory_core_zero() {
    let mut limits = CpuLimits::new();

    // Valid: mandatory core 0 present
    limits.enabled_cores = Some(vec![0]);
    assert!(limits.validate().is_ok());

    limits.enabled_cores = Some(vec![0, 1, 2, 3]);
    assert!(limits.validate().is_ok());

    // Invalid: missing core 0
    limits.enabled_cores = Some(vec![1, 2, 3]);
    assert_eq!(
        limits.validate(),
        Err(ValidationError::InvalidCoreSelection(
            "core 0 is mandatory and cannot be disabled".to_string()
        ))
    );

    // Invalid: empty enabled_cores
    limits.enabled_cores = Some(vec![]);
    assert_eq!(
        limits.validate(),
        Err(ValidationError::InvalidCoreSelection(
            "enabled_cores must not be empty".to_string()
        ))
    );
}

#[test]
fn test_core_selection_sparse_and_duplicates() {
    let mut limits = CpuLimits::new();

    // Valid: sparse core IDs (e.g. only Performance cores or alternate cores)
    limits.enabled_cores = Some(vec![0, 2, 4, 6, 8, 10]);
    assert!(
        limits.validate().is_ok(),
        "Sparse core IDs must be accepted without altering identity"
    );

    // Invalid: duplicate core IDs
    limits.enabled_cores = Some(vec![0, 1, 2, 2, 3]);
    assert_eq!(
        limits.validate(),
        Err(ValidationError::InvalidCoreSelection(
            "duplicate core ID 2".to_string()
        ))
    );

    // Invalid: core ID exceeds 1_048_575
    limits.enabled_cores = Some(vec![0, 1_048_576]);
    assert_eq!(
        limits.validate(),
        Err(ValidationError::InvalidCoreSelection(
            "core ID 1048576 out of range".to_string()
        ))
    );
}

#[test]
fn test_online_cores_invariants() {
    let mut limits = CpuLimits::new();

    // 0 online cores is invalid
    limits.online_cores = Some(0);
    assert_eq!(
        limits.validate(),
        Err(ValidationError::InvalidCoreSelection(
            "online_cores must be > 0".to_string()
        ))
    );

    // 1..=4096 online cores are valid
    limits.online_cores = Some(1);
    assert!(limits.validate().is_ok());

    limits.online_cores = Some(16);
    assert!(limits.validate().is_ok());

    limits.online_cores = Some(128);
    assert!(limits.validate().is_ok());
}

// ========================================================================
// 4. Monotonicity of Fan Curves & Random Fuzzing
// ========================================================================

#[test]
fn test_fan_curve_empty_and_single_point() {
    // Empty fan curve rejected
    let empty_curve = FanCurve::new(vec![]);
    assert_eq!(empty_curve, Err(ValidationError::EmptyFanCurve));

    // Single point fan curve accepted
    let single_point = FanCurve::new(vec![FanPoint::new(50, 40)]);
    assert!(single_point.is_ok());
}

#[test]
fn test_fan_curve_point_bounds() {
    // Valid point at boundary
    assert!(FanPoint::new(0, 0).validate().is_ok());
    assert!(FanPoint::new(100, 100).validate().is_ok());
    assert!(FanPoint::new(-20, 50).validate().is_ok());
    assert!(FanPoint::new(150, 50).validate().is_ok());

    // Invalid fan speed > 100%
    assert_eq!(
        FanPoint::new(50, 101).validate(),
        Err(ValidationError::InvalidFanSpeed { speed: 101 })
    );
    assert_eq!(
        FanPoint::new(50, 255).validate(),
        Err(ValidationError::InvalidFanSpeed { speed: 255 })
    );

    // Invalid temperature < -20°C
    assert_eq!(
        FanPoint::new(-21, 50).validate(),
        Err(ValidationError::InvalidTemperature { temp: -21 })
    );

    // Invalid temperature > 150°C
    assert_eq!(
        FanPoint::new(151, 50).validate(),
        Err(ValidationError::InvalidTemperature { temp: 151 })
    );
}

#[test]
fn test_fan_curve_temperature_monotonicity() {
    // Strictly increasing temperatures: valid
    let valid = FanCurve::new(vec![
        FanPoint::new(30, 20),
        FanPoint::new(45, 35),
        FanPoint::new(60, 50),
        FanPoint::new(80, 80),
    ]);
    assert!(valid.is_ok());

    // Duplicate temperature at index 1: rejected
    let dup_temp = FanCurve::new(vec![
        FanPoint::new(30, 20),
        FanPoint::new(30, 35),
    ]);
    assert_eq!(
        dup_temp,
        Err(ValidationError::NonMonotonicTemperature {
            index: 1,
            prev: 30,
            curr: 30
        })
    );

    // Decreasing temperature at index 2: rejected
    let dec_temp = FanCurve::new(vec![
        FanPoint::new(30, 20),
        FanPoint::new(50, 35),
        FanPoint::new(45, 50),
    ]);
    assert_eq!(
        dec_temp,
        Err(ValidationError::NonMonotonicTemperature {
            index: 2,
            prev: 50,
            curr: 45
        })
    );
}

#[test]
fn test_fan_curve_speed_monotonicity() {
    // Equal speeds (flat plateau): valid
    let flat_plateau = FanCurve::new(vec![
        FanPoint::new(30, 25),
        FanPoint::new(45, 25),
        FanPoint::new(60, 50),
    ]);
    assert!(flat_plateau.is_ok(), "Flat fan speed plateau must be accepted");

    // Strictly increasing speeds: valid
    let increasing = FanCurve::new(vec![
        FanPoint::new(30, 10),
        FanPoint::new(50, 30),
        FanPoint::new(70, 70),
        FanPoint::new(90, 100),
    ]);
    assert!(increasing.is_ok());

    // Decreasing speed at index 1: rejected
    let dec_speed = FanCurve::new(vec![
        FanPoint::new(30, 40),
        FanPoint::new(50, 30),
    ]);
    assert_eq!(
        dec_speed,
        Err(ValidationError::NonMonotonicSpeed {
            index: 1,
            prev: 40,
            curr: 30
        })
    );

    // Decreasing speed at index 2: rejected
    let dec_middle = FanCurve::new(vec![
        FanPoint::new(30, 20),
        FanPoint::new(50, 60),
        FanPoint::new(70, 55),
    ]);
    assert_eq!(
        dec_middle,
        Err(ValidationError::NonMonotonicSpeed {
            index: 2,
            prev: 60,
            curr: 55
        })
    );
}

#[test]
fn test_fan_curve_fuzzing_and_invariants() {
    let mut rng = FuzzRng::new(0xFEED_FACE_CAFE_0002);

    for _ in 0..2000 {
        let point_count = rng.gen_range_u32(2, 12) as usize;

        // Generate strictly increasing temperatures in range [0, 110]
        let mut temps = BTreeSet::new();
        while temps.len() < point_count {
            temps.insert(rng.gen_range_i32(0, 110));
        }
        let temp_vec: Vec<i32> = temps.into_iter().collect();

        // Generate non-decreasing speeds in range [0, 100]
        let mut speeds = Vec::with_capacity(point_count);
        for _ in 0..point_count {
            speeds.push(rng.gen_range_u8(0, 100));
        }
        speeds.sort();

        let points: Vec<FanPoint> = temp_vec
            .iter()
            .zip(speeds.iter())
            .map(|(&t, &s)| FanPoint::new(t, s))
            .collect();

        // Monotonic curve must be strictly accepted
        let curve = FanCurve::new(points.clone()).expect("valid generated curve must pass");

        // Verify interpolation invariants:
        // 1. Below lowest temp -> clamps to first point speed
        assert_eq!(
            curve.get_speed_for_temp(temp_vec[0] - 15),
            speeds[0] as i32
        );

        // 2. Above highest temp -> clamps to last point speed
        assert_eq!(
            curve.get_speed_for_temp(temp_vec[point_count - 1] + 15),
            speeds[point_count - 1] as i32
        );

        // 3. Exact temperature matches exact speed
        for (i, &t) in temp_vec.iter().enumerate() {
            assert_eq!(curve.get_speed_for_temp(t), speeds[i] as i32);
        }

        // 4. Query monotonicity: speed at t2 >= speed at t1 when t2 >= t1
        let mut prev_speed = -1;
        for t in (temp_vec[0] - 5)..=(temp_vec[point_count - 1] + 5) {
            let speed = curve.get_speed_for_temp(t);
            assert!(
                speed >= prev_speed,
                "Query speed must be monotonic: t={}, speed={}, prev_speed={}",
                t,
                speed,
                prev_speed
            );
            prev_speed = speed;
        }

        // 5. Inject perturbation: decrease speed at random index and assert rejection
        let perturb_idx = rng.gen_range_u32(1, (point_count - 1) as u32) as usize;
        if speeds[perturb_idx - 1] > 0 {
            let mut bad_points = points.clone();
            bad_points[perturb_idx].speed = speeds[perturb_idx - 1] - 1;
            let res = FanCurve::new(bad_points);
            assert!(
                matches!(res, Err(ValidationError::NonMonotonicSpeed { .. })),
                "Injected non-monotonic speed must be rejected"
            );
        }

        // 6. Inject perturbation: duplicate temperature and assert rejection
        let mut dup_temp_points = points.clone();
        dup_temp_points[perturb_idx].temp = dup_temp_points[perturb_idx - 1].temp;
        let dup_res = FanCurve::new(dup_temp_points);
        assert!(
            matches!(dup_res, Err(ValidationError::NonMonotonicTemperature { .. })),
            "Injected duplicate temperature must be rejected"
        );
    }
}

#[test]
fn test_fan_curve_linear_interpolation_accuracy() {
    let curve = FanCurve::new(vec![
        FanPoint::new(30, 0),
        FanPoint::new(40, 10),
        FanPoint::new(50, 25),
        FanPoint::new(60, 40),
        FanPoint::new(70, 55),
        FanPoint::new(80, 75),
        FanPoint::new(90, 90),
        FanPoint::new(100, 100),
    ])
    .expect("standard balanced curve");

    // Exact midpoints:
    // Between (30, 0) and (40, 10): at 35 -> 5
    assert_eq!(curve.get_speed_for_temp(35), 5);

    // Between (40, 10) and (50, 25): diff=15 over 10 deg -> 1.5 per deg
    // at 45 -> 10 + 5 * 1.5 = 17.5 -> round to 18
    assert_eq!(curve.get_speed_for_temp(45), 18);

    // Between (70, 55) and (80, 75): diff=20 over 10 deg -> 2.0 per deg
    // at 75 -> 55 + 5 * 2 = 65
    assert_eq!(curve.get_speed_for_temp(75), 65);

    // Between (80, 75) and (90, 90): diff=15 over 10 deg -> 1.5 per deg
    // at 85 -> 75 + 5 * 1.5 = 82.5 -> round to 83
    assert_eq!(curve.get_speed_for_temp(85), 83);
}

// ========================================================================
// 5. Discrete Pump Voltage Codes & Step-Wise Lookup
// ========================================================================

#[test]
fn test_discrete_pump_voltage_codes_parity() {
    // Valid codes according to UCC specification:
    // - 0: 11V (PumpVoltage::V11)
    // - 2: 7V  (PumpVoltage::V7)
    // - 3: 8V  (PumpVoltage::V8)
    // - 4: Off (PumpVoltage::Off)
    assert_eq!(PumpVoltage::from_discrete_code(0), Ok(PumpVoltage::V11));
    assert_eq!(PumpVoltage::from_discrete_code(2), Ok(PumpVoltage::V7));
    assert_eq!(PumpVoltage::from_discrete_code(3), Ok(PumpVoltage::V8));
    assert_eq!(PumpVoltage::from_discrete_code(4), Ok(PumpVoltage::Off));

    assert!(PumpVoltage::is_valid_manual_code(0));
    assert!(PumpVoltage::is_valid_manual_code(2));
    assert!(PumpVoltage::is_valid_manual_code(3));
    assert!(PumpVoltage::is_valid_manual_code(4));

    // Code 1 is strictly forbidden / rejected on production hardware
    assert_eq!(
        PumpVoltage::from_discrete_code(1),
        Err(ValidationError::InvalidPumpVoltage(1))
    );
    assert!(!PumpVoltage::is_valid_manual_code(1));

    // Negative codes rejected
    let negative_codes = [-1, -2, -5, -12, i32::MIN];
    for &code in &negative_codes {
        assert_eq!(
            PumpVoltage::from_discrete_code(code),
            Err(ValidationError::InvalidPumpVoltage(code)),
            "Negative code {} must be rejected",
            code
        );
        assert!(!PumpVoltage::is_valid_manual_code(code));
    }

    // Out-of-range positive codes rejected
    let out_of_range = [5, 6, 7, 8, 9, 10, 11, 12, 100, 255, 1000, i32::MAX];
    for &code in &out_of_range {
        assert_eq!(
            PumpVoltage::from_discrete_code(code),
            Err(ValidationError::InvalidPumpVoltage(code)),
            "Code {} must be rejected",
            code
        );
        assert!(!PumpVoltage::is_valid_manual_code(code));
    }
}

#[test]
fn test_pump_point_speed_level_invariants() {
    // Valid speed levels: 0=Off, 1=V7, 2=V8, 3=V11, 4=V12
    for speed in 0..=4 {
        assert!(
            PumpPoint::new(45, speed).validate().is_ok(),
            "Speed level {} must be accepted",
            speed
        );
    }

    // Invalid speed levels > 4
    for speed in 5..=10 {
        assert_eq!(
            PumpPoint::new(45, speed).validate(),
            Err(ValidationError::InvalidPumpSpeedLevel(speed)),
            "Speed level {} must be rejected",
            speed
        );
    }
}

#[test]
fn test_pump_step_wise_lookup_property() {
    // Standard pump threshold table:
    // Temp < 50  => speed 0 (Off)
    // 50 <= Temp < 65 => speed 1 (V7)
    // 65 <= Temp < 80 => speed 2 (V8)
    // Temp >= 80 => speed 3 (V11)
    let table = vec![
        PumpPoint::new(50, 1),
        PumpPoint::new(65, 2),
        PumpPoint::new(80, 3),
    ];

    // Below first threshold: Off
    assert_eq!(get_pump_speed_for_temp(&table, 20), PumpVoltage::Off);
    assert_eq!(get_pump_speed_for_temp(&table, 49), PumpVoltage::Off);

    // At exactly first threshold: V7
    assert_eq!(get_pump_speed_for_temp(&table, 50), PumpVoltage::V7);
    assert_eq!(get_pump_speed_for_temp(&table, 64), PumpVoltage::V7);

    // At second threshold: V8
    assert_eq!(get_pump_speed_for_temp(&table, 65), PumpVoltage::V8);
    assert_eq!(get_pump_speed_for_temp(&table, 79), PumpVoltage::V8);

    // At and above highest threshold: V11
    assert_eq!(get_pump_speed_for_temp(&table, 80), PumpVoltage::V11);
    assert_eq!(get_pump_speed_for_temp(&table, 95), PumpVoltage::V11);
    assert_eq!(get_pump_speed_for_temp(&table, 120), PumpVoltage::V11);
}
