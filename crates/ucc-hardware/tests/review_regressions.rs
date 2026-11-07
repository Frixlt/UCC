use ucc_hardware::nvml::NvidiaPowerGuard;

#[test]
fn active_nvidia_audio_does_not_allow_probing_suspended_gpu() {
    let dir = tempfile::tempdir().unwrap();
    for (id, class, status) in [("0000:02:00.0", "0x030000", "suspended"), ("0000:02:00.1", "0x040300", "active")] {
        let base = dir.path().join(id);
        std::fs::create_dir_all(base.join("power")).unwrap();
        std::fs::write(base.join("vendor"), "0x10de").unwrap();
        std::fs::write(base.join("class"), class).unwrap();
        std::fs::write(base.join("power/runtime_status"), status).unwrap();
    }
    assert!(!NvidiaPowerGuard::new(dir.path()).is_dgpu_awake(), "An audio function must not bypass the graphics-device runtime guard");
}
