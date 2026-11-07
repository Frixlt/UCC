use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use tempfile::TempDir;
use ucc_hardware::sysfs::*;

/// Helper to initialize a mock sysfs/procfs tree inside an isolated temporary directory.
struct MockSysfsHarness {
    _temp: TempDir,
    root: SysfsRoot,
}

impl MockSysfsHarness {
    fn new() -> Self {
        let temp = TempDir::new().expect("Failed to create temporary directory for mock sysfs");
        let root = SysfsRoot::custom(temp.path());
        Self { _temp: temp, root }
    }

    fn create_file(&self, rel_path: &str, content: &str) -> PathBuf {
        let full = self.root.resolve(rel_path).expect("resolve path");
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).expect("create parent directories");
        }
        let mut f = File::create(&full).expect("create file");
        f.write_all(content.as_bytes()).expect("write content");
        f.flush().expect("flush content");
        full
    }

    fn set_readonly(&self, rel_path: &str) {
        let full = self.root.resolve(rel_path).expect("resolve path");
        fs::set_permissions(&full, fs::Permissions::from_mode(0o444))
            .expect("set read-only permissions");
    }

    fn set_no_permissions(&self, rel_path: &str) {
        let full = self.root.resolve(rel_path).expect("resolve path");
        fs::set_permissions(&full, fs::Permissions::from_mode(0o000))
            .expect("set zero permissions");
    }

    fn restore_permissions(&self, rel_path: &str) {
        let full = self.root.resolve(rel_path).expect("resolve path");
        let _ = fs::set_permissions(&full, fs::Permissions::from_mode(0o644));
    }
}

// --------------------------------------------------------------------------
// 1. Missing Nodes Matrix Test
// --------------------------------------------------------------------------

#[test]
fn test_matrix_missing_nodes() {
    let harness = MockSysfsHarness::new();

    let key_paths = [
        "/sys/devices/system/cpu/cpu0/online",
        "/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor",
        "/sys/devices/system/cpu/cpu0/cpufreq/scaling_min_freq",
        "/sys/devices/system/cpu/cpu0/cpufreq/scaling_max_freq",
        "/sys/devices/system/cpu/cpu0/cpufreq/energy_performance_preference",
        "/sys/devices/system/cpu/intel_pstate/no_turbo",
        "/sys/class/backlight/intel_backlight/brightness",
        "/sys/class/power_supply/BAT0/charge_control_start_threshold",
        "/sys/class/power_supply/BAT0/charge_control_end_threshold",
        "/sys/devices/platform/tuxedo_keyboard/fn_lock",
        "/sys/module/pcie_aspm/parameters/policy",
        "/proc/cpuinfo",
    ];

    for path in key_paths {
        // String node
        let str_node = SysfsNode::<String>::new(&harness.root, path);
        assert!(!str_node.is_available(), "Missing node {path} must not be available");
        match str_node.read() {
            Err(SysfsError::NotFound(p)) => assert!(p.to_string_lossy().contains(path.trim_start_matches('/'))),
            other => panic!("Expected NotFound on missing node {path}, got {:?}", other),
        }
        match str_node.write(&"test".to_string()) {
            Err(SysfsError::NotFound(_)) => (),
            other => panic!("Expected NotFound on write to missing node {path}, got {:?}", other),
        }

        // Integer node
        let int_node = SysfsNode::<i32>::new(&harness.root, path);
        assert!(!int_node.is_available());
        assert!(matches!(int_node.read(), Err(SysfsError::NotFound(_))));
        assert!(matches!(int_node.write(&100), Err(SysfsError::NotFound(_))));

        // Boolean node
        let bool_node = SysfsNode::<bool>::new(&harness.root, path);
        assert!(!bool_node.is_available());
        assert!(matches!(bool_node.read(), Err(SysfsError::NotFound(_))));
        assert!(matches!(bool_node.write(&true), Err(SysfsError::NotFound(_))));
    }
}

// --------------------------------------------------------------------------
// 2. Read-Only Nodes Matrix Test (Write Resilience)
// --------------------------------------------------------------------------

#[test]
fn test_matrix_readonly_nodes() {
    let harness = MockSysfsHarness::new();

    let test_matrix = [
        ("sys/devices/system/cpu/cpu1/online", "1", 0),
        ("sys/devices/system/cpu/cpu0/cpufreq/scaling_governor", "powersave", 0),
        ("sys/devices/system/cpu/cpu0/cpufreq/scaling_min_freq", "800000", 1200000),
        ("sys/class/backlight/intel_backlight/brightness", "75", 90),
        ("sys/class/power_supply/BAT0/charge_control_end_threshold", "80", 60),
        ("sys/devices/platform/tuxedo_keyboard/fn_lock", "0", 1),
    ];

    for (rel_path, initial_val, _new_val) in test_matrix {
        harness.create_file(rel_path, initial_val);
        harness.set_readonly(rel_path);

        let node_str = SysfsNode::<String>::new(&harness.root, rel_path);
        assert!(node_str.is_available());

        // Read must succeed even if read-only
        let read_val = node_str.read().expect("read from read-only node must succeed");
        assert_eq!(read_val, initial_val);

        // Write must fail gracefully with PermissionDenied
        let write_res = node_str.write(&"modified".to_string());
        match write_res {
            Err(SysfsError::PermissionDenied(_)) => (),
            other => panic!("Expected PermissionDenied on read-only {rel_path}, got {:?}", other),
        }

        // Verify write journal recorded the failed attempt
        let entries = harness.root.journal().entries();
        let last = entries.last().expect("must record journal entry");
        assert!(!last.success, "Journal must mark read-only write as failed");
        assert_eq!(last.payload, "modified");

        // Verify file content on disk remained intact
        let verify_val = node_str.read().expect("verify intact");
        assert_eq!(verify_val, initial_val);

        harness.restore_permissions(rel_path);
    }
}

// --------------------------------------------------------------------------
// 3. Permission Denied Matrix Test (No Read / No Write)
// --------------------------------------------------------------------------

#[test]
fn test_matrix_permission_denied() {
    let harness = MockSysfsHarness::new();

    let rel_path = "sys/devices/system/cpu/cpu0/cpufreq/scaling_governor";
    harness.create_file(rel_path, "performance");
    harness.set_no_permissions(rel_path);

    let node = SysfsNode::<String>::new(&harness.root, rel_path);
    assert!(node.is_available());

    // Both read and write must fail with PermissionDenied
    match node.read() {
        Err(SysfsError::PermissionDenied(_)) => (),
        other => panic!("Expected PermissionDenied on read, got {:?}", other),
    }

    match node.write(&"powersave".to_string()) {
        Err(SysfsError::PermissionDenied(_)) => (),
        other => panic!("Expected PermissionDenied on write, got {:?}", other),
    }

    harness.restore_permissions(rel_path);
}

// --------------------------------------------------------------------------
// 4. Corrupted and Invalid Values Matrix Test
// --------------------------------------------------------------------------

#[test]
fn test_matrix_corrupted_and_invalid_values() {
    let harness = MockSysfsHarness::new();

    // 4.1 Boolean Node Corruption
    let bool_path = "sys/devices/system/cpu/intel_pstate/no_turbo";
    let invalid_bools = ["not_a_bool", "2", "-1", "undefined", "true_extra", "null"];

    for invalid in invalid_bools {
        harness.create_file(bool_path, invalid);
        let node = SysfsNode::<bool>::new(&harness.root, bool_path);
        match node.read() {
            Err(SysfsError::ParseError { reason, .. }) => {
                assert!(reason.contains("Invalid boolean"));
            }
            other => panic!("Expected ParseError on '{invalid}', got {:?}", other),
        }
    }

    // 4.2 Integer Node Corruption
    let int_path = "sys/class/backlight/intel_backlight/brightness";
    let invalid_ints = ["abc", "NaN", "12.34", "9999999999999999999999999999999999", "0xG1", ""];

    for invalid in invalid_ints {
        harness.create_file(int_path, invalid);
        let node = SysfsNode::<i32>::new(&harness.root, int_path);
        match node.read() {
            Err(SysfsError::ParseError { .. }) => (),
            other => panic!("Expected ParseError on int '{invalid}', got {:?}", other),
        }
    }

    // 4.3 CPU Range Node Corruption
    let range_path = "sys/devices/system/cpu/online";
    let invalid_ranges = [
        "7-2",        // Inverted range
        "abc-def",    // Non-numeric
        "0-999999",   // Exceeds max boundary (4096)
        "-1-7",       // Negative
        "0-3,foo",    // Corrupted middle element
        "0--3",       // Double dash
        "0, ,3",      // Empty segment
    ];

    for invalid in invalid_ranges {
        harness.create_file(range_path, invalid);
        let node = SysfsNode::<Vec<i32>>::new(&harness.root, range_path);
        match node.read() {
            Err(SysfsError::ParseError { .. }) => (),
            other => panic!("Expected ParseError on range '{invalid}', got {:?}", other),
        }
    }
}

// --------------------------------------------------------------------------
// 5. Zero-Length and Whitespace Trimming Matrix Test
// --------------------------------------------------------------------------

#[test]
fn test_matrix_zero_length_and_whitespace() {
    let harness = MockSysfsHarness::new();

    // Zero-length file
    let empty_path = "sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq";
    harness.create_file(empty_path, "");

    let int_node = SysfsNode::<i32>::new(&harness.root, empty_path);
    assert!(matches!(int_node.read(), Err(SysfsError::ParseError { .. })));

    let str_node = SysfsNode::<String>::new(&harness.root, empty_path);
    assert_eq!(str_node.read().unwrap(), "");

    let range_node = SysfsNode::<Vec<i32>>::new(&harness.root, empty_path);
    assert_eq!(range_node.read().unwrap(), Vec::<i32>::new());

    // Whitespace and CRLF trimming
    let ws_int_path = "sys/class/power_supply/BAT0/charge_control_end_threshold";
    harness.create_file(ws_int_path, "   85  \r\n\n");
    let ws_int_node = SysfsNode::<i32>::new(&harness.root, ws_int_path);
    assert_eq!(ws_int_node.read().unwrap(), 85);

    let ws_str_path = "sys/devices/system/cpu/cpu0/cpufreq/scaling_governor";
    harness.create_file(ws_str_path, "  performance  \n");
    let ws_str_node = SysfsNode::<String>::new(&harness.root, ws_str_path);
    assert_eq!(ws_str_node.read().unwrap(), "performance");

    let ws_range_path = "sys/devices/system/cpu/online";
    harness.create_file(ws_range_path, " \t 0-3,6-7 \r\n");
    let ws_range_node = SysfsNode::<Vec<i32>>::new(&harness.root, ws_range_path);
    assert_eq!(ws_range_node.read().unwrap(), vec![0, 1, 2, 3, 6, 7]);
}

// --------------------------------------------------------------------------
// 6. Security & Path Traversal Prevention Matrix Test
// --------------------------------------------------------------------------

#[test]
fn test_matrix_path_traversal_prevention() {
    let harness = MockSysfsHarness::new();

    let malicious_paths = [
        "../../etc/shadow",
        "sys/devices/../../../root/.bashrc",
        "/proc/../../../../etc/passwd",
        "sys/bus/../../../../../bin/sh",
    ];

    for bad_path in malicious_paths {
        let node = SysfsNode::<String>::new(&harness.root, bad_path);
        match node.read() {
            Err(SysfsError::UnsafePath(p)) => assert_eq!(p, PathBuf::from(bad_path)),
            other => panic!("Expected UnsafePath on '{bad_path}', got {:?}", other),
        }

        match node.write(&"malicious_payload".to_string()) {
            Err(SysfsError::UnsafePath(p)) => assert_eq!(p, PathBuf::from(bad_path)),
            other => panic!("Expected UnsafePath on write '{bad_path}', got {:?}", other),
        }
    }
}

// --------------------------------------------------------------------------
// 7. Write Journal Audit and Containment Invariant Test
// --------------------------------------------------------------------------

#[test]
fn test_matrix_write_journal_containment() {
    let harness = MockSysfsHarness::new();

    let path1 = "sys/devices/system/cpu/cpu0/cpufreq/scaling_governor";
    let path2 = "sys/class/backlight/intel_backlight/brightness";
    let path3 = "sys/class/power_supply/BAT0/charge_control_start_threshold";

    harness.create_file(path1, "powersave");
    harness.create_file(path2, "50");
    harness.create_file(path3, "40");

    let node1 = SysfsNode::<String>::new(&harness.root, path1);
    let node2 = SysfsNode::<i32>::new(&harness.root, path2);
    let node3 = SysfsNode::<i32>::new(&harness.root, path3);

    node1.write(&"performance".to_string()).unwrap();
    node2.write(&100).unwrap();
    node3.write(&50).unwrap();

    let journal = harness.root.journal();
    assert_eq!(journal.count(), 3);

    let entries = journal.entries();
    assert_eq!(entries[0].payload, "performance");
    assert_eq!(entries[1].payload, "100");
    assert_eq!(entries[2].payload, "50");

    // Containment invariant: ALL write operations MUST strictly be contained
    // within the mock sandbox root and NEVER point to real `/sys` or `/proc`.
    let temp_root = harness.root.root_path();
    for entry in &entries {
        assert!(
            entry.path.starts_with(temp_root),
            "Write escaped sandbox root! Path: {:?}",
            entry.path
        );
        assert!(
            !entry.path.starts_with("/sys"),
            "CRITICAL: Uncontrolled write to host /sys: {:?}",
            entry.path
        );
        assert!(
            !entry.path.starts_with("/proc"),
            "CRITICAL: Uncontrolled write to host /proc: {:?}",
            entry.path
        );
    }
}

// --------------------------------------------------------------------------
// 8. CPU Range Bidirectional Property Invariants
// --------------------------------------------------------------------------

#[test]
fn test_cpu_range_bidirectional_property() {
    let test_cases: Vec<(Vec<i32>, &str)> = vec![
        (vec![], ""),
        (vec![0], "0"),
        (vec![0, 1, 2, 3], "0-3"),
        (vec![0, 2, 4, 6], "0,2,4,6"),
        (vec![0, 1, 2, 5, 6, 7, 10], "0-2,5-7,10"),
        (vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15], "0-15"),
        (vec![1, 3, 5, 7, 9, 11], "1,3,5,7,9,11"),
        (vec![4, 5, 6, 8, 12, 13, 14], "4-6,8,12-14"),
    ];

    for (cpus, formatted_expected) in test_cases {
        // Format test
        let formatted = format_cpu_range(&cpus);
        assert_eq!(formatted, formatted_expected);

        // Parse test
        let parsed = parse_cpu_range(formatted_expected).unwrap();
        assert_eq!(parsed, cpus);

        // Round-trip equivalence
        let roundtrip_str = format_cpu_range(&parsed);
        assert_eq!(roundtrip_str, formatted_expected);
    }

    // Permutation & Deduplication Invariant
    let unorganized = "3, 1, 2, 0, 0, 1, 2, 3, 7-5"; // contains inverted sub-token
    assert!(parse_cpu_range(unorganized).is_err());

    let unsorted_clean = "3, 1, 2, 0, 5-7, 6, 7";
    let parsed = parse_cpu_range(unsorted_clean).unwrap();
    assert_eq!(parsed, vec![0, 1, 2, 3, 5, 6, 7]);
    assert_eq!(format_cpu_range(&parsed), "0-3,5-7");
}
