// SPDX-License-Identifier: GPL-3.0-or-later
#include <QTemporaryDir>
#include <QTest>
#include "LidController.hpp"
#include "LidDisplayHandler.hpp"
#include "WakeupPolicy.hpp"
#include <filesystem>
#include <fstream>
#include <string>
#include <utility>
#include <vector>

namespace {
void writeText(const std::filesystem::path &path, const std::string &content) {
  std::filesystem::create_directories(path.parent_path());
  std::ofstream file(path);
  file << content;
}

std::string readText(const std::filesystem::path &path) {
  std::ifstream file(path);
  std::string content;
  std::getline(file, content);
  return content;
}
} // namespace

class TestLidWakeup : public QObject {
  Q_OBJECT
private slots:
  void wakeupPolicyAllowsOnlyLidAndBuiltinKeyboard() {
    QTemporaryDir root;
    const auto base = std::filesystem::path(root.path().toStdString());
    const auto sys = base / "sys";
    const auto proc = base / "proc";

    writeText(sys / "devices/platform/PNP0C0D:01/power/wakeup", "disabled\n");
    writeText(sys / "devices/platform/i8042/serio0/description", "i8042 KBD port\n");
    writeText(sys / "devices/platform/i8042/serio0/power/wakeup", "disabled\n");
    writeText(sys / "devices/platform/i8042/serio1/description", "i8042 AUX port\n");
    writeText(sys / "devices/platform/i8042/serio1/power/wakeup", "enabled\n");
    writeText(sys / "devices/pci0000:80/0000:80:14.0/usb3/3-1/3-1.2/power/wakeup", "enabled\n");
    writeText(sys / "devices/platform/ACPI0003:00/power_supply/AC0/power/wakeup", "enabled\n");
    writeText(sys / "devices/platform/rtc_cmos/power/wakeup", "enabled\n");
    writeText(proc / "acpi/wakeup",
              "Device\tS-state\t  Status   Sysfs node\n"
              "XHCI\t  S4\t*enabled   pci:0000:80:14.0\n"
              "AWAC\t  S4\t*enabled   platform:ACPI000E:00\n"
              "GLAN\t  S5\t*disabled\n");

    const auto res = applyWakeupPolicy(true, sys, proc);
    QCOMPARE(res.enabled, 2);
    QCOMPARE(res.disabled, 6);
    QCOMPARE(res.failed, 0);

    QCOMPARE(readText(sys / "devices/platform/PNP0C0D:01/power/wakeup"), std::string("enabled"));
    QCOMPARE(readText(sys / "devices/platform/i8042/serio0/power/wakeup"), std::string("enabled"));
    QCOMPARE(readText(sys / "devices/platform/i8042/serio1/power/wakeup"), std::string("disabled"));
    QCOMPARE(readText(sys / "devices/pci0000:80/0000:80:14.0/usb3/3-1/3-1.2/power/wakeup"), std::string("disabled"));
    QCOMPARE(readText(sys / "devices/platform/ACPI0003:00/power_supply/AC0/power/wakeup"), std::string("disabled"));
    QCOMPARE(readText(sys / "devices/platform/rtc_cmos/power/wakeup"), std::string("disabled"));
  }

  void wakeupPolicyDisabledLeavesSourcesUntouched() {
    QTemporaryDir root;
    const auto sys = std::filesystem::path(root.path().toStdString()) / "sys";
    writeText(sys / "devices/pci0000:80/0000:80:14.0/usb3/3-1/3-1.2/power/wakeup", "enabled\n");
    const auto res = applyWakeupPolicy(false, sys, root.path().toStdString());
    QCOMPARE(res.enabled, 0);
    QCOMPARE(res.disabled, 0);
    QCOMPARE(readText(sys / "devices/pci0000:80/0000:80:14.0/usb3/3-1/3-1.2/power/wakeup"), std::string("enabled"));
  }

  void lidControllerHandlesAcBatteryLockAndUnplugDebounce() {
    std::vector<std::string> events;
    LidActions actions;
    actions.powerChangeDebounceMs = 80;
    actions.applyWakeup = [&](bool wakeOnly) {
      events.push_back(std::string("wakeup:") + (wakeOnly ? "true" : "false"));
    };
    actions.performAction = [&](const std::string &action) {
      events.push_back("action:" + action);
    };

    LidController ctrl(false, actions);
    ctrl.setSettings({{"ac", "screen_off"}, {"type_c", "lock"}, {"bat", "suspend"}, {"wakeOnlyLidAndKeyboard", true}});
    QCOMPARE(events.size(), size_t(1));
    QCOMPARE(events[0], std::string("wakeup:true"));
    events.clear();

    // Initial state open
    ctrl.onLidStateChanged(false);
    // Close on AC -> screen_off (no daemon suspend/lock)
    ctrl.setPowerSource("ac");
    ctrl.onLidStateChanged(true);
    QVERIFY(events.empty());

    // Unplug to battery while closed -> triggers suspend after debounce
    ctrl.setPowerSource("bat");
    QTRY_COMPARE_WITH_TIMEOUT(events.size(), size_t(2), 1000);
    QCOMPARE(events[0], std::string("wakeup:true"));
    QCOMPARE(events[1], std::string("action:suspend"));
    events.clear();

    // Reopen, plug AC, close -> unplug to bat -> reopen before debounce cancels suspend
    ctrl.onLidStateChanged(false);
    ctrl.setPowerSource("ac");
    ctrl.onLidStateChanged(true);
    ctrl.setPowerSource("bat");
    ctrl.onLidStateChanged(false);
    QTest::qWait(150);
    QVERIFY(events.empty());

    // Close on Type-C (lock) -> immediate lock action
    ctrl.setPowerSource("type_c");
    ctrl.onLidStateChanged(true);
    QCOMPARE(events.size(), size_t(1));
    QCOMPARE(events[0], std::string("action:lock"));
  }

  void lidDisplayHandlerControlsInternalPanelDpms() {
    std::vector<std::pair<QString, QStringList>> calls;
    ucc::LidDisplayHandler handler(
        ucc::RefreshBackend::Hyprland, "hyprctl", false,
        [&](const QString &prog, const QStringList &args) { calls.emplace_back(prog, args); });

    handler.deviceSettingsChanged(
        R"({"lid":{"ac":"screen_off","type_c":"lock","bat":"suspend","wakeOnlyLidAndKeyboard":true}})");
    handler.policyChanged(R"({"real_power_source":"ac"})");

    handler.onLidStateChanged(false); // initial state
    QVERIFY(calls.empty());

    // Close on AC (screen_off) -> dpms off
    handler.onLidStateChanged(true);
    QCOMPARE(calls.size(), size_t(1));
    QVERIFY(calls[0].second.join(' ').contains("dispatch dpms off"));

    // Open -> dpms on
    handler.onLidStateChanged(false);
    QCOMPARE(calls.size(), size_t(2));
    QVERIFY(calls[1].second.join(' ').contains("dispatch dpms on"));

    // Close on Battery (suspend) -> no DPMS off
    calls.clear();
    handler.policyChanged(R"({"real_power_source":"bat"})");
    handler.onLidStateChanged(true);
    QVERIFY(calls.empty());
  }
};

QTEST_GUILESS_MAIN(TestLidWakeup)
#include "test_lid_wakeup.moc"
