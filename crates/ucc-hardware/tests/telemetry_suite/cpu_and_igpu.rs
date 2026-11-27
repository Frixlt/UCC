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
    sysfs.set_file(
        i915_card.join("device/hwmon/hwmon3/power1_average"),
        "4250000\n",
    );

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
    s2.set_file(
        non_intel.join("device/uevent"),
        "PCI_SLOT_NAME=0000:05:00.0\n",
    );
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
    s2.set_file(
        amd_hwmon.join("device/pp_dpm_sclk"),
        "0: 400Mhz\n1: 2200Mhz *\n",
    );
    s2.set_file(
        amd_hwmon.join("device/uevent"),
        "PCI_SLOT_NAME=0000:c4:00.0\n",
    );
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
    s.set_file(
        "/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq",
        "2000000",
    );
    s.set_file(
        "/sys/devices/system/cpu/cpu1/cpufreq/scaling_cur_freq",
        "4000000",
    );
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
    sysfs.files.lock().unwrap().remove(Path::new(
        "/sys/devices/system/cpu/cpu4/topology/physical_package_id",
    ));
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
    sysfs
        .files
        .lock()
        .unwrap()
        .remove(&card.join("device/local_cpulist"));
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
