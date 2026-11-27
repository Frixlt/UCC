use std::fs;
use std::path::Path;
use std::process::Command;
use ucc_hardware::SysfsRoot;

#[test]
fn contained_write_survives_symlink_swap_after_validation() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    fs::create_dir(&root).unwrap();
    let target = root.join("node");
    let outside = dir.path().join("outside");
    fs::write(&target, "old").unwrap();
    fs::write(&outside, "unchanged").unwrap();

    // Check if open-swap preload is active
    let shim_active = {
        let probe = dir.path().join("shim_probe");
        fs::write(&probe, "probe").unwrap();
        std::env::set_var("UCC_AUDIT_SWAP_TARGET", &probe);
        std::env::set_var("UCC_AUDIT_SWAP_DESTINATION", &outside);
        let _ = std::fs::OpenOptions::new().write(true).open(&probe);
        std::env::remove_var("UCC_AUDIT_SWAP_TARGET");
        std::env::remove_var("UCC_AUDIT_SWAP_DESTINATION");
        fs::symlink_metadata(&probe)
            .map(|m| m.is_symlink())
            .unwrap_or(false)
    };

    if !shim_active {
        assert!(
            std::env::var_os("UCC_VERIFY_3F4_REEXEC").is_none(),
            "open-swap.so LD_PRELOAD shim failed to activate in re-execed child process"
        );
        let shim_src = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stand-fixtures/open-swap.c")
            .canonicalize()
            .expect("stand-fixtures/open-swap.c must exist");
        let shim_so = dir.path().join("open-swap.so");
        let compile = Command::new("cc")
            .args(["-shared", "-fPIC", "-O2"])
            .arg(&shim_src)
            .arg("-o")
            .arg(&shim_so)
            .arg("-ldl")
            .output()
            .expect("cc must be available to compile open-swap.c");
        assert!(
            compile.status.success(),
            "compiling open-swap.c failed: {}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let exe = std::env::current_exe().expect("locate current test executable");
        let child = Command::new(&exe)
            .args([
                "--exact",
                "contained_write_survives_symlink_swap_after_validation",
                "--nocapture",
            ])
            .env("LD_PRELOAD", &shim_so)
            .env("UCC_VERIFY_3F4_REEXEC", "1")
            .output()
            .expect("re-exec sysfs_path_containment with LD_PRELOAD");
        assert!(
            child.status.success(),
            "LD_PRELOAD sysfs_path_containment child failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );
        return;
    }

    std::env::set_var("UCC_AUDIT_SWAP_TARGET", &target);
    std::env::set_var("UCC_AUDIT_SWAP_DESTINATION", &outside);
    let result = SysfsRoot::custom(root).write_string("/node", "changed");
    std::env::remove_var("UCC_AUDIT_SWAP_TARGET");
    std::env::remove_var("UCC_AUDIT_SWAP_DESTINATION");
    assert!(
        fs::symlink_metadata(target).unwrap().is_symlink(),
        "the audit shim must swap the node"
    );
    assert_eq!(
        fs::read_to_string(outside).unwrap(),
        "unchanged",
        "{result:?}"
    );
}
