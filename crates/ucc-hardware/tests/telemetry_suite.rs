use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};
use ucc_hardware::{
    decode_history_binary, dgpu_to_legacy_json, find_intel_gpu_rapl_path, igpu_to_legacy_json,
    nvidia_telemetry_due, read_amd_gpu_hwmon, read_intel_igpu_sysfs, CpuTelemetryReader,
    GpuVendor, HistoryCodecError, MetricId, MetricsHistoryStore, MockSysfs, NvidiaPowerGuard,
    NvmlWrapper, RaplEnergyCounter, TelemetryCollector, TelemetryConfig, MAX_POINTS_PER_SERIES,
};

#[test]
fn rapl_handles_normal_delta_wraparound_zero_dt_and_constraints() {
    let sysfs = MockSysfs::new();
    let domain = Path::new("/sys/class/powercap/intel-rapl:0");
    sysfs.set_file(domain.join("name"), "package-0\n");
    sysfs.set_file(domain.join("max_energy_range_uj"), "10000000\n"); // 10 J range
    sysfs.set_file(domain.join("constraint_0_power_limit_uw"), "45000000\n"); // 45 W

    let mut rapl = RaplEnergyCounter::new(domain);
    let t0 = Instant::now();

    // First sample initializes baseline and returns None
    sysfs.set_file(domain.join("energy_uj"), "8000000\n");
    assert_eq!(rapl.sample_power_watts(&sysfs, t0), None);

    // Zero dt returns None (updates timestamp/value baseline safely)
    assert_eq!(rapl.sample_power_watts(&sysfs, t0), None);

    // Normal 1.5 J delta in 0.5 s => 3.0 W
    let t1 = t0 + Duration::from_millis(500);
    sysfs.set_file(domain.join("energy_uj"), "9500000\n");
    let w1 = rapl.sample_power_watts(&sysfs, t1).unwrap();
    assert!((w1 - 3.0).abs() < 1e-6, "w1={w1}");

    // Wrap-around: 9_500_000 -> 500_000 across 10_000_000 => 1_000_000 uJ in 0.25 s => 4.0 W
    let t2 = t1 + Duration::from_millis(250);
    sysfs.set_file(domain.join("energy_uj"), "500000\n");
    let w2 = rapl.sample_power_watts(&sysfs, t2).unwrap();
    assert!((w2 - 4.0).abs() < 1e-6, "w2={w2}");

    assert_eq!(
        RaplEnergyCounter::read_constraint_watts(&sysfs, domain, 0),
        Some(45.0)
    );
}

#[test]
fn intel_i915_xe_and_amd_hwmon_readers_isolate_sensors() {
    let sysfs = MockSysfs::new();
    let now = Instant::now();

    // 1. Intel i915 layout
    let i915_card = Path::new("/sys/class/drm/card1");
    sysfs.set_file(i915_card.join("gt_act_freq_mhz"), "450\n");
    sysfs.set_file(i915_card.join("gt_RP0_freq_mhz"), "2000\n");
    sysfs.set_file(i915_card.join("device/hwmon/hwmon3/temp1_input"), "51000\n");
    sysfs.set_file(i915_card.join("device/hwmon/hwmon3/power1_average"), "4250000\n");

    let snap_i915 = read_intel_igpu_sysfs(&sysfs, i915_card, now);
    assert_eq!(snap_i915.vendor, GpuVendor::Intel);
    assert_eq!(snap_i915.core_freq_mhz, Some(450.0));
    assert_eq!(snap_i915.max_core_freq_mhz, Some(2000.0));
    assert_eq!(snap_i915.temp_c, Some(51.0));
    assert_eq!(snap_i915.power_w, Some(4.25));

    // 2. Intel xe layout (missing hwmon temp -> stays None rather than reading CPU package temp)
    let xe_card = Path::new("/sys/class/drm/card2");
    sysfs.set_file(xe_card.join("device/tile0/gt0/freq0/act_freq"), "300\n");
    sysfs.set_file(xe_card.join("device/tile0/gt0/freq0/rp0_freq"), "2250\n");
    let snap_xe = read_intel_igpu_sysfs(&sysfs, xe_card, now);
    assert_eq!(snap_xe.core_freq_mhz, Some(300.0));
    assert_eq!(snap_xe.max_core_freq_mhz, Some(2250.0));
    assert_eq!(snap_xe.temp_c, None);

    // 3. Intel GPU RAPL discovery
    sysfs.set_file("/sys/class/powercap/intel-rapl:0:0/name", "core");
    sysfs.set_file("/sys/class/powercap/intel-rapl:0:1/name", "uncore");
    assert_eq!(
        find_intel_gpu_rapl_path(&sysfs, Path::new("/sys/class/powercap")),
        Some(Path::new("/sys/class/powercap/intel-rapl:0:1").to_path_buf())
    );

    // 4. AMD hwmon (temp mC, power uW fallback to power1_input, freq Hz -> MHz)
    let amd_hwmon = Path::new("/sys/class/hwmon/hwmon9");
    sysfs.set_file(amd_hwmon.join("temp1_input"), "62500");
    sysfs.set_file(amd_hwmon.join("power1_input"), "18500000");
    sysfs.set_file(amd_hwmon.join("freq1_input"), "1800000000");
    assert_eq!(
        read_amd_gpu_hwmon(&sysfs, amd_hwmon),
        (Some(62.5), Some(18.5), Some(1800.0))
    );
}

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
    empty_sysfs.set_file("/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq", "2400000");
    empty_sysfs.set_file("/sys/class/hwmon/hwmon0/name", "coretemp");
    empty_sysfs.set_file("/sys/class/hwmon/hwmon0/temp1_input", "55000");

    let history = Arc::new(MetricsHistoryStore::new());
    let mut collector = TelemetryCollector::new(empty_sysfs, TelemetryConfig::default(), history.clone());
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
        assert_eq!(arr.len(), 2, "eviction should drop 10_000 point for {:?}", id);
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
        Err(HistoryCodecError::TruncatedPayload { metric_id: 0, expected_bytes: 32, remaining_bytes: 8 })
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

    let nvml = Arc::new(NvmlWrapper::with_library_path(NvidiaPowerGuard::new(&pci_root), so_path).unwrap());
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
    assert_eq!(CpuTelemetryReader::new(tmp.path()).read_average_freq_mhz(&MockSysfs::new()), None);
}

fn write_nvidia_pci_dev(pci_root: &Path, bdf: &str, status: &str, control: &str) {
    let base = pci_root.join(bdf);
    std::fs::create_dir_all(base.join("power")).unwrap();
    std::fs::write(base.join("vendor"), "0x10de").unwrap();
    std::fs::write(base.join("class"), "0x030000").unwrap();
    std::fs::write(base.join("power/runtime_status"), status).unwrap();
    std::fs::write(base.join("power/control"), control).unwrap();
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
fn igpu_uses_rapl_when_hwmon_power_is_not_exposed_and_preserves_valid_zero() {
    let s = Arc::new(MockSysfs::new());
    let card = Path::new("/sys/class/drm/card0");
    let domain = Path::new("/sys/class/powercap/intel-rapl:0:0");
    s.set_file(card.join("gt_act_freq_mhz"), "900");
    s.set_file(domain.join("name"), "uncore");
    s.set_file(domain.join("energy_uj"), "1000000");

    let t = Instant::now();
    let mut collector = TelemetryCollector::new(
        s.clone(),
        TelemetryConfig {
            intel_card_path: Some(card.to_path_buf()),
            ..Default::default()
        },
        Arc::new(MetricsHistoryStore::new()),
    );
    assert_eq!(collector.poll(t, 1000).igpu.unwrap().power_w, None);
    s.set_file(domain.join("energy_uj"), "5000000");
    let sample = collector
        .poll(t + Duration::from_secs(1), 2000)
        .igpu
        .unwrap();
    assert_eq!(sample.power_w, Some(4.0));

    // Explicit hwmon 0.0 W takes precedence and is NOT overwritten by RAPL.
    s.set_file(card.join("device/hwmon/hwmon2/power1_average"), "0");
    s.set_file(domain.join("energy_uj"), "9000000");
    let zero_sample = collector
        .poll(t + Duration::from_secs(2), 3000)
        .igpu
        .unwrap();
    assert_eq!(zero_sample.power_w, Some(0.0));

    // Non-Intel vendor must NOT consume Intel GPU RAPL.
    let s2 = Arc::new(MockSysfs::new());
    let non_intel = Path::new("/sys/class/drm/card1");
    s2.set_file(non_intel.join("device/vendor"), "0x1002");
    s2.set_file(non_intel.join("device/uevent"), "PCI_SLOT_NAME=0000:05:00.0\n");
    s2.set_file("/sys/class/powercap/intel-rapl:0:0/name", "uncore");
    s2.set_file("/sys/class/powercap/intel-rapl:0:0/energy_uj", "1000000");
    let mut c2 = TelemetryCollector::new(
        s2.clone(),
        TelemetryConfig {
            intel_card_path: Some(non_intel.to_path_buf()),
            ..Default::default()
        },
        Arc::new(MetricsHistoryStore::new()),
    );
    let _ = c2.poll(t, 1000);
    s2.set_file("/sys/class/powercap/intel-rapl:0:0/energy_uj", "5000000");
    let non_intel_snap = c2.poll(t + Duration::from_secs(1), 2000).igpu.unwrap();
    assert_eq!(non_intel_snap.power_w, None);
    assert_eq!(non_intel_snap.bdf.as_deref(), Some("0000:05:00.0"));

    // AMD iGPU hwmon branch + BDF + max clock
    let amd_hwmon = Path::new("/sys/class/hwmon/hwmon8");
    s2.set_file(amd_hwmon.join("temp1_input"), "54000");
    s2.set_file(amd_hwmon.join("freq1_input"), "1100000000");
    s2.set_file(amd_hwmon.join("power1_average"), "12500000");
    s2.set_file(amd_hwmon.join("device/pp_dpm_sclk"), "0: 400Mhz\n1: 2200Mhz *\n");
    s2.set_file(amd_hwmon.join("device/uevent"), "PCI_SLOT_NAME=0000:c4:00.0\n");
    let mut amd_col = TelemetryCollector::new(
        s2,
        TelemetryConfig {
            amd_igpu_hwmon: Some(amd_hwmon.to_path_buf()),
            ..Default::default()
        },
        Arc::new(MetricsHistoryStore::new()),
    );
    let amd_igpu = amd_col.poll(t, 1000).igpu.unwrap();
    assert_eq!(amd_igpu.vendor, GpuVendor::Amd);
    assert_eq!(amd_igpu.bdf.as_deref(), Some("0000:c4:00.0"));
    assert_eq!(amd_igpu.max_core_freq_mhz, Some(2200.0));
    assert_eq!(amd_igpu.power_w, Some(12.5));
}

#[test]
fn cpu_frequency_excludes_offline_cores_and_handles_rapl_constraints_and_reset() {
    let s = MockSysfs::new();
    // cpu0 typically has no online file on Linux; cpu1 is offline with stale 4 GHz
    s.set_file("/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq", "2000000");
    s.set_file("/sys/devices/system/cpu/cpu1/cpufreq/scaling_cur_freq", "4000000");
    s.set_file("/sys/devices/system/cpu/cpu1/online", "0");
    let mut reader = CpuTelemetryReader::new("/");
    assert_eq!(reader.read_average_freq_mhz(&s), Some(2000));

    // Hotplug cpu1 back online -> average includes cpu1
    s.set_file("/sys/devices/system/cpu/cpu1/online", "1");
    assert_eq!(reader.read_average_freq_mhz(&s), Some(3000));

    // Multi-constraint RAPL (0/1/2) & domain rebind/reset
    let dom = Path::new("/sys/class/powercap/intel-rapl:0");
    s.set_file(dom.join("name"), "package-0");
    s.set_file(dom.join("energy_uj"), "1000000");
    s.set_file(dom.join("constraint_0_max_power_uw"), "45000000");
    s.set_file(dom.join("constraint_1_max_power_uw"), "95000000");
    s.set_file(dom.join("constraint_2_power_limit_uw"), "65000000");
    let t0 = Instant::now();
    let s0 = reader.sample(&s, t0);
    assert_eq!(s0.power_w, None);
    assert_eq!(s0.max_power_limit_w, Some(95.0));

    // Unbind energy_uj cleanly resets baseline rather than keeping stale energy
    s.files.lock().unwrap().remove(&dom.join("energy_uj"));
    assert_eq!(reader.sample(&s, t0 + Duration::from_secs(1)).power_w, None);
    s.set_file(dom.join("energy_uj"), "2000000");
    assert_eq!(reader.sample(&s, t0 + Duration::from_secs(2)).power_w, None);
    s.set_file(dom.join("energy_uj"), "5000000");
    assert_eq!(
        reader.sample(&s, t0 + Duration::from_secs(3)).power_w,
        Some(3.0)
    );
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
        Err(HistoryCodecError::NonFiniteValue { metric_id: 0, index: 0 })
    ));
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
    let base_src =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extra-fixtures/nvml.c"))
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
    let s_v2 = nvml_v2.get_telemetry_snapshot_by_bdf("0000:01:00.0").unwrap();
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
    let s_both = nvml_both.get_telemetry_snapshot_by_bdf("0000:01:00.0").unwrap();
    assert_eq!(s_both.memory.map(|m| m.total), Some(1024));
}

#[test]
fn nvapi_voltage_binding_rejects_bdf_replacement_and_topology_drift() {
    let tmp = tempfile::tempdir().unwrap();
    let pci = tmp.path().join("sys/bus/pci/devices");
    write_nvidia_pci_dev(&pci, "0000:01:00.0", "active", "on");

    let base_nvml =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extra-fixtures/nvml.c"))
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
    let nvml = NvmlWrapper::with_library_paths(
        NvidiaPowerGuard::new(&pci),
        &nvml_so,
        Some(&nvapi_so),
    )
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
    let nvml2 = NvmlWrapper::with_library_paths(
        NvidiaPowerGuard::new(&pci2),
        &nvml_so,
        Some(&nvapi_so),
    )
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
    let nvml3 = NvmlWrapper::with_library_paths(
        NvidiaPowerGuard::new(&pci3),
        &nvml_so,
        Some(&nvapi_so),
    )
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
fn intel_rapl_matches_card_package_and_fails_closed_when_ambiguous() {
    let sysfs = Arc::new(MockSysfs::new());
    let card = Path::new("/sys/class/drm/card0");
    let wrong = Path::new("/sys/class/powercap/intel-rapl:0:0");
    let matching = Path::new("/sys/class/powercap/intel-rapl:1:0");
    sysfs.set_file(card.join("device/vendor"), "0x8086");
    sysfs.set_file(card.join("device/local_cpulist"), "4-7");
    for cpu in 4..=7 {
        sysfs.set_file(
            format!("/sys/devices/system/cpu/cpu{cpu}/topology/physical_package_id"),
            "1",
        );
    }
    for domain in [wrong, matching] {
        sysfs.set_file(domain.join("name"), "uncore");
        sysfs.set_file(domain.join("energy_uj"), "1000000");
    }

    let mut collector = TelemetryCollector::new(
        sysfs.clone(),
        TelemetryConfig {
            intel_card_path: Some(card.to_path_buf()),
            ..Default::default()
        },
        Arc::new(MetricsHistoryStore::new()),
    );
    let start = Instant::now();
    assert_eq!(collector.poll(start, 1000).igpu.unwrap().power_w, None);
    sysfs.set_file(wrong.join("energy_uj"), "50000000");
    sysfs.set_file(matching.join("energy_uj"), "5000000");
    let power = collector
        .poll(start + Duration::from_secs(1), 2000)
        .igpu
        .unwrap()
        .power_w;
    assert_eq!(power, Some(4.0));

    // Incomplete CPU affinity ("0,4" where cpu4 package ID cannot be read) must fail closed -> None
    sysfs.set_file(card.join("device/local_cpulist"), "0,4");
    sysfs.set_file(
        "/sys/devices/system/cpu/cpu0/topology/physical_package_id",
        "0",
    );
    sysfs
        .files
        .lock()
        .unwrap()
        .remove(Path::new("/sys/devices/system/cpu/cpu4/topology/physical_package_id"));
    sysfs.set_file(wrong.join("energy_uj"), "70000000");
    sysfs.set_file(matching.join("energy_uj"), "7000000");
    assert_eq!(
        collector
            .poll(start + Duration::from_secs(2), 3000)
            .igpu
            .unwrap()
            .power_w,
        None
    );

    // Ambiguous 2 uncore domains without local_cpulist -> None instead of guessing package 0
    sysfs.files.lock().unwrap().remove(&card.join("device/local_cpulist"));
    sysfs.set_file(wrong.join("energy_uj"), "90000000");
    sysfs.set_file(matching.join("energy_uj"), "9000000");
    let ambiguous_power = collector
        .poll(start + Duration::from_secs(3), 4000)
        .igpu
        .unwrap()
        .power_w;
    assert_eq!(ambiguous_power, None);

    // hwmon power still takes priority over RAPL when exposed
    sysfs.set_file(card.join("device/hwmon/hwmon0/power1_average"), "2500000");
    let hwmon_power = collector
        .poll(start + Duration::from_secs(4), 5000)
        .igpu
        .unwrap()
        .power_w;
    assert_eq!(hwmon_power, Some(2.5));
}

#[test]
fn nvapi_singleton_drift_before_first_sample_and_domain_collisions_fail_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let base_nvml =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extra-fixtures/nvml.c"))
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
    let nvml_dom = NvmlWrapper::with_library_paths(
        NvidiaPowerGuard::new(&pci_dom),
        &nvml_so,
        Some(&nvapi_so),
    )
    .unwrap();
    assert_eq!(
        nvml_dom
            .get_telemetry_snapshot_by_bdf("0001:01:00.0")
            .unwrap()
            .core_voltage_mv,
        None
    );
}

#[test]
fn intel_igpu_discovery_skips_discrete_arc_and_selects_integrated_card() {
    let s = Arc::new(MockSysfs::new());
    // card0: Intel Arc dGPU (0x56A0) — NOT in Intel iGPU ID list
    s.set_file("/sys/class/drm/card0/device/vendor", "0x8086");
    s.set_file("/sys/class/drm/card0/device/class", "0x030000");
    s.set_file("/sys/class/drm/card0/device/device", "0x56a0");
    s.set_file("/sys/class/drm/card0/gt_act_freq_mhz", "650");
    // card1: Intel UHD/Iris iGPU (0x46A6)
    s.set_file("/sys/class/drm/card1/device/vendor", "0x8086");
    s.set_file("/sys/class/drm/card1/device/class", "0x030000");
    s.set_file("/sys/class/drm/card1/device/device", "0x46a6");
    s.set_file("/sys/class/drm/card1/gt_act_freq_mhz", "950");

    let mut collector = TelemetryCollector::new(
        s,
        TelemetryConfig {
            sys_root: Path::new("/").to_path_buf(),
            intel_card_path: None,
            ..Default::default()
        },
        Arc::new(MetricsHistoryStore::new()),
    );
    let snap = collector.poll(Instant::now(), 1000);
    assert_eq!(snap.igpu.unwrap().core_freq_mhz, Some(950.0));
}

