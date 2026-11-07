use std::fs;
use ucc_config::ConfigManager;

#[test]
fn normal_load_validates_known_profile_bounds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings");
    let doc =
        r#"{"profiles":{"p":{"id":"p","name":"P","fan":{"tableCPU":[{"temp":60,"speed":200}]}}}}"#;
    fs::write(&path, doc).unwrap();
    let result = ConfigManager::with_custom_path(&path).load();
    assert!(
        result.is_err(),
        "a profile containing 200% duty cycle was accepted"
    );
}
