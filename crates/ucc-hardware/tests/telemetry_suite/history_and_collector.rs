// SPDX-License-Identifier: GPL-3.0-or-later

#![allow(unused_imports)]

use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};
use ucc_hardware::{
    decode_history_binary, dgpu_to_legacy_json, find_intel_gpu_rapl_path, igpu_to_legacy_json,
    nvidia_telemetry_due, read_amd_gpu_hwmon, read_intel_igpu_sysfs, CpuTelemetryReader, GpuVendor,
    HistoryCodecError, MetricId, MetricsHistoryStore, MockSysfs, NvidiaPowerGuard, NvmlWrapper,
    RaplEnergyCounter, TelemetryCollector, TelemetryConfig, MAX_POINTS_PER_SERIES,
};

#[allow(unused_imports)]
use super::common::*;

#[test]
fn telemetry_history_store_tracks_all_10_series_and_rejects_corrupt_binary_payloads() {
    let store = MetricsHistoryStore::new();
    // Horizon clamp 60..=7200
    store.set_horizon_seconds(10);
    assert_eq!(store.horizon_seconds(), 60);
    store.set_horizon_seconds(99_999);
    assert_eq!(store.horizon_seconds(), 7200);
    store.set_horizon_seconds(120);

    // Non-finite samples ignored
    store.push(MetricId::CpuTemp, 1_000, f64::NAN);
    store.push(MetricId::CpuTemp, 1_000, f64::INFINITY);

    // Push across all 10 series, triggering eviction on ts=200_000 (cutoff=80_000)
    for (idx, id) in MetricId::ALL.iter().enumerate() {
        store.push(*id, 10_000, 1.0);
        store.push(*id, 150_000, 10.0 + idx as f64);
        store.push(*id, 200_000, 20.0 + idx as f64);
    }

    let json: serde_json::Value = serde_json::from_str(&store.query_since_json(0)).unwrap();
    for (idx, id) in MetricId::ALL.iter().enumerate() {
        let arr = json[id.name()].as_array().unwrap();
        assert_eq!(
            arr.len(),
            2,
            "eviction should drop 10_000 point for {:?}",
            id
        );
        assert_eq!(arr[1][1].as_f64().unwrap(), 20.0 + idx as f64);
    }

    // Binary round-trip preserves exact series, timestamps, and values
    let bin = store.query_since_binary(160_000);
    let decoded = decode_history_binary(&bin).unwrap();
    assert_eq!(decoded.len(), MetricId::COUNT);
    for (idx, (id, pts)) in decoded.iter().enumerate() {
        assert_eq!(*id, MetricId::ALL[idx]);
        assert_eq!(pts.len(), 1);
        assert_eq!(pts[0].timestamp_ms, 200_000);
        assert_eq!(pts[0].value, 20.0 + idx as f64);
    }

    // Truncated header rejected
    assert_eq!(
        decode_history_binary(&[0, 1, 2]),
        Err(HistoryCodecError::TruncatedHeader)
    );
    // Unknown metric ID rejected
    assert_eq!(
        decode_history_binary(&[99, 1, 0, 0, 0]),
        Err(HistoryCodecError::UnknownMetricId(99))
    );
    // Truncated payload rejected
    let mut bad_payload = vec![0u8];
    bad_payload.extend_from_slice(&2u32.to_ne_bytes());
    bad_payload.extend_from_slice(&[0u8; 8]);
    assert!(matches!(
        decode_history_binary(&bad_payload),
        Err(HistoryCodecError::TruncatedPayload {
            metric_id: 0,
            expected_bytes: 32,
            remaining_bytes: 8
        })
    ));

    // Bounded capacity cap
    let cap_store = MetricsHistoryStore::new();
    cap_store.set_horizon_seconds(7200);
    for i in 0..(MAX_POINTS_PER_SERIES + 25) {
        cap_store.push(MetricId::GpuPower, i as i64, 15.0);
    }
    let decoded_cap = decode_history_binary(&cap_store.query_since_binary(0)).unwrap();
    assert_eq!(decoded_cap[0].1.len(), MAX_POINTS_PER_SERIES);
}

#[test]
fn reading_the_cache_does_not_create_a_fresh_history_point() {
    let tmp = tempfile::tempdir().unwrap();
    let pci = tmp.path().join("sys/bus/pci/devices");
    write_nvidia_pci_dev(&pci, "0000:01:00.0", "active", "auto");

    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extra-fixtures/nvml.c");
    let mock = tmp.path().join("nvml.so");
    assert!(Command::new("cc")
        .args(["-shared", "-fPIC", "-DAUDIT_WITH_BDF"])
        .arg(src)
        .arg("-o")
        .arg(&mock)
        .status()
        .unwrap()
        .success());
    let nvml = NvmlWrapper::with_library_path(NvidiaPowerGuard::new(&pci), mock).unwrap();
    let mut collector = TelemetryCollector::new(
        Arc::new(ucc_hardware::RealSysfs),
        TelemetryConfig {
            sys_root: tmp.path().to_path_buf(),
            nvml: Some(Arc::new(nvml)),
            ..Default::default()
        },
        Arc::new(MetricsHistoryStore::new()),
    );

    let t = Instant::now();
    let first_bundle = collector.poll(t, 1000);
    let first = first_bundle.dgpu.as_ref().unwrap();
    assert!(!first_bundle.dgpu_from_cache());

    // Cycle 1 with control=auto serves cached reading with original collected_at=t.
    let t1 = t + Duration::from_secs(1);
    let second_bundle = collector.poll(t1, 2000);
    let second = second_bundle.dgpu.as_ref().unwrap();
    assert_eq!(first.collected_at, second.collected_at);
    assert!(second_bundle.dgpu_from_cache());
    assert!(!second_bundle.is_dgpu_fresh(t1, Duration::from_millis(500)));
    assert!(!second_bundle.is_fresh(t1, Duration::from_millis(500)));

    let json = collector.history().query_since_json(0);
    let points: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(points["gpuTemp"].as_array().unwrap().len(), 1, "{json}");
    assert_eq!(points["gpuPower"].as_array().unwrap().len(), 1, "{json}");
}

#[test]
fn binary_and_json_since_queries_apply_same_filter_after_wall_clock_regresses() {
    let store = MetricsHistoryStore::new();
    for t in [1000, 3000, 2000] {
        store.push(MetricId::GpuPower, t, 8.0);
    }

    let json: serde_json::Value = serde_json::from_str(&store.query_since_json(2500)).unwrap();
    let from_json = json["gpuPower"]
        .as_array()
        .unwrap()
        .iter()
        .map(|point| point[0].as_f64().unwrap() as i64)
        .collect::<Vec<_>>();
    assert_eq!(from_json, [3000]);

    let binary = decode_history_binary(&store.query_since_binary(2500)).unwrap();
    let from_bin = binary
        .iter()
        .flat_map(|(_, pts)| pts.iter().map(|pt| pt.timestamp_ms))
        .collect::<Vec<_>>();
    assert_eq!(from_bin, from_json);

    let full_bin = decode_history_binary(&store.query_since_binary(0)).unwrap();
    let full_ts: Vec<i64> = full_bin[0].1.iter().map(|p| p.timestamp_ms).collect();
    assert_eq!(full_ts, [1000, 2000, 3000]);
}

#[test]
fn binary_history_rejects_duplicate_metric_records_and_non_finite_values() {
    let store = MetricsHistoryStore::new();
    store.push(MetricId::GpuPower, 1000, 8.0);
    let series = store.query_since_binary(0);
    let duplicate = [series.as_slice(), series.as_slice()].concat();
    assert!(matches!(
        decode_history_binary(&duplicate),
        Err(HistoryCodecError::DuplicateMetricId(6))
    ));

    // Reject NaN payload value
    let mut nan_payload = vec![MetricId::CpuTemp as u8];
    nan_payload.extend_from_slice(&1u32.to_ne_bytes());
    nan_payload.extend_from_slice(&1000i64.to_ne_bytes());
    nan_payload.extend_from_slice(&f64::NAN.to_ne_bytes());
    assert!(matches!(
        decode_history_binary(&nan_payload),
        Err(HistoryCodecError::NonFiniteValue {
            metric_id: 0,
            index: 0
        })
    ));
}
