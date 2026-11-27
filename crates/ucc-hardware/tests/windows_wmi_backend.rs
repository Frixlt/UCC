// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::Arc;
use ucc_hardware::{
    ec_reg, MockWmiEcTransport, PlatformBackend, PlatformBackendKind, WindowsWmiEcBackend,
    WmiPlatformLimits,
};

#[test]
fn windows_wmi_backend_reads_and_writes_ec_within_bounds() {
    let transport = Arc::new(MockWmiEcTransport::new(true));
    transport.set_reg(ec_reg::CPU_TEMP_C, 61);
    transport.set_reg(ec_reg::GPU_TEMP_C, 54);

    let backend =
        WindowsWmiEcBackend::new(Arc::clone(&transport) as _, WmiPlatformLimits::default());
    assert_eq!(backend.kind(), PlatformBackendKind::WindowsWmi);
    assert!(backend.is_available());
    assert_eq!(backend.read_fan_temp(0).unwrap(), 61);
    assert_eq!(backend.read_fan_temp(1).unwrap(), 54);

    backend.set_fan_speed_percent(0, 50).unwrap();
    assert_eq!(transport.get_reg(ec_reg::FAN_MANUAL_MODE), Some(1));
    assert_eq!(transport.get_reg(ec_reg::CPU_FAN_DUTY), Some(100));
    assert_eq!(backend.read_fan_speed_percent(0).unwrap(), 50);

    backend.set_fans_auto().unwrap();
    assert_eq!(transport.get_reg(ec_reg::FAN_MANUAL_MODE), Some(0));

    backend.set_perf_profile(3).unwrap();
    assert_eq!(backend.read_perf_profile().unwrap(), 3);
    assert!(backend.set_perf_profile(0).is_err());

    backend.set_tdp(0, 45).unwrap();
    assert_eq!(backend.read_tdp(0).unwrap(), 45);
    assert!(backend.set_tdp(0, 200).is_err());

    transport.set_available(false);
    assert!(!backend.is_available());
    assert!(backend.read_fan_temp(0).is_err());
}
