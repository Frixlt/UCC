// SPDX-License-Identifier: GPL-3.0-or-later

#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use ucc_config::{
    create_default_settings, default_high_performance_profile, default_quiet_profile, ConfigManager,
};
use ucc_dbus::{AllowAllAuthorizer, UccdProxy, DBUS_PATH};
use ucc_hardware::{
    default_process_runner, DeviceCapabilities, DeviceVendor, MockPlatformBackend, MockSysfs,
    NvidiaPowerGuard, NvmlWrapper, PlatformBackend, PlatformBackendKind, PlatformCapabilities,
    PowerDeviceController, TelemetryConfig,
};
use ucc_lighting::{KeyboardCapabilities, LightbarCapabilities, MockLightingTransport};
use uccd::{DaemonRuntime, LidPolicyConfig, LiveUccdBackend, UccdDependencies};

pub(crate) type RecheckStartupGate = (std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>);

pub(crate) type CustomBackendParts = (
    Arc<LiveUccdBackend>,
    Arc<MockPlatformBackend>,
    Arc<MockSysfs>,
    Arc<MockLightingTransport>,
);

pub(crate) fn build_custom_backend(
    tmp: &Path,
    explicit_intel_path: bool,
    prepare_sysfs: impl FnOnce(&MockSysfs),
    prepare_platform: impl FnOnce(Arc<MockPlatformBackend>) -> Arc<dyn PlatformBackend>,
) -> CustomBackendParts {
    build_custom_backend_with_sysfs(
        tmp,
        explicit_intel_path,
        None,
        prepare_sysfs,
        prepare_platform,
    )
}

pub(crate) fn build_custom_backend_with_sysfs(
    tmp: &Path,
    explicit_intel_path: bool,
    shared_sysfs: Option<Arc<MockSysfs>>,
    prepare_sysfs: impl FnOnce(&MockSysfs),
    prepare_platform: impl FnOnce(Arc<MockPlatformBackend>) -> Arc<dyn PlatformBackend>,
) -> CustomBackendParts {
    let sysfs = shared_sysfs.unwrap_or_else(|| Arc::new(MockSysfs::new()));
    sysfs.set_file(
        tmp.join("sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq"),
        "3200000",
    );
    sysfs.set_file(
        tmp.join("sys/devices/system/cpu/cpu1/cpufreq/scaling_cur_freq"),
        "3200000",
    );
    sysfs.set_file(tmp.join("sys/devices/system/cpu/cpu1/online"), "1");
    sysfs.set_file(tmp.join("sys/class/hwmon/hwmon0/name"), "coretemp");
    sysfs.set_file(tmp.join("sys/class/hwmon/hwmon0/temp1_input"), "58000");
    sysfs.set_file(tmp.join("sys/class/power_supply/AC/type"), "Mains");
    sysfs.set_file(tmp.join("sys/class/power_supply/AC/online"), "1");

    let card = tmp.join("sys/class/drm/card0");
    let rapl = tmp.join("sys/class/powercap/intel-rapl:0:0");
    sysfs.set_file(card.join("gt_act_freq_mhz"), "950");
    sysfs.set_file(card.join("gt_RP0_freq_mhz"), "2200");
    sysfs.set_file(card.join("device/hwmon/hwmon1/temp1_input"), "49000");
    sysfs.set_file(rapl.join("name"), "uncore");
    sysfs.set_file(rapl.join("energy_uj"), "2000000");

    prepare_sysfs(&sysfs);

    let caps = PlatformCapabilities {
        backend: PlatformBackendKind::Mock,
        device: DeviceCapabilities {
            vendor: DeviceVendor::Uniwill,
            number_of_fans: 2,
            fans_min_speed: 20,
            fans_off_available: true,
            fan_control: true,
            water_cooler_supported: true,
            ctgp_supported: true,
            hwp_dynamic_boost_supported: true,
            tdp_control: true,
            ..Default::default()
        },
        tdp_channels: 3,
        perf_profiles_available: 3,
    };
    let platform = Arc::new(MockPlatformBackend::new(caps));
    platform.set_fan_temp(0, 62);
    platform.set_fan_temp(1, 51);
    platform.set_fan_speed(0, 40);
    platform.set_fan_speed(1, 35);
    platform.set_tdp_limits(0, (15, 90));
    platform.set_tdp_limits(1, (20, 120));
    platform.set_tdp_limits(2, (25, 140));

    let lighting = Arc::new(MockLightingTransport::new());
    lighting.set_keyboard_caps(Some(KeyboardCapabilities {
        zones: 1,
        max_brightness: 255,
        max_red: 255,
        max_green: 255,
        max_blue: 255,
        led_ids: vec![0],
    }));
    lighting.set_lightbar_caps(Some(LightbarCapabilities {
        zones: 4,
        max_brightness: 255,
    }));
    let config_path = tmp.join("settings.json");

    let nvml_library = tmp.join("fixture_nvml.so");
    let nvml = nvml_library.is_file().then(|| {
        Arc::new(
            NvmlWrapper::with_library_path(
                NvidiaPowerGuard::new(tmp.join("sys/bus/pci/devices")),
                &nvml_library,
            )
            .unwrap(),
        )
    });

    let backend = Arc::new(LiveUccdBackend::new(UccdDependencies {
        device_name: "Test Laptop".into(),
        platform: prepare_platform(platform.clone()),
        sysfs: sysfs.clone(),
        lighting_transport: lighting.clone(),
        config_manager: ConfigManager::with_custom_path(config_path),
        sys_root: tmp.to_path_buf(),
        telemetry_config: TelemetryConfig {
            sys_root: tmp.to_path_buf(),
            intel_card_path: explicit_intel_path.then_some(card),
            nvml,
            ..Default::default()
        },
        power_devices: PowerDeviceController::new(
            tmp.join("sys"),
            tmp.join("proc"),
            default_process_runner(),
            tmp.join("bin"),
        ),
        lid_policy: LidPolicyConfig {
            ignore_lid_on_ac: true,
            ac_unplug_debounce: Duration::from_secs(3),
        },
    }));

    (backend, platform, sysfs, lighting)
}

pub(crate) fn build_test_backend(
    tmp: &Path,
) -> (
    Arc<LiveUccdBackend>,
    Arc<MockPlatformBackend>,
    Arc<MockSysfs>,
    Arc<MockLightingTransport>,
) {
    build_custom_backend(tmp, true, |_| {}, |p| p)
}

pub(crate) async fn session_conn() -> zbus::Connection {
    zbus::connection::Builder::session()
        .unwrap()
        .build()
        .await
        .unwrap()
}

pub(crate) async fn start_runtime(
    backend: Arc<LiveUccdBackend>,
    name: &str,
    workers: bool,
) -> DaemonRuntime {
    DaemonRuntime::start_on_connection(
        session_conn().await,
        name,
        backend,
        Arc::new(AllowAllAuthorizer),
        workers,
    )
    .await
    .unwrap()
}

pub(crate) async fn uccd_proxy<'a>(conn: &'a zbus::Connection, name: &'a str) -> UccdProxy<'a> {
    UccdProxy::builder(conn)
        .destination(name)
        .unwrap()
        .path(DBUS_PATH)
        .unwrap()
        .build()
        .await
        .unwrap()
}

pub(crate) fn set_battery_files(sysfs: &MockSysfs, root: &Path) {
    sysfs.set_file(root.join("sys/class/power_supply/AC/online"), "0");
    sysfs.set_file(root.join("sys/class/power_supply/BAT0/type"), "Battery");
}

pub(crate) fn write_distinct_power_profiles(root: &Path) {
    let mut settings = create_default_settings();
    settings.set_active_profile_for_mode("power_ac", default_high_performance_profile().id);
    settings.set_active_profile_for_mode("power_bat", default_quiet_profile().id);
    ConfigManager::with_custom_path(root.join("settings.json"))
        .save(&settings)
        .unwrap();
}

pub(crate) fn active_profile_id(profile_json: &str) -> String {
    let profile: serde_json::Value = serde_json::from_str(profile_json).unwrap();
    profile["id"].as_str().unwrap().to_owned()
}

pub(crate) fn handoff_profile_with_tdps(id: &str, watts: [u32; 3]) -> ucc_core::UccProfile {
    let mut profile = default_quiet_profile();
    profile.id = id.into();
    profile.odm_profile_name = None;
    profile.odm_power_limits = watts.to_vec();
    profile
}

pub(crate) fn handoff_save_profiles(root: &Path, profiles: &[&ucc_core::UccProfile]) {
    let mut config = create_default_settings();
    for p in profiles {
        config.profiles.insert(p.id.clone(), (*p).clone());
    }
    ConfigManager::with_custom_path(root.join("settings.json"))
        .save(&config)
        .unwrap();
}

pub(crate) async fn handoff_wait_gate(entered: std::sync::mpsc::Receiver<()>) {
    tokio::task::spawn_blocking(move || entered.recv_timeout(Duration::from_secs(3)))
        .await
        .unwrap()
        .unwrap();
}

pub(crate) async fn handoff_wait_name(conn: &zbus::Connection, name: &str) {
    let bus = zbus::fdo::DBusProxy::new(conn).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !bus.name_has_owner(name.try_into().unwrap()).await.unwrap() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
