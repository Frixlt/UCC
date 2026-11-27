// SPDX-License-Identifier: GPL-3.0-or-later

//! Cross-language D-Bus interop harness hosting `LiveUccdBackend` on the bus
//! with mocked hardware sysfs/platform/lighting for C++ `UccdClient` testing.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use ucc_config::ConfigManager;
use ucc_dbus::AllowAllAuthorizer;
use ucc_hardware::{
    default_process_runner, DeviceCapabilities, DeviceVendor, MockPlatformBackend, MockSysfs,
    PlatformBackendKind, PlatformCapabilities, PowerDeviceController, TelemetryConfig,
};
use ucc_lighting::{KeyboardCapabilities, LightbarCapabilities, MockLightingTransport};
use uccd::{DaemonRuntime, LidPolicyConfig, LiveUccdBackend, UccdDependencies};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    let config_dir = std::env::var_os("UCC_INTEROP_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("ucc-interop-{}", std::process::id()))
        });
    std::fs::create_dir_all(&config_dir)?;
    let tmp = config_dir.join("rootfs");
    std::fs::create_dir_all(&tmp)?;

    let sysfs = Arc::new(MockSysfs::new());
    sysfs.set_file(
        tmp.join("sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq"),
        "3200000",
    );
    sysfs.set_file(
        tmp.join("sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_min_freq"),
        "800000",
    );
    sysfs.set_file(
        tmp.join("sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq"),
        "4200000",
    );
    sysfs.set_file(
        tmp.join("sys/devices/system/cpu/cpu0/cpufreq/scaling_min_freq"),
        "800000",
    );
    sysfs.set_file(
        tmp.join("sys/devices/system/cpu/cpu0/cpufreq/scaling_max_freq"),
        "4200000",
    );
    sysfs.set_file(
        tmp.join("sys/devices/system/cpu/cpu1/cpufreq/scaling_cur_freq"),
        "3200000",
    );
    sysfs.set_file(tmp.join("sys/devices/system/cpu/cpu1/online"), "1");
    sysfs.set_file(tmp.join("sys/devices/system/cpu/present"), "0-1");

    sysfs.set_file(tmp.join("sys/class/hwmon/hwmon0/name"), "coretemp");
    sysfs.set_file(tmp.join("sys/class/hwmon/hwmon0/temp1_input"), "58000");

    sysfs.set_file(tmp.join("sys/class/power_supply/AC/type"), "Mains");
    sysfs.set_file(tmp.join("sys/class/power_supply/AC/online"), "1");

    sysfs.set_file(
        tmp.join("sys/class/power_supply/BAT0/charge_control_start_threshold"),
        "40",
    );
    sysfs.set_file(
        tmp.join("sys/class/power_supply/BAT0/charge_control_end_threshold"),
        "80",
    );
    sysfs.set_file(
        tmp.join("sys/class/power_supply/BAT0/charge_control_start_available_thresholds"),
        "40 50 60",
    );
    sysfs.set_file(
        tmp.join("sys/class/power_supply/BAT0/charge_control_end_available_thresholds"),
        "60 80 90",
    );

    let card = tmp.join("sys/class/drm/card0");
    let rapl = tmp.join("sys/class/powercap/intel-rapl:0:0");
    sysfs.set_file(card.join("gt_act_freq_mhz"), "950");
    sysfs.set_file(card.join("gt_RP0_freq_mhz"), "2200");
    sysfs.set_file(card.join("device/hwmon/hwmon1/temp1_input"), "49000");
    sysfs.set_file(rapl.join("name"), "uncore");
    sysfs.set_file(rapl.join("energy_uj"), "2000000");

    let bl = tmp.join("sys/class/backlight/intel_backlight");
    sysfs.set_file(bl.join("brightness"), "50");
    sysfs.set_file(bl.join("max_brightness"), "100");
    sysfs.set_file(bl.join("type"), "raw");

    let caps = PlatformCapabilities {
        backend: PlatformBackendKind::Mock,
        device: DeviceCapabilities {
            vendor: DeviceVendor::Uniwill,
            number_of_fans: 2,
            fans_min_speed: 20,
            fans_off_available: true,
            fan_control: true,
            tdp_control: true,
            water_cooler_supported: true,
            charging_thresholds: true,
            ctgp_supported: true,
            hwp_dynamic_boost_supported: true,
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

    let config_path = config_dir.join("settings.json");
    let backend = Arc::new(LiveUccdBackend::new(UccdDependencies {
        device_name: "Interop Test Laptop".into(),
        platform,
        sysfs,
        lighting_transport: lighting,
        config_manager: ConfigManager::with_custom_path(config_path),
        sys_root: tmp.clone(),
        telemetry_config: TelemetryConfig {
            sys_root: tmp,
            intel_card_path: Some(card),
            nvml: None,
            ..Default::default()
        },
        power_devices: PowerDeviceController::new(
            config_dir.join("sys"),
            config_dir.join("proc"),
            default_process_runner(),
            config_dir.join("bin"),
        ),
        lid_policy: LidPolicyConfig {
            ignore_lid_on_ac: true,
            ac_unplug_debounce: Duration::from_secs(3),
        },
    }));

    let connection = zbus::connection::Builder::system()?.build().await?;
    let runtime = DaemonRuntime::start_on_connection(
        connection,
        "com.uniwill.uccd",
        backend,
        Arc::new(AllowAllAuthorizer),
        true,
    )
    .await?;

    println!("READY");
    std::io::stdout().flush()?;

    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        _ = sigint.recv() => {}
        _ = sigterm.recv() => {}
    }

    runtime.shutdown(Duration::from_secs(2)).await;
    Ok(())
}
