# Инвентаризация базы перед переходом на Rust

Дата: 7 ноября 2026. Основной commit: `45921b0b7021448a81575a840b2b77d93da8b298`.

Это статическая инвентаризация исходников, а не отчёт об аппаратной проверке. SHA-256 каждого отслеживаемого файла находится в [baseline-manifest.json](baseline-manifest.json). Игнорируемые приватные artifacts и установленные настройки в перечень не включены.

## Размер компонентов

| Компонент | Файлы кода | Строки, включая комментарии | Назначение |
|---|---:|---:|---|
| `uccd` | 54 | 21,928 | ucc-hardware / uccd / packaging |
| `libucc-dbus` | 5 | 1,710 | ucc-dbus |
| `ucc-gui` | 54 | 15,779 | ucc-gui |
| `ucc-cli` | 1 | 2,159 | ucc-cli |
| `ucc-desktop` | 9 | 603 | ucc-desktop + ucc-desktop-tray |
| `ucc-session` | 7 | 754 | ucc-session-agent + ucc-desktop |
| `ucc-tray` | 9 | 2,228 | shell integration decision (M1/M11) |
| `ucc-gnome` | 2 | 1,532 | shell integration decision (M1/M11) |
| `include` | 5 | 671 | ucc-core / ucc-hardware / ucc-desktop |
| `tests` | 46 | 4,889 | Rust tests + migration contract fixtures |
| `contrib` | 8 | 994 | ucc-migrate / xtask / historical tooling |
| `nvidia_findings` | 10 | 2,818 | research archive; runtime subset → ucc-hardware |

## D-Bus: публичные методы

В `public slots` адаптера объявлено **122 методов**, отдельно **7 сигналов**. Сигнатуры ниже выведены из Qt-типов; на этапе M0 необходимо сверить их с настоящим XML introspection на изолированной шине. Столбец Polkit показывает только явный `checkAuth` в теле метода; `—` не является рекомендацией убрать проверку.

| Метод | Вход → выход D-Bus | Явная проверка Polkit | Строка реализации |
|---|---|---|---:|
| `GetDeviceName` | `∅` → `s` | — | 367 |
| `GetSystemInfoJSON` | `∅` → `s` | — | 373 |
| `IsDeviceSupported` | `∅` → `b` | — | 379 |
| `GetDisplayModesJSON` | `∅` → `s` | — | 384 |
| `GetIsX11` | `∅` → `b` | — | 390 |
| `TuxedoWmiAvailable` | `∅` → `b` | — | 395 |
| `FanHwmonAvailable` | `∅` → `b` | — | 400 |
| `UccdVersion` | `∅` → `s` | — | 405 |
| `GetFanDataCPU` | `∅` → `a{sv}` | — | 414 |
| `GetFanDataGPU1` | `∅` → `a{sv}` | — | 424 |
| `GetFanDataGPU2` | `∅` → `a{sv}` | — | 434 |
| `WebcamSWAvailable` | `∅` → `b` | — | 445 |
| `GetWebcamSWStatus` | `∅` → `b` | — | 450 |
| `GetForceYUV420OutputSwitchAvailable` | `∅` → `b` | — | 455 |
| `GetDisplayBrightness` | `∅` → `i` | — | 482 |
| `GetKeyboardBacklightControlSupported` | `∅` → `b` | — | 475 |
| `SetDisplayBrightness` | `i` → `b` | CONTROL | 487 |
| `SetDisplayRefreshRate` | `si` → `b` | CONTROL | 460 |
| `GetDGpuInfoValuesJSON` | `∅` → `s` | — | 498 |
| `GetIGpuInfoValuesJSON` | `∅` → `s` | — | 505 |
| `GetCpuPowerValuesJSON` | `∅` → `s` | — | 512 |
| `GetPrimeState` | `∅` → `s` | — | 521 |
| `ConsumeModeReapplyPending` | `∅` → `b` | — | 527 |
| `GetActiveProfileJSON` | `∅` → `s` | — | 534 |
| `GetPowerState` | `∅` → `s` | — | 1168 |
| `GetPowerPolicyJSON` | `∅` → `s` | READ | 1185 |
| `GetPowerDeviceSettingsJSON` | `∅` → `s` | READ | 1193 |
| `GetPowerDeviceStatusJSON` | `∅` → `s` | READ | 1199 |
| `SetPowerDeviceSettingsJSON` | `s` → `b` | CONTROL | 1205 |
| `ScanPowerDevices` | `∅` → `b` | CONTROL | 1222 |
| `GetLightbarSettingsJSON` | `∅` → `s` | READ | 1229 |
| `GetLightbarStatusJSON` | `∅` → `s` | READ | 1243 |
| `GetDriverHealthJSON` | `∅` → `s` | READ | 1235 |
| `SetLightbarSettingsJSON` | `s` → `b` | CONTROL | 1249 |
| `TriggerLightbarAnimation` | `b` → `b` | CONTROL | 1265 |
| `SetManualPowerMode` | `s` → `b` | CONTROL | 1271 |
| `RegisterPowerRequest` | `sss` → `b` | CONTROL | 1277 |
| `UnregisterPowerRequest` | `s` → `b` | CONTROL | 1288 |
| `SetTempProfile` | `s` → `b` | CONTROL | 733 |
| `SetActiveProfile` | `s` → `b` | CONTROL | 741 |
| `ApplyProfile` | `s` → `b` | CONTROL | 748 |
| `GetProfilesJSON` | `∅` → `s` | — | 757 |
| `GetCustomProfilesJSON` | `∅` → `s` | — | 763 |
| `SetFanProfileCPU` | `s` → `b` | CONTROL | 540 |
| `SetFanProfileDGPU` | `s` → `b` | CONTROL | 582 |
| `ApplyFanProfiles` | `s` → `b` | CONTROL | 624 |
| `RevertFanProfiles` | `∅` → `b` | CONTROL | 704 |
| `GetDefaultProfilesJSON` | `∅` → `s` | — | 771 |
| `GetCpuFrequencyLimitsJSON` | `∅` → `s` | — | 777 |
| `GetDefaultValuesProfileJSON` | `∅` → `s` | — | 787 |
| `AddCustomProfile` | `s` → `b` | CONTROL | 793 |
| `SaveCustomProfile` | `s` → `b` | CONTROL | 927 |
| `DeleteCustomProfile` | `s` → `b` | CONTROL | 841 |
| `UpdateCustomProfile` | `s` → `b` | CONTROL | 867 |
| `GetFanProfile` | `s` → `s` | — | 1135 |
| `GetFanProfileNames` | `∅` → `s` | — | 1140 |
| `SetFanProfile` | `ss` → `b` | CONTROL | 1154 |
| `GetSettingsJSON` | `∅` → `s` | — | 1162 |
| `SetStateMap` | `ss` → `b` | CONTROL | 1295 |
| `SetBatchStateMap` | `s` → `b` | CONTROL | 1360 |
| `ODMProfilesAvailable` | `∅` → `as` | — | 1437 |
| `ODMPowerLimitsJSON` | `∅` → `s` | — | 1446 |
| `GetKeyboardBacklightCapabilitiesJSON` | `∅` → `s` | — | 1454 |
| `GetKeyboardBacklightStatesJSON` | `∅` → `s` | — | 1460 |
| `SetKeyboardBacklightStatesJSON` | `s` → `b` | CONTROL | 1466 |
| `GetCustomKeyboardProfilesJSON` | `∅` → `s` | — | 1584 |
| `SaveCustomKeyboardProfile` | `sss` → `b` | CONTROL | 1599 |
| `DeleteCustomKeyboardProfile` | `s` → `b` | CONTROL | 1617 |
| `GetCustomFanProfilesJSON` | `∅` → `s` | — | 1633 |
| `SaveCustomFanProfile` | `sss` → `b` | CONTROL | 1647 |
| `DeleteCustomFanProfile` | `s` → `b` | CONTROL | 1660 |
| `GetFansMinSpeed` | `∅` → `i` | — | 1679 |
| `GetFansOffAvailable` | `∅` → `b` | — | 1684 |
| `GetChargingProfilesAvailable` | `∅` → `s` | — | 1691 |
| `GetCurrentChargingProfile` | `∅` → `s` | — | 1697 |
| `SetChargingProfile` | `s` → `b` | CONTROL | 1703 |
| `GetChargingPrioritiesAvailable` | `∅` → `s` | — | 1718 |
| `GetCurrentChargingPriority` | `∅` → `s` | — | 1724 |
| `SetChargingPriority` | `s` → `b` | CONTROL | 1730 |
| `GetChargeStartAvailableThresholds` | `∅` → `s` | — | 1745 |
| `GetChargeEndAvailableThresholds` | `∅` → `s` | — | 1751 |
| `GetChargeStartThreshold` | `∅` → `i` | — | 1757 |
| `GetChargeEndThreshold` | `∅` → `i` | — | 1762 |
| `SetChargeStartThreshold` | `i` → `b` | MANAGE_HARDWARE | 1767 |
| `SetChargeEndThreshold` | `i` → `b` | MANAGE_HARDWARE | 1782 |
| `GetChargeType` | `∅` → `s` | — | 1797 |
| `SetChargeType` | `s` → `b` | MANAGE_HARDWARE | 1803 |
| `GetFnLockSupported` | `∅` → `b` | — | 1820 |
| `GetFnLockStatus` | `∅` → `b` | — | 1825 |
| `SetFnLockStatus` | `b` → `∅` | CONTROL | 1830 |
| `SetSensorDataCollectionStatus` | `b` → `∅` | CONTROL | 1838 |
| `GetSensorDataCollectionStatus` | `∅` → `b` | — | 1844 |
| `SetDGpuD0Metrics` | `b` → `∅` | CONTROL | 1849 |
| `GetNVIDIAPowerCTRLDefaultPowerLimit` | `∅` → `i` | — | 1857 |
| `GetNVIDIAPowerCTRLMaxPowerLimit` | `∅` → `i` | — | 1862 |
| `GetNVIDIAPowerCTRLAvailable` | `∅` → `b` | — | 1867 |
| `SetNVIDIAPowerOffset` | `i` → `b` | CONTROL | 1872 |
| `GetAvailableGovernors` | `∅` → `s` | — | 1881 |
| `GetAvailableEPPs` | `∅` → `s` | — | 1901 |
| `GetCpuCoreCount` | `∅` → `i` | — | 1921 |
| `GetCpuTopologyJSON` | `∅` → `s` | — | 1927 |
| `GetWaterCoolerAvailable` | `∅` → `b` | — | 1935 |
| `GetWaterCoolerConnected` | `∅` → `b` | — | 1940 |
| `GetWaterCoolerFanSpeed` | `∅` → `i` | — | 1945 |
| `GetWaterCoolerPumpLevel` | `∅` → `i` | — | 1948 |
| `EnableWaterCooler` | `b` → `b` | CONTROL | 1951 |
| `IsWaterCoolerEnabled` | `∅` → `b` | — | 1976 |
| `SetWaterCoolerFanSpeed` | `i` → `b` | CONTROL | 1983 |
| `SetWaterCoolerPumpVoltage` | `i` → `b` | CONTROL | 2002 |
| `SetWaterCoolerLEDColor` | `iiii` → `b` | CONTROL | 2022 |
| `TurnOffWaterCoolerLED` | `∅` → `b` | CONTROL | 2039 |
| `TurnOffWaterCoolerFan` | `∅` → `b` | CONTROL | 2049 |
| `TurnOffWaterCoolerPump` | `∅` → `b` | CONTROL | 2059 |
| `IsWaterCoolerAutoControlEnabled` | `∅` → `b` | — | 2088 |
| `SetWaterCoolerAutoControl` | `b` → `b` | CONTROL | 2080 |
| `GetWaterCoolerSupported` | `∅` → `b` | — | 2097 |
| `GetCTGPAdjustmentSupported` | `∅` → `b` | — | 2102 |
| `GetHwpDynamicBoostSupported` | `∅` → `b` | — | 2107 |
| `GetMonitorDataSince` | `x` → `ay` | — | 2114 |
| `SetMonitorHistoryHorizon` | `i` → `∅` | CONTROL | 2123 |
| `GetMonitorHistoryHorizon` | `∅` → `i` | — | 2130 |
| `GetCpuFrequencyMHz` | `∅` → `i` | — | 2135 |

### Сигналы

- `ProfileChanged(const QString &profileId, const QString &keyboardProfileId, const QString &fanProfileId)`
- `ModeReapplyPendingChanged(bool pending)`
- `PowerStateChanged(const QString &state)`
- `PowerPolicyChanged(const QString &json)`
- `PowerDeviceStatusChanged(const QString &json)`
- `LightbarSettingsChanged(const QString &json)`
- `WaterCoolerStatusChanged(const QString &status)`

## Карта исходных файлов → новые компоненты

Ниже перечислены все отслеживаемые файлы кода основного проекта. Рецепты сборки, документация, изображения, результаты исследований и бинарные probe-файлы также учтены в JSON-манифесте. Назначение здесь архитектурное: это не предложение создавать отдельный Rust-файл для каждого старого файла.

| Исходный файл | Строк | Цель |
|---|---:|---|
| `contrib/frixit/capture-gallery.py` | 49 | ucc-migrate / xtask / historical tooling |
| `contrib/frixit/deploy-system.py` | 209 | ucc-migrate / xtask / historical tooling |
| `contrib/frixit/desktop-transition.py` | 115 | ucc-migrate / xtask / historical tooling |
| `contrib/frixit/migrate-controlcenter.py` | 167 | ucc-migrate / xtask / historical tooling |
| `contrib/frixit/prepare-deployment.py` | 138 | ucc-migrate / xtask / historical tooling |
| `contrib/frixit/tcc_import.py` | 151 | ucc-migrate / xtask / historical tooling |
| `contrib/mechrevo/migrate-settings.py` | 86 | ucc-migrate / xtask / historical tooling |
| `contrib/mechrevo/tests/test_migration.py` | 79 | ucc-migrate / xtask / historical tooling |
| `include/CommonTypes.hpp` | 58 | ucc-core / ucc-hardware / ucc-desktop |
| `include/DriverHealth.hpp` | 35 | ucc-core / ucc-hardware / ucc-desktop |
| `include/SysfsNode.hpp` | 298 | ucc-core / ucc-hardware / ucc-desktop |
| `include/Utils.hpp` | 254 | ucc-core / ucc-hardware / ucc-desktop |
| `include/WindowActivation.hpp` | 26 | ucc-core / ucc-hardware / ucc-desktop |
| `libucc-dbus/AsyncRead.hpp` | 47 | ucc-dbus |
| `libucc-dbus/GuiLauncher.hpp` | 57 | ucc-dbus |
| `libucc-dbus/PreviewMode.hpp` | 17 | ucc-dbus |
| `libucc-dbus/UccdClient.cpp` | 1309 | ucc-dbus |
| `libucc-dbus/UccdClient.hpp` | 280 | ucc-dbus |
| `run-preview.sh` | 8 | Cargo / xtask / distribution packaging |
| `tests/GuiVisualFixture.hpp` | 51 | Rust tests + migration contract fixtures |
| `tests/run_gnome_async_test.sh` | 13 | Rust tests + migration contract fixtures |
| `tests/run_gui_audit_test.sh` | 7 | Rust tests + migration contract fixtures |
| `tests/run_private_system_bus.sh` | 5 | Rust tests + migration contract fixtures |
| `tests/test_app_logging.cpp` | 84 | Rust tests + migration contract fixtures |
| `tests/test_aquaris_selection.cpp` | 14 | Rust tests + migration contract fixtures |
| `tests/test_bluetooth_retry.py` | 76 | Rust tests + migration contract fixtures |
| `tests/test_bluetooth_startup.py` | 104 | Rust tests + migration contract fixtures |
| `tests/test_control_authorization.py` | 41 | Rust tests + migration contract fixtures |
| `tests/test_controlcenter_migration.py` | 74 | Rust tests + migration contract fixtures |
| `tests/test_cpu_selection.cpp` | 83 | Rust tests + migration contract fixtures |
| `tests/test_deployment.py` | 145 | Rust tests + migration contract fixtures |
| `tests/test_desktop_transition.py` | 51 | Rust tests + migration contract fixtures |
| `tests/test_device_identification.py` | 83 | Rust tests + migration contract fixtures |
| `tests/test_display_backlight.cpp` | 66 | Rust tests + migration contract fixtures |
| `tests/test_display_identity.py` | 36 | Rust tests + migration contract fixtures |
| `tests/test_driver_alert.cpp` | 35 | Rust tests + migration contract fixtures |
| `tests/test_driver_health.cpp` | 30 | Rust tests + migration contract fixtures |
| `tests/test_fan_curve_editor.cpp` | 51 | Rust tests + migration contract fixtures |
| `tests/test_fan_profile.cpp` | 196 | Rust tests + migration contract fixtures |
| `tests/test_gnome_async.js` | 80 | Rust tests + migration contract fixtures |
| `tests/test_gnome_launch.cjs` | 21 | Rust tests + migration contract fixtures |
| `tests/test_gnome_refresh.cjs` | 77 | Rust tests + migration contract fixtures |
| `tests/test_gpu_sysfs.cpp` | 44 | Rust tests + migration contract fixtures |
| `tests/test_gui_audit.cpp` | 1013 | Rust tests + migration contract fixtures |
| `tests/test_gui_instance.cpp` | 98 | Rust tests + migration contract fixtures |
| `tests/test_import_check.py` | 63 | Rust tests + migration contract fixtures |
| `tests/test_ite8291_brightness.py` | 110 | Rust tests + migration contract fixtures |
| `tests/test_keyboard_controller.cpp` | 42 | Rust tests + migration contract fixtures |
| `tests/test_lightbar.cpp` | 107 | Rust tests + migration contract fixtures |
| `tests/test_metrics_history.cpp` | 190 | Rust tests + migration contract fixtures |
| `tests/test_notification_monitor.cpp` | 20 | Rust tests + migration contract fixtures |
| `tests/test_power_devices.cpp` | 98 | Rust tests + migration contract fixtures |
| `tests/test_power_policy.cpp` | 120 | Rust tests + migration contract fixtures |
| `tests/test_prepare_deployment.py` | 35 | Rust tests + migration contract fixtures |
| `tests/test_process_terminator.cpp` | 54 | Rust tests + migration contract fixtures |
| `tests/test_profile_manager.cpp` | 222 | Rust tests + migration contract fixtures |
| `tests/test_refresh_advisor.cpp` | 348 | Rust tests + migration contract fixtures |
| `tests/test_session_telemetry.cpp` | 43 | Rust tests + migration contract fixtures |
| `tests/test_settings_manager.cpp` | 171 | Rust tests + migration contract fixtures |
| `tests/test_sleep_install.py` | 91 | Rust tests + migration contract fixtures |
| `tests/test_sleep_state.cpp` | 182 | Rust tests + migration contract fixtures |
| `tests/test_state_utils.cpp` | 45 | Rust tests + migration contract fixtures |
| `tests/test_system_monitor.cpp` | 155 | Rust tests + migration contract fixtures |
| `tests/test_tcc_import.py` | 79 | Rust tests + migration contract fixtures |
| `tests/test_watercooler_override.py` | 136 | Rust tests + migration contract fixtures |
| `ucc-cli/main.cpp` | 2159 | ucc-cli |
| `ucc-desktop/DesktopTray.cpp` | 167 | ucc-desktop + ucc-desktop-tray |
| `ucc-desktop/DesktopTray.hpp` | 46 | ucc-desktop + ucc-desktop-tray |
| `ucc-desktop/ProcessTerminator.cpp` | 74 | ucc-desktop + ucc-desktop-tray |
| `ucc-desktop/ProcessTerminator.hpp` | 31 | ucc-desktop + ucc-desktop-tray |
| `ucc-desktop/SessionProcessesWidget.cpp` | 58 | ucc-desktop + ucc-desktop-tray |
| `ucc-desktop/SessionProcessesWidget.hpp` | 33 | ucc-desktop + ucc-desktop-tray |
| `ucc-desktop/SessionTelemetry.cpp` | 116 | ucc-desktop + ucc-desktop-tray |
| `ucc-desktop/SessionTelemetry.hpp` | 60 | ucc-desktop + ucc-desktop-tray |
| `ucc-desktop/main.cpp` | 18 | ucc-desktop + ucc-desktop-tray |
| `ucc-gnome/extension.js` | 1214 | shell integration decision (M1/M11) |
| `ucc-gnome/uccdClient.js` | 318 | shell integration decision (M1/M11) |
| `ucc-gui/inc/AboutPage.hpp` | 6 | ucc-gui |
| `ucc-gui/inc/AppLogging.hpp` | 14 | ucc-gui |
| `ucc-gui/inc/AssignKeyZones.hpp` | 9 | ucc-gui |
| `ucc-gui/inc/BatterySettingsWidget.hpp` | 107 | ucc-gui |
| `ucc-gui/inc/ColorPickerWidget.hpp` | 98 | ucc-gui |
| `ucc-gui/inc/CpuCoreSelectionWidget.hpp` | 141 | ucc-gui |
| `ucc-gui/inc/DashboardTab.hpp` | 158 | ucc-gui |
| `ucc-gui/inc/DesktopPreferences.hpp` | 21 | ucc-gui |
| `ucc-gui/inc/DetectLayout.hpp` | 13 | ucc-gui |
| `ucc-gui/inc/FanControlTab.hpp` | 168 | ucc-gui |
| `ucc-gui/inc/FanCurveEditorWidget.hpp` | 76 | ucc-gui |
| `ucc-gui/inc/FluentEntrance.hpp` | 23 | ucc-gui |
| `ucc-gui/inc/FluentSidebar.hpp` | 15 | ucc-gui |
| `ucc-gui/inc/FluentTheme.hpp` | 36 | ucc-gui |
| `ucc-gui/inc/FluentToggle.hpp` | 22 | ucc-gui |
| `ucc-gui/inc/GuiInstance.hpp` | 26 | ucc-gui |
| `ucc-gui/inc/HardwareTab.hpp` | 75 | ucc-gui |
| `ucc-gui/inc/KeyboardVisualizerWidget.hpp` | 140 | ucc-gui |
| `ucc-gui/inc/LCTWaterCoolerController.hpp` | 108 | ucc-gui |
| `ucc-gui/inc/LightbarTab.hpp` | 33 | ucc-gui |
| `ucc-gui/inc/MainWindow.hpp` | 275 | ucc-gui |
| `ucc-gui/inc/MonitorTab.hpp` | 298 | ucc-gui |
| `ucc-gui/inc/NotificationCenter.hpp` | 29 | ucc-gui |
| `ucc-gui/inc/PowerDevicesTab.hpp` | 36 | ucc-gui |
| `ucc-gui/inc/ProfileManager.hpp` | 169 | ucc-gui |
| `ucc-gui/inc/PumpCurveEditorWidget.hpp` | 83 | ucc-gui |
| `ucc-gui/inc/SystemMonitor.hpp` | 213 | ucc-gui |
| `ucc-gui/qml/FanControlTab.qml` | 43 | ucc-gui |
| `ucc-gui/qml/FanCurveEditor.qml` | 179 | ucc-gui |
| `ucc-gui/qml/main.qml` | 469 | ucc-gui |
| `ucc-gui/src/AboutPage.cpp` | 152 | ucc-gui |
| `ucc-gui/src/AppLogging.cpp` | 96 | ucc-gui |
| `ucc-gui/src/AssignKeyZones.cpp` | 934 | ucc-gui |
| `ucc-gui/src/DashboardTab.cpp` | 701 | ucc-gui |
| `ucc-gui/src/DetectLayout.cpp` | 259 | ucc-gui |
| `ucc-gui/src/FanControlTab.cpp` | 671 | ucc-gui |
| `ucc-gui/src/FanCurveEditorWidget.cpp` | 462 | ucc-gui |
| `ucc-gui/src/FluentEntrance.cpp` | 82 | ucc-gui |
| `ucc-gui/src/FluentSidebar.cpp` | 61 | ucc-gui |
| `ucc-gui/src/FluentTheme.cpp` | 265 | ucc-gui |
| `ucc-gui/src/GuiInstance.cpp` | 73 | ucc-gui |
| `ucc-gui/src/HardwareTab.cpp` | 174 | ucc-gui |
| `ucc-gui/src/KeyboardTab.cpp` | 598 | ucc-gui |
| `ucc-gui/src/KeyboardVisualizerWidget.cpp` | 498 | ucc-gui |
| `ucc-gui/src/LCTWaterCoolerController.cpp` | 306 | ucc-gui |
| `ucc-gui/src/LightbarTab.cpp` | 107 | ucc-gui |
| `ucc-gui/src/MainWindow.cpp` | 3129 | ucc-gui |
| `ucc-gui/src/MonitorTab.cpp` | 2088 | ucc-gui |
| `ucc-gui/src/NotificationCenter.cpp` | 92 | ucc-gui |
| `ucc-gui/src/PowerDevicesTab.cpp` | 184 | ucc-gui |
| `ucc-gui/src/ProfileManager.cpp` | 882 | ucc-gui |
| `ucc-gui/src/PumpCurveEditorWidget.cpp` | 396 | ucc-gui |
| `ucc-gui/src/SystemMonitor.cpp` | 352 | ucc-gui |
| `ucc-gui/src/main.cpp` | 134 | ucc-gui |
| `ucc-session/DriverAlert.hpp` | 39 | ucc-session-agent + ucc-desktop |
| `ucc-session/NotificationMonitor.hpp` | 48 | ucc-session-agent + ucc-desktop |
| `ucc-session/PowerSourceAlert.hpp` | 43 | ucc-session-agent + ucc-desktop |
| `ucc-session/RefreshAdvisor.cpp` | 444 | ucc-session-agent + ucc-desktop |
| `ucc-session/RefreshAdvisor.hpp` | 80 | ucc-session-agent + ucc-desktop |
| `ucc-session/SessionSignals.hpp` | 29 | ucc-session-agent + ucc-desktop |
| `ucc-session/main.cpp` | 71 | ucc-session-agent + ucc-desktop |
| `ucc-tray/applet/DashboardTab.qml` | 211 | shell integration decision (M1/M11) |
| `ucc-tray/applet/HardwareTab.qml` | 116 | shell integration decision (M1/M11) |
| `ucc-tray/applet/PowerProfileTab.qml` | 170 | shell integration decision (M1/M11) |
| `ucc-tray/applet/WaterCoolerTab.qml` | 245 | shell integration decision (M1/M11) |
| `ucc-tray/applet/main.qml` | 192 | shell integration decision (M1/M11) |
| `ucc-tray/src/TrayBackend.cpp` | 906 | shell integration decision (M1/M11) |
| `ucc-tray/src/TrayBackend.hpp` | 355 | shell integration decision (M1/M11) |
| `ucc-tray/src/plugin.cpp` | 16 | shell integration decision (M1/M11) |
| `ucc-tray/src/plugin.h` | 17 | shell integration decision (M1/M11) |
| `uccd/3rdparty/tuxedo_io_lib/targets/clevo.h` | 238 | ucc-hardware / ioctl ABI; N-API → archive |
| `uccd/3rdparty/tuxedo_io_lib/targets/dummy.h` | 107 | ucc-hardware / ioctl ABI; N-API → archive |
| `uccd/3rdparty/tuxedo_io_lib/targets/interface.h` | 56 | ucc-hardware / ioctl ABI; N-API → archive |
| `uccd/3rdparty/tuxedo_io_lib/targets/uniwill.h` | 378 | ucc-hardware / ioctl ABI; N-API → archive |
| `uccd/3rdparty/tuxedo_io_lib/tuxedo_io.h` | 146 | ucc-hardware / ioctl ABI; N-API → archive |
| `uccd/3rdparty/tuxedo_io_lib/tuxedo_io_api.hh` | 141 | ucc-hardware / ioctl ABI; N-API → archive |
| `uccd/3rdparty/tuxedo_io_lib/tuxedo_io_ioctl.h` | 112 | ucc-hardware / ioctl ABI; N-API → archive |
| `uccd/3rdparty/tuxedo_io_napi.cc` | 369 | ucc-hardware / ioctl ABI; N-API → archive |
| `uccd/inc/AquarisSelection.hpp` | 18 | ucc-cooler |
| `uccd/inc/AutosaveManager.hpp` | 203 | ucc-core + ucc-config |
| `uccd/inc/CpuController.hpp` | 547 | ucc-hardware / uccd / packaging |
| `uccd/inc/CpuTopology.hpp` | 56 | ucc-hardware / uccd / packaging |
| `uccd/inc/DisplayBacklight.hpp` | 135 | ucc-hardware / uccd / packaging |
| `uccd/inc/FnLockController.hpp` | 78 | ucc-hardware / uccd / packaging |
| `uccd/inc/GpuSysfs.hpp` | 67 | ucc-telemetry + ucc-hardware |
| `uccd/inc/KeyboardBacklightController.hpp` | 545 | ucc-lighting + ucc-hardware |
| `uccd/inc/LightbarController.hpp` | 201 | ucc-lighting + ucc-hardware |
| `uccd/inc/MetricsHistoryStore.hpp` | 276 | ucc-telemetry + ucc-hardware |
| `uccd/inc/NvmlWrapper.hpp` | 437 | ucc-telemetry + ucc-hardware |
| `uccd/inc/PolkitAuthority.hpp` | 158 | ucc-hardware / uccd / packaging |
| `uccd/inc/PowerDeviceController.hpp` | 231 | ucc-hardware / uccd / packaging |
| `uccd/inc/PowerPolicy.hpp` | 97 | ucc-hardware / uccd / packaging |
| `uccd/inc/PowerSupplyController.hpp` | 290 | ucc-hardware / uccd / packaging |
| `uccd/inc/ProfileManager.hpp` | 1057 | ucc-core + ucc-config |
| `uccd/inc/SettingsManager.hpp` | 354 | ucc-core + ucc-config |
| `uccd/inc/SleepState.hpp` | 186 | ucc-sleep + ucc-config |
| `uccd/inc/StateUtils.hpp` | 48 | ucc-hardware / uccd / packaging |
| `uccd/inc/SystemInfo.hpp` | 78 | ucc-hardware / uccd / packaging |
| `uccd/inc/TccSettings.hpp` | 74 | ucc-core + ucc-config |
| `uccd/inc/UccDBusService.hpp` | 671 | uccd + ucc-dbus + ucc-core |
| `uccd/inc/profiles/DefaultProfiles.hpp` | 90 | ucc-core + ucc-config |
| `uccd/inc/profiles/FanProfile.hpp` | 240 | ucc-core + ucc-config |
| `uccd/inc/profiles/UccProfile.hpp` | 277 | ucc-core + ucc-config |
| `uccd/inc/workers/CpuWorker.hpp` | 532 | ucc-hardware / uccd / packaging |
| `uccd/inc/workers/DaemonWorker.hpp` | 256 | ucc-hardware / uccd / packaging |
| `uccd/inc/workers/DisplayWorker.hpp` | 161 | ucc-hardware / uccd / packaging |
| `uccd/inc/workers/FanControlWorker.hpp` | 600 | ucc-cooling |
| `uccd/inc/workers/HardwareMonitorWorker.hpp` | 422 | ucc-telemetry + ucc-hardware |
| `uccd/inc/workers/LCTWaterCoolerWorker.hpp` | 305 | ucc-cooler |
| `uccd/inc/workers/ProfileSettingsWorker.hpp` | 288 | ucc-hardware / uccd / packaging |
| `uccd/src/NvmlWrapper.cpp` | 913 | ucc-telemetry + ucc-hardware |
| `uccd/src/SystemInfo.cpp` | 583 | ucc-hardware / uccd / packaging |
| `uccd/src/UccDBusService.cpp` | 4869 | uccd + ucc-dbus + ucc-core |
| `uccd/src/hardware-helper.cpp` | 32 | ucc-hardware / uccd / packaging |
| `uccd/src/import-check.cpp` | 107 | ucc-hardware / uccd / packaging |
| `uccd/src/profiles/DefaultProfiles.cpp` | 288 | ucc-core + ucc-config |
| `uccd/src/profiles/FanProfiles.cpp` | 281 | ucc-core + ucc-config |
| `uccd/src/profiles/UccProfile.cpp` | 63 | ucc-core + ucc-config |
| `uccd/src/sleep-helper.cpp` | 116 | ucc-sleep + ucc-config |
| `uccd/src/uccd.cpp` | 467 | uccd + ucc-dbus + ucc-core |
| `uccd/src/workers/DisplayWorker.cpp` | 592 | ucc-hardware / uccd / packaging |
| `uccd/src/workers/HardwareMonitorWorker.cpp` | 1005 | ucc-telemetry + ucc-hardware |
| `uccd/src/workers/LCTWaterCoolerWorker.cpp` | 1377 | ucc-cooler |
| `uccd/src/workers/ProfileSettingsWorker.cpp` | 710 | ucc-hardware / uccd / packaging |

| `nvidia_findings/clockboost_scan.c` | 183 | research archive; runtime subset → ucc-hardware |
| `nvidia_findings/deep_fields.c` | 410 | research archive; runtime subset → ucc-hardware |
| `nvidia_findings/deep_nvapi.c` | 459 | research archive; runtime subset → ucc-hardware |
| `nvidia_findings/deep_nvml_probe.c` | 636 | research archive; runtime subset → ucc-hardware |
| `nvidia_findings/final_detail.c` | 121 | research archive; runtime subset → ucc-hardware |
| `nvidia_findings/nvml_detail.c` | 218 | research archive; runtime subset → ucc-hardware |
| `nvidia_findings/nvml_fine_scan.c` | 246 | research archive; runtime subset → ucc-hardware |
| `nvidia_findings/nvml_scan2.c` | 151 | research archive; runtime subset → ucc-hardware |
| `nvidia_findings/probe_nvapi.c` | 85 | research archive; runtime subset → ucc-hardware |
| `nvidia_findings/probe_nvapi_data.c` | 309 | research archive; runtime subset → ucc-hardware |

## Исторический Python ControlCenter

Commit: `ba6fd4ae793bcd345ac39cdef0b4103841eee2e7`. Архив используется как источник исходного поведения; его FastAPI/Unix socket/SSE не является текущим API UCC.

| Исходный файл | Строк |
|---|---:|
| `daemon/api/__init__.py` | 2 |
| `daemon/api/models.py` | 16 |
| `daemon/api/server.py` | 142 |
| `daemon/core/__init__.py` | 2 |
| `daemon/core/config.py` | 21 |
| `daemon/core/engine.py` | 122 |
| `daemon/core/hardware.py` | 195 |
| `daemon/core/paths.py` | 25 |
| `daemon/core/scanner.py` | 168 |
| `daemon/main.py` | 131 |
| `daemon/tests/test_power.py` | 322 |
| `daemon/tray.py` | 803 |
| `daemon/tray_gtk.py` | 907 |
| `gui/__init__.py` | 5 |
| `gui/core/__init__.py` | 3 |
| `gui/core/config.py` | 20 |
| `gui/core/daemon.py` | 90 |
| `gui/core/display.py` | 88 |
| `gui/core/gui_settings.py` | 37 |
| `gui/core/lightbar.py` | 105 |
| `gui/core/metrics.py` | 123 |
| `gui/core/paths.py` | 29 |
| `gui/core/poller.py` | 121 |
| `gui/install-autostart.sh` | 190 |
| `gui/kbd_control.py` | 239 |
| `gui/main.py` | 100 |
| `gui/ui/__init__.py` | 3 |
| `gui/ui/app_icon.py` | 60 |
| `gui/ui/main_window.py` | 247 |
| `gui/ui/styles.py` | 365 |
| `gui/ui/tabs/__init__.py` | 3 |
| `gui/ui/tabs/dashboard.py` | 323 |
| `gui/ui/tabs/devices.py` | 249 |
| `gui/ui/tabs/keyboard_tab.py` | 989 |
| `gui/ui/tabs/lightbar.py` | 367 |
| `gui/ui/tabs/power_settings.py` | 229 |
| `gui/ui/tabs/settings_tab.py` | 258 |
| `gui/ui/theme.py` | 154 |
| `gui/ui/widgets.py` | 211 |
| `launch.sh` | 42 |
