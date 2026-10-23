// SPDX-License-Identifier: GPL-3.0-or-later
#include <QtTest>
#include <QTemporaryDir>
#include "DisplayBacklight.hpp"
namespace fs = std::filesystem;
class DisplayBacklightTest : public QObject {
  Q_OBJECT
  void put(const fs::path &path, const std::string &value) {
    fs::create_directories(path.parent_path()); std::ofstream(path) << value;
  }
private slots:
  void hybridPanelAndMuxSwitch() {
    QTemporaryDir dir; const fs::path root = dir.path().toStdString();
    const auto intel = root/"devices/intel/drm/card1/card1-eDP-1/intel_backlight";
    const auto nvidia = root/"devices/nvidia/backlight/nvidia_0";
    const auto ni = root/"devices/nvidia/drm/card0/card0-eDP-2";
    for (const auto &device : {intel,nvidia}) { put(device/"brightness","65"); put(device/"max_brightness","100"); }
    fs::create_directories(root/"class/backlight");
    fs::create_directory_symlink(nvidia,root/"class/backlight/nvidia_0");
    fs::create_directory_symlink(intel,root/"class/backlight/intel_backlight");
    put(intel.parent_path()/"status","connected"); put(intel.parent_path()/"enabled","enabled");
    put(ni/"status","disconnected"); put(ni/"enabled","disabled");
    auto selected = display_backlight::select(root/"class/backlight");
    QVERIFY(selected); QCOMPARE(fs::path(selected->path).filename().string(),std::string("intel_backlight"));
    put(intel.parent_path()/"status","disconnected"); put(intel.parent_path()/"enabled","disabled");
    put(ni/"status","connected"); put(ni/"enabled","enabled");
    selected = display_backlight::select(root/"class/backlight");
    QVERIFY(selected); QCOMPARE(fs::path(selected->path).filename().string(),std::string("nvidia_0"));
    put(ni/"enabled","disabled"); QVERIFY(!display_backlight::select(root/"class/backlight"));
  }
  void genericFallbackAndInvalidDevices() {
    QTemporaryDir dir; const fs::path root = dir.path().toStdString();
    put(root/"firmware/brightness","40"); put(root/"firmware/max_brightness","100");
    put(root/"broken/brightness","40"); put(root/"broken/max_brightness","0");
    auto selected = display_backlight::select(root); QVERIFY(selected);
    QCOMPARE(fs::path(selected->path).filename().string(),std::string("firmware"));
    put(root/"native/brightness","40"); put(root/"native/max_brightness","100");
    put(root/"native/type","raw"); put(root/"firmware/type","firmware");
    selected = display_backlight::select(root); QVERIFY(selected);
    QCOMPARE(fs::path(selected->path).filename().string(),std::string("native"));
    fs::remove_all(root/"native"); fs::remove_all(root/"firmware"); QVERIFY(!display_backlight::select(root));
    QVERIFY(!display_backlight::select(root/"missing"));
  }
  void liveReadRoundingAndFailedWrite() {
    QTemporaryDir dir; const fs::path base = dir.path().toStdString();
    put(base/"brightness","273");
    DisplayBacklightController controller(base.string(),496,false);
    QCOMPARE(controller.getBrightness(),55);
    QVERIFY(controller.setBrightness(65)); QCOMPARE(controller.getBrightness(),65);
    QCOMPARE(SysfsNode<int>((base/"brightness").string()).read().value(),322);
    put(base/"brightness","99"); QCOMPARE(controller.getBrightness(),20);
    QVERIFY(controller.setBrightness(120)); QCOMPARE(controller.getBrightness(),100);
    fs::remove(base/"brightness"); fs::create_directory(base/"brightness");
    QVERIFY(!controller.setBrightness(20)); QCOMPARE(controller.getBrightness(),-1);
    fs::remove(base/"brightness"); fs::create_symlink("/dev/full",base/"brightness");
    QVERIFY(!controller.setBrightness(20)); // catches deferred write/flush errors
  }
  void existingAmdInversion() {
    QTemporaryDir dir; const fs::path base = dir.path().toStdString(); put(base/"brightness","80");
    DisplayBacklightController controller(base.string(),100,true);
    QCOMPARE(controller.getBrightness(),20); QVERIFY(controller.setBrightness(30));
    QCOMPARE(SysfsNode<int>((base/"brightness").string()).read().value(),70);
  }
};
QTEST_GUILESS_MAIN(DisplayBacklightTest)
#include "test_display_backlight.moc"
