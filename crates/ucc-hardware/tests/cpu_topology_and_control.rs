// SPDX-License-Identifier: GPL-3.0-or-later
use std::fs;
use std::path::Path;
use tempfile::TempDir;
use ucc_core::UccProfile;
use ucc_hardware::cpu::{
    find_closest_value, parse_cpu_range_list, CpuController, CpuTopology, CpuWorker,
};

fn create_cpu_fixture(root: &Path) {
    fs::create_dir_all(root).unwrap_or(());
    fs::write(root.join("present"), "0-1,8-9").unwrap();
    fs::write(root.join("possible"), "0-1,8-9").unwrap();
    fs::write(root.join("online"), "0-1").unwrap();

    let pstate_dir = root.join("intel_pstate");
    fs::create_dir_all(&pstate_dir).unwrap();
    fs::write(pstate_dir.join("no_turbo"), "1").unwrap();
    fs::write(pstate_dir.join("hwp_dynamic_boost"), "0").unwrap();

    for id in [0, 1, 8, 9] {
        let cpu_dir = root.join(format!("cpu{}", id));
        fs::create_dir_all(&cpu_dir).unwrap();

        // Core 0 has no online node (mandatory, non-hotpluggable)
        if id != 0 {
            fs::write(cpu_dir.join("online"), if id == 1 { "1" } else { "0" }).unwrap();
        }

        let topo_dir = cpu_dir.join("topology");
        fs::create_dir_all(&topo_dir).unwrap();
        fs::write(topo_dir.join("core_id"), (id / 2).to_string()).unwrap();
        fs::write(topo_dir.join("thread_siblings_list"), id.to_string()).unwrap();

        let cpufreq_dir = cpu_dir.join("cpufreq");
        fs::create_dir_all(&cpufreq_dir).unwrap();
        fs::write(cpufreq_dir.join("cpuinfo_min_freq"), "800000").unwrap();
        fs::write(
            cpufreq_dir.join("cpuinfo_max_freq"),
            if id < 8 { "5400000" } else { "3600000" },
        )
        .unwrap();
        fs::write(
            cpufreq_dir.join("base_frequency"),
            if id < 8 { "2700000" } else { "2100000" },
        )
        .unwrap();
        fs::write(cpufreq_dir.join("scaling_min_freq"), "800000").unwrap();
        fs::write(cpufreq_dir.join("scaling_max_freq"), "2700000").unwrap();
        fs::write(cpufreq_dir.join("scaling_governor"), "performance").unwrap();
        fs::write(
            cpufreq_dir.join("scaling_available_governors"),
            "performance powersave schedutil",
        )
        .unwrap();
        fs::write(
            cpufreq_dir.join("energy_performance_preference"),
            "performance",
        )
        .unwrap();
        fs::write(
            cpufreq_dir.join("energy_performance_available_preferences"),
            "performance power default",
        )
        .unwrap();
        fs::write(cpufreq_dir.join("scaling_driver"), "intel_pstate").unwrap();
        fs::write(
            cpufreq_dir.join("scaling_available_frequencies"),
            "800000 1200000 1800000 2100000 2700000 3600000 5400000",
        )
        .unwrap();
    }
}

#[test]
fn test_parse_cpu_range_list() {
    assert_eq!(parse_cpu_range_list("0-1,8-9"), vec![0, 1, 8, 9]);
    assert_eq!(parse_cpu_range_list("0,2,4-7"), vec![0, 2, 4, 5, 6, 7]);
    assert_eq!(parse_cpu_range_list("0-3\n"), vec![0, 1, 2, 3]);
    assert_eq!(parse_cpu_range_list(""), Vec::<u32>::new());
    assert_eq!(parse_cpu_range_list("  5  "), vec![5]);
}

#[test]
fn test_find_closest_value() {
    let freqs = [800000, 1200000, 2000000, 3000000];
    assert_eq!(find_closest_value(700000, &freqs), 800000);
    assert_eq!(find_closest_value(1100000, &freqs), 1200000);
    assert_eq!(find_closest_value(1700000, &freqs), 2000000);
    assert_eq!(find_closest_value(4000000, &freqs), 3000000);
    assert_eq!(find_closest_value(2500000, &[]), 2500000);
}

#[test]
fn test_topology_discovery_sparse_and_offline() {
    let temp = TempDir::new().unwrap();
    create_cpu_fixture(temp.path());

    let mut topology = CpuTopology::new(temp.path());
    let cores = topology.read();

    assert_eq!(cores.len(), 4);
    assert_eq!(cores[0].id, 0);
    assert!(cores[0].online);
    assert!(!cores[0].hotpluggable); // Core 0 has no online node

    assert_eq!(cores[1].id, 1);
    assert!(cores[1].online);
    assert!(cores[1].hotpluggable);

    // Cores 8 and 9 are present but offline
    assert_eq!(cores[2].id, 8);
    assert!(!cores[2].online);
    assert!(cores[2].hotpluggable);
    assert_eq!(cores[2].core_type, "unknown"); // Offline cores not probed via CPUID

    assert_eq!(cores[3].id, 9);
    assert!(!cores[3].online);

    // Verify offline core was not touched or enabled on disk
    let cpu8_online = fs::read_to_string(temp.path().join("cpu8/online")).unwrap();
    assert_eq!(cpu8_online.trim(), "0");
}

#[test]
fn test_core_selection_validation_rules() {
    let temp = TempDir::new().unwrap();
    create_cpu_fixture(temp.path());

    let controller = CpuController::new(temp.path());

    // Valid selection
    assert!(controller.validate_enabled_cores(&[0, 1]).is_ok());
    assert!(controller.validate_enabled_cores(&[0, 8, 9]).is_ok());

    // Empty selection rejected
    assert!(controller.validate_enabled_cores(&[]).is_err());

    // Missing mandatory Core 0 rejected
    assert!(controller.validate_enabled_cores(&[8, 9]).is_err());

    // Duplicate core ID rejected
    assert!(controller.validate_enabled_cores(&[0, 8, 8]).is_err());

    // Unknown core ID rejected
    assert!(controller.validate_enabled_cores(&[0, 999]).is_err());
}

#[test]
fn test_enable_before_disable_core_scheduling() {
    let temp = TempDir::new().unwrap();
    create_cpu_fixture(temp.path());

    let controller = CpuController::new(temp.path());

    let mut log = Vec::new();
    let result = controller.set_enabled_cores(&[0, 8, 9], |core, enable| {
        log.push((core.core_index, enable));
        core.write_online(enable).is_ok()
    });

    assert!(result.is_ok());
    // Strict enable-before-disable ordering:
    // Core 8 enabled first, Core 9 enabled second, Core 1 disabled third
    assert_eq!(log, vec![(8, true), (9, true), (1, false)]);

    // Check disk state
    assert_eq!(
        fs::read_to_string(temp.path().join("cpu8/online")).unwrap().trim(),
        "1"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("cpu1/online")).unwrap().trim(),
        "0"
    );

    // Switching back using use_cores(2)
    assert!(controller.use_cores(Some(2)).is_ok());
    assert_eq!(
        fs::read_to_string(temp.path().join("cpu1/online")).unwrap().trim(),
        "1"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("cpu8/online")).unwrap().trim(),
        "0"
    );
}

#[test]
fn test_failed_enable_aborts_without_disabling_existing_cores() {
    let temp = TempDir::new().unwrap();
    create_cpu_fixture(temp.path());

    let controller = CpuController::new(temp.path());

    // Fail the write when attempting to enable core 8
    let result = controller.set_enabled_cores(&[0, 8], |_core, _enable| false);

    assert!(result.is_err());
    // Core 1 must NOT have been disabled!
    assert_eq!(
        fs::read_to_string(temp.path().join("cpu1/online")).unwrap().trim(),
        "1"
    );
}

#[test]
fn test_cpu_worker_lifecycle_clamping_and_restoration() {
    let temp = TempDir::new().unwrap();
    create_cpu_fixture(temp.path());

    let mut worker = CpuWorker::new(temp.path());

    let mut profile = UccProfile::new("test", "Test Profile");
    profile.cpu.enabled_cores = Some(vec![0, 8, 9]);
    profile.cpu.online_cores = Some(2);
    profile.cpu.no_turbo = true;
    profile.cpu.governor = "powersave".to_string();
    profile.cpu.energy_performance_preference = "power".to_string();
    profile.cpu.min_freq_khz = Some(800000);
    // Request 2700000 kHz, but E-core (cpu8) base_frequency is 2100000 kHz
    profile.cpu.max_freq_khz = Some(2700000);

    assert!(worker.apply_cpu_profile(&profile).is_ok());

    // Cores state
    assert_eq!(
        fs::read_to_string(temp.path().join("cpu1/online")).unwrap().trim(),
        "0"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("cpu8/online")).unwrap().trim(),
        "1"
    );

    // Governor and EPP
    assert_eq!(
        fs::read_to_string(temp.path().join("cpu8/cpufreq/scaling_governor")).unwrap().trim(),
        "powersave"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("cpu9/cpufreq/energy_performance_preference")).unwrap().trim(),
        "power"
    );

    // E-core max frequency clamped to its base_frequency (2100000) under no_turbo
    assert_eq!(
        fs::read_to_string(temp.path().join("cpu8/cpufreq/scaling_max_freq")).unwrap().trim(),
        "2100000"
    );

    // Validation passes
    assert!(worker.validate_cpu_freq(&profile));
    assert!(worker.on_work(&profile));

    // High minimum frequency requests must also clamp to non-turbo limit
    profile.cpu.min_freq_khz = Some(2700000);
    assert!(worker.reapply_profile(&profile).is_ok());
    assert_eq!(
        fs::read_to_string(temp.path().join("cpu8/cpufreq/scaling_min_freq")).unwrap().trim(),
        "2100000"
    );

    // Worker exit restores all cores online and resets no_turbo
    worker.set_cpu_default_config();
    assert_eq!(
        fs::read_to_string(temp.path().join("cpu1/online")).unwrap().trim(),
        "1"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("cpu8/online")).unwrap().trim(),
        "1"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("intel_pstate/no_turbo")).unwrap().trim(),
        "0"
    );
}
