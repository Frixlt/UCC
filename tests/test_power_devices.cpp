// SPDX-License-Identifier: GPL-3.0-or-later
#include <QTest>
#include <QTemporaryDir>
#include "PowerDeviceController.hpp"

class TestPowerDevices:public QObject {
  Q_OBJECT
  using Json=nlohmann::json;
  static Json device(const char *type,const char *path,const char *ac="on",const char *tc="auto",const char *bat="auto") {
    return {{"type",type},{"path",path},{"name","Test device"},{"ac_val",ac},{"type_c_val",tc},{"bat_val",bat}};
  }
  static void file(const std::filesystem::path &path,const std::string &value) {
    std::filesystem::create_directories(path.parent_path());std::ofstream(path)<<value;
  }
  static std::string read(const std::filesystem::path &path) {std::string value;std::ifstream(path)>>value;return value;}
private slots:
  void appliesEveryModeWithoutTouchingRealSysfs() {
    QTemporaryDir root;const auto sys=std::filesystem::path(root.path().toStdString())/"sys";
    const auto proc=std::filesystem::path(root.path().toStdString())/"proc";
    const auto path="/sys/bus/pci/devices/0000:02:00.0/power/control";
    file(sys/"bus/pci/devices/0000:02:00.0/power/control","auto");
    file(proc/"sys/vm/dirty_writeback_centisecs","500");
    std::vector<std::string> commands;
    PowerDeviceController controller(sys,proc,[&](const auto &cmd,const auto &args){commands.push_back(cmd+":"+args.back());return true;});
    Json config={{"enabled",true},{"devices",Json::array({device("pci",path),
      device("sys","/proc/sys/vm/dirty_writeback_centisecs","500","1500","2000"),
      device("wifi","wlan0","off","on","on"),device("service","nvidia-powerd","active","inactive","inactive")})}};
    std::string error;QVERIFY2(controller.setSettings(config,error),error.c_str());
    controller.apply("ac");QCOMPARE(read(sys/"bus/pci/devices/0000:02:00.0/power/control"),std::string("on"));
    controller.apply("type_c");QCOMPARE(read(proc/"sys/vm/dirty_writeback_centisecs"),std::string("1500"));
    controller.apply("bat");QCOMPARE(read(sys/"bus/pci/devices/0000:02:00.0/power/control"),std::string("auto"));
    QCOMPARE(read(proc/"sys/vm/dirty_writeback_centisecs"),std::string("2000"));
    QCOMPARE(commands.size(),size_t(6));
    QVERIFY(controller.status()["devices"][0]["ok"].get<bool>());
  }
  void rejectsArbitraryPathsServicesAndInvalidValues() {
    for(auto entry:{device("sys","/etc/shadow","0","0","0"),device("service","sshd","active","active","active"),
        device("usb","/sys/bus/usb/devices/../module/power/control"),device("wifi","wlan0;reboot","on","on","on"),
        device("pci","/sys/bus/pci/devices/0000:02:00.0/power/control","unsafe")}) {
      std::string error;QVERIFY(!PowerDeviceController::validate({{"devices",Json::array({entry})}},error));
    }
  }
  void rescanPreservesManualAndMissingDevicesAndTurbo() {
    QTemporaryDir root;const auto sys=std::filesystem::path(root.path().toStdString())/"sys";
    file(sys/"bus/pci/devices/0000:00:00.0/power/control","on");
    file(sys/"devices/system/cpu/intel_pstate/no_turbo","0");
    auto saved=device("usb","/sys/bus/usb/devices/1-7/power/control","on","on","on");
    PowerDeviceController controller(sys,sys/"fake-proc",{},sys/"bin");std::string error;
    QVERIFY(controller.setSettings({{"devices",Json::array({saved})}},error));
    const auto scanned=controller.scan();
    QCOMPARE(scanned["devices"].size(),size_t(3));
    QCOMPARE(scanned["devices"][0].dump(),saved.dump());
    QVERIFY(PowerDeviceController::validate(scanned,error));
    bool turbo=false;for(const auto &entry:scanned["devices"]) turbo|=entry["path"]=="/sys/devices/system/cpu/intel_pstate/no_turbo";
    QVERIFY(turbo);
  }
  void cpuOverlayAndBlackwellGuardAgreeWithDeviceTable() {
    QTemporaryDir root;const auto sys=std::filesystem::path(root.path().toStdString())/"sys";
    const auto base=sys/"bus/pci/devices/0000:02:00.0";
    file(base/"vendor","0x10de");file(base/"class","0x030000");file(base/"power/control","auto");
    file(sys/"devices/system/cpu/intel_pstate/no_turbo","0");
    PowerDeviceController controller(sys,sys/"proc");std::string error;
    QVERIFY(controller.setSettings({{"enabled",true},{"keepNvidiaAwake",true},
      {"cpu",{{"bat",{{"governor","powersave"},{"epp","power"}}}}},
      {"devices",Json::array({device("pci","/sys/bus/pci/devices/0000:02:00.0/power/control"),
        device("sys","/sys/devices/system/cpu/intel_pstate/no_turbo","0","0","1")})}},error));
    UccProfile profile;controller.overlayCpu(profile,"bat");
    QCOMPARE(profile.cpu.governor,std::string("powersave"));QVERIFY(profile.cpu.noTurbo);
    controller.apply("bat");QCOMPARE(read(base/"power/control"),std::string("on"));
    QVERIFY(controller.status()["devices"][0]["guarded"].get<bool>());
  }
  void absentHardwareIsReportedAsFailure() {
    QTemporaryDir root;PowerDeviceController controller(root.path().toStdString(),root.path().toStdString());std::string error;
    QVERIFY(controller.setSettings({{"enabled",true},{"devices",Json::array({device("pci","/sys/bus/pci/devices/0000:02:00.0/power/control")})}},error));
    controller.apply("ac");QVERIFY(!controller.status()["devices"][0]["ok"].get<bool>());
  }
  void disablingKeepAwakeAndSwitchingToBatteryUndoPersistence() {
    QTemporaryDir root;const auto sys=std::filesystem::path(root.path().toStdString())/"sys",proc=sys/"proc";
    const auto gpu=sys/"bus/pci/devices/0000:02:00.0";
    file(gpu/"vendor","0x10de");file(gpu/"class","0x030000");file(gpu/"power/control","auto");
    file(proc/"driver/nvidia/version","test");
    std::vector<std::string> calls;
    PowerDeviceController controller(sys,proc,[&](const auto& cmd,const auto& args) {
      calls.push_back(cmd+":"+args.back());return true;
    });
    auto config=Json{{"enabled",true},{"keepNvidiaAwake",true},{"devices",Json::array({device("pci","/sys/bus/pci/devices/0000:02:00.0/power/control")})}};
    std::string error;QVERIFY(controller.setSettings(config,error));
    controller.apply("bat");QCOMPARE(calls.back(),std::string("nvidia-smi:1"));
    config["keepNvidiaAwake"]=false;QVERIFY(controller.setSettings(config,error));
    controller.apply("bat");QCOMPARE(calls.back(),std::string("nvidia-smi:0"));
    QCOMPARE(read(gpu/"power/control"),std::string("auto"));
    controller.apply("ac");QCOMPARE(calls.back(),std::string("nvidia-smi:1"));
    controller.apply("type_c");QCOMPARE(calls.back(),std::string("nvidia-smi:0"));
    QVERIFY(!controller.status()["nvidiaPersistenceEnabled"].get<bool>());
  }
};
QTEST_GUILESS_MAIN(TestPowerDevices)
#include "test_power_devices.moc"
