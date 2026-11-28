// SPDX-License-Identifier: GPL-3.0-or-later
// Standalone QtTest reproduction suite for confirmed findings on commit 1d07126
// (N02, N07, N08, N09, N10, N11, N12, N13, N26, N28, N31, P0-14, P0-15, M-GUI-05).
//
// Drop-in CMake usage (in tests/CMakeLists.txt alongside test_gui_audit):
//   add_executable(test_gui_regressions test_gui_regressions.cpp ${UCC_GUI_AUDIT_SOURCES})
//   target_include_directories(test_gui_regressions PRIVATE ../ucc-gui/inc ../include ../libucc-dbus)
//   target_link_libraries(test_gui_regressions PRIVATE ucc-desktop-tray-support ucc-dbus
//     Qt6::Widgets Qt6::Charts Qt6::Test ucc-window-activation)
//   add_test(NAME test_gui_regressions
//     COMMAND ${DBUS_RUN_SESSION_EXECUTABLE} -- bash ${CMAKE_CURRENT_SOURCE_DIR}/run_gui_audit_test.sh
//       $<TARGET_FILE:test_gui_regressions>)

#include <QtTest>
#include <QApplication>
#include <QComboBox>
#include <QDBusConnection>
#include <QDBusMessage>
#include <QDBusVirtualObject>
#include <QDir>
#include <QFile>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QListWidget>
#include <QMouseEvent>
#include <QProcess>
#include <QPushButton>
#include <QScopeGuard>
#include <QSlider>
#include <QStatusBar>
#include <QTabWidget>
#include <QTemporaryDir>
#include <QCheckBox>
#include <QElapsedTimer>
#include <QGraphicsItem>
#include <QGraphicsRectItem>
#include <QGraphicsScene>
#include <QTextStream>
#include <QTimer>
#include <QtCharts/QChartView>

#include <cmath>
#include <memory>
#include <sys/stat.h>
#include <unistd.h>

#include "FanControlTab.hpp"
#include "FanCurveEditorWidget.hpp"
#include "FluentTheme.hpp"
#include "GuiVisualFixture.hpp"
#include "MainWindow.hpp"
#include "PreviewMode.hpp"
#include "ProfileManager.hpp"
#include "SysfsNode.hpp"
#include "HardwareTab.hpp"
#include "KeyboardVisualizerWidget.hpp"
#include "MonitorTab.hpp"
#include "SystemMonitor.hpp"
#include "UccdClient.hpp"

namespace {

class ReproAuditDaemon : public QDBusVirtualObject {
  QJsonArray m_writes;
  QString m_customProfiles;
  QString m_defaultProfiles;
  QString m_activeProfile;
  QString m_settingsJson;
  QString m_keyboardInfoJson;
  QString m_customKeyboardProfilesJson;
  QString m_builtinFanProfilesJson;
  QString m_customFanProfilesJson;
  QString m_lastAppliedFanProfiles;
  QString m_lastKeyboardBacklight;
  QString m_lastSavedProfile;
  QJsonObject m_stateMap;
  bool m_failControl = false;
  QString m_dGpuInfoJson;
  int m_capabilityDelayMs = 0;
  bool m_visualFixture = false;

  QString currentSettingsJson() const {
    if (!m_settingsJson.isEmpty()) {
      QJsonObject obj = QJsonDocument::fromJson(m_settingsJson.toUtf8()).object();
      if (!m_stateMap.isEmpty())
        obj["stateMap"] = m_stateMap;
      return QString::fromUtf8(QJsonDocument(obj).toJson(QJsonDocument::Compact));
    }
    QJsonObject obj;
    obj["stateMap"] = m_stateMap;
    return QString::fromUtf8(QJsonDocument(obj).toJson(QJsonDocument::Compact));
  }

public:
  QString introspect(const QString &) const override {
    return QStringLiteral(
        "<interface name=\"com.uniwill.uccd\">"
        "<method name=\"IsDeviceSupported\"><arg type=\"b\" direction=\"out\"/></method>"
        "</interface>");
  }

  bool handleMessage(const QDBusMessage &msg, const QDBusConnection &bus) override {
    const QString method = msg.member();

    // Test control helpers
    if (method == QStringLiteral("TestReset")) {
      m_writes = QJsonArray();
      m_customProfiles.clear();
      m_defaultProfiles.clear();
      m_activeProfile.clear();
      m_settingsJson.clear();
      m_keyboardInfoJson.clear();
      m_customKeyboardProfilesJson.clear();
      m_builtinFanProfilesJson.clear();
      m_customFanProfilesJson.clear();
      m_lastAppliedFanProfiles.clear();
      m_lastKeyboardBacklight.clear();
      m_lastSavedProfile.clear();
      m_stateMap = QJsonObject();
      m_failControl = false;
      m_dGpuInfoJson.clear();
      m_capabilityDelayMs = 0;
      m_visualFixture = false;
      bus.send(msg.createReply(true));
      return true;
    }
    if (method == QStringLiteral("TestVisualFixture")) {
      m_visualFixture = true;
      bus.send(msg.createReply(true));
      return true;
    }
    if (method == QStringLiteral("TestFailControl")) {
      m_failControl = msg.arguments().value(0).toBool();
      bus.send(msg.createReply(true));
      return true;
    }
    if (method == QStringLiteral("TestCustomProfiles")) {
      m_customProfiles = msg.arguments().value(0).toString();
      bus.send(msg.createReply(true));
      return true;
    }
    if (method == QStringLiteral("TestDefaultProfiles")) {
      m_defaultProfiles = msg.arguments().value(0).toString();
      bus.send(msg.createReply(true));
      return true;
    }
    if (method == QStringLiteral("TestActiveProfile")) {
      m_activeProfile = msg.arguments().value(0).toString();
      bus.send(msg.createReply(true));
      return true;
    }
    if (method == QStringLiteral("TestStateMap")) {
      m_stateMap = QJsonDocument::fromJson(msg.arguments().value(0).toString().toUtf8()).object();
      bus.send(msg.createReply(true));
      return true;
    }
    if (method == QStringLiteral("TestKeyboardInfo")) {
      m_keyboardInfoJson = msg.arguments().value(0).toString();
      bus.send(msg.createReply(true));
      return true;
    }
    if (method == QStringLiteral("TestCustomKeyboardProfiles")) {
      m_customKeyboardProfilesJson = msg.arguments().value(0).toString();
      bus.send(msg.createReply(true));
      return true;
    }
    if (method == QStringLiteral("TestBuiltinFanProfiles")) {
      m_builtinFanProfilesJson = msg.arguments().value(0).toString();
      bus.send(msg.createReply(true));
      return true;
    }
    if (method == QStringLiteral("TestCustomFanProfiles")) {
      m_customFanProfilesJson = msg.arguments().value(0).toString();
      bus.send(msg.createReply(true));
      return true;
    }
    if (method == QStringLiteral("TestCapabilityDelayMs")) {
      m_capabilityDelayMs = msg.arguments().value(0).toInt();
      bus.send(msg.createReply(true));
      return true;
    }
    if (method == QStringLiteral("TestDGpuInfo")) {
      m_dGpuInfoJson = msg.arguments().value(0).toString();
      bus.send(msg.createReply(true));
      return true;
    }
    if (method == QStringLiteral("TestStats")) {
      QJsonObject stats{
          {"writes", m_writes},
          {"lastAppliedFanProfiles", m_lastAppliedFanProfiles},
          {"lastKeyboardBacklight", m_lastKeyboardBacklight},
          {"lastSavedProfile", m_lastSavedProfile},
          {"stateMap", m_stateMap},
      };
      bus.send(msg.createReply(QString::fromUtf8(QJsonDocument(stats).toJson(QJsonDocument::Compact))));
      return true;
    }

    // Read overrides
    if ((method == QStringLiteral("GetKeyboardBacklightCapabilitiesJSON") ||
         method == QStringLiteral("GetKeyboardBacklightInfoJSON")) &&
        !m_keyboardInfoJson.isEmpty()) {
      bus.send(msg.createReply(m_keyboardInfoJson));
      return true;
    }
    if (method == QStringLiteral("GetCustomKeyboardProfilesJSON") && !m_customKeyboardProfilesJson.isEmpty()) {
      bus.send(msg.createReply(m_customKeyboardProfilesJson));
      return true;
    }
    if (method == QStringLiteral("GetCustomProfilesJSON") && !m_customProfiles.isEmpty()) {
      bus.send(msg.createReply(m_customProfiles));
      return true;
    }
    if (method == QStringLiteral("GetDefaultProfilesJSON") && !m_defaultProfiles.isEmpty()) {
      bus.send(msg.createReply(m_defaultProfiles));
      return true;
    }
    if (method == QStringLiteral("GetActiveProfileJSON") && !m_activeProfile.isEmpty()) {
      bus.send(msg.createReply(m_activeProfile));
      return true;
    }
    if (method == QStringLiteral("GetSettingsJSON") && (!m_settingsJson.isEmpty() || !m_stateMap.isEmpty())) {
      bus.send(msg.createReply(currentSettingsJson()));
      return true;
    }
    if (method == QStringLiteral("GetFanProfileNames") && !m_builtinFanProfilesJson.isEmpty()) {
      bus.send(msg.createReply(m_builtinFanProfilesJson));
      return true;
    }
    if (method == QStringLiteral("GetCustomFanProfilesJSON") && !m_customFanProfilesJson.isEmpty()) {
      bus.send(msg.createReply(m_customFanProfilesJson));
      return true;
    }
    if (method == QStringLiteral("GetDGpuInfoValuesJSON") && !m_dGpuInfoJson.isEmpty()) {
      bus.send(msg.createReply(m_dGpuInfoJson));
      return true;
    }
    if (m_capabilityDelayMs > 0 &&
        (method == QStringLiteral("GetWaterCoolerSupported") ||
         method == QStringLiteral("GetHwpDynamicBoostSupported"))) {
      msg.setDelayedReply(true);
      QTimer::singleShot(m_capabilityDelayMs, [bus, msg]() {
        bus.send(msg.createReply(false));
      });
      return true;
    }

    if (m_visualFixture) {
      if (auto reply = visualFixtureReply(method)) {
        bus.send(msg.createReply(*reply));
        return true;
      }
    }

    // Mutating methods
    if (method == QStringLiteral("ApplyFanProfiles") ||
        method == QStringLiteral("SetKeyboardBacklightStatesJSON") ||
        method == QStringLiteral("SaveCustomProfile") ||
        method == QStringLiteral("DeleteCustomProfile") ||
        method == QStringLiteral("SetStateMap") ||
        method == QStringLiteral("SetBatchStateMap") ||
        method == QStringLiteral("ClearStateMap") ||
        method.startsWith(QStringLiteral("Set")) ||
        method.startsWith(QStringLiteral("Save")) ||
        method.startsWith(QStringLiteral("Delete")) ||
        method.startsWith(QStringLiteral("Apply")) ||
        method.startsWith(QStringLiteral("Revert"))) {
      if (m_failControl) {
        bus.send(msg.createReply(false));
        return true;
      }
      m_writes.append(method);
      if (method == QStringLiteral("ApplyFanProfiles")) {
        m_lastAppliedFanProfiles = msg.arguments().value(0).toString();
      } else if (method == QStringLiteral("SetKeyboardBacklightStatesJSON")) {
        m_lastKeyboardBacklight = msg.arguments().value(0).toString();
      } else if (method == QStringLiteral("SaveCustomProfile")) {
        m_lastSavedProfile = msg.arguments().value(0).toString();
      } else if (method == QStringLiteral("SetStateMap")) {
        const QString key = msg.arguments().value(0).toString();
        const QString val = msg.arguments().value(1).toString();
        if (val.isEmpty())
          m_stateMap.remove(key);
        else
          m_stateMap[key] = val;
      } else if (method == QStringLiteral("SetBatchStateMap")) {
        // Mirror uccd's dbus_impl.rs:590 behavior: inserts entries from JSON map,
        // and removes keys if explicitly mapped to empty string or null.
        const QJsonObject updates =
            QJsonDocument::fromJson(msg.arguments().value(0).toString().toUtf8()).object();
        for (auto it = updates.begin(); it != updates.end(); ++it) {
          if (it.value().isNull() || it.value().toString().isEmpty())
            m_stateMap.remove(it.key());
          else
            m_stateMap[it.key()] = it.value().toString();
        }
      } else if (method == QStringLiteral("ClearStateMap")) {
        m_stateMap.remove(msg.arguments().value(0).toString());
      }
      bus.send(msg.createReply(true));
      return true;
    }

    // Default read replies
    if (method == QStringLiteral("IsDeviceSupported")) {
      bus.send(msg.createReply(true));
      return true;
    }
    if (method.endsWith(QStringLiteral("Supported")) ||
        method == QStringLiteral("GetWebcamSWStatus") ||
        method == QStringLiteral("GetFnLockStatus") ||
        method == QStringLiteral("IsWaterCoolerEnabled") ||
        method == QStringLiteral("GetWaterCoolerConnected") ||
        method == QStringLiteral("GetWaterCoolerAvailable")) {
      bus.send(msg.createReply(false));
      return true;
    }
    if (method == QStringLiteral("GetSettingsJSON") || method == QStringLiteral("GetActiveProfileJSON")) {
      bus.send(msg.createReply(QStringLiteral("{}")));
      return true;
    }
    if (method.endsWith(QStringLiteral("JSON")) ||
        method.endsWith(QStringLiteral("Available")) ||
        method.endsWith(QStringLiteral("Thresholds")) ||
        method == QStringLiteral("GetFanProfileNames")) {
      bus.send(msg.createReply(QStringLiteral("[]")));
      return true;
    }
    if (method.startsWith(QStringLiteral("GetCurrent")) || method == QStringLiteral("GetPowerState")) {
      bus.send(msg.createReply(QString()));
      return true;
    }
    bus.send(msg.createReply(0));
    return true;
  }
};

void sendMouseMove(QWidget &widget, const QPoint &pos) {
  QMouseEvent moveEvent(QEvent::MouseMove, pos, widget.mapToGlobal(pos),
                        Qt::NoButton, Qt::LeftButton, Qt::NoModifier);
  QApplication::sendEvent(&widget, &moveEvent);
}

QPushButton *findButtonByText(QWidget *root, const QString &text) {
  for (auto *btn : root->findChildren<QPushButton *>()) {
    if (btn->text() == text)
      return btn;
  }
  return nullptr;
}

}  // namespace

class TestRepro1d07126Gui : public QObject {
  Q_OBJECT
  QProcess m_daemon;
  std::unique_ptr<ucc::UccdClient> m_client;
  std::unique_ptr<ucc::ProfileManager> m_profiles;

  QVariant control(const QString &method, const QVariantList &args = {}) {
    auto msg = QDBusMessage::createMethodCall(
        QStringLiteral("com.uniwill.uccd"),
        QStringLiteral("/com/uniwill/uccd"),
        QStringLiteral("com.uniwill.uccd"),
        method);
    msg.setArguments(args);
    return QDBusConnection::systemBus().call(msg).arguments().value(0);
  }

  QJsonObject stats() {
    return QJsonDocument::fromJson(control(QStringLiteral("TestStats")).toString().toUtf8()).object();
  }

private slots:
  void initTestCase() {
    m_daemon.start(QCoreApplication::applicationFilePath(), {QStringLiteral("--serve")});
    QVERIFY(m_daemon.waitForStarted());
    QVERIFY(m_daemon.waitForReadyRead());
    QCOMPARE(m_daemon.readAllStandardOutput().trimmed(), QByteArray("READY"));
    m_client = std::make_unique<ucc::UccdClient>();
    m_profiles = std::make_unique<ucc::ProfileManager>();
  }

  void init() {
    control(QStringLiteral("TestReset"));
    ucc::FluentTheme::setMode(ucc::FluentTheme::Mode::Light, false);
  }

  void cleanupTestCase() {
    m_profiles.reset();
    m_client.reset();
    if (m_daemon.state() != QProcess::NotRunning) {
      m_daemon.terminate();
      m_daemon.waitForFinished();
    }
  }

  // N02: FanCurveEditorWidget drag produces fractional duty; ApplyFanProfiles
  // payload must serialize integer-rounded speeds accepted by uccd.
  void n02_fan_curve_drag_serializes_integer_duty() {
    control(QStringLiteral("TestVisualFixture"));
    ucc::MainWindow window;
    window.resize(1280, 960);
    window.show();

    auto *fanTab = window.findChild<ucc::FanControlTab *>();
    QVERIFY(fanTab != nullptr);
    auto *cpuEditor = fanTab->cpuEditor();
    QVERIFY(cpuEditor != nullptr);

    // Use a non-multiple-of-100 plot height so dragging by a few pixels produces
    // a fractional duty in pixel->duty conversion unless rounded.
    cpuEditor->resize(720, 340);
    cpuEditor->setEditable(true);
    cpuEditor->setPoints({{20, 20}, {40, 35}, {60, 50}, {80, 75}, {100, 100}});
    QCoreApplication::processEvents();

    // Drag the 60C point vertically by a non-round pixel delta.
    const QRect rect = cpuEditor->rect();
    const QPoint start(rect.width() / 2, rect.height() / 2);
    const QPoint dragged = start - QPoint(0, 17);
    QTest::mousePress(cpuEditor, Qt::LeftButton, Qt::NoModifier, start);
    sendMouseMove(*cpuEditor, dragged);
    QTest::mouseRelease(cpuEditor, Qt::LeftButton, Qt::NoModifier, dragged);

    QVERIFY(fanTab->applyButton() != nullptr);
    fanTab->applyButton()->click();

    QTRY_VERIFY(!stats()["lastAppliedFanProfiles"].toString().isEmpty());
    const QJsonObject payload =
        QJsonDocument::fromJson(stats()["lastAppliedFanProfiles"].toString().toUtf8()).object();
    const QJsonArray cpuArr = payload["cpu"].toArray();
    QVERIFY(!cpuArr.isEmpty());

    for (const QJsonValue &entry : cpuArr) {
      const QJsonObject pt = entry.toObject();
      const double speed = pt["speed"].toDouble();
      const double temp = pt["temp"].toDouble();
      QVERIFY2(std::floor(speed) == speed,
               qPrintable(QString("N02: ApplyFanProfiles serialized non-integer speed=%1").arg(speed, 0, 'g', 15)));
      QVERIFY2(std::floor(temp) == temp,
               qPrintable(QString("N02: ApplyFanProfiles serialized non-integer temp=%1").arg(temp, 0, 'g', 15)));
    }
  }

  // N07: Single-zone keyboard (zones == 1) creates brightness slider without
  // KeyboardVisualizerWidget; moving/releasing slider must still dispatch D-Bus.
  void n07_single_zone_keyboard_brightness_slider_dispatches_dbus() {
    control(QStringLiteral("TestKeyboardInfo"),
            {QStringLiteral(R"({"zones":1,"maxBrightness":50,"maxRed":255,"maxGreen":255,"maxBlue":255})")});

    ucc::MainWindow window;
    window.resize(1280, 960);
    window.show();

    // Single-zone keyboard intentionally has no KeyboardVisualizerWidget.
    QVERIFY(window.findChild<ucc::KeyboardVisualizerWidget *>() == nullptr);

    auto *lightingTabs = window.findChild<QTabWidget *>(QStringLiteral("lightingTabs"));
    QVERIFY(lightingTabs != nullptr);
    QSlider *brightnessSlider = nullptr;
    for (auto *slider : lightingTabs->findChildren<QSlider *>()) {
      if (slider->maximum() == 50) {
        brightnessSlider = slider;
        break;
      }
    }
    QVERIFY2(brightnessSlider != nullptr, "Expected single-zone keyboard brightness slider (max=50)");

    brightnessSlider->setValue(25);
    emit brightnessSlider->sliderReleased();
    QCoreApplication::processEvents();

    const QJsonObject s = stats();
    const QJsonArray writes = s["writes"].toArray();
    QVERIFY2(writes.contains(QStringLiteral("SetKeyboardBacklightStatesJSON")),
             "N07: moving single-zone keyboard brightness slider did not call SetKeyboardBacklightStatesJSON");
    const QString payloadStr = s["lastKeyboardBacklight"].toString();
    QVERIFY(!payloadStr.isEmpty());
    QVERIFY(payloadStr.contains(QStringLiteral("25")));
  }

  // N08: Applying a legacy keyboard profile with top-level brightness=31 and
  // per-zone RGB states lacking per-state brightness must transmit brightness=31.
  void n08_keyboard_profile_preserves_top_level_brightness() {
    control(QStringLiteral("TestKeyboardInfo"),
            {QStringLiteral(R"({"zones":4,"maxBrightness":50,"maxRed":255,"maxGreen":255,"maxBlue":255})")});
    control(QStringLiteral("TestCustomKeyboardProfiles"),
            {QStringLiteral(R"([
              {"id":"kb-initial","name":"Initial","json":{"brightness":50,"states":[{"mode":0,"brightness":50,"red":255,"green":255,"blue":255}]}},
              {"id":"kb-legacy","name":"Legacy 31","json":{"brightness":31,"states":[{"mode":0,"red":10,"green":20,"blue":30}]}}
            ])")});

    ucc::MainWindow window;
    window.resize(1280, 960);
    window.show();

    auto *lightingTabs = window.findChild<QTabWidget *>(QStringLiteral("lightingTabs"));
    QVERIFY(lightingTabs != nullptr);
    QComboBox *kbCombo = nullptr;
    for (auto *combo : lightingTabs->findChildren<QComboBox *>()) {
      if (combo->findData(QStringLiteral("kb-legacy")) >= 0) {
        kbCombo = combo;
        break;
      }
    }
    QVERIFY2(kbCombo != nullptr, "Expected keyboard profile QComboBox containing kb-legacy");

    const int legacyIdx = kbCombo->findData(QStringLiteral("kb-legacy"));
    QVERIFY(legacyIdx >= 0);
    kbCombo->setCurrentIndex(legacyIdx);
    QCoreApplication::processEvents();

    const QString payloadStr = stats()["lastKeyboardBacklight"].toString();
    QVERIFY2(!payloadStr.isEmpty(), "Expected SetKeyboardBacklightStatesJSON call when selecting keyboard profile");

    const QJsonObject wrapper = QJsonDocument::fromJson(payloadStr.toUtf8()).object();
    QCOMPARE(wrapper["keyboardProfileId"].toString(), QStringLiteral("kb-legacy"));

    const bool hasTopLevelBrightness = wrapper.contains(QStringLiteral("brightness")) &&
                                       wrapper["brightness"].toInt(-1) == 31;
    bool allStatesHaveBrightness = false;
    const QJsonArray states = wrapper["states"].toArray();
    if (!states.isEmpty()) {
      allStatesHaveBrightness = true;
      for (const QJsonValue &st : states) {
        if (st.toObject().value(QStringLiteral("brightness")).toInt(-1) != 31) {
          allStatesHaveBrightness = false;
          break;
        }
      }
    }
    QVERIFY2(hasTopLevelBrightness || allStatesHaveBrightness,
             qPrintable(QString("N08: top-level brightness=31 dropped from D-Bus payload: %1").arg(payloadStr)));
  }

  // N09: Unchecking power_bat ("Battery") and clicking Save must remove the
  // power_bat binding from stateMap rather than leaving it untouched.
  void n09_unchecking_power_bat_removes_state_map_binding() {
    control(QStringLiteral("TestVisualFixture"));
    control(QStringLiteral("TestCustomProfiles"),
            {QStringLiteral(R"([{"id":"custom-bat","name":"Custom Bat","fan":{"fanProfile":"fan-balanced"}}])")});
    control(QStringLiteral("TestStateMap"),
            {QStringLiteral(R"({"power_ac":"custom-bat","power_bat":"custom-bat"})")});

    ucc::MainWindow window;
    window.resize(1280, 960);
    window.show();

    auto *nav = window.findChild<QListWidget *>(QStringLiteral("navigation"));
    QVERIFY(nav != nullptr);
    nav->setCurrentRow(1);

    auto *profileCombo = window.findChild<QComboBox *>(QStringLiteral("profileName"));
    QVERIFY(profileCombo != nullptr);
    const int idx = profileCombo->findData(QStringLiteral("custom-bat"));
    QVERIFY(idx >= 0);
    profileCombo->setCurrentIndex(idx);
    QCoreApplication::processEvents();

    QPushButton *batteryBtn = findButtonByText(&window, QStringLiteral("Battery"));
    QVERIFY(batteryBtn != nullptr);
    QVERIFY2(batteryBtn->isChecked(), "Precondition: Battery button should reflect power_bat stateMap binding");

    // Uncheck Battery and click Save.
    batteryBtn->setChecked(false);
    auto *saveBtn = window.findChild<QPushButton *>(QStringLiteral("saveProfile"));
    QVERIFY(saveBtn != nullptr);
    saveBtn->click();
    QCoreApplication::processEvents();

    const QJsonObject finalStateMap = stats()["stateMap"].toObject();
    QCOMPARE(finalStateMap["power_ac"].toString(), QStringLiteral("custom-bat"));
    QVERIFY2(!finalStateMap.contains(QStringLiteral("power_bat")) ||
                 finalStateMap["power_bat"].toString() != QStringLiteral("custom-bat"),
             "N09: unchecking Battery and saving left stale power_bat binding in stateMap");
  }

  // N10: When saveProfile fails synchronously, MainWindow must keep the error
  // status and must NOT overwrite it with "Saving profile..." / stuck m_saveInProgress.
  void n10_save_failure_not_overwritten_by_saving_profile_status() {
    control(QStringLiteral("TestVisualFixture"));
    control(QStringLiteral("TestCustomProfiles"),
            {QStringLiteral(R"([{"id":"custom-err","name":"Custom Error","fan":{"fanProfile":"fan-balanced"}}])")});

    ucc::MainWindow window;
    window.resize(1280, 960);
    window.show();

    auto *nav = window.findChild<QListWidget *>(QStringLiteral("navigation"));
    QVERIFY(nav != nullptr);
    nav->setCurrentRow(1);

    auto *profileCombo = window.findChild<QComboBox *>(QStringLiteral("profileName"));
    QVERIFY(profileCombo != nullptr);
    const int idx = profileCombo->findData(QStringLiteral("custom-err"));
    QVERIFY(idx >= 0);
    profileCombo->setCurrentIndex(idx);
    QCoreApplication::processEvents();

    // Inject D-Bus failure on SaveCustomProfile / SetBatchStateMap.
    control(QStringLiteral("TestFailControl"), {true});

    auto *saveBtn = window.findChild<QPushButton *>(QStringLiteral("saveProfile"));
    QVERIFY(saveBtn != nullptr);
    saveBtn->click();
    QCoreApplication::processEvents();

    const QString statusMsg = window.statusBar()->currentMessage();
    QVERIFY2(statusMsg != QStringLiteral("Saving profile..."),
             "N10: failed saveProfile was overwritten by stuck 'Saving profile...' status");
    QVERIFY2(statusMsg.contains(QStringLiteral("Error"), Qt::CaseInsensitive) ||
                 statusMsg.contains(QStringLiteral("Failed"), Qt::CaseInsensitive),
             qPrintable(QString("N10: expected error status message, got: '%1'").arg(statusMsg)));
  }

  // N11: Saving or deleting an inactive custom profile calls
  // loadCustomProfilesFromSettings(), which must preserve m_activeProfileId.
  void n11_save_or_delete_inactive_profile_preserves_active_profile_id() {
    control(QStringLiteral("TestActiveProfile"),
            {QStringLiteral(R"({"id":"active-prof","name":"Active Profile"})")});
    control(QStringLiteral("TestCustomProfiles"),
            {QStringLiteral(R"([
              {"id":"active-prof","name":"Active Profile"},
              {"id":"inactive-prof","name":"Inactive Profile"}
            ])")});

    m_profiles->refresh();
    QCOMPARE(m_profiles->activeProfileId(), QStringLiteral("active-prof"));

    // Save an inactive profile (daemon returns true without emitting ProfileChanged).
    m_profiles->saveProfile(QStringLiteral(R"({"id":"inactive-prof","name":"Inactive Profile Updated"})"));
    QCOMPARE(m_profiles->activeProfileId(), QStringLiteral("active-prof"));

    // Delete an inactive profile (daemon returns true without emitting ProfileChanged).
    m_profiles->deleteProfile(QStringLiteral("inactive-prof"));
    QCOMPARE(m_profiles->activeProfileId(), QStringLiteral("active-prof"));
  }

  // N26: Read-only preview allowlist must permit read-only WebcamSWAvailable.
  void n26_preview_mode_allows_readonly_webcam_sw_available() {
#ifdef UCC_READ_ONLY_PREVIEW
    QVERIFY2(ucc::mayDispatchUccdMethod(QStringLiteral("WebcamSWAvailable")),
             "N26: preview mode blocked read-only WebcamSWAvailable");
    QVERIFY(!ucc::mayDispatchUccdMethod(QStringLiteral("SetWebcamSWStatus")));
    QVERIFY(!ucc::mayDispatchUccdMethod(QStringLiteral("SetActiveProfile")));
#else
    // Even when built without -DUCC_READ_ONLY_PREVIEW, verify both the runtime
    // call and that PreviewMode.hpp explicitly allowlists WebcamSWAvailable.
    QVERIFY(ucc::mayDispatchUccdMethod(QStringLiteral("WebcamSWAvailable")));
    QFile previewHeader(QString::fromUtf8(__FILE__).section('/', 0, -3) +
                        QStringLiteral("/libucc-dbus/PreviewMode.hpp"));
    if (previewHeader.open(QIODevice::ReadOnly)) {
      const QString content = QString::fromUtf8(previewHeader.readAll());
      QVERIFY2(content.contains(QStringLiteral("WebcamSWAvailable")),
               "N26: libucc-dbus/PreviewMode.hpp does not allowlist read-only WebcamSWAvailable");
    } else {
      QFAIL("N26: rebuild with -DUCC_READ_ONLY_PREVIEW or run inside repo tree to verify PreviewMode.hpp");
    }
#endif
  }

  // N31: FanControlTab must disable m_removeFanProfileButton for builtin
  // fan-balanced AND whenever uccd is disconnected.
  void n31_fan_control_tab_disables_remove_for_builtin_profile() {
    control(QStringLiteral("TestBuiltinFanProfiles"),
            {QStringLiteral(R"([{"id":"fan-balanced","name":"Balanced"}])")});
    control(QStringLiteral("TestCustomFanProfiles"),
            {QStringLiteral(R"([{"id":"custom-fan-1","name":"Custom Fan"}])")});

    m_profiles->refresh();

    ucc::FanControlTab tab(m_client.get(), m_profiles.get(), false);
    tab.reloadFanProfiles();

    QComboBox *combo = tab.fanProfileCombo();
    QPushButton *removeBtn = tab.removeButton();
    QVERIFY(combo != nullptr);
    QVERIFY(removeBtn != nullptr);

    const int builtinIdx = combo->findData(QStringLiteral("fan-balanced"));
    const int customIdx = combo->findData(QStringLiteral("custom-fan-1"));
    QVERIFY(builtinIdx >= 0);
    QVERIFY(customIdx >= 0);

    // Built-in fan-balanced: Remove must be disabled even when connected.
    combo->setCurrentIndex(builtinIdx);
    tab.updateButtonStates(true);
    QVERIFY2(!removeBtn->isEnabled(),
             "N31: Remove button must be disabled for built-in fan-balanced profile");

    // Custom profile when connected: Remove must be enabled.
    combo->setCurrentIndex(customIdx);
    tab.updateButtonStates(true);
    QVERIFY2(removeBtn->isEnabled(),
             "Remove button should be enabled for connected custom fan profile");

    // Custom profile when disconnected: Remove must be disabled.
    tab.updateButtonStates(false);
    QVERIFY2(!removeBtn->isEnabled(),
             "N31: Remove button must be disabled when disconnected from uccd");
  }

  // N28: SysfsNode::isAvailable() is declared noexcept; calling it on an
  // unreadable path (EACCES) must return false without calling std::terminate.
  void n28_sysfs_node_is_available_does_not_terminate_on_eacces() {
    if (::geteuid() == 0)
      QSKIP("EACCES probe requires non-root UID");

    QTemporaryDir tmpDir;
    QVERIFY(tmpDir.isValid());
    const QString restrictedDir = tmpDir.path() + QStringLiteral("/noaccess");
    QVERIFY(QDir().mkpath(restrictedDir));

    const QString targetPath = restrictedDir + QStringLiteral("/node");
    {
      QFile f(targetPath);
      QVERIFY(f.open(QIODevice::WriteOnly));
      f.write("1\n");
    }

    QVERIFY(::chmod(restrictedDir.toLocal8Bit().constData(), 0000) == 0);
    const auto restorePerms = qScopeGuard([&] {
      ::chmod(restrictedDir.toLocal8Bit().constData(), 0700);
    });

    QProcess probe;
    probe.start(QCoreApplication::applicationFilePath(),
                {QStringLiteral("--probe-sysfs-eacces"), targetPath});
    QVERIFY(probe.waitForStarted());
    QVERIFY(probe.waitForFinished(5000));

    QVERIFY2(probe.exitStatus() == QProcess::NormalExit,
             "N28: SysfsNode::isAvailable() terminated process (SIGABRT/std::terminate) on EACCES");
    QCOMPARE(probe.exitCode(), 0);
  }

  // N12: MainWindow ctor must not block the UI thread with synchronous
  // getWaterCoolerSupported() / getHwpDynamicBoostSupported() D-Bus calls.
  void n12_mainwindow_ctor_avoids_sync_dbus_capability_calls() {
    control(QStringLiteral("TestCapabilityDelayMs"), {350});

    QElapsedTimer timer;
    timer.start();
    ucc::MainWindow window;
    const qint64 elapsedMs = timer.elapsed();

    QVERIFY2(elapsedMs < 300,
             qPrintable(QString("N12: MainWindow ctor blocked UI thread for %1 ms on sync "
                                "GetWaterCoolerSupported/GetHwpDynamicBoostSupported")
                            .arg(elapsedMs)));
  }

  // N13: KeyboardVisualizerWidget ctor / DetectLayout must not block the UI
  // thread with sequential QProcess::waitForFinished(3000) calls.
  void n13_keyboard_visualizer_avoids_blocking_detect_layout_in_ctor() {
    QTemporaryDir fakeBinDir;
    QVERIFY(fakeBinDir.isValid());

    const QStringList slowTools = {
        QStringLiteral("dbus-send"),
        QStringLiteral("gsettings"),
        QStringLiteral("localectl"),
        QStringLiteral("setxkbmap"),
        QStringLiteral("xkb-switch"),
    };
    for (const QString &tool : slowTools) {
      const QString scriptPath = fakeBinDir.path() + QLatin1Char('/') + tool;
      QFile script(scriptPath);
      QVERIFY(script.open(QIODevice::WriteOnly));
      script.write("#!/bin/sh\nsleep 1\nexit 1\n");
      script.close();
      QVERIFY(::chmod(scriptPath.toLocal8Bit().constData(), 0755) == 0);
    }

    const QByteArray origPath = qgetenv("PATH");
    const bool hadDesktop = qEnvironmentVariableIsSet("XDG_CURRENT_DESKTOP");
    const QByteArray origDesktop = qgetenv("XDG_CURRENT_DESKTOP");
    const auto restoreEnv = qScopeGuard([&] {
      qputenv("PATH", origPath);
      if (hadDesktop)
        qputenv("XDG_CURRENT_DESKTOP", origDesktop);
      else
        qunsetenv("XDG_CURRENT_DESKTOP");
    });

    qputenv("PATH", (fakeBinDir.path() + QStringLiteral(":/usr/bin:/bin")).toLocal8Bit());
    qputenv("XDG_CURRENT_DESKTOP", "KDE");

    QElapsedTimer timer;
    timer.start();
    ucc::KeyboardVisualizerWidget visualizer(4, 50, nullptr, false);
    const qint64 elapsedMs = timer.elapsed();

    QVERIFY2(elapsedMs < 400,
             qPrintable(QString("N13: KeyboardVisualizerWidget ctor blocked UI thread for %1 ms "
                                "in synchronous DetectLayout/waitForFinished")
                            .arg(elapsedMs)));
  }

  // P0-14: Clicking a ClickableRectItem sticky mark in MonitorTab must not
  // synchronously self-delete inside mousePressEvent (Heap-UAF under ASan/UBSan).
  void p0_14_monitor_tab_sticky_mark_click_no_sync_self_delete_uaf() {
    control(QStringLiteral("TestVisualFixture"));

    ucc::MonitorTab tab(m_client.get());
    tab.resize(1280, 900);
    tab.show();
    tab.setMonitoringActive(true);
    QTest::qWait(150);

    QCheckBox *unifiedCheck = nullptr;
    for (auto *cb : tab.findChildren<QCheckBox *>()) {
      if (cb->text() == QStringLiteral("Unified Graph")) {
        unifiedCheck = cb;
        break;
      }
    }
    QVERIFY2(unifiedCheck != nullptr, "Expected 'Unified Graph' checkbox in MonitorTab");
    unifiedCheck->setChecked(true);
    QCoreApplication::processEvents();
    QTest::qWait(100);

    QChartView *unifiedView = nullptr;
    for (auto *view : tab.findChildren<QChartView *>()) {
      if (view->isVisible() && view->chart() && view->scene()) {
        unifiedView = view;
        break;
      }
    }
    QVERIFY2(unifiedView != nullptr, "Expected visible unified QChartView in MonitorTab");

    QWidget *vp = unifiedView->viewport();
    QVERIFY(vp != nullptr);
    const QPoint plotCenter = unifiedView->mapFromScene(unifiedView->chart()->plotArea().center());

    // Annotations start enabled by default; left-click pins a sticky mark at plotCenter.
    QTest::mouseClick(vp, Qt::LeftButton, Qt::NoModifier, plotCenter);
    QCoreApplication::processEvents();

    QGraphicsRectItem *stickyBg = nullptr;
    for (QGraphicsItem *item : unifiedView->scene()->items()) {
      auto *rectItem = dynamic_cast<QGraphicsRectItem *>(item);
      if (rectItem && rectItem->isVisible() && rectItem->zValue() == 90.0 &&
          (rectItem->acceptedMouseButtons() & Qt::LeftButton)) {
        stickyBg = rectItem;
        break;
      }
    }
    QVERIFY2(stickyBg != nullptr, "Expected visible ClickableRectItem sticky mark in unified scene");

    // Clicking the sticky mark invokes ClickableRectItem::mousePressEvent -> m_onClick();
    // synchronous delete of stickyBg triggers Heap-Use-After-Free under ASan/UBSan.
    const QPoint markPt = unifiedView->mapFromScene(stickyBg->sceneBoundingRect().center());
    QTest::mouseClick(vp, Qt::LeftButton, Qt::NoModifier, markPt);
    QCoreApplication::processEvents();

    QVERIFY2(!unifiedView->scene()->items().contains(stickyBg),
             "P0-14: sticky mark should be removed cleanly after click");
  }

  // P0-15: HardwareTab(monitor, nullptr) must not dereference null parent in setupUI().
  void p0_15_hardware_tab_accepts_null_parent_without_crash() {
    QProcess probe;
    probe.start(QCoreApplication::applicationFilePath(),
                {QStringLiteral("--probe-hardware-tab-null-parent")});
    QVERIFY(probe.waitForStarted());
    QVERIFY(probe.waitForFinished(5000));

    QVERIFY2(probe.exitStatus() == QProcess::NormalExit,
             "P0-15: HardwareTab(monitor, nullptr) crashed (SIGSEGV on null parent->layout())");
    QCOMPARE(probe.exitCode(), 0);
  }

  // M-GUI-05: readJsonInt() must preserve negative GPU clock offsets (e.g. -150)
  // while getDGpuGrClockOffsetMHz / getDGpuMemClockOffsetMHz still reject -999 sentinel.
  void m_gui_05_read_json_int_preserves_negative_gpu_clock_offsets() {
    control(QStringLiteral("TestDGpuInfo"),
            {QStringLiteral(R"({"grClockOffsetMHz":-150,"memClockOffsetMHz":-150})")});

    const auto grNeg = m_client->getDGpuGrClockOffsetMHz();
    const auto memNeg = m_client->getDGpuMemClockOffsetMHz();
    QVERIFY2(grNeg.has_value() && *grNeg == -150,
             "M-GUI-05: getDGpuGrClockOffsetMHz dropped valid negative offset -150");
    QVERIFY2(memNeg.has_value() && *memNeg == -150,
             "M-GUI-05: getDGpuMemClockOffsetMHz dropped valid negative offset -150");

    control(QStringLiteral("TestDGpuInfo"),
            {QStringLiteral(R"({"grClockOffsetMHz":-999,"memClockOffsetMHz":-999})")});

    QVERIFY2(!m_client->getDGpuGrClockOffsetMHz().has_value(),
             "M-GUI-05: getDGpuGrClockOffsetMHz must reject -999 sentinel");
    QVERIFY2(!m_client->getDGpuMemClockOffsetMHz().has_value(),
             "M-GUI-05: getDGpuMemClockOffsetMHz must reject -999 sentinel");
  }

  // SystemMonitor asynchronous snapshot updates CPU/GPU/fan metrics and
  // preserves negative dGPU clock offsets without blocking or losing state on deactivate.
  void system_monitor_snapshot_updates_metrics_and_preserves_negative_clock_offsets() {
    control(QStringLiteral("TestVisualFixture"));
    control(QStringLiteral("TestDGpuInfo"),
            {QStringLiteral(
                R"({"temp":54,"coreFrequency":1850,"powerDraw":68.5,"computeUtilPct":12,"memoryUtilPct":6,"currentPstate":2,"grClockOffsetMHz":-120,"memClockOffsetMHz":-200})")});

    ucc::SystemMonitor monitor;
    QSignalSpy updatedSpy(&monitor, &ucc::SystemMonitor::metricsUpdated);
    QVERIFY(updatedSpy.isValid());

    monitor.setMonitoringActive(true);
    QVERIFY(monitor.monitoringActive());
    QTRY_VERIFY_WITH_TIMEOUT(updatedSpy.count() >= 1, 3000);

    QCOMPARE(monitor.cpuTemp(), QStringLiteral("52\u00b0C"));
    QCOMPARE(monitor.cpuFanSpeed(), QStringLiteral("31 %"));
    QCOMPARE(monitor.gpuFanSpeed(), QStringLiteral("31 %"));
    QCOMPARE(monitor.gpuTemp(), QStringLiteral("54\u00b0C"));
    QCOMPARE(monitor.gpuFrequency(), QStringLiteral("1850 MHz"));
    QCOMPARE(monitor.gpuPower(), QStringLiteral("68.5 W"));
    QCOMPARE(monitor.dGpuComputeUtil(), 12);
    QCOMPARE(monitor.dGpuMemoryUtil(), 6);
    QCOMPARE(monitor.dGpuPstate(), 2);
    QCOMPARE(monitor.dGpuGrClockOffset(), -120);
    QCOMPARE(monitor.dGpuMemClockOffset(), -200);

    monitor.setMonitoringActive(false);
    QVERIFY(!monitor.monitoringActive());
  }
};

int main(int argc, char **argv) {
  for (int i = 1; i + 1 < argc; ++i) {
    if (QString::fromLocal8Bit(argv[i]) == QStringLiteral("--probe-sysfs-eacces")) {
      SysfsNode<int> node(argv[i + 1]);
      return node.isAvailable() ? 1 : 0;
    }
  }

  QApplication app(argc, argv);
  ucc::FluentTheme::apply(app);
  if (app.arguments().contains(QStringLiteral("--probe-hardware-tab-null-parent"))) {
    ucc::SystemMonitor monitor;
    ucc::HardwareTab tab(&monitor, nullptr);
    Q_UNUSED(tab);
    return 0;
  }
  if (app.arguments().contains(QStringLiteral("--serve"))) {
    ReproAuditDaemon service;
    auto bus = QDBusConnection::systemBus();
    if (!bus.registerVirtualObject(QStringLiteral("/com/uniwill/uccd"), &service) ||
        !bus.registerService(QStringLiteral("com.uniwill.uccd"))) {
      return 2;
    }
    QTextStream(stdout) << "READY\n" << Qt::flush;
    return app.exec();
  }

  TestRepro1d07126Gui test;
  return QTest::qExec(&test, argc, argv);
}

#include "test_gui_regressions.moc"
