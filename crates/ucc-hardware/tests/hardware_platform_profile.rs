// SPDX-License-Identifier: GPL-3.0-or-later

//! Regression tests for V2-P2 hardware fixes:
//! - V2-P2-09: `SysfsRoot::write_string` enforces `/sys` and `/proc` boundary when `root_path == "/"`
//! - V2-P2-10 (N21): `TuxedoIoBackend::set_perf_profile` falls back to `platform_profile_path`
//!   even when `perf_profiles_available == 0`, and `map_perf_profile_code_to_str` never falls
//!   back to host `std::fs` when `sysfs` is `Some`.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use ucc_hardware::ioctl::requests;
use ucc_hardware::traits::MockHardwareIo;
use ucc_hardware::{
    MockSysfs, PlatformBackend, PlatformError, SysfsError, SysfsRoot, TuxedoIoBackend,
};

#[test]
fn v2_p2_09_sysfs_write_string_enforces_root_sys_proc_boundary() {
    let root_sysfs = SysfsRoot::production();
    let err = root_sysfs
        .write_string("/tmp/ucc_v2_p2_09_forbidden_write", "1")
        .expect_err("root SysfsRoot::write_string must reject paths outside /sys and /proc");
    assert!(matches!(err, SysfsError::UnsafePath(_)));

    // Also verify symlink escape protection under a custom root.
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        use tempfile::TempDir;
        let tmp = TempDir::new().expect("temp dir");
        let root_dir = tmp.path().join("sysroot");
        let outside_dir = tmp.path().join("outside");
        fs::create_dir_all(&root_dir).expect("create sysroot");
        fs::create_dir_all(&outside_dir).expect("create outside");
        let victim = outside_dir.join("victim");
        fs::write(&victim, "orig").expect("write victim");
        symlink(&victim, root_dir.join("escape")).expect("create symlink");

        let custom_sysfs = SysfsRoot::custom(&root_dir);
        let write_err = custom_sysfs.write_string("/escape", "pwn");
        assert!(
            write_err.is_err(),
            "SysfsRoot::write_string must reject symlink escaping custom root"
        );
        assert_eq!(fs::read_to_string(&victim).expect("read victim"), "orig");
    }

    // Verify descriptor-level `/sys` & `/proc` boundary exists in `write_string`.
    let sysfs_src = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/sysfs.rs"))
        .expect("read src/sysfs.rs");
    let ws_start = sysfs_src
        .find("pub fn write_string")
        .expect("write_string must exist");
    let ws_body = &sysfs_src[ws_start..sysfs_src.len().min(ws_start + 1600)];
    assert!(
        ws_body.contains("self.root_path == Path::new(\"/\")")
            && ws_body.contains("!canon_fd.starts_with(\"/sys\") && !canon_fd.starts_with(\"/proc\")"),
        "V2-P2-09: write_string must enforce /sys and /proc descriptor check when root_path == \"/\""
    );
}

#[test]
fn v2_p2_10_uniwill_zero_perf_profiles_falls_back_to_platform_profile_sysfs() {
    let hw = MockHardwareIo::new();
    hw.set_i32_read(requests::R_HWCHECK_UW, 1);
    hw.set_i32_read(requests::R_UW_PROFS_AVAILABLE, 0);
    hw.set_error(requests::W_UW_PERF_PROF);

    let sysfs = Arc::new(MockSysfs::new());
    let profile_path =
        Path::new("/sys/bus/platform/devices/tuxedo_platform_profile/platform_profile");
    let choices_path =
        Path::new("/sys/bus/platform/devices/tuxedo_platform_profile/platform_profile_choices");
    sysfs.set_file(profile_path, "balanced\n");
    sysfs.set_file(choices_path, "low-power balanced performance\n");

    let backend = TuxedoIoBackend::with_sysfs(hw, None, Arc::clone(&sysfs) as _);
    assert_eq!(backend.capabilities().perf_profiles_available, 0);

    backend.set_perf_profile(3).expect(
        "V2-P2-10: set_perf_profile must fall back to sysfs when perf_profiles_available == 0",
    );

    assert!(
        sysfs
            .journal()
            .iter()
            .any(|w| w.path == profile_path && w.content == "performance"),
        "Expected sysfs write of `performance` to {}",
        profile_path.display()
    );
    assert_eq!(backend.read_perf_profile().expect("readback"), 3);

    // Invalid wire code is still rejected even with sysfs fallback available.
    assert!(matches!(
        backend.set_perf_profile(99),
        Err(PlatformError::InvalidArgument(_))
    ));
}

#[test]
fn v2_p2_10_uniwill_zero_perf_profiles_without_sysfs_remains_unsupported() {
    let hw = MockHardwareIo::new();
    hw.set_i32_read(requests::R_HWCHECK_UW, 1);
    hw.set_i32_read(requests::R_UW_PROFS_AVAILABLE, 0);

    let sysfs = Arc::new(MockSysfs::new());
    let backend = TuxedoIoBackend::with_sysfs(hw, None, sysfs);
    assert!(matches!(
        backend.set_perf_profile(2),
        Err(PlatformError::Unsupported(_))
    ));
}

#[test]
fn v2_p2_10_map_perf_profile_never_falls_back_to_host_fs_when_sysfs_is_some() {
    let hw = MockHardwareIo::new();
    hw.set_i32_read(requests::R_HWCHECK_UW, 1);
    hw.set_i32_read(requests::R_UW_PROFS_AVAILABLE, 0);

    let sysfs = Arc::new(MockSysfs::new());
    let profile_path =
        Path::new("/sys/bus/platform/devices/tuxedo_platform_profile/platform_profile");
    sysfs.set_file(profile_path, "balanced\n");
    // Deliberately omit `platform_profile_choices` in MockSysfs.

    let backend = TuxedoIoBackend::with_sysfs(hw, None, Arc::clone(&sysfs) as _);
    backend
        .set_perf_profile(1)
        .expect("fallback should still pick first Uniwill candidate without host fs");
    assert!(
        sysfs
            .journal()
            .iter()
            .any(|w| w.path == profile_path && w.content == "low-power"),
        "Expected first Uniwill candidate `low-power`"
    );

    let platform_src =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/platform.rs"))
            .expect("read src/platform.rs");
    assert!(
        !platform_src.contains(".and_then(|s| s.read(&choices_path).ok())\n            .or_else(|| std::fs::read_to_string(&choices_path).ok())"),
        "V2-P2-10: map_perf_profile_code_to_str must not fall back to host std::fs when sysfs is Some"
    );
}

#[test]
fn tuxedo_io_platform_profile_priority_and_ibpg10amd_quirk() {
    use ucc_hardware::{DmiInfo, UniwillDeviceID};

    let tuxedo_profile =
        Path::new("/sys/bus/platform/devices/tuxedo_platform_profile/platform_profile");
    let tuxedo_choices =
        Path::new("/sys/bus/platform/devices/tuxedo_platform_profile/platform_profile_choices");
    let acpi_profile = Path::new("/sys/firmware/acpi/platform_profile");
    let acpi_choices = Path::new("/sys/firmware/acpi/platform_profile_choices");

    // 1. tuxedo_platform_profile takes priority over /sys/firmware/acpi/platform_profile
    let hw_prio = MockHardwareIo::new();
    hw_prio.set_i32_read(requests::R_HWCHECK_UW, 1);
    hw_prio.set_i32_read(requests::R_UW_PROFS_AVAILABLE, 0);

    let sysfs_prio = Arc::new(MockSysfs::new());
    sysfs_prio.set_file(tuxedo_profile, "low-power\n");
    sysfs_prio.set_file(tuxedo_choices, "low-power balanced performance\n");
    sysfs_prio.set_file(acpi_profile, "performance\n");
    sysfs_prio.set_file(acpi_choices, "low-power balanced performance\n");

    let backend_prio = TuxedoIoBackend::with_sysfs(hw_prio, None, Arc::clone(&sysfs_prio) as _);
    assert_eq!(backend_prio.read_perf_profile().unwrap(), 1);
    backend_prio.set_perf_profile(2).unwrap();
    assert!(sysfs_prio
        .journal()
        .iter()
        .any(|w| w.path == tuxedo_profile && w.content == "balanced"));
    assert!(!sysfs_prio.journal().iter().any(|w| w.path == acpi_profile));

    // 2. Non-quirked device falls back to /sys/firmware/acpi/platform_profile when tuxedo path is absent
    let hw_acpi = MockHardwareIo::new();
    hw_acpi.set_i32_read(requests::R_HWCHECK_UW, 1);
    hw_acpi.set_i32_read(requests::R_UW_PROFS_AVAILABLE, 0);

    let sysfs_acpi = Arc::new(MockSysfs::new());
    sysfs_acpi.set_file(acpi_profile, "performance\n");
    sysfs_acpi.set_file(acpi_choices, "low-power balanced performance\n");

    let stellaris_dmi = DmiInfo {
        sys_vendor: "TUXEDO".into(),
        product_sku: "STELLARIS16I07".into(),
        ..Default::default()
    };
    let backend_acpi =
        TuxedoIoBackend::with_sysfs(hw_acpi, Some(&stellaris_dmi), Arc::clone(&sysfs_acpi) as _);
    assert_eq!(backend_acpi.read_perf_profile().unwrap(), 3);

    // 3. IBPG10AMD quirk: /sys/firmware/acpi/platform_profile must be ignored completely
    let hw_quirk = MockHardwareIo::new();
    hw_quirk.set_i32_read(requests::R_HWCHECK_UW, 1);
    hw_quirk.set_i32_read(requests::R_UW_PROFS_AVAILABLE, 0);

    let sysfs_quirk = Arc::new(MockSysfs::new());
    sysfs_quirk.set_file(acpi_profile, "balanced\n");
    sysfs_quirk.set_file(acpi_choices, "low-power balanced performance\n");

    let ibpg10_dmi = DmiInfo {
        sys_vendor: "TUXEDO".into(),
        product_sku: "IBP14A10MK1 / IBP15A10MK1".into(),
        ..Default::default()
    };
    let backend_quirk =
        TuxedoIoBackend::with_sysfs(hw_quirk, Some(&ibpg10_dmi), Arc::clone(&sysfs_quirk) as _);
    assert_eq!(
        backend_quirk.capabilities().device.device_id,
        Some(UniwillDeviceID::IBPG10AMD)
    );
    assert!(matches!(
        backend_quirk.read_perf_profile(),
        Err(PlatformError::Unsupported(_))
    ));
    assert!(matches!(
        backend_quirk.set_perf_profile(2),
        Err(PlatformError::Unsupported(_))
    ));
    assert!(sysfs_quirk.journal().is_empty());

    // 4. IBPG10AMD still works if tuxedo_platform_profile is present
    let hw_quirk_tux = MockHardwareIo::new();
    hw_quirk_tux.set_i32_read(requests::R_HWCHECK_UW, 1);
    hw_quirk_tux.set_i32_read(requests::R_UW_PROFS_AVAILABLE, 0);

    let sysfs_quirk_tux = Arc::new(MockSysfs::new());
    sysfs_quirk_tux.set_file(tuxedo_profile, "balanced\n");
    sysfs_quirk_tux.set_file(tuxedo_choices, "low-power balanced performance\n");
    sysfs_quirk_tux.set_file(acpi_profile, "performance\n");

    let backend_quirk_tux = TuxedoIoBackend::with_sysfs(
        hw_quirk_tux,
        Some(&ibpg10_dmi),
        Arc::clone(&sysfs_quirk_tux) as _,
    );
    assert_eq!(backend_quirk_tux.read_perf_profile().unwrap(), 2);
}

#[test]
fn tuxedo_io_perf_profile_unifies_read_fallback_and_propagates_write_errors() {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use ucc_hardware::traits::Sysfs;

    struct ToggleFailSysfs {
        inner: MockSysfs,
        fail_reads: AtomicBool,
        fail_writes: AtomicBool,
    }

    impl Sysfs for ToggleFailSysfs {
        fn read(&self, path: &Path) -> std::io::Result<String> {
            if self.fail_reads.load(Ordering::SeqCst) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "simulated read error",
                ));
            }
            self.inner.read(path)
        }

        fn write(&self, path: &Path, content: &str) -> std::io::Result<()> {
            if self.fail_writes.load(Ordering::SeqCst) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "simulated write error",
                ));
            }
            self.inner.write(path, content)
        }

        fn exists(&self, path: &Path) -> bool {
            self.inner.exists(path)
        }

        fn read_dir(&self, path: &Path) -> std::io::Result<Vec<PathBuf>> {
            self.inner.read_dir(path)
        }
    }

    let hw = MockHardwareIo::new();
    hw.set_i32_read(requests::R_HWCHECK_UW, 1);
    hw.set_i32_read(requests::R_UW_PROFS_AVAILABLE, 0);

    let inner = MockSysfs::new();
    let profile_path =
        Path::new("/sys/bus/platform/devices/tuxedo_platform_profile/platform_profile");
    let choices_path =
        Path::new("/sys/bus/platform/devices/tuxedo_platform_profile/platform_profile_choices");
    inner.set_file(
        profile_path,
        "balanced
",
    );
    inner.set_file(
        choices_path,
        "low-power balanced performance
",
    );

    let sysfs = Arc::new(ToggleFailSysfs {
        inner,
        fail_reads: AtomicBool::new(false),
        fail_writes: AtomicBool::new(false),
    });

    let backend = TuxedoIoBackend::with_sysfs(hw, None, Arc::clone(&sysfs) as _);
    assert_eq!(backend.read_perf_profile().unwrap(), 2);

    // If sysfs read subsequently fails, unified fallback returns cached profile
    sysfs.fail_reads.store(true, Ordering::SeqCst);
    assert_eq!(backend.read_perf_profile().unwrap(), 2);

    // If sysfs write fails, set_perf_profile propagates PlatformError::Io instead of Unsupported
    sysfs.fail_reads.store(false, Ordering::SeqCst);
    sysfs.fail_writes.store(true, Ordering::SeqCst);
    let err = backend
        .set_perf_profile(3)
        .expect_err("sysfs write failure must propagate as PlatformError::Io");
    assert!(
        matches!(err, PlatformError::Io { ref source, .. } if source.kind() == std::io::ErrorKind::PermissionDenied),
        "expected PlatformError::Io(PermissionDenied), got {err:?}"
    );
}
