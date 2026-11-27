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
fn nvidia_cadence_sleeping_isolation_and_no_gpu_machine_behavior() {
    let sysfs = MockSysfs::new();
    let pci_root = Path::new("/sys/bus/pci/devices");
    let dev = pci_root.join("0000:01:00.0");
    sysfs.set_file(dev.join("vendor"), "0x10de");
    sysfs.set_file(dev.join("class"), "0x030000");
    sysfs.set_file(dev.join("power/runtime_status"), "suspended");
    sysfs.set_file(dev.join("power/control"), "auto");

    // Suspended -> NEVER due on any cycle
    assert!(!nvidia_telemetry_due(&sysfs, 0, pci_root));
    assert!(!nvidia_telemetry_due(&sysfs, 20, pci_root));

    // Active + auto -> due only every 20 cycles
    sysfs.set_file(dev.join("power/runtime_status"), "active");
    assert!(nvidia_telemetry_due(&sysfs, 0, pci_root));
    assert!(!nvidia_telemetry_due(&sysfs, 1, pci_root));
    assert!(nvidia_telemetry_due(&sysfs, 20, pci_root));

    // Active + on -> due on every cycle
    sysfs.set_file(dev.join("power/control"), "on");
    assert!(nvidia_telemetry_due(&sysfs, 1, pci_root));

    // No-GPU machine: CPU-only TelemetryCollector runs cleanly with None GPUs
    let empty_sysfs = Arc::new(MockSysfs::new());
    empty_sysfs.set_file(
        "/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq",
        "2400000",
    );
    empty_sysfs.set_file("/sys/class/hwmon/hwmon0/name", "coretemp");
    empty_sysfs.set_file("/sys/class/hwmon/hwmon0/temp1_input", "55000");

    let history = Arc::new(MetricsHistoryStore::new());
    let mut collector =
        TelemetryCollector::new(empty_sysfs, TelemetryConfig::default(), history.clone());
    let t0 = Instant::now();
    let snap = collector.poll(t0, 10_000);
    assert!(snap.is_fresh(t0 + Duration::from_millis(500), Duration::from_secs(2)));
    assert!(!snap.is_fresh(t0 + Duration::from_secs(5), Duration::from_secs(2)));
    assert_eq!(snap.cpu.temp_c, Some(55.0));
    assert_eq!(snap.cpu.freq_mhz, Some(2400));
    assert!(snap.igpu.is_none());
    assert!(snap.dgpu.is_none());

    // Sleeping dGPU must NEVER borrow iGPU data and maps Missing -> -1.0 at IPC boundary
    let dir = tempfile::tempdir().unwrap();
    let pci_real = dir.path().join("sys/bus/pci/devices/0000:01:00.0");
    std::fs::create_dir_all(pci_real.join("power")).unwrap();
    std::fs::write(pci_real.join("vendor"), "0x10de").unwrap();
    std::fs::write(pci_real.join("class"), "0x030000").unwrap();
    std::fs::write(pci_real.join("power/runtime_status"), "suspended").unwrap();

    let igpu_card = dir.path().join("sys/class/drm/card1");
    std::fs::create_dir_all(igpu_card.join("device/hwmon/hwmon1")).unwrap();
    std::fs::write(igpu_card.join("gt_act_freq_mhz"), "900").unwrap();
    std::fs::write(igpu_card.join("gt_RP0_freq_mhz"), "2000").unwrap();
    std::fs::write(igpu_card.join("device/hwmon/hwmon1/temp1_input"), "48000").unwrap();

    let real_sysfs = Arc::new(ucc_hardware::RealSysfs);
    let mut hybrid = TelemetryCollector::new(
        real_sysfs,
        TelemetryConfig {
            sys_root: dir.path().to_path_buf(),
            intel_card_path: Some(igpu_card),
            ..Default::default()
        },
        Arc::new(MetricsHistoryStore::new()),
    );
    let hybrid_snap = hybrid.poll(Instant::now(), 20_000);
    let ig = hybrid_snap.igpu.unwrap();
    let dg = hybrid_snap.dgpu.unwrap();
    assert!(dg.is_sleeping);
    assert_eq!(dg.temp_c, None);
    assert_eq!(ig.temp_c, Some(48.0));
    assert_eq!(dgpu_to_legacy_json(&dg)["temp"], -1.0);
    assert_eq!(igpu_to_legacy_json(&ig)["temp"], 48.0);
}

#[test]
fn telemetry_collector_uses_option_nvml_snapshot_for_active_gpu() {
    let tmp = tempfile::tempdir().unwrap();
    let pci_root = tmp.path().join("sys/bus/pci/devices");
    let dev = pci_root.join("0000:01:00.0");
    std::fs::create_dir_all(dev.join("power")).unwrap();
    std::fs::write(dev.join("vendor"), "0x10de").unwrap();
    std::fs::write(dev.join("class"), "0x030000").unwrap();
    std::fs::write(dev.join("power/runtime_status"), "active").unwrap();
    std::fs::write(dev.join("power/control"), "on").unwrap();

    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extra-fixtures/nvml.c");
    let text = std::fs::read_to_string(src)
        .unwrap()
        .replace("*out = 10000; return 0;", "(void)out; return 999;");
    let c_path = tmp.path().join("nvml.c");
    let so_path = tmp.path().join("nvml.so");
    std::fs::write(&c_path, text).unwrap();
    assert!(Command::new("cc")
        .args(["-shared", "-fPIC", "-DAUDIT_WITH_BDF"])
        .arg(&c_path)
        .arg("-o")
        .arg(&so_path)
        .status()
        .unwrap()
        .success());

    let nvml = Arc::new(
        NvmlWrapper::with_library_path(NvidiaPowerGuard::new(&pci_root), so_path).unwrap(),
    );
    let history = Arc::new(MetricsHistoryStore::new());
    let mut collector = TelemetryCollector::new(
        Arc::new(ucc_hardware::RealSysfs),
        TelemetryConfig {
            sys_root: tmp.path().to_path_buf(),
            nvml: Some(nvml),
            ..Default::default()
        },
        history.clone(),
    );

    let snap = collector.poll(Instant::now(), 50_000);
    let dgpu = snap.dgpu.unwrap();
    assert!(!dgpu.is_sleeping);
    assert_eq!(dgpu.temp_c, Some(42.0));
    assert_eq!(dgpu.power_w, None); // failed getter stays None, NEVER 0.0
    assert_eq!(dgpu_to_legacy_json(&dgpu)["powerDraw"], -1.0);
    assert_eq!(
        CpuTelemetryReader::new(tmp.path()).read_average_freq_mhz(&MockSysfs::new()),
        None
    );
}
#[test]
fn cached_nvidia_snapshot_is_not_reused_for_a_different_bdf() {
    let tmp = tempfile::tempdir().unwrap();
    let pci = tmp.path().join("sys/bus/pci/devices");
    write_nvidia_pci_dev(&pci, "0000:01:00.0", "active", "auto");
    write_nvidia_pci_dev(&pci, "0000:02:00.0", "active", "auto");

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
    let first = collector.poll(t, 1000).dgpu.unwrap();
    assert_eq!(first.bdf.as_deref(), Some("0000:01:00.0"));
    assert!(!first.from_cache);

    // 0000:01:00.0 enters D3cold; 0000:02:00.0 remains active -> NEVER reuse 01:00.0 cache.
    std::fs::write(pci.join("0000:01:00.0/power/runtime_status"), "suspended").unwrap();
    let second = collector.poll(t + Duration::from_secs(1), 2000).dgpu;
    assert!(
        second.is_none() || second.as_ref().unwrap().bdf.as_deref() == Some("0000:02:00.0"),
        "cross-BDF cache leak: {second:?}"
    );
}

#[test]
fn legacy_dgpu_json_contract_and_extended_nvml_nvapi_metrics() {
    // 1. Golden contract check for unavailable/sleeping dGPU sentinels
    let sleeping = dgpu_to_legacy_json(&ucc_hardware::DGpuSnapshot::sleeping_nvidia(
        None,
        Instant::now(),
    ));
    for (key, expected) in [
        ("perfLimitReason", serde_json::json!("")),
        ("encoderUtilPct", serde_json::json!(-1)),
        ("decoderUtilPct", serde_json::json!(-1)),
        ("currentPstate", serde_json::json!(-1)),
        ("grClockOffsetMHz", serde_json::json!(-999)),
        ("memClockOffsetMHz", serde_json::json!(-999)),
        ("coreVoltageMv", serde_json::json!(-1)),
        ("vramUsedMiB", serde_json::json!(-1)),
        ("vramTotalMiB", serde_json::json!(-1)),
    ] {
        assert_eq!(sleeping.get(key), Some(&expected), "mismatch for {key}");
    }

    // 2. Extended NVML + NvAPI mock libraries populate all GPU history series & legacy JSON
    let tmp = tempfile::tempdir().unwrap();
    let pci = tmp.path().join("sys/bus/pci/devices");
    write_nvidia_pci_dev(&pci, "0000:01:00.0", "active", "on");

    let ext_nvml_c = tmp.path().join("ext_nvml.c");
    let ext_nvml_so = tmp.path().join("ext_nvml.so");
    std::fs::write(
        &ext_nvml_c,
        r#"
        #include <string.h>
        #include <stdint.h>
        typedef struct { unsigned version; unsigned type; unsigned pstate; int offset; int min_off; int max_off; } NvmlClockOffset;
        unsigned nvmlInit_v2(void) { return 0; }
        unsigned nvmlShutdown(void) { return 0; }
        unsigned nvmlDeviceGetCount_v2(unsigned *out) { *out = 1; return 0; }
        unsigned nvmlDeviceGetHandleByIndex_v2(unsigned i, void **out) { static int d; *out = &d; return 0; }
        unsigned nvmlDeviceGetHandleByPciBusId_v2(const char *b, void **out) { static int d; *out = &d; return 0; }
        unsigned nvmlDeviceGetName(void *d, char *out, unsigned n) { strcpy(out, "RTX Mock"); return 0; }
        unsigned nvmlDeviceGetTemperature(void *d, unsigned s, unsigned *out) { *out = 61; return 0; }
        unsigned nvmlDeviceGetPowerUsage(void *d, unsigned *out) { *out = 42500; return 0; }
        unsigned nvmlDeviceGetEnforcedPowerLimit(void *d, unsigned *out) { *out = 115000; return 0; }
        unsigned nvmlDeviceGetPowerManagementLimitConstraints(void *d, unsigned *mn, unsigned *mx) { *mn = 30000; *mx = 140000; return 0; }
        unsigned nvmlDeviceSetPowerManagementLimit(void *d, unsigned l) { return 0; }
        unsigned nvmlDeviceGetUtilizationRates(void *d, unsigned *out) { out[0] = 77; out[1] = 35; return 0; }
        typedef struct { unsigned int version; unsigned long long total, reserved, free, used; } MemoryV2;
        unsigned nvmlDeviceGetMemoryInfo_v2(void *d, MemoryV2 *out) {
            if (out->version != (unsigned int)(sizeof(MemoryV2) | (2U << 24))) return 25;
            out->total = 8589934592ULL; out->reserved = 0ULL; out->free = 4294967296ULL; out->used = 4294967296ULL;
            return 0;
        }
        unsigned nvmlDeviceGetClockInfo(void *d, unsigned type, unsigned *out) { *out = (type == 0) ? 1950 : 10501; return 0; }
        unsigned nvmlDeviceGetMaxClockInfo(void *d, unsigned type, unsigned *out) { *out = 2550; return 0; }
        unsigned nvmlDeviceGetPerformanceState(void *d, unsigned *out) { *out = 0; return 0; }
        unsigned nvmlDeviceGetCurrentClocksThrottleReasons(void *d, unsigned long long *out) { *out = 0x4ULL; return 0; }
        unsigned nvmlDeviceGetEncoderUtilization(void *d, unsigned *u, unsigned *s) { *u = 12; *s = 1000; return 0; }
        unsigned nvmlDeviceGetDecoderUtilization(void *d, unsigned *u, unsigned *s) { *u = 5; *s = 1000; return 0; }
        unsigned nvmlDeviceGetClockOffsets(void *d, NvmlClockOffset *info) {
            info->offset = (info->type == 0) ? 120 : 400;
            return 0;
        }
        "#,
    )
    .unwrap();

    let mock_nvapi_c = tmp.path().join("mock_nvapi.c");
    let mock_nvapi_so = tmp.path().join("mock_nvapi.so");
    std::fs::write(
        &mock_nvapi_c,
        r#"
        #include <stdint.h>
        typedef struct { uint32_t version; uint32_t flags; uint32_t pad[8]; uint32_t valueUv; uint32_t pad2[8]; } NvApiVoltage;
        static int fake_gpu;
        int32_t init_fn(void) { return 0; }
        int32_t unload_fn(void) { return 0; }
        int32_t enum_fn(void *handles[64], uint32_t *count) { handles[0] = &fake_gpu; *count = 1; return 0; }
        int32_t volt_fn(void *h, NvApiVoltage *v) { v->valueUv = 925000; return 0; }
        void *nvapi_QueryInterface(uint32_t id) {
            if (id == 0x0150E828U) return (void*)init_fn;
            if (id == 0xD22BDD7EU) return (void*)unload_fn;
            if (id == 0xE5AC921FU) return (void*)enum_fn;
            if (id == 0x465F9BCFU) return (void*)volt_fn;
            return 0;
        }
        "#,
    )
    .unwrap();

    assert!(Command::new("cc")
        .args(["-shared", "-fPIC"])
        .arg(&ext_nvml_c)
        .arg("-o")
        .arg(&ext_nvml_so)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("cc")
        .args(["-shared", "-fPIC"])
        .arg(&mock_nvapi_c)
        .arg("-o")
        .arg(&mock_nvapi_so)
        .status()
        .unwrap()
        .success());

    let nvml = NvmlWrapper::with_library_paths(
        NvidiaPowerGuard::new(&pci),
        ext_nvml_so,
        Some(&mock_nvapi_so),
    )
    .unwrap();
    let history = Arc::new(MetricsHistoryStore::new());
    let mut collector = TelemetryCollector::new(
        Arc::new(ucc_hardware::RealSysfs),
        TelemetryConfig {
            sys_root: tmp.path().to_path_buf(),
            nvml: Some(Arc::new(nvml)),
            ..Default::default()
        },
        history.clone(),
    );

    let snap = collector.poll(Instant::now(), 10_000);
    let dgpu = snap.dgpu.as_ref().unwrap();
    assert_eq!(dgpu.core_freq_mhz, Some(1950.0));
    assert_eq!(dgpu.max_core_freq_mhz, Some(2550.0));
    assert_eq!(dgpu.vram_freq_mhz, Some(10501.0));
    assert_eq!(dgpu.perf_limit_reason.as_deref(), Some("Power Limit"));
    assert_eq!(dgpu.encoder_util_pct, Some(12));
    assert_eq!(dgpu.decoder_util_pct, Some(5));
    assert_eq!(dgpu.current_pstate, Some(0));
    assert_eq!(dgpu.gr_clock_offset_mhz, Some(120));
    assert_eq!(dgpu.mem_clock_offset_mhz, Some(400));
    assert_eq!(dgpu.core_voltage_mv, Some(925));

    let hist_json: serde_json::Value = serde_json::from_str(&history.query_since_json(0)).unwrap();
    assert_eq!(hist_json["gpuFrequency"][0][1], 1950.0);
    assert_eq!(hist_json["gpuVramFrequency"][0][1], 10501.0);
    assert_eq!(hist_json["gpuCoreVoltage"][0][1], 925.0);

    let legacy = dgpu_to_legacy_json(dgpu);
    assert_eq!(legacy["perfLimitReason"], "Power Limit");
    assert_eq!(legacy["encoderUtilPct"], 12);
    assert_eq!(legacy["decoderUtilPct"], 5);
    assert_eq!(legacy["currentPstate"], 0);
    assert_eq!(legacy["grClockOffsetMHz"], 120);
    assert_eq!(legacy["memClockOffsetMHz"], 400);
    assert_eq!(legacy["coreVoltageMv"], 925);
}

#[test]
fn nvml_memory_abi_handles_v1_only_v2_only_and_both_symbols() {
    let base_src = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extra-fixtures/nvml.c"),
    )
    .unwrap();

    // 1. v2-only validating NVIDIA MemoryV2 version macro
    let tmp = tempfile::tempdir().unwrap();
    let pci = tmp.path().join("pci");
    write_nvidia_pci_dev(&pci, "0000:01:00.0", "active", "on");

    let v2_only_c = tmp.path().join("v2_only.c");
    let v2_only_so = tmp.path().join("v2_only.so");
    std::fs::write(
        &v2_only_c,
        base_src.replace("nvmlDeviceGetMemoryInfo(", "unused_memory_v1(")
            + r#"
            typedef struct { unsigned int version; unsigned long long total, reserved, free, used; } MemoryV2;
            unsigned nvmlDeviceGetMemoryInfo_v2(void *device, MemoryV2 *out) {
                (void)device;
                if (out->version != (unsigned int)(sizeof(MemoryV2) | (2U << 24))) return 25;
                out->total = 8192ULL * 1024 * 1024;
                out->reserved = 128ULL * 1024 * 1024;
                out->free = 4096ULL * 1024 * 1024;
                out->used = 3968ULL * 1024 * 1024;
                return 0;
            }
            "#,
    )
    .unwrap();
    assert!(Command::new("cc")
        .args(["-shared", "-fPIC", "-DAUDIT_WITH_BDF"])
        .arg(&v2_only_c)
        .arg("-o")
        .arg(&v2_only_so)
        .status()
        .unwrap()
        .success());
    let nvml_v2 = NvmlWrapper::with_library_path(NvidiaPowerGuard::new(&pci), v2_only_so).unwrap();
    let s_v2 = nvml_v2
        .get_telemetry_snapshot_by_bdf("0000:01:00.0")
        .unwrap();
    assert_eq!(s_v2.memory.map(|m| m.total / (1024 * 1024)), Some(8192));
    assert_eq!(s_v2.memory.map(|m| m.used / (1024 * 1024)), Some(3968));

    // 2. both v1 & v2 present (v2 returns NOT_SUPPORTED -> falls back cleanly to v1)
    let both_c = tmp.path().join("both.c");
    let both_so = tmp.path().join("both.so");
    std::fs::write(
        &both_c,
        base_src
            + r#"
            typedef struct { unsigned int version; unsigned long long total, reserved, free, used; } MemoryV2;
            unsigned nvmlDeviceGetMemoryInfo_v2(void *device, MemoryV2 *out) {
                (void)device;
                if (out->version != (unsigned int)(sizeof(MemoryV2) | (2U << 24))) return 25;
                return 3;
            }
            "#,
    )
    .unwrap();
    assert!(Command::new("cc")
        .args(["-shared", "-fPIC", "-DAUDIT_WITH_BDF"])
        .arg(&both_c)
        .arg("-o")
        .arg(&both_so)
        .status()
        .unwrap()
        .success());
    let nvml_both = NvmlWrapper::with_library_path(NvidiaPowerGuard::new(&pci), both_so).unwrap();
    let s_both = nvml_both
        .get_telemetry_snapshot_by_bdf("0000:01:00.0")
        .unwrap();
    assert_eq!(s_both.memory.map(|m| m.total), Some(1024));
}

#[test]
fn nvapi_voltage_binding_rejects_bdf_replacement_and_topology_drift() {
    let tmp = tempfile::tempdir().unwrap();
    let pci = tmp.path().join("sys/bus/pci/devices");
    write_nvidia_pci_dev(&pci, "0000:01:00.0", "active", "on");

    let base_nvml = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extra-fixtures/nvml.c"),
    )
    .unwrap();
    let nvml_c = tmp.path().join("nvml.c");
    let nvml_so = tmp.path().join("nvml.so");
    std::fs::write(&nvml_c, base_nvml).unwrap();

    let nvapi_c = tmp.path().join("nvapi.c");
    let nvapi_so = tmp.path().join("nvapi.so");
    std::fs::write(
        &nvapi_c,
        r#"
        #include <stdint.h>
        typedef struct { uint32_t version; uint32_t flags; uint32_t pad[8]; uint32_t valueUv; uint32_t pad2[8]; } NvApiVoltage;
        static int fake_gpu;
        int32_t init_fn(void) { return 0; }
        int32_t unload_fn(void) { return 0; }
        int32_t enum_fn(void *handles[64], uint32_t *count) { handles[0] = &fake_gpu; *count = 1; return 0; }
        int32_t volt_fn(void *h, NvApiVoltage *v) { v->valueUv = 925000; return 0; }
        void *nvapi_QueryInterface(uint32_t id) {
            if (id == 0x0150E828U) return (void*)init_fn;
            if (id == 0xD22BDD7EU) return (void*)unload_fn;
            if (id == 0xE5AC921FU) return (void*)enum_fn;
            if (id == 0x465F9BCFU) return (void*)volt_fn;
            return 0;
        }
        "#,
    )
    .unwrap();

    assert!(Command::new("cc")
        .args(["-shared", "-fPIC", "-DAUDIT_WITH_BDF"])
        .arg(&nvml_c)
        .arg("-o")
        .arg(&nvml_so)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("cc")
        .args(["-shared", "-fPIC"])
        .arg(&nvapi_c)
        .arg("-o")
        .arg(&nvapi_so)
        .status()
        .unwrap()
        .success());

    // 1. one -> one BDF replacement returns None
    let nvml =
        NvmlWrapper::with_library_paths(NvidiaPowerGuard::new(&pci), &nvml_so, Some(&nvapi_so))
            .unwrap();
    assert_eq!(
        nvml.get_telemetry_snapshot_by_bdf("0000:01:00.0")
            .unwrap()
            .core_voltage_mv,
        Some(925)
    );
    std::fs::remove_dir_all(pci.join("0000:01:00.0")).unwrap();
    write_nvidia_pci_dev(&pci, "0000:02:00.0", "active", "on");
    assert_eq!(
        nvml.get_telemetry_snapshot_by_bdf("0000:02:00.0")
            .unwrap()
            .core_voltage_mv,
        None
    );

    // 2. one -> many -> one never reuses unverified handle
    let pci2 = tmp.path().join("pci2");
    write_nvidia_pci_dev(&pci2, "0000:01:00.0", "active", "on");
    let nvml2 =
        NvmlWrapper::with_library_paths(NvidiaPowerGuard::new(&pci2), &nvml_so, Some(&nvapi_so))
            .unwrap();
    assert_eq!(
        nvml2
            .get_telemetry_snapshot_by_bdf("0000:01:00.0")
            .unwrap()
            .core_voltage_mv,
        Some(925)
    );
    write_nvidia_pci_dev(&pci2, "0000:02:00.0", "active", "on");
    assert_eq!(
        nvml2
            .get_telemetry_snapshot_by_bdf("0000:01:00.0")
            .unwrap()
            .core_voltage_mv,
        None
    );
    std::fs::remove_dir_all(pci2.join("0000:02:00.0")).unwrap();
    assert_eq!(
        nvml2
            .get_telemetry_snapshot_by_bdf("0000:01:00.0")
            .unwrap()
            .core_voltage_mv,
        None
    );

    // 3. many -> one also returns None without NvAPI PCI verification
    let pci3 = tmp.path().join("pci3");
    write_nvidia_pci_dev(&pci3, "0000:01:00.0", "active", "on");
    write_nvidia_pci_dev(&pci3, "0000:02:00.0", "active", "on");
    let nvml3 =
        NvmlWrapper::with_library_paths(NvidiaPowerGuard::new(&pci3), &nvml_so, Some(&nvapi_so))
            .unwrap();
    std::fs::remove_dir_all(pci3.join("0000:01:00.0")).unwrap();
    assert_eq!(
        nvml3
            .get_telemetry_snapshot_by_bdf("0000:02:00.0")
            .unwrap()
            .core_voltage_mv,
        None
    );
}

#[test]
fn nvapi_singleton_drift_before_first_sample_and_domain_collisions_fail_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let base_nvml = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extra-fixtures/nvml.c"),
    )
    .unwrap();
    let nvml_c = tmp.path().join("nvml.c");
    let nvml_so = tmp.path().join("nvml.so");
    std::fs::write(&nvml_c, base_nvml).unwrap();

    let nvapi_c = tmp.path().join("nvapi_bus.c");
    let nvapi_so = tmp.path().join("nvapi_bus.so");
    std::fs::write(
        &nvapi_c,
        r#"
        #include <stdint.h>
        typedef struct { uint32_t version; uint32_t flags; uint32_t pad[8]; uint32_t valueUv; uint32_t pad2[8]; } NvApiVoltage;
        static int fake_gpu;
        int32_t init_fn(void) { return 0; }
        int32_t unload_fn(void) { return 0; }
        int32_t enum_fn(void *handles[64], uint32_t *count) { handles[0] = &fake_gpu; *count = 1; return 0; }
        int32_t bus_fn(void *h, uint32_t *v) { *v = 1; return 0; }
        int32_t slot_fn(void *h, uint32_t *v) { *v = 0; return 0; }
        int32_t volt_fn(void *h, NvApiVoltage *v) { v->valueUv = 925000; return 0; }
        void *nvapi_QueryInterface(uint32_t id) {
            if (id == 0x0150E828U) return (void*)init_fn;
            if (id == 0xD22BDD7EU) return (void*)unload_fn;
            if (id == 0xE5AC921FU) return (void*)enum_fn;
            if (id == 0x1BE0B8E5U) return (void*)bus_fn;
            if (id == 0x2A0A350FU) return (void*)slot_fn;
            if (id == 0x465F9BCFU) return (void*)volt_fn;
            return 0;
        }
        "#,
    )
    .unwrap();

    let nvapi_nobus_c = tmp.path().join("nvapi_nobus.c");
    let nvapi_nobus_so = tmp.path().join("nvapi_nobus.so");
    std::fs::write(
        &nvapi_nobus_c,
        r#"
        #include <stdint.h>
        typedef struct { uint32_t version; uint32_t flags; uint32_t pad[8]; uint32_t valueUv; uint32_t pad2[8]; } NvApiVoltage;
        static int fake_gpu;
        int32_t init_fn(void) { return 0; }
        int32_t unload_fn(void) { return 0; }
        int32_t enum_fn(void *handles[64], uint32_t *count) { handles[0] = &fake_gpu; *count = 1; return 0; }
        int32_t volt_fn(void *h, NvApiVoltage *v) { v->valueUv = 925000; return 0; }
        void *nvapi_QueryInterface(uint32_t id) {
            if (id == 0x0150E828U) return (void*)init_fn;
            if (id == 0xD22BDD7EU) return (void*)unload_fn;
            if (id == 0xE5AC921FU) return (void*)enum_fn;
            if (id == 0x465F9BCFU) return (void*)volt_fn;
            return 0;
        }
        "#,
    )
    .unwrap();

    for (src, dst, bdf) in [
        (&nvml_c, &nvml_so, true),
        (&nvapi_c, &nvapi_so, false),
        (&nvapi_nobus_c, &nvapi_nobus_so, false),
    ] {
        let mut cmd = Command::new("cc");
        cmd.args(["-shared", "-fPIC"]);
        if bdf {
            cmd.arg("-DAUDIT_WITH_BDF");
        }
        assert!(cmd.arg(src).arg("-o").arg(dst).status().unwrap().success());
    }

    // 1. Singleton drift BEFORE the very first sample -> None
    let pci_drift = tmp.path().join("pci_drift");
    write_nvidia_pci_dev(&pci_drift, "0000:01:00.0", "active", "on");
    let nvml_drift = NvmlWrapper::with_library_paths(
        NvidiaPowerGuard::new(&pci_drift),
        &nvml_so,
        Some(&nvapi_nobus_so),
    )
    .unwrap();
    std::fs::remove_dir_all(pci_drift.join("0000:01:00.0")).unwrap();
    write_nvidia_pci_dev(&pci_drift, "0000:02:00.0", "active", "on");
    assert_eq!(
        nvml_drift
            .get_telemetry_snapshot_by_bdf("0000:02:00.0")
            .unwrap()
            .core_voltage_mv,
        None
    );

    // 2. Equal (bus, slot) in different PCI domains -> None
    let pci_dom = tmp.path().join("pci_dom");
    write_nvidia_pci_dev(&pci_dom, "0000:01:00.0", "active", "on");
    write_nvidia_pci_dev(&pci_dom, "0001:01:00.0", "active", "on");
    let nvml_dom =
        NvmlWrapper::with_library_paths(NvidiaPowerGuard::new(&pci_dom), &nvml_so, Some(&nvapi_so))
            .unwrap();
    assert_eq!(
        nvml_dom
            .get_telemetry_snapshot_by_bdf("0001:01:00.0")
            .unwrap()
            .core_voltage_mv,
        None
    );
}
