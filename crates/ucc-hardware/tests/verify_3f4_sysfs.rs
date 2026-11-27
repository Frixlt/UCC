use std::fs;
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
        eprintln!(
            "Notice: open-swap.so LD_PRELOAD shim not active; skipping symlink swap race probe"
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
