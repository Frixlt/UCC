// SPDX-License-Identifier: GPL-3.0-or-later
#include <QTest>
#include <QTemporaryDir>
#include "KeyboardBacklightController.hpp"

class TestKeyboardController : public QObject {
  Q_OBJECT
  static std::string node(const QString &root, const char *name) {
    const auto path=std::filesystem::path(root.toStdString())/name;
    std::filesystem::create_directory(path);
    std::ofstream(path/"brightness") << 0;
    std::ofstream(path/"multi_intensity") << "0 0 0";
    return path.string();
  }
  static std::string contents(const std::string &path) {
    std::ifstream file(path); return {std::istreambuf_iterator<char>(file),{}};
  }
private slots:
  void brightnessAndColorsReachEveryPhysicalLed() {
    QTemporaryDir root;
    const auto first=node(root.path(),"rgb:kbd_backlight");
    const auto second=node(root.path(),"rgb:kbd_backlight_56");
    KeyboardBacklightController controller({first,second},{2,50,255,255,255});
    QVERIFY(controller.applyStatesFromJSON(R"([{"brightness":17,"red":1,"green":2,"blue":3},{"brightness":17,"red":4,"green":5,"blue":6}])"));
    QCOMPARE(contents(first+"/brightness"),std::string("17"));
    QCOMPARE(contents(second+"/brightness"),std::string("17"));
    QCOMPARE(contents(first+"/multi_intensity"),std::string("1 2 3"));
    QCOMPARE(contents(second+"/multi_intensity"),std::string("4 5 6"));
  }
  void failedHardwareWriteDoesNotReportSuccess() {
    QTemporaryDir root;
    const auto path=node(root.path(),"rgb:kbd_backlight");
    KeyboardBacklightController controller({path},{1,50,255,255,255});
    QVERIFY(controller.applyStatesFromJSON(R"([{"brightness":20,"red":10,"green":20,"blue":30}])"));
    const auto before=controller.currentStatesJSON();
    std::filesystem::remove(path+"/multi_intensity");
    QVERIFY(!controller.applyStatesFromJSON(R"([{"brightness":20,"red":100,"green":20,"blue":30}])"));
    QCOMPARE(controller.currentStatesJSON(),before);
  }
};
QTEST_GUILESS_MAIN(TestKeyboardController)
#include "test_keyboard_controller.moc"
