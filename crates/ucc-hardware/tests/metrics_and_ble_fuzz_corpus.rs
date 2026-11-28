// SPDX-License-Identifier: GPL-3.0-or-later

//! Deterministic fuzz and mutation corpus test harness for legacy binary metrics history
//! (`telemetry::history`) and BLE LCT water-cooler frames (`cooler` / `bluez_cooler`).
//!
//! Zero external fuzzing dependencies and zero raw host hardware I/O.

use std::time::{Duration, Instant};
use ucc_hardware::bluez_cooler::{
    detect_lct_model, select_cooler, DiscoveredCooler, LctDeviceModel, SavedCoolerIdentity,
};
use ucc_hardware::cooler::{
    BleTransport, CoolerCommand, CoolerCommandQueue, CoolerController, CoolerError, CoolerMode,
    CoolerPacketEncoder, FanTarget, MockBleTransport, PumpTarget, PumpVoltage, RgbState, CMD_FAN,
    CMD_PUMP, CMD_RESET, CMD_RGB, PACKET_FOOTER, PACKET_HEADER, PACKET_LEN,
};
use ucc_hardware::telemetry::history::{
    decode_history_binary, HistoryCodecError, MetricDataPoint, MetricId, MetricsHistoryStore,
};

// ---------------------------------------------------------------------------
// Deterministic SplitMix64 PRNG and byte-level mutation primitives
// ---------------------------------------------------------------------------

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

    fn next_u8(&mut self) -> u8 {
        self.next_u64() as u8
    }

    fn gen_range(&mut self, upper: usize) -> usize {
        if upper <= 1 {
            return 0;
        }
        (self.next_u64() as usize) % upper
    }

    fn fill_bytes(&mut self, buf: &mut [u8]) {
        let mut chunks = buf.chunks_exact_mut(8);
        for chunk in &mut chunks {
            chunk.copy_from_slice(&self.next_u64().to_le_bytes());
        }
        let rem = chunks.into_remainder();
        if !rem.is_empty() {
            let tail = self.next_u64().to_le_bytes();
            rem.copy_from_slice(&tail[..rem.len()]);
        }
    }

    fn gen_vec(&mut self, max_len: usize) -> Vec<u8> {
        let len = self.gen_range(max_len + 1);
        let mut out = vec![0u8; len];
        self.fill_bytes(&mut out);
        out
    }
}

const BOUNDARY_U8: [u8; 14] = [
    0x00, 0x01, 0x02, 0x04, 0x05, 0x09, 0x0a, 0x19, 0x1b, 0x1c, 0x1e, 100, 101, 0xff,
];

const BOUNDARY_U32: [u32; 10] = [
    0,
    1,
    2,
    10_000,
    1_000_000,
    1_000_001,
    0x1000_0000,
    0x7fff_ffff,
    u32::MAX - 1,
    u32::MAX,
];

const BOUNDARY_I64: [i64; 8] = [
    i64::MIN,
    i64::MIN + 1,
    -1,
    0,
    1,
    1_700_000_000_000,
    i64::MAX - 1,
    i64::MAX,
];

const BOUNDARY_F64_BITS: [u64; 9] = [
    0.0_f64.to_bits(),
    (-0.0_f64).to_bits(),
    1,                     // smallest positive subnormal
    0x8000_0000_0000_0001, // smallest negative subnormal
    f64::MIN_POSITIVE.to_bits(),
    f64::MAX.to_bits(),
    f64::INFINITY.to_bits(),
    f64::NEG_INFINITY.to_bits(),
    f64::NAN.to_bits(),
];

fn mutate_bit_flip(buf: &mut [u8], rng: &mut SplitMix64) {
    if buf.is_empty() {
        return;
    }
    let idx = rng.gen_range(buf.len());
    let bit = 1u8 << (rng.next_u8() & 7);
    buf[idx] ^= bit;
}

fn mutate_byte_insert(buf: &mut Vec<u8>, rng: &mut SplitMix64, max_len: usize) {
    if buf.len() >= max_len {
        return;
    }
    let pos = rng.gen_range(buf.len() + 1);
    let val = if (rng.next_u8() & 1) == 0 {
        BOUNDARY_U8[rng.gen_range(BOUNDARY_U8.len())]
    } else {
        rng.next_u8()
    };
    buf.insert(pos, val);
}

fn mutate_byte_delete(buf: &mut Vec<u8>, rng: &mut SplitMix64) {
    if buf.is_empty() {
        return;
    }
    let pos = rng.gen_range(buf.len());
    buf.remove(pos);
}

fn mutate_truncate(buf: &mut Vec<u8>, rng: &mut SplitMix64) {
    if buf.is_empty() {
        return;
    }
    let new_len = rng.gen_range(buf.len());
    buf.truncate(new_len);
}

fn mutate_length_field_corruption(buf: &mut [u8], rng: &mut SplitMix64) {
    if buf.len() < 4 {
        return;
    }
    let count = BOUNDARY_U32[rng.gen_range(BOUNDARY_U32.len())];
    let bytes = count.to_ne_bytes();
    let offset = if buf.len() >= 5 && (rng.next_u8() & 1) == 0 {
        1
    } else {
        rng.gen_range(buf.len() - 3)
    };
    buf[offset..offset + 4].copy_from_slice(&bytes);
}

fn mutate_boundary_int_injection(buf: &mut [u8], rng: &mut SplitMix64) {
    if buf.is_empty() {
        return;
    }
    match rng.gen_range(4) {
        0 => {
            let idx = rng.gen_range(buf.len());
            buf[idx] = BOUNDARY_U8[rng.gen_range(BOUNDARY_U8.len())];
        }
        1 if buf.len() >= 4 => {
            let idx = rng.gen_range(buf.len() - 3);
            let val = BOUNDARY_U32[rng.gen_range(BOUNDARY_U32.len())];
            buf[idx..idx + 4].copy_from_slice(&val.to_ne_bytes());
        }
        2 if buf.len() >= 8 => {
            let idx = rng.gen_range(buf.len() - 7);
            let val = BOUNDARY_I64[rng.gen_range(BOUNDARY_I64.len())];
            buf[idx..idx + 8].copy_from_slice(&val.to_ne_bytes());
        }
        _ if buf.len() >= 8 => {
            let idx = rng.gen_range(buf.len() - 7);
            let bits = BOUNDARY_F64_BITS[rng.gen_range(BOUNDARY_F64_BITS.len())];
            buf[idx..idx + 8].copy_from_slice(&bits.to_ne_bytes());
        }
        _ => {
            let idx = rng.gen_range(buf.len());
            buf[idx] = BOUNDARY_U8[rng.gen_range(BOUNDARY_U8.len())];
        }
    }
}

fn mutate_corpus_entry(seed: &[u8], rng: &mut SplitMix64, max_len: usize) -> Vec<u8> {
    let mut buf = seed.to_vec();
    let steps = 1 + rng.gen_range(4);
    for _ in 0..steps {
        match rng.gen_range(6) {
            0 => mutate_bit_flip(&mut buf, rng),
            1 => mutate_byte_insert(&mut buf, rng, max_len),
            2 => mutate_byte_delete(&mut buf, rng),
            3 => mutate_truncate(&mut buf, rng),
            4 => mutate_length_field_corruption(&mut buf, rng),
            _ => mutate_boundary_int_injection(&mut buf, rng),
        }
    }
    buf
}

// ---------------------------------------------------------------------------
// Target 1: Legacy binary metrics history decoder (telemetry::history)
// ---------------------------------------------------------------------------

fn encode_raw_series(metric_id: u8, count: u32, points: &[(i64, f64)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + points.len() * 16);
    out.push(metric_id);
    out.extend_from_slice(&count.to_ne_bytes());
    for &(ts, val) in points {
        out.extend_from_slice(&ts.to_ne_bytes());
        out.extend_from_slice(&val.to_ne_bytes());
    }
    out
}

fn build_valid_history_corpus() -> Vec<Vec<u8>> {
    let mut corpus = Vec::new();

    // Empty payload
    corpus.push(Vec::new());

    // Single-series store encoding
    let single = MetricsHistoryStore::new();
    single.push(MetricId::CpuTemp, 1_000, 52.5);
    single.push(MetricId::CpuTemp, 2_000, 61.25);
    corpus.push(single.query_since_binary(0));

    // All 10 series populated via MetricsHistoryStore
    let all_series = MetricsHistoryStore::new();
    for (idx, &id) in MetricId::ALL.iter().enumerate() {
        all_series.push(id, 10_000 + (idx as i64) * 100, 12.5 + idx as f64);
        all_series.push(id, 20_000 + (idx as i64) * 100, 24.75 + idx as f64);
    }
    corpus.push(all_series.query_since_binary(0));

    // Zero-count series header
    corpus.push(encode_raw_series(MetricId::GpuPower as u8, 0, &[]));

    // Boundary timestamps and subnormal finite floats
    let subnormal_pos = f64::from_bits(1);
    let subnormal_neg = f64::from_bits(0x8000_0000_0000_0001);
    corpus.push(encode_raw_series(
        MetricId::CpuFrequency as u8,
        4,
        &[
            (i64::MIN, subnormal_pos),
            (-1, subnormal_neg),
            (0, -0.0),
            (i64::MAX, f64::MAX),
        ],
    ));

    corpus
}

fn assert_history_decode_invariants(input: &[u8]) {
    if let Ok(decoded) = decode_history_binary(input) {
        assert!(
            decoded.len() <= MetricId::COUNT,
            "decoded series count {} exceeded MetricId::COUNT",
            decoded.len()
        );
        let mut seen = [false; MetricId::COUNT];
        let mut total_points = 0usize;
        let mut reencoded = Vec::new();

        for (id, pts) in &decoded {
            assert!(
                !seen[*id as usize],
                "decoder returned duplicate MetricId {:?}",
                id
            );
            seen[*id as usize] = true;
            total_points = total_points.saturating_add(pts.len());
            assert!(
                total_points <= input.len() / 16,
                "decoded points {total_points} exceeded physical input bound {}",
                input.len() / 16
            );

            reencoded.push(*id as u8);
            reencoded.extend_from_slice(&(pts.len() as u32).to_ne_bytes());
            for pt in pts {
                assert!(
                    pt.value.is_finite(),
                    "decoder accepted non-finite value for {:?}",
                    id
                );
                reencoded.extend_from_slice(&pt.timestamp_ms.to_ne_bytes());
                reencoded.extend_from_slice(&pt.value.to_ne_bytes());
            }
        }

        // Exact round-trip stability on every accepted payload
        let round_trip = decode_history_binary(&reencoded).expect("re-encoded payload must decode");
        assert_eq!(round_trip.len(), decoded.len());
        for ((id_a, pts_a), (id_b, pts_b)) in round_trip.iter().zip(decoded.iter()) {
            assert_eq!(id_a, id_b);
            assert_eq!(pts_a.len(), pts_b.len());
            for (pa, pb) in pts_a.iter().zip(pts_b.iter()) {
                assert_eq!(pa.timestamp_ms, pb.timestamp_ms);
                assert_eq!(pa.value.to_bits(), pb.value.to_bits());
            }
        }
    }
}

#[test]
fn history_valid_frames_mutated_at_every_byte_offset() {
    let corpus = build_valid_history_corpus();
    let xor_masks = [0x01u8, 0x80u8, 0xffu8];
    let replacements = [0x00u8, 0x0au8, 0x7fu8, 0xffu8];

    for seed in &corpus {
        assert!(decode_history_binary(seed).is_ok());
        assert_history_decode_invariants(seed);

        for offset in 0..seed.len() {
            for mask in xor_masks {
                let mut mutated = seed.clone();
                mutated[offset] ^= mask;
                assert_history_decode_invariants(&mutated);
            }
            for rep in replacements {
                let mut mutated = seed.clone();
                mutated[offset] = rep;
                assert_history_decode_invariants(&mutated);
            }
        }
    }
}

#[test]
fn history_crafted_adversarial_inputs_never_panic_or_oom() {
    // 1. Empty slice decodes to empty series list
    assert_eq!(decode_history_binary(&[]), Ok(Vec::new()));

    // 2. Truncated headers (1..=4 bytes)
    for len in 1..=4 {
        let buf = vec![0u8; len];
        assert_eq!(
            decode_history_binary(&buf),
            Err(HistoryCodecError::TruncatedHeader)
        );
    }

    // 3. Adversarial count_u32 with short buffers: must NOT panic or OOM-allocate
    for bad_count in [u32::MAX, u32::MAX - 1, 0x1000_0000, 1_000_001] {
        let short = encode_raw_series(MetricId::CpuTemp as u8, bad_count, &[]);
        assert_eq!(
            decode_history_binary(&short),
            Err(HistoryCodecError::CountOverflow(bad_count))
        );

        let with_one_point =
            encode_raw_series(MetricId::CpuTemp as u8, bad_count, &[(1_000, 42.5)]);
        assert_eq!(
            decode_history_binary(&with_one_point),
            Err(HistoryCodecError::CountOverflow(bad_count))
        );
    }

    // Exact cap (1_000_000) with short buffer must reject as TruncatedPayload before allocating
    let capped_short = encode_raw_series(MetricId::GpuTemp as u8, 1_000_000, &[]);
    assert_eq!(
        decode_history_binary(&capped_short),
        Err(HistoryCodecError::TruncatedPayload {
            metric_id: MetricId::GpuTemp as u8,
            expected_bytes: 1_000_000 * 16,
            remaining_bytes: 0,
        })
    );

    // 4. Exhaustive unknown metric_id bytes (10..=255)
    for raw_id in 10u8..=u8::MAX {
        let payload = encode_raw_series(raw_id, 0, &[]);
        assert_eq!(
            decode_history_binary(&payload),
            Err(HistoryCodecError::UnknownMetricId(raw_id))
        );
    }

    // 5. Duplicate metric_id across all valid IDs
    for &id in &MetricId::ALL {
        let mut dup = encode_raw_series(id as u8, 1, &[(1_000, 10.0)]);
        dup.extend_from_slice(&encode_raw_series(id as u8, 1, &[(2_000, 20.0)]));
        assert_eq!(
            decode_history_binary(&dup),
            Err(HistoryCodecError::DuplicateMetricId(id as u8))
        );
    }

    // 6. Partial trailing record (1..=15 bytes)
    for trailing in 1usize..=15 {
        let mut buf = Vec::with_capacity(5 + trailing);
        buf.push(MetricId::CpuPower as u8);
        buf.extend_from_slice(&1u32.to_ne_bytes());
        buf.resize(5 + trailing, 0xaa);
        assert_eq!(
            decode_history_binary(&buf),
            Err(HistoryCodecError::TruncatedPayload {
                metric_id: MetricId::CpuPower as u8,
                expected_bytes: 16,
                remaining_bytes: trailing,
            })
        );
    }

    // 7. Non-finite f64 values: NaN (canonical, quiet, signaling), +Inf, -Inf
    let non_finite_values = [
        f64::NAN,
        f64::from_bits(0x7ff8_0000_0000_0001),
        f64::from_bits(0x7ff0_0000_0000_0001),
        f64::INFINITY,
        f64::NEG_INFINITY,
    ];
    for val in non_finite_values {
        let payload =
            encode_raw_series(MetricId::GpuCoreVoltage as u8, 2, &[(10, 1.25), (20, val)]);
        assert_eq!(
            decode_history_binary(&payload),
            Err(HistoryCodecError::NonFiniteValue {
                metric_id: MetricId::GpuCoreVoltage as u8,
                index: 1,
            })
        );
    }

    // 8. Subnormal finite f64 and extreme i64::MIN / i64::MAX timestamps
    let subnormal_pos = f64::from_bits(1);
    let subnormal_neg = f64::from_bits(0x8000_0000_0000_0001);
    let edge_points = [
        (i64::MIN, subnormal_pos),
        (i64::MIN + 1, subnormal_neg),
        (0, -0.0),
        (i64::MAX - 1, f64::MIN_POSITIVE),
        (i64::MAX, f64::MAX),
    ];
    let edge_payload = encode_raw_series(
        MetricId::CpuFanDuty as u8,
        edge_points.len() as u32,
        &edge_points,
    );
    let decoded = decode_history_binary(&edge_payload).expect("finite edge values must decode");
    assert_eq!(decoded.len(), 1);
    assert_eq!(decoded[0].0, MetricId::CpuFanDuty);
    assert_eq!(decoded[0].1.len(), edge_points.len());
    for (pt, &(expected_ts, expected_val)) in decoded[0].1.iter().zip(edge_points.iter()) {
        assert_eq!(pt.timestamp_ms, expected_ts);
        assert_eq!(pt.value.to_bits(), expected_val.to_bits());
    }

    // Verify MetricsHistoryStore handles extreme timestamps and drops non-finite samples safely
    let store = MetricsHistoryStore::new();
    store.push(MetricId::CpuTemp, i64::MIN, subnormal_pos);
    store.push(MetricId::CpuTemp, i64::MAX, f64::NAN);
    store.push(MetricId::CpuTemp, i64::MAX, f64::INFINITY);
    store.push(MetricId::CpuTemp, i64::MAX, f64::NEG_INFINITY);
    let encoded_min = store.query_since_binary(i64::MIN);
    let decoded_min = decode_history_binary(&encoded_min).expect("store binary must decode");
    assert_eq!(
        decoded_min,
        vec![(
            MetricId::CpuTemp,
            vec![MetricDataPoint {
                timestamp_ms: i64::MIN,
                value: subnormal_pos,
            }]
        )]
    );
    store.push(MetricId::CpuTemp, i64::MAX, 75.5);
    let encoded_max = store.query_since_binary(i64::MIN);
    assert_history_decode_invariants(&encoded_max);

    // 9. Multi-series concatenated frames with mid-stream corruption
    let s0 = encode_raw_series(MetricId::CpuTemp as u8, 2, &[(100, 45.0), (200, 46.0)]);
    let s1_bad_id = encode_raw_series(42, 1, &[(300, 12.0)]);
    let s1_bad_count = encode_raw_series(MetricId::GpuTemp as u8, u32::MAX, &[]);
    let s1_nan = encode_raw_series(MetricId::GpuTemp as u8, 1, &[(300, f64::NAN)]);
    let s2 = encode_raw_series(MetricId::GpuPower as u8, 1, &[(400, 35.0)]);

    let mut mid_bad_id = s0.clone();
    mid_bad_id.extend_from_slice(&s1_bad_id);
    mid_bad_id.extend_from_slice(&s2);
    assert_eq!(
        decode_history_binary(&mid_bad_id),
        Err(HistoryCodecError::UnknownMetricId(42))
    );

    let mut mid_bad_count = s0.clone();
    mid_bad_count.extend_from_slice(&s1_bad_count);
    mid_bad_count.extend_from_slice(&s2);
    assert_eq!(
        decode_history_binary(&mid_bad_count),
        Err(HistoryCodecError::CountOverflow(u32::MAX))
    );

    let mut mid_nan = s0.clone();
    mid_nan.extend_from_slice(&s1_nan);
    mid_nan.extend_from_slice(&s2);
    assert_eq!(
        decode_history_binary(&mid_nan),
        Err(HistoryCodecError::NonFiniteValue {
            metric_id: MetricId::GpuTemp as u8,
            index: 0,
        })
    );

    let mut mid_trunc_header = s0;
    mid_trunc_header.extend_from_slice(&[MetricId::GpuTemp as u8, 0x01, 0x00]);
    assert_eq!(
        decode_history_binary(&mid_trunc_header),
        Err(HistoryCodecError::TruncatedHeader)
    );
}

#[test]
fn history_deterministic_fuzz_corpus_random_and_mutated_buffers() {
    let mut rng = SplitMix64::new(0x51a7_9920_cafe_babe);
    let corpus = build_valid_history_corpus();

    // 4,000 mutated corpus buffers
    for _ in 0..4_000 {
        let seed = &corpus[rng.gen_range(corpus.len())];
        let mutated = mutate_corpus_entry(seed, &mut rng, 1024);
        assert_history_decode_invariants(&mutated);
    }

    // 3,000 completely random byte buffers
    for _ in 0..3_000 {
        let buf = rng.gen_vec(512);
        assert_history_decode_invariants(&buf);
    }

    // 2,000 structure-biased random buffers (valid metric_id + small count + random payload)
    for _ in 0..2_000 {
        let series_count = 1 + rng.gen_range(4);
        let mut buf = Vec::new();
        for _ in 0..series_count {
            let id = rng.gen_range(12) as u8; // includes 10..11 invalid IDs
            let count = rng.gen_range(6) as u32;
            buf.push(id);
            buf.extend_from_slice(&count.to_ne_bytes());
            let body_len = rng.gen_range((count as usize) * 16 + 17);
            let mut body = vec![0u8; body_len];
            rng.fill_bytes(&mut body);
            buf.extend_from_slice(&body);
        }
        assert_history_decode_invariants(&buf);
    }
}

// ---------------------------------------------------------------------------
// Target 2: BLE LCT water-cooler frame decoder/parser (cooler & bluez_cooler)
// ---------------------------------------------------------------------------

const ALL_RGB_STATES: [RgbState; 5] = [
    RgbState::Static,
    RgbState::Breathe,
    RgbState::Colorful,
    RgbState::BreatheColor,
    RgbState::Temperature,
];

fn build_valid_ble_corpus() -> Vec<[u8; PACKET_LEN]> {
    let mut corpus = vec![
        CoolerPacketEncoder::reset(),
        CoolerPacketEncoder::fan_speed(0).unwrap(),
        CoolerPacketEncoder::fan_speed(50).unwrap(),
        CoolerPacketEncoder::fan_speed(100).unwrap(),
        CoolerPacketEncoder::pump_voltage(PumpVoltage::V11).unwrap(),
        CoolerPacketEncoder::pump_voltage(PumpVoltage::V7).unwrap(),
        CoolerPacketEncoder::pump_voltage(PumpVoltage::V8).unwrap(),
        CoolerPacketEncoder::pump_voltage(PumpVoltage::Off).unwrap(),
        CoolerPacketEncoder::turn_off(CMD_FAN),
        CoolerPacketEncoder::turn_off(CMD_PUMP),
        CoolerPacketEncoder::turn_off(CMD_RGB),
        CoolerPacketEncoder::turn_off(CMD_RESET),
    ];
    for state in ALL_RGB_STATES {
        corpus.push(CoolerPacketEncoder::rgb(0, 0, 0, state));
        corpus.push(CoolerPacketEncoder::rgb(255, 128, 64, state));
        corpus.push(CoolerPacketEncoder::rgb(255, 255, 255, state));
    }
    corpus
}

fn assert_ble_packet_invariants(raw: &[u8]) {
    let res = CoolerPacketEncoder::validate_packet(raw);
    if raw.len() != PACKET_LEN {
        assert_eq!(res, Err(CoolerError::InvalidPacketLength(raw.len())));
    } else if raw[0] != PACKET_HEADER || raw[7] != PACKET_FOOTER {
        assert_eq!(res, Err(CoolerError::InvalidFraming(raw[0], raw[7])));
    } else {
        assert_eq!(res, Ok(()));
    }

    if let Ok(arr) = <[u8; PACKET_LEN]>::try_from(raw) {
        let connected = MockBleTransport::new(true);
        assert_eq!(connected.write_packet(arr), res);
        assert_eq!(connected.packets().len(), usize::from(res.is_ok()));

        let disconnected = MockBleTransport::new(false);
        assert_eq!(
            disconnected.write_packet(arr),
            Err(CoolerError::NotConnected)
        );
    }
}

#[test]
fn ble_valid_lct_packets_mutated_at_every_byte_offset() {
    let corpus = build_valid_ble_corpus();
    let xor_masks = [0x01u8, 0x80u8, 0xffu8];
    let replacements = [0x00u8, 0x01u8, 0x7fu8, 0xefu8, 0xfeu8, 0xffu8];

    for &seed in &corpus {
        assert_eq!(CoolerPacketEncoder::validate_packet(&seed), Ok(()));
        assert_ble_packet_invariants(&seed);

        for offset in 0..PACKET_LEN {
            for mask in xor_masks {
                let mut mutated = seed;
                mutated[offset] ^= mask;
                assert_ble_packet_invariants(&mutated);
                if offset == 0 || offset == 7 {
                    assert!(matches!(
                        CoolerPacketEncoder::validate_packet(&mutated),
                        Err(CoolerError::InvalidFraming(_, _))
                    ));
                }
            }
            for rep in replacements {
                let mut mutated = seed;
                mutated[offset] = rep;
                assert_ble_packet_invariants(&mutated);
            }
        }
    }
}

#[test]
fn ble_crafted_adversarial_frames_and_semantic_guards() {
    // 1. Empty, 1-byte, short (2..=7), and oversized (9..=4096) frames
    let adversarial_lengths = [
        0usize, 1, 2, 3, 4, 5, 6, 7, 9, 10, 15, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096,
    ];
    for len in adversarial_lengths {
        let mut buf = vec![0xaa; len];
        if !buf.is_empty() {
            buf[0] = PACKET_HEADER;
            let last = buf.len() - 1;
            buf[last] = PACKET_FOOTER;
        }
        assert_eq!(
            CoolerPacketEncoder::validate_packet(&buf),
            Err(CoolerError::InvalidPacketLength(len))
        );
    }

    // 2. Exhaustive command/opcode bytes (0..=255) and corrupted trailer/checksum positions
    for opcode in 0u8..=u8::MAX {
        let framed = [PACKET_HEADER, opcode, 0x01, 50, 0x00, 0x00, 0x00, PACKET_FOOTER];
        assert_ble_packet_invariants(&framed);

        let bad_footer = [
            PACKET_HEADER,
            opcode,
            0x01,
            50,
            0x00,
            0x00,
            0x00,
            opcode ^ PACKET_FOOTER,
        ];
        assert_ble_packet_invariants(&bad_footer);
    }

    // 3. Exhaustive header and footer corruption sweep
    for b in 0u8..=u8::MAX {
        if b != PACKET_HEADER {
            let pkt = [b, CMD_RESET, 0x00, 0x01, 0x00, 0x00, 0x00, PACKET_FOOTER];
            assert_eq!(
                CoolerPacketEncoder::validate_packet(&pkt),
                Err(CoolerError::InvalidFraming(b, PACKET_FOOTER))
            );
        }
        if b != PACKET_FOOTER {
            let pkt = [PACKET_HEADER, CMD_RESET, 0x00, 0x01, 0x00, 0x00, 0x00, b];
            assert_eq!(
                CoolerPacketEncoder::validate_packet(&pkt),
                Err(CoolerError::InvalidFraming(PACKET_HEADER, b))
            );
        }
    }

    // 4. Forbidden pump code 1 (PumpVoltage::V12) and invalid pump voltage codes (5..=255)
    assert_eq!(PumpVoltage::try_from(1), Ok(PumpVoltage::V12));
    assert_eq!(
        CoolerPacketEncoder::pump_voltage(PumpVoltage::V12),
        Err(CoolerError::V12Forbidden)
    );
    assert_eq!(
        CoolerCommand::PumpVoltage(PumpVoltage::V12).validate(),
        Err(CoolerError::V12Forbidden)
    );
    assert_eq!(
        CoolerCommand::PumpVoltage(PumpVoltage::V12).encode(),
        Err(CoolerError::V12Forbidden)
    );

    let mut queue = CoolerCommandQueue::default();
    assert_eq!(
        queue.enqueue(CoolerCommand::PumpVoltage(PumpVoltage::V12)),
        Err(CoolerError::V12Forbidden)
    );
    assert_eq!(queue.pending_len(), 0);

    let mut controller = CoolerController::new();
    assert_eq!(controller.mode(), CoolerMode::Auto);
    assert_eq!(
        controller.set_pump_voltage(PumpVoltage::V12),
        Err(CoolerError::V12Forbidden)
    );
    assert_eq!(
        controller.set_manual_pump_voltage(PumpVoltage::V12),
        Err(CoolerError::V12Forbidden)
    );
    assert_eq!(controller.mode(), CoolerMode::Auto);
    assert_eq!(controller.desired_pump_target(), PumpTarget::Unset);

    // Manual profile target with forbidden code 1 or out-of-range codes must not set V12
    controller.apply_manual_profile_targets(None, Some(1));
    controller.apply_manual_profile_targets(None, Some(-1));
    controller.apply_manual_profile_targets(None, Some(99));
    controller.set_mode(CoolerMode::Manual);
    assert_eq!(controller.desired_pump_target(), PumpTarget::Unset);

    for raw_code in 5u8..=u8::MAX {
        assert_eq!(
            PumpVoltage::try_from(raw_code),
            Err(CoolerError::InvalidVoltageCode(raw_code))
        );
    }

    // Crafted raw packet containing forbidden pump code 1 never panics in validate_packet
    let raw_v12_pkt = [PACKET_HEADER, CMD_PUMP, 0x01, 60, 0x01, 0x00, 0x00, PACKET_FOOTER];
    assert_ble_packet_invariants(&raw_v12_pkt);

    // 5. Out-of-range fan speeds (101..=255)
    let mut fan_ctrl = CoolerController::new();
    for speed in 101u8..=u8::MAX {
        assert_eq!(
            CoolerPacketEncoder::fan_speed(speed),
            Err(CoolerError::InvalidFanSpeed(speed))
        );
        assert_eq!(
            CoolerCommand::FanSpeed(speed).validate(),
            Err(CoolerError::InvalidFanSpeed(speed))
        );
        assert_eq!(
            CoolerCommand::FanSpeed(speed).encode(),
            Err(CoolerError::InvalidFanSpeed(speed))
        );
        assert_eq!(
            fan_ctrl.set_manual_fan_speed(speed),
            Err(CoolerError::InvalidFanSpeed(speed))
        );
        assert_eq!(fan_ctrl.mode(), CoolerMode::Auto);
        assert_eq!(fan_ctrl.desired_fan_target(), FanTarget::Unset);

        let raw_bad_fan = [PACKET_HEADER, CMD_FAN, 0x01, speed, 0x00, 0x00, 0x00, PACKET_FOOTER];
        assert_ble_packet_invariants(&raw_bad_fan);
    }

    // 6. RGB LED modes (valid + crafted invalid mode bytes 5..=255) and boundary RGB channels
    let rgb_edges = [0u8, 1, 127, 128, 254, 255];
    for state in ALL_RGB_STATES {
        for &c in &rgb_edges {
            let pkt = CoolerPacketEncoder::rgb(c, 255 - c, c ^ 0x55, state);
            assert_eq!(pkt[1], CMD_RGB);
            assert_eq!(pkt[6], state as u8);
            assert_ble_packet_invariants(&pkt);
        }
    }
    for bad_led_mode in 5u8..=u8::MAX {
        let raw_bad_rgb = [
            PACKET_HEADER,
            CMD_RGB,
            0x01,
            255,
            0,
            255,
            bad_led_mode,
            PACKET_FOOTER,
        ];
        assert_ble_packet_invariants(&raw_bad_rgb);
    }
}

#[test]
fn ble_deterministic_fuzz_corpus_random_and_mutated_frames() {
    let mut rng = SplitMix64::new(0x0b1e_c001_ea75_4242);
    let corpus = build_valid_ble_corpus();

    // 4,000 mutated LCT frames (up to 4096 bytes)
    for _ in 0..4_000 {
        let seed = &corpus[rng.gen_range(corpus.len())];
        let mutated = mutate_corpus_entry(seed, &mut rng, 4096);
        assert_ble_packet_invariants(&mutated);
    }

    // 3,000 arbitrary-length random buffers (0..=64 and periodic oversized up to 4096)
    for i in 0..3_000 {
        let max_len = if i % 25 == 0 { 4096 } else { 64 };
        let buf = rng.gen_vec(max_len);
        assert_ble_packet_invariants(&buf);
    }

    // 2,000 8-byte frames with valid header/footer and random inner payloads
    for _ in 0..2_000 {
        let mut pkt = [0u8; PACKET_LEN];
        rng.fill_bytes(&mut pkt);
        pkt[0] = PACKET_HEADER;
        pkt[7] = PACKET_FOOTER;
        assert_ble_packet_invariants(&pkt);
    }

    // 1,000 pseudo-random controller command sequences ensuring safety invariants on wire output
    let mut ctrl = CoolerController::new();
    let transport = MockBleTransport::new(true);
    ctrl.on_connected();
    let mut now = Instant::now();

    for _ in 0..1_000 {
        match rng.gen_range(6) {
            0 => {
                let speed = rng.next_u8();
                let res = ctrl.set_fan_speed(speed);
                if speed > 100 {
                    assert_eq!(res, Err(CoolerError::InvalidFanSpeed(speed)));
                } else {
                    assert_eq!(res, Ok(()));
                }
            }
            1 => {
                let code = rng.next_u8();
                match PumpVoltage::try_from(code) {
                    Ok(PumpVoltage::V12) => {
                        assert_eq!(
                            ctrl.set_pump_voltage(PumpVoltage::V12),
                            Err(CoolerError::V12Forbidden)
                        );
                    }
                    Ok(valid_v) => {
                        assert_eq!(ctrl.set_pump_voltage(valid_v), Ok(()));
                    }
                    Err(err) => {
                        assert_eq!(err, CoolerError::InvalidVoltageCode(code));
                    }
                }
            }
            2 => {
                let state = ALL_RGB_STATES[rng.gen_range(ALL_RGB_STATES.len())];
                assert_eq!(
                    ctrl.set_rgb(rng.next_u8(), rng.next_u8(), rng.next_u8(), state),
                    Ok(())
                );
            }
            3 => {
                assert_eq!(ctrl.turn_off_fan(), Ok(()));
            }
            4 => {
                assert_eq!(ctrl.turn_off_pump(), Ok(()));
            }
            _ => {
                assert_eq!(ctrl.turn_off_led(), Ok(()));
            }
        }

        now += Duration::from_millis(100);
        ctrl.flush_ready(&transport, now)
            .expect("connected mock transport flush must succeed");
    }

    let packets = transport.packets();
    assert!(
        !packets.is_empty(),
        "randomized cooler sequence must emit at least one wire packet"
    );
    for pkt in packets {
        assert_eq!(CoolerPacketEncoder::validate_packet(&pkt), Ok(()));
        if pkt[1] == CMD_FAN && pkt[2] == 0x01 {
            assert!(pkt[3] <= 100, "emitted out-of-range fan speed {}", pkt[3]);
        }
        if pkt[1] == CMD_PUMP && pkt[2] == 0x01 {
            assert_ne!(pkt[4], PumpVoltage::V12 as u8, "emitted forbidden V12 code");
        }
    }
}

#[test]
fn bluez_cooler_discovery_and_selection_fuzz_invariants() {
    let mut rng = SplitMix64::new(0x0b10_e200_1234_9876);
    let seed_names: [&[u8]; 6] = [
        b"LCT21001",
        b"LCT22002",
        b"CoolingSystem",
        b"Aquaris-Pro",
        b"Nordic-UART-Watch",
        b"",
    ];

    // 2,000 mutated & random device names through detect_lct_model
    for i in 0..2_000 {
        let raw = if i < 1_200 {
            let base = seed_names[rng.gen_range(seed_names.len())];
            mutate_corpus_entry(base, &mut rng, 1024)
        } else {
            rng.gen_vec(256)
        };
        let name = String::from_utf8_lossy(&raw);
        let lower = name.to_ascii_lowercase();
        let expected = if lower.contains("lct22002") {
            Some(LctDeviceModel::Lct22002)
        } else if lower.contains("lct21001")
            || lower.contains("coolingsystem")
            || lower.contains("aquaris")
        {
            Some(LctDeviceModel::Lct21001)
        } else {
            None
        };
        assert_eq!(detect_lct_model(&name), expected);
    }

    // 500 deterministic candidate pools through select_cooler
    for _ in 0..500 {
        let count = rng.gen_range(6);
        let mut devices = Vec::with_capacity(count);
        for idx in 0..count {
            let model = if (rng.next_u8() & 1) == 0 {
                LctDeviceModel::Lct21001
            } else {
                LctDeviceModel::Lct22002
            };
            let name = match rng.gen_range(3) {
                0 => "LCT21001".to_string(),
                1 => "LCT22002".to_string(),
                _ => format!("Aquaris-{idx}"),
            };
            let addr = format!(
                "AA:BB:CC:DD:EE:{:02X}",
                rng.gen_range(8) // allow occasional duplicate/shared prefix patterns
            );
            devices.push(DiscoveredCooler {
                device_path: format!("/org/bluez/hci0/dev_{idx}"),
                address: addr,
                name,
                model,
                tx_char_path: None,
                rx_char_path: None,
                connected: false,
            });
        }

        let saved = SavedCoolerIdentity {
            address: if (rng.next_u8() & 1) == 0 {
                Some(format!("aa:bb:cc:dd:ee:{:02x}", rng.gen_range(10)))
            } else {
                None
            },
            name: if (rng.next_u8() & 1) == 0 {
                Some("lct21001".to_string())
            } else {
                None
            },
        };

        let chosen = select_cooler(&devices, Some(&saved));
        if let Some(dev) = chosen {
            assert!(devices.contains(dev));
        }

        // Unsaved selection must return Some iff devices.len() == 1
        let unsaved = select_cooler(&devices, None);
        assert_eq!(unsaved.is_some(), devices.len() == 1);
    }
}
