//! Bounded 10-series metrics history ring buffer and native-endian binary codec (`M7.6`).
//!
//! Wire format matches `uccd/inc/MetricsHistoryStore.hpp` for full D-Bus compatibility:
//! per non-empty series: `u8 metric_id`, `u32 count`, `count * (i64 timestamp_ms, f64 value)`.

use std::collections::VecDeque;
use std::sync::RwLock;
use thiserror::Error;

/// Maximum per-series count accepted by the binary decoder to guard against OOM on corrupt payloads.
const MAX_DECODE_POINTS_PER_SERIES: usize = 1_000_000;

/// Maximum total points across all series accepted in one binary decode call.
const MAX_DECODE_TOTAL_POINTS: usize = 1_000_000;

/// Maximum bounded sample count retained in memory per series.
pub const MAX_POINTS_PER_SERIES: usize = 10_000;

/// Identifiers for the 10 hardware monitor time series matching `MetricId` in C++.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MetricId {
    CpuTemp = 0,
    CpuFanDuty = 1,
    CpuPower = 2,
    CpuFrequency = 3,
    GpuTemp = 4,
    GpuFanDuty = 5,
    GpuPower = 6,
    GpuFrequency = 7,
    GpuVramFrequency = 8,
    GpuCoreVoltage = 9,
}

impl MetricId {
    pub const COUNT: usize = 10;

    pub const ALL: [Self; Self::COUNT] = [
        Self::CpuTemp,
        Self::CpuFanDuty,
        Self::CpuPower,
        Self::CpuFrequency,
        Self::GpuTemp,
        Self::GpuFanDuty,
        Self::GpuPower,
        Self::GpuFrequency,
        Self::GpuVramFrequency,
        Self::GpuCoreVoltage,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::CpuTemp => "cpuTemp",
            Self::CpuFanDuty => "cpuFanDuty",
            Self::CpuPower => "cpuPower",
            Self::CpuFrequency => "cpuFrequency",
            Self::GpuTemp => "gpuTemp",
            Self::GpuFanDuty => "gpuFanDuty",
            Self::GpuPower => "gpuPower",
            Self::GpuFrequency => "gpuFrequency",
            Self::GpuVramFrequency => "gpuVramFrequency",
            Self::GpuCoreVoltage => "gpuCoreVoltage",
        }
    }
}

impl TryFrom<u8> for MetricId {
    type Error = HistoryCodecError;

    fn try_from(val: u8) -> Result<Self, Self::Error> {
        match val {
            0 => Ok(Self::CpuTemp),
            1 => Ok(Self::CpuFanDuty),
            2 => Ok(Self::CpuPower),
            3 => Ok(Self::CpuFrequency),
            4 => Ok(Self::GpuTemp),
            5 => Ok(Self::GpuFanDuty),
            6 => Ok(Self::GpuPower),
            7 => Ok(Self::GpuFrequency),
            8 => Ok(Self::GpuVramFrequency),
            9 => Ok(Self::GpuCoreVoltage),
            other => Err(HistoryCodecError::UnknownMetricId(other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MetricDataPoint {
    pub timestamp_ms: i64,
    pub value: f64,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum HistoryCodecError {
    #[error("Unknown metric ID in history payload: {0}")]
    UnknownMetricId(u8),
    #[error("Duplicate metric ID in history payload: {0}")]
    DuplicateMetricId(u8),
    #[error("Truncated history block header")]
    TruncatedHeader,
    #[error("Truncated payload for metric {metric_id}: expected {expected_bytes} bytes, got {remaining_bytes}")]
    TruncatedPayload {
        metric_id: u8,
        expected_bytes: usize,
        remaining_bytes: usize,
    },
    #[error("Sample count overflow or exceeds bound: {0}")]
    CountOverflow(u32),
    #[error("Non-finite floating point value in history payload for metric {metric_id} at index {index}")]
    NonFiniteValue { metric_id: u8, index: usize },
}

struct HistoryInner {
    buffers: [VecDeque<MetricDataPoint>; MetricId::COUNT],
    horizon_ms: i64,
}

/// Thread-safe bounded history store for all 10 hardware metrics (`M7.6`).
pub struct MetricsHistoryStore {
    inner: RwLock<HistoryInner>,
}

impl Default for MetricsHistoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsHistoryStore {
    pub const DEFAULT_HORIZON_S: i32 = 1800;
    pub const MIN_HORIZON_S: i32 = 60;
    pub const MAX_HORIZON_S: i32 = 7200;

    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HistoryInner {
                buffers: std::array::from_fn(|_| VecDeque::new()),
                horizon_ms: i64::from(Self::DEFAULT_HORIZON_S) * 1000,
            }),
        }
    }

    pub fn set_horizon_seconds(&self, seconds: i32) {
        let clamped = seconds.clamp(Self::MIN_HORIZON_S, Self::MAX_HORIZON_S);
        let mut inner = self.inner.write().unwrap();
        inner.horizon_ms = i64::from(clamped) * 1000;
    }

    pub fn horizon_seconds(&self) -> i32 {
        (self.inner.read().unwrap().horizon_ms / 1000) as i32
    }

    /// Pushes a finite sample maintaining chronological order by `timestamp_ms`, and evicts
    /// points older than `(newest_timestamp_ms - horizon_ms)` or exceeding [`MAX_POINTS_PER_SERIES`].
    /// Non-finite values are silently dropped.
    pub fn push(&self, id: MetricId, timestamp_ms: i64, value: f64) {
        if !value.is_finite() {
            return;
        }
        let mut inner = self.inner.write().unwrap();
        let horizon_ms = inner.horizon_ms;
        let buf = &mut inner.buffers[id as usize];
        let pt = MetricDataPoint {
            timestamp_ms,
            value,
        };
        if buf.back().is_none_or(|last| last.timestamp_ms <= timestamp_ms) {
            buf.push_back(pt);
        } else {
            let idx = buf.partition_point(|existing| existing.timestamp_ms <= timestamp_ms);
            buf.insert(idx, pt);
        }
        let newest_ts = buf.back().map_or(timestamp_ms, |p| p.timestamp_ms);
        let cutoff = newest_ts.saturating_sub(horizon_ms);
        while buf.front().is_some_and(|p| p.timestamp_ms < cutoff) {
            buf.pop_front();
        }
        while buf.len() > MAX_POINTS_PER_SERIES {
            buf.pop_front();
        }
    }

    /// Serializes points with `timestamp_ms >= since_ms` matching C++ `querySinceJSON`.
    pub fn query_since_json(&self, since_ms: i64) -> String {
        let inner = self.inner.read().unwrap();
        let mut map = serde_json::Map::new();
        for id in MetricId::ALL {
            let buf = &inner.buffers[id as usize];
            if buf.is_empty() {
                continue;
            }
            let points: Vec<[f64; 2]> = buf
                .iter()
                .filter(|pt| pt.timestamp_ms >= since_ms)
                .map(|pt| [pt.timestamp_ms as f64, pt.value])
                .collect();
            if !points.is_empty() {
                map.insert(id.name().to_string(), serde_json::json!(points));
            }
        }
        serde_json::Value::Object(map).to_string()
    }

    /// Serializes points with `timestamp_ms >= since_ms` into C++ `MetricsHistoryStore` binary format.
    pub fn query_since_binary(&self, since_ms: i64) -> Vec<u8> {
        let inner = self.inner.read().unwrap();
        let mut out = Vec::with_capacity(2048);

        for id in MetricId::ALL {
            let buf = &inner.buffers[id as usize];
            if buf.is_empty() {
                continue;
            }
            let slice_start = buf.partition_point(|pt| pt.timestamp_ms < since_ms);
            let count = buf.len().saturating_sub(slice_start);
            if count == 0 {
                continue;
            }

            out.push(id as u8);
            out.extend_from_slice(&(count as u32).to_ne_bytes());
            for pt in buf.iter().skip(slice_start) {
                out.extend_from_slice(&pt.timestamp_ms.to_ne_bytes());
                out.extend_from_slice(&pt.value.to_ne_bytes());
            }
        }

        out
    }
}

/// Fail-closed decoder for `MetricsHistoryStore::query_since_binary` payloads (`M7.6`).
pub fn decode_history_binary(
    bytes: &[u8],
) -> Result<Vec<(MetricId, Vec<MetricDataPoint>)>, HistoryCodecError> {
    let mut series = Vec::new();
    let mut seen = [false; MetricId::COUNT];
    let mut total_points = 0usize;
    let mut offset = 0usize;

    while offset < bytes.len() {
        if bytes.len() - offset < 5 {
            return Err(HistoryCodecError::TruncatedHeader);
        }
        let raw_id = bytes[offset];
        let metric_id = MetricId::try_from(raw_id)?;
        if seen[metric_id as usize] {
            return Err(HistoryCodecError::DuplicateMetricId(raw_id));
        }
        seen[metric_id as usize] = true;

        let count_bytes: [u8; 4] = bytes[offset + 1..offset + 5]
            .try_into()
            .map_err(|_| HistoryCodecError::TruncatedHeader)?;
        let count_u32 = u32::from_ne_bytes(count_bytes);
        offset += 5;

        let count = count_u32 as usize;
        if count > MAX_DECODE_POINTS_PER_SERIES
            || total_points
                .checked_add(count)
                .is_none_or(|t| t > MAX_DECODE_TOTAL_POINTS)
        {
            return Err(HistoryCodecError::CountOverflow(count_u32));
        }
        total_points += count;

        let expected_bytes = count
            .checked_mul(16)
            .ok_or(HistoryCodecError::CountOverflow(count_u32))?;
        let remaining = bytes.len() - offset;
        if remaining < expected_bytes {
            return Err(HistoryCodecError::TruncatedPayload {
                metric_id: raw_id,
                expected_bytes,
                remaining_bytes: remaining,
            });
        }

        let mut points = Vec::with_capacity(count);
        for idx in 0..count {
            let ts = i64::from_ne_bytes(bytes[offset..offset + 8].try_into().unwrap());
            let val = f64::from_ne_bytes(bytes[offset + 8..offset + 16].try_into().unwrap());
            offset += 16;
            if !val.is_finite() {
                return Err(HistoryCodecError::NonFiniteValue {
                    metric_id: raw_id,
                    index: idx,
                });
            }
            points.push(MetricDataPoint {
                timestamp_ms: ts,
                value: val,
            });
        }
        series.push((metric_id, points));
    }

    Ok(series)
}
