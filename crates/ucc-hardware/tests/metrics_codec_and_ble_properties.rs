// SPDX-License-Identifier: GPL-3.0-or-later
//! Deterministic property-based tests for the binary metrics history codec
//! (`telemetry::history`) and LCT BLE cooler packet codec + `CoolerCommandQueue`.

use std::time::{Duration, Instant};
use ucc_core::PumpVoltage as CorePumpVoltage;
use ucc_hardware::cooler::{PACKET_FOOTER, PACKET_HEADER, PACKET_LEN};
use ucc_hardware::{
    decode_history_binary, BleWriteLimiter, CoolerCommand, CoolerCommandQueue, CoolerController,
    CoolerError, CoolerMode, CoolerPacketEncoder, CoolerState, HistoryCodecError, MetricDataPoint,
    MetricId, MetricsHistoryStore, MockBleTransport, PumpVoltage, RgbState, BLE_WRITE_GAP,
    CMD_FAN, CMD_PUMP, CMD_RESET, CMD_RGB,
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

    fn gen_u8(&mut self) -> u8 {
        (self.next_u64() & 0xff) as u8
    }

    fn gen_u8_range(&mut self, low: u8, high_inclusive: u8) -> u8 {
        debug_assert!(low <= high_inclusive);
        let span = u64::from(high_inclusive - low) + 1;
        low + (self.next_u64() % span) as u8
    }

    fn gen_i32_range(&mut self, low: i32, high_inclusive: i32) -> i32 {
        debug_assert!(low <= high_inclusive);
        let span = (i64::from(high_inclusive) - i64::from(low) + 1) as u64;
        (i64::from(low) + (self.next_u64() % span) as i64) as i32
    }

    fn gen_i64_range(&mut self, low: i64, high_inclusive: i64) -> i64 {
        debug_assert!(low <= high_inclusive);
        let span = (i128::from(high_inclusive) - i128::from(low) + 1) as u128;
        let offset = (u128::from(self.next_u64()) % span) as i128;
        (i128::from(low) + offset) as i64
    }

    fn gen_finite_f64(&mut self) -> f64 {
        loop {
            let bits = self.next_u64();
            let val = f64::from_bits(bits);
            if val.is_finite() {
                return val;
            }
        }
    }
}

const ALL_RGB_STATES: [RgbState; 5] = [
    RgbState::Static,
    RgbState::Breathe,
    RgbState::Colorful,
    RgbState::BreatheColor,
    RgbState::Temperature,
];

const ALLOWED_PUMP_VOLTAGES: [PumpVoltage; 4] = [
    PumpVoltage::V11,
    PumpVoltage::V7,
    PumpVoltage::V8,
    PumpVoltage::Off,
];

fn rgb_state_from_u8(byte: u8) -> Option<RgbState> {
    match byte {
        0x00 => Some(RgbState::Static),
        0x01 => Some(RgbState::Breathe),
        0x02 => Some(RgbState::Colorful),
        0x03 => Some(RgbState::BreatheColor),
        0x04 => Some(RgbState::Temperature),
        _ => None,
    }
}

/// Pure reference decoder for 8-byte LCT BLE packets verifying round-trip symmetry.
fn decode_cooler_packet(raw: &[u8]) -> Result<CoolerCommand, CoolerError> {
    CoolerPacketEncoder::validate_packet(raw)?;
    let cmd = raw[1];
    let enable = raw[2];
    match (cmd, enable) {
        (CMD_RESET, 0x00) if raw[3] == 0x01 => Ok(CoolerCommand::Reset),
        (CMD_FAN, 0x00) => Ok(CoolerCommand::TurnOffFan),
        (CMD_FAN, 0x01) => {
            if raw[3] > 100 {
                return Err(CoolerError::InvalidFanSpeed(raw[3]));
            }
            Ok(CoolerCommand::FanSpeed(raw[3]))
        }
        (CMD_PUMP, 0x00) => Ok(CoolerCommand::TurnOffPump),
        (CMD_PUMP, 0x01) => {
            let voltage = PumpVoltage::try_from(raw[4])?;
            if voltage == PumpVoltage::V12 {
                return Err(CoolerError::V12Forbidden);
            }
            Ok(CoolerCommand::PumpVoltage(voltage))
        }
        (CMD_RGB, 0x00) => Ok(CoolerCommand::TurnOffLed),
        (CMD_RGB, 0x01) => {
            let state = rgb_state_from_u8(raw[6])
                .ok_or(CoolerError::InvalidFraming(raw[0], raw[7]))?;
            Ok(CoolerCommand::Rgb {
                red: raw[3],
                green: raw[4],
                blue: raw[5],
                state,
            })
        }
        _ => Err(CoolerError::InvalidFraming(raw[0], raw[7])),
    }
}

fn gen_valid_cooler_command(rng: &mut SplitMix64) -> CoolerCommand {
    match rng.gen_usize_range(0, 6) {
        0 => CoolerCommand::FanSpeed(rng.gen_u8_range(0, 100)),
        1 => {
            let idx = rng.gen_usize_range(0, ALLOWED_PUMP_VOLTAGES.len() - 1);
            CoolerCommand::PumpVoltage(ALLOWED_PUMP_VOLTAGES[idx])
        }
        2 => {
            let state = ALL_RGB_STATES[rng.gen_usize_range(0, ALL_RGB_STATES.len() - 1)];
            CoolerCommand::Rgb {
                red: rng.gen_u8(),
                green: rng.gen_u8(),
                blue: rng.gen_u8(),
                state,
            }
        }
        3 => CoolerCommand::TurnOffFan,
        4 => CoolerCommand::TurnOffPump,
        5 => CoolerCommand::TurnOffLed,
        _ => CoolerCommand::Reset,
    }
}

// ============================================================================
// Metrics History Binary Codec Properties
// ============================================================================

#[test]
fn prop_history_binary_roundtrip_and_since_filtering() {
    let mut rng = SplitMix64::new(0xB10A_0001_2026_0001);

    // Empty store encodes to empty slice and decodes to empty Vec
    let empty_store = MetricsHistoryStore::new();
    let empty_bin = empty_store.query_since_binary(i64::MIN);
    assert!(empty_bin.is_empty());
    assert_eq!(decode_history_binary(&empty_bin), Ok(Vec::new()));

    for case in 0..200 {
        let store = MetricsHistoryStore::new();
        let horizon_s = rng.gen_i32_range(60, 7200);
        store.set_horizon_seconds(horizon_s);
        let horizon_ms = i64::from(horizon_s) * 1000;

        let base_ts = rng.gen_i64_range(10_000_000, 1_700_000_000_000);
        let mut expected: Vec<Vec<MetricDataPoint>> = vec![Vec::new(); MetricId::COUNT];

        for id in MetricId::ALL {
            if !rng.gen_bool() {
                continue;
            }
            let count = rng.gen_usize_range(0, 35);
            let series = &mut expected[id as usize];
            for _ in 0..count {
                // Generate timestamps around base_ts with out-of-order jitter within horizon
                let ts = base_ts + rng.gen_i64_range(-horizon_ms / 2, horizon_ms / 2);
                let val = if rng.gen_usize_range(0, 15) == 0 {
                    // Occasionally push non-finite values; store must drop them
                    match rng.gen_usize_range(0, 2) {
                        0 => f64::NAN,
                        1 => f64::INFINITY,
                        _ => f64::NEG_INFINITY,
                    }
                } else {
                    rng.gen_finite_f64()
                };

                store.push(id, ts, val);
                if val.is_finite() {
                    let pt = MetricDataPoint {
                        timestamp_ms: ts,
                        value: val,
                    };
                    let pos = series.partition_point(|existing| existing.timestamp_ms <= ts);
                    series.insert(pos, pt);
                    let newest = series.last().unwrap().timestamp_ms;
                    let cutoff = newest.saturating_sub(horizon_ms);
                    series.retain(|p| p.timestamp_ms >= cutoff);
                }
            }
        }

        // Full round-trip at since = i64::MIN
        let bin_all = store.query_since_binary(i64::MIN);
        let decoded_all = decode_history_binary(&bin_all).expect("valid store binary must decode");

        let expected_non_empty: Vec<(MetricId, &Vec<MetricDataPoint>)> = MetricId::ALL
            .iter()
            .filter_map(|&id| {
                let s = &expected[id as usize];
                if s.is_empty() {
                    None
                } else {
                    Some((id, s))
                }
            })
            .collect();

        assert_eq!(
            decoded_all.len(),
            expected_non_empty.len(),
            "case {case}: series count mismatch"
        );
        for ((dec_id, dec_pts), (exp_id, exp_pts)) in
            decoded_all.iter().zip(expected_non_empty.iter())
        {
            assert_eq!(dec_id, exp_id, "case {case}: canonical MetricId order mismatch");
            assert_eq!(dec_pts.len(), exp_pts.len(), "case {case}: point count mismatch");
            for (d, e) in dec_pts.iter().zip(exp_pts.iter()) {
                assert_eq!(d.timestamp_ms, e.timestamp_ms);
                assert_eq!(
                    d.value.to_bits(),
                    e.value.to_bits(),
                    "case {case}: bit-exact f64 round-trip mismatch"
                );
            }
            for win in dec_pts.windows(2) {
                assert!(
                    win[0].timestamp_ms <= win[1].timestamp_ms,
                    "case {case}: decoded series not sorted"
                );
            }
        }

        // Random `since` filtering queries (including boundary values)
        let since_probes = [
            i64::MIN,
            base_ts - horizon_ms,
            base_ts + rng.gen_i64_range(-horizon_ms / 2, horizon_ms / 2),
            base_ts + horizon_ms,
            i64::MAX,
        ];
        for since in since_probes {
            let bin = store.query_since_binary(since);
            let dec = decode_history_binary(&bin).expect("filtered binary must decode");

            let exp_filtered: Vec<(MetricId, Vec<MetricDataPoint>)> = MetricId::ALL
                .iter()
                .filter_map(|&id| {
                    let pts: Vec<MetricDataPoint> = expected[id as usize]
                        .iter()
                        .copied()
                        .filter(|p| p.timestamp_ms >= since)
                        .collect();
                    if pts.is_empty() {
                        None
                    } else {
                        Some((id, pts))
                    }
                })
                .collect();

            assert_eq!(dec.len(), exp_filtered.len(), "case {case} since={since}");
            for ((dec_id, dec_pts), (exp_id, exp_pts)) in dec.iter().zip(exp_filtered.iter()) {
                assert_eq!(dec_id, exp_id);
                assert_eq!(dec_pts.len(), exp_pts.len());
                for (d, e) in dec_pts.iter().zip(exp_pts.iter()) {
                    assert_eq!(d.timestamp_ms, e.timestamp_ms);
                    assert_eq!(d.value.to_bits(), e.value.to_bits());
                }
            }
        }
    }
}

#[test]
fn prop_history_boundary_timestamps_wall_clock_jumps_and_horizon_clamping() {
    let mut rng = SplitMix64::new(0xB10A_0002_2026_0002);

    // 1. Horizon clamping across 300 random i32 inputs + explicit extremes
    let edge_horizons = [
        i32::MIN,
        -3600,
        -1,
        0,
        59,
        60,
        61,
        1800,
        7199,
        7200,
        7201,
        i32::MAX,
    ];
    let clamp_store = MetricsHistoryStore::new();
    for h in edge_horizons {
        clamp_store.set_horizon_seconds(h);
        assert_eq!(
            clamp_store.horizon_seconds(),
            h.clamp(
                MetricsHistoryStore::MIN_HORIZON_S,
                MetricsHistoryStore::MAX_HORIZON_S
            )
        );
    }
    for _ in 0..300 {
        let raw = rng.gen_i32_range(i32::MIN + 1, i32::MAX - 1);
        clamp_store.set_horizon_seconds(raw);
        let actual = clamp_store.horizon_seconds();
        assert!((60..=7200).contains(&actual));
        assert_eq!(actual, raw.clamp(60, 7200));
    }

    // 2. Explicit boundary timestamps: 0, i64::MIN, i64::MAX
    let min_store = MetricsHistoryStore::new();
    min_store.set_horizon_seconds(60);
    min_store.push(MetricId::CpuTemp, i64::MIN, 42.5);
    min_store.push(MetricId::CpuTemp, i64::MIN + 1000, 43.0);
    let dec_min = decode_history_binary(&min_store.query_since_binary(i64::MIN)).unwrap();
    assert_eq!(dec_min.len(), 1);
    assert_eq!(dec_min[0].1.len(), 2);
    assert_eq!(dec_min[0].1[0].timestamp_ms, i64::MIN);
    assert_eq!(dec_min[0].1[1].timestamp_ms, i64::MIN + 1000);

    // Jump from i64::MIN to i64::MAX evicts old points without overflow
    min_store.push(MetricId::CpuTemp, i64::MAX, 99.25);
    let dec_max = decode_history_binary(&min_store.query_since_binary(i64::MIN)).unwrap();
    assert_eq!(dec_max.len(), 1);
    assert_eq!(dec_max[0].1.len(), 1);
    assert_eq!(dec_max[0].1[0].timestamp_ms, i64::MAX);
    assert_eq!(dec_max[0].1[0].value.to_bits(), 99.25f64.to_bits());

    // Zero timestamp + exact horizon boundary retention
    let zero_store = MetricsHistoryStore::new();
    zero_store.set_horizon_seconds(60);
    zero_store.push(MetricId::GpuTemp, 0, 50.0);
    zero_store.push(MetricId::GpuTemp, 60_000, 55.0);
    let dec_zero = decode_history_binary(&zero_store.query_since_binary(0)).unwrap();
    assert_eq!(dec_zero[0].1.len(), 2);
    // One ms past horizon evicts ts = 0
    zero_store.push(MetricId::GpuTemp, 60_001, 56.0);
    let dec_evicted = decode_history_binary(&zero_store.query_since_binary(0)).unwrap();
    assert_eq!(dec_evicted[0].1.len(), 2);
    assert_eq!(dec_evicted[0].1[0].timestamp_ms, 60_000);
    assert_eq!(dec_evicted[0].1[1].timestamp_ms, 60_001);

    // 3. Randomized wall-clock forward/backward jump stress (150 cases)
    for case in 0..150 {
        let store = MetricsHistoryStore::new();
        let horizon_s = rng.gen_i32_range(60, 7200);
        store.set_horizon_seconds(horizon_s);
        let horizon_ms = i64::from(horizon_s) * 1000;

        let mut ts = rng.gen_i64_range(-1_000_000_000, 1_000_000_000);
        for _ in 0..25 {
            // Mix normal ticks, backward jumps, and forward jumps
            let step = match rng.gen_usize_range(0, 3) {
                0 => rng.gen_i64_range(1, 5_000),
                1 => -rng.gen_i64_range(1, horizon_ms * 2),
                2 => rng.gen_i64_range(horizon_ms / 2, horizon_ms * 2),
                _ => 0,
            };
            ts = ts.saturating_add(step);
            store.push(MetricId::CpuPower, ts, 25.0);
        }

        let decoded = decode_history_binary(&store.query_since_binary(i64::MIN)).unwrap();
        assert_eq!(decoded.len(), 1, "case {case}: non-empty series expected");
        let pts = &decoded[0].1;
        let newest = pts.last().unwrap().timestamp_ms;
        let oldest = pts.first().unwrap().timestamp_ms;
        assert!(
            oldest >= newest.saturating_sub(horizon_ms),
            "case {case}: {oldest} older than cutoff for newest {newest} ({horizon_ms}ms)"
        );
        for win in pts.windows(2) {
            assert!(
                win[0].timestamp_ms <= win[1].timestamp_ms,
                "case {case}: wall-clock jumps broke sorted invariant"
            );
        }

        // JSON and binary `since` queries agree on point count for safe integer timestamps
        let mid_ts = pts[pts.len() / 2].timestamp_ms;
        let json_str = store.query_since_json(mid_ts);
        let json: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        let json_len = json[MetricId::CpuPower.name()].as_array().unwrap().len();
        let bin_len = decode_history_binary(&store.query_since_binary(mid_ts)).unwrap()[0]
            .1
            .len();
        assert_eq!(json_len, bin_len, "case {case}: JSON vs binary since count mismatch");
    }
}

#[test]
fn prop_history_binary_decoder_rejects_truncated_and_corrupted_frames_without_panic() {
    let mut rng = SplitMix64::new(0xB10A_0003_2026_0003);

    // 1. Systematic truncation of every prefix of valid multi-series frames
    let store = MetricsHistoryStore::new();
    for (idx, id) in MetricId::ALL.iter().enumerate() {
        for pt in 0..3 {
            store.push(*id, 10_000 + pt * 1_000, 10.0 + idx as f64 + pt as f64);
        }
    }
    let full = store.query_since_binary(0);
    let block_len = 5 + 3 * 16; // 53 bytes per series
    assert_eq!(full.len(), MetricId::COUNT * block_len);

    for cut in 1..full.len() {
        let prefix = &full[..cut];
        let rem = cut % block_len;
        let res = decode_history_binary(prefix);
        if rem == 0 {
            let ok = res.expect("exact series-boundary prefix must decode");
            assert_eq!(ok.len(), cut / block_len);
        } else if rem < 5 {
            assert_eq!(
                res,
                Err(HistoryCodecError::TruncatedHeader),
                "cut={cut} (rem={rem}) must fail with TruncatedHeader"
            );
        } else {
            assert!(
                matches!(res, Err(HistoryCodecError::TruncatedPayload { .. })),
                "cut={cut} (rem={rem}) must fail with TruncatedPayload, got {res:?}"
            );
        }
    }

    // 2. Every unknown metric ID 10..=255 rejected
    for bad_id in 10u8..=u8::MAX {
        let mut frame = vec![bad_id];
        frame.extend_from_slice(&0u32.to_ne_bytes());
        assert_eq!(
            decode_history_binary(&frame),
            Err(HistoryCodecError::UnknownMetricId(bad_id))
        );
    }

    // 3. Duplicate metric ID rejected for all 10 IDs
    for id in MetricId::ALL {
        let mut frame = Vec::new();
        for _ in 0..2 {
            frame.push(id as u8);
            frame.extend_from_slice(&0u32.to_ne_bytes());
        }
        assert_eq!(
            decode_history_binary(&frame),
            Err(HistoryCodecError::DuplicateMetricId(id as u8))
        );
    }

    // 4. Oversized counts rejected (single series > 1_000_000 and cumulative total > 1_000_000)
    for bad_count in [1_000_001u32, 10_000_000, u32::MAX] {
        let mut frame = vec![MetricId::CpuTemp as u8];
        frame.extend_from_slice(&bad_count.to_ne_bytes());
        assert_eq!(
            decode_history_binary(&frame),
            Err(HistoryCodecError::CountOverflow(bad_count))
        );
    }

    // 5. Non-finite f64 bit patterns rejected
    let non_finite_bits = [
        f64::NAN.to_bits(),
        f64::INFINITY.to_bits(),
        f64::NEG_INFINITY.to_bits(),
        0x7ff8_0000_0000_0001u64, // quiet NaN payload
        0xfff0_0000_0000_0001u64, // negative signaling NaN
    ];
    for bits in non_finite_bits {
        let mut frame = vec![MetricId::GpuPower as u8];
        frame.extend_from_slice(&1u32.to_ne_bytes());
        frame.extend_from_slice(&1234i64.to_ne_bytes());
        frame.extend_from_slice(&bits.to_ne_bytes());
        assert_eq!(
            decode_history_binary(&frame),
            Err(HistoryCodecError::NonFiniteValue {
                metric_id: MetricId::GpuPower as u8,
                index: 0,
            })
        );
    }

    // 6. 500 random byte buffers must never panic and any Ok result must uphold all invariants
    for _ in 0..500 {
        let len = rng.gen_usize_range(0, 256);
        let buf: Vec<u8> = (0..len).map(|_| rng.gen_u8()).collect();
        if let Ok(decoded) = decode_history_binary(&buf) {
            let mut seen = [false; MetricId::COUNT];
            let mut expected_bytes = 0usize;
            for (id, pts) in &decoded {
                assert!(!seen[*id as usize], "duplicate MetricId in Ok decode");
                seen[*id as usize] = true;
                expected_bytes += 5 + pts.len() * 16;
                for pt in pts {
                    assert!(pt.value.is_finite(), "non-finite value in Ok decode");
                }
            }
            assert_eq!(expected_bytes, buf.len());
        }
    }
}

// ============================================================================
// BLE Packet Codec & CoolerCommandQueue Properties
// ============================================================================

#[test]
fn prop_ble_packet_encoder_decoder_roundtrip_and_invariants() {
    let mut rng = SplitMix64::new(0xB1E0_0001_2026_0001);

    // 1. Exhaustive fan speed 0..=255
    for speed in 0u8..=u8::MAX {
        let cmd = CoolerCommand::FanSpeed(speed);
        if speed <= 100 {
            assert!(cmd.validate().is_ok());
            let pkt = CoolerPacketEncoder::fan_speed(speed).unwrap();
            assert_eq!(cmd.encode().unwrap(), pkt);
            assert_eq!(pkt.len(), PACKET_LEN);
            assert_eq!(pkt[0], PACKET_HEADER);
            assert_eq!(pkt[1], CMD_FAN);
            assert_eq!(pkt[2], 0x01);
            assert_eq!(pkt[3], speed);
            assert_eq!(pkt[7], PACKET_FOOTER);
            assert_eq!(decode_cooler_packet(&pkt), Ok(CoolerCommand::FanSpeed(speed)));
        } else {
            assert_eq!(cmd.validate(), Err(CoolerError::InvalidFanSpeed(speed)));
            assert_eq!(
                CoolerPacketEncoder::fan_speed(speed),
                Err(CoolerError::InvalidFanSpeed(speed))
            );
        }
    }

    // 2. Exhaustive pump voltage codes 0..=255: 0,2,3,4 allowed; 1 (V12) rejected; >=5 invalid
    for code in 0u8..=u8::MAX {
        match code {
            0 | 2 | 3 | 4 => {
                let voltage = PumpVoltage::try_from(code).expect("allowed code must convert");
                assert!(CorePumpVoltage::from_discrete_code(i32::from(code)).is_ok());
                let cmd = CoolerCommand::PumpVoltage(voltage);
                assert!(cmd.validate().is_ok());
                let pkt = CoolerPacketEncoder::pump_voltage(voltage).unwrap();
                assert_eq!(cmd.encode().unwrap(), pkt);
                assert!(CoolerPacketEncoder::validate_packet(&pkt).is_ok());
                assert_eq!(pkt[1], CMD_PUMP);
                if voltage == PumpVoltage::Off {
                    assert_eq!(pkt, CoolerPacketEncoder::turn_off(CMD_PUMP));
                    assert_eq!(decode_cooler_packet(&pkt), Ok(CoolerCommand::TurnOffPump));
                } else {
                    assert_eq!(pkt[2], 0x01);
                    assert_eq!(pkt[3], 60);
                    assert_eq!(pkt[4], code);
                    assert_eq!(
                        decode_cooler_packet(&pkt),
                        Ok(CoolerCommand::PumpVoltage(voltage))
                    );
                }
            }
            1 => {
                assert_eq!(PumpVoltage::try_from(1), Ok(PumpVoltage::V12));
                assert!(CorePumpVoltage::from_discrete_code(1).is_err());
                assert_eq!(
                    CoolerPacketEncoder::pump_voltage(PumpVoltage::V12),
                    Err(CoolerError::V12Forbidden)
                );
                assert_eq!(
                    CoolerCommand::PumpVoltage(PumpVoltage::V12).validate(),
                    Err(CoolerError::V12Forbidden)
                );
            }
            other => {
                assert_eq!(
                    PumpVoltage::try_from(other),
                    Err(CoolerError::InvalidVoltageCode(other))
                );
                assert!(CorePumpVoltage::from_discrete_code(i32::from(other)).is_err());
            }
        }
    }

    // 3. Randomized LED RGB + all 5 modes round-trip (300 cases)
    for _ in 0..300 {
        let r = rng.gen_u8();
        let g = rng.gen_u8();
        let b = rng.gen_u8();
        let state = ALL_RGB_STATES[rng.gen_usize_range(0, ALL_RGB_STATES.len() - 1)];
        let cmd = CoolerCommand::Rgb {
            red: r,
            green: g,
            blue: b,
            state,
        };
        assert!(cmd.validate().is_ok());
        let pkt = CoolerPacketEncoder::rgb(r, g, b, state);
        assert_eq!(cmd.encode().unwrap(), pkt);
        assert_eq!(pkt[0], PACKET_HEADER);
        assert_eq!(pkt[1], CMD_RGB);
        assert_eq!(pkt[2], 0x01);
        assert_eq!(pkt[3], r);
        assert_eq!(pkt[4], g);
        assert_eq!(pkt[5], b);
        assert_eq!(pkt[6], state as u8);
        assert_eq!(pkt[7], PACKET_FOOTER);
        assert_eq!(decode_cooler_packet(&pkt), Ok(cmd));
    }

    // 4. Turn-off and Reset round-trip
    assert_eq!(
        decode_cooler_packet(&CoolerCommand::Reset.encode().unwrap()),
        Ok(CoolerCommand::Reset)
    );
    assert_eq!(
        decode_cooler_packet(&CoolerCommand::TurnOffFan.encode().unwrap()),
        Ok(CoolerCommand::TurnOffFan)
    );
    assert_eq!(
        decode_cooler_packet(&CoolerCommand::TurnOffPump.encode().unwrap()),
        Ok(CoolerCommand::TurnOffPump)
    );
    assert_eq!(
        decode_cooler_packet(&CoolerCommand::TurnOffLed.encode().unwrap()),
        Ok(CoolerCommand::TurnOffLed)
    );
    assert_eq!(CoolerPacketEncoder::reset()[1], CMD_RESET);

    // 5. Fuzzed validate_packet over 500 arbitrary byte slices
    for _ in 0..500 {
        let len = rng.gen_usize_range(0, 32);
        let raw: Vec<u8> = (0..len).map(|_| rng.gen_u8()).collect();
        let res = CoolerPacketEncoder::validate_packet(&raw);
        if raw.len() != PACKET_LEN {
            assert_eq!(res, Err(CoolerError::InvalidPacketLength(raw.len())));
        } else if raw[0] != PACKET_HEADER || raw[7] != PACKET_FOOTER {
            assert_eq!(res, Err(CoolerError::InvalidFraming(raw[0], raw[7])));
        } else {
            assert_eq!(res, Ok(()));
        }
    }
}

#[test]
fn prop_cooler_command_queue_coalescing_throttle_and_generation_cancellation() {
    let mut rng = SplitMix64::new(0xB1E0_0002_2026_0002);
    let base = Instant::now();

    // 1. Coalescing latest value per subsystem slot across 250 random bursts
    for case in 0..250 {
        let mut queue = CoolerCommandQueue::default();
        let burst_len = rng.gen_usize_range(1, 30);

        let mut last_fan: Option<CoolerCommand> = None;
        let mut last_pump: Option<CoolerCommand> = None;
        let mut last_rgb: Option<CoolerCommand> = None;
        let mut last_reset: Option<CoolerCommand> = None;

        for _ in 0..burst_len {
            // Interleave invalid commands that must be rejected without mutating queue
            if rng.gen_usize_range(0, 7) == 0 {
                let before_len = queue.pending_len();
                let bad = if rng.gen_bool() {
                    CoolerCommand::FanSpeed(rng.gen_u8_range(101, u8::MAX))
                } else {
                    CoolerCommand::PumpVoltage(PumpVoltage::V12)
                };
                assert!(queue.enqueue(bad).is_err());
                assert_eq!(queue.pending_len(), before_len);
                continue;
            }

            let cmd = gen_valid_cooler_command(&mut rng);
            queue.enqueue(cmd).unwrap();
            match cmd {
                CoolerCommand::FanSpeed(_) | CoolerCommand::TurnOffFan => last_fan = Some(cmd),
                CoolerCommand::PumpVoltage(_) | CoolerCommand::TurnOffPump => last_pump = Some(cmd),
                CoolerCommand::Rgb { .. } | CoolerCommand::TurnOffLed => last_rgb = Some(cmd),
                CoolerCommand::Reset => last_reset = Some(cmd),
            }
            assert!(
                queue.pending_len() <= 4,
                "case {case}: coalesced queue exceeded 4 subsystem slots"
            );
        }

        let expected_count = [last_fan, last_pump, last_rgb, last_reset]
            .into_iter()
            .flatten()
            .count();
        assert_eq!(
            queue.pending_len(),
            expected_count,
            "case {case}: coalesced slot count mismatch"
        );

        // Drain with exact 80ms spacing and verify latest value per subsystem
        let mut now = base;
        let mut drained = Vec::new();
        while queue.pending_len() > 0 {
            let popped = queue
                .pop_ready_at(now)
                .expect("command must be ready at 80ms boundary");
            assert_eq!(popped.generation, queue.generation());
            drained.push(popped.command);
            now += BLE_WRITE_GAP;
        }
        assert_eq!(drained.len(), expected_count);
        if let Some(f) = last_fan {
            assert!(drained.contains(&f));
        }
        if let Some(p) = last_pump {
            assert!(drained.contains(&p));
        }
        if let Some(r) = last_rgb {
            assert!(drained.contains(&r));
        }
        if let Some(rst) = last_reset {
            assert!(drained.contains(&rst));
        }
    }

    // 2. Throttle >= 80ms gap and generation cancellation across 200 random timelines
    for case in 0..200 {
        let mut queue = CoolerCommandQueue::default();
        let mut limiter_ref = BleWriteLimiter::new(BLE_WRITE_GAP);
        let mut now = base;
        let mut last_write: Option<Instant> = None;
        let mut expected_gen = 0u64;

        for _ in 0..30 {
            let action = rng.gen_usize_range(0, 4);
            match action {
                0 | 1 => {
                    let cmd = gen_valid_cooler_command(&mut rng);
                    queue.enqueue(cmd).unwrap();
                }
                2 => {
                    // Generation bump cancels all pending stale commands
                    expected_gen += 1;
                    let new_gen = queue.bump_generation();
                    assert_eq!(new_gen, expected_gen);
                    assert_eq!(queue.generation(), expected_gen);
                    assert_eq!(queue.pending_len(), 0);
                    assert!(queue.peek_ready_at(now).is_none());
                }
                3 => {
                    // Advance clock by random 0..=140ms and check throttle invariants
                    let dt = Duration::from_millis(u64::from(rng.gen_u8_range(0, 140)));
                    now += dt;
                    let expected_delay = limiter_ref.required_delay_at(now);
                    assert_eq!(queue.next_delay_at(now), expected_delay);

                    if queue.pending_len() > 0 {
                        if !expected_delay.is_zero() {
                            assert!(
                                queue.peek_ready_at(now).is_none(),
                                "case {case}: command leaked before 80ms gap elapsed"
                            );
                            assert!(queue.pop_ready_at(now).is_none());
                        } else {
                            let popped = queue
                                .pop_ready_at(now)
                                .expect("ready command must pop once >=80ms elapsed");
                            assert_eq!(popped.generation, expected_gen);
                            if let Some(prev) = last_write {
                                assert!(
                                    now.duration_since(prev) >= BLE_WRITE_GAP,
                                    "case {case}: gap {:?} < {:?}",
                                    now.duration_since(prev),
                                    BLE_WRITE_GAP
                                );
                            }
                            last_write = Some(now);
                            limiter_ref.record_write_at(now);
                        }
                    }
                }
                _ => {
                    // Failed write attempt keeps head queued while advancing throttle timer
                    if let Some(head) = queue.peek_ready_at(now) {
                        let len_before = queue.pending_len();
                        queue.record_write_attempt_at(now);
                        limiter_ref.record_write_at(now);
                        last_write = Some(now);
                        assert_eq!(queue.pending_len(), len_before);
                        assert!(queue.peek_ready_at(now + Duration::from_millis(79)).is_none());
                        assert_eq!(queue.peek_ready_at(now + BLE_WRITE_GAP), Some(head));
                    }
                }
            }
        }
    }
}

#[test]
fn prop_cooler_controller_reconnect_generation_bump_and_restore() {
    let mut rng = SplitMix64::new(0xB1E0_0003_2026_0003);
    let base = Instant::now();

    for case in 0..150 {
        let mut ctrl = CoolerController::new();
        let transport = MockBleTransport::new(true);
        ctrl.set_mode(CoolerMode::Manual);

        // Configure random subset of Fan, Pump, RGB targets (including explicit Off vs Unset)
        let mut expected_packets = Vec::new();
        match rng.gen_usize_range(0, 2) {
            0 => {} // Unset
            1 => {
                ctrl.turn_off_fan().unwrap();
                expected_packets.push(CoolerPacketEncoder::turn_off(CMD_FAN));
            }
            _ => {
                let spd = rng.gen_u8_range(0, 100);
                ctrl.set_fan_speed(spd).unwrap();
                expected_packets.push(CoolerPacketEncoder::fan_speed(spd).unwrap());
            }
        }
        match rng.gen_usize_range(0, 2) {
            0 => {} // Unset
            1 => {
                ctrl.turn_off_pump().unwrap();
                expected_packets.push(CoolerPacketEncoder::turn_off(CMD_PUMP));
            }
            _ => {
                let v = ALLOWED_PUMP_VOLTAGES[rng.gen_usize_range(0, 2)];
                ctrl.set_pump_voltage(v).unwrap();
                expected_packets.push(CoolerPacketEncoder::pump_voltage(v).unwrap());
            }
        }
        match rng.gen_usize_range(0, 2) {
            0 => {} // Unset
            1 => {
                ctrl.turn_off_led().unwrap();
                expected_packets.push(CoolerPacketEncoder::turn_off(CMD_RGB));
            }
            _ => {
                let r = rng.gen_u8();
                let g = rng.gen_u8();
                let b = rng.gen_u8();
                let st = ALL_RGB_STATES[rng.gen_usize_range(0, ALL_RGB_STATES.len() - 1)];
                ctrl.set_rgb(r, g, b, st).unwrap();
                expected_packets.push(CoolerPacketEncoder::rgb(r, g, b, st));
            }
        }

        // Disconnect bumps generation and clears pending queue; flush sends nothing
        let gen0 = ctrl.generation();
        ctrl.on_disconnected();
        assert_eq!(ctrl.state(), CoolerState::Disconnected);
        assert_eq!(ctrl.generation(), gen0 + 1);
        assert_eq!(ctrl.pending_len(), 0);
        assert_eq!(ctrl.flush_ready(&transport, base).unwrap(), 0);

        // Sleep prep also bumps generation and blocks flushes while suspending
        ctrl.on_prepare_for_sleep(true);
        assert_eq!(ctrl.generation(), gen0 + 2);
        assert_eq!(ctrl.pending_len(), 0);
        ctrl.on_prepare_for_sleep(false);

        // Reconnect bumps generation and restores all active targets (including explicit Off)
        ctrl.on_connected();
        assert_eq!(ctrl.state(), CoolerState::Connected);
        assert_eq!(ctrl.generation(), gen0 + 3);
        assert_eq!(
            ctrl.pending_len(),
            expected_packets.len(),
            "case {case}: reconnect restored wrong number of active targets"
        );

        let mut now = base;
        let mut total_sent = 0usize;
        for _ in 0..expected_packets.len() {
            let sent = ctrl.flush_ready(&transport, now).unwrap();
            assert_eq!(sent, 1, "case {case}: expected exactly 1 packet per 80ms tick");
            total_sent += sent;
            // Immediate sub-80ms flush must send 0
            assert_eq!(
                ctrl.flush_ready(&transport, now + Duration::from_millis(79))
                    .unwrap(),
                0
            );
            now += BLE_WRITE_GAP;
        }
        assert_eq!(total_sent, expected_packets.len());
        assert_eq!(ctrl.pending_len(), 0);
        assert_eq!(
            transport.packets(),
            expected_packets,
            "case {case}: restored packets did not match active targets"
        );
    }
}
