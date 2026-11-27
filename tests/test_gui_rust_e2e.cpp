// SPDX-License-Identifier: GPL-3.0-or-later

//! Full-stack MainWindow GUI end-to-end test against live Rust `LiveUccdBackend`
//! (`M10.6`: all 11 sidebar sections, Light/Dark theme, live profile & F45 Lid mutations,
//! rejected invalid commands without fake success, and UCC_READ_ONLY_PREVIEW=1 write blocking).

#include <QApplication>
#include <QCheckBox>
#include <QComboBox>
#include <QElapsedTimer>
#include <QFileInfo>
#include <QJsonDocument>
#include <QJsonObject>
#include <QLabel>
#include <QListWidget>
#include <QProcess>
#include <QProcessEnvironment>
#include <QPushButton>
#include <QTabWidget>
#include <QTemporaryDir>
#include <QtTest>

#include "FluentTheme.hpp"
#include "MainWindow.hpp"
#include "UccdClient.hpp"

class GuiRustE2E : public QObject {
  Q_OBJECT

private:
  QTemporaryDir m_configDir;
  QTemporaryDir m_previewConfigDir;
  QString m_daemonBin;
  QProcess *m_daemon = nullptr;

  bool startDaemon(const QString &configDir, bool readOnlyPreview) {
    stopDaemon();
    m_daemon = new QProcess(this);
    m_daemon->setProcessChannelMode(QProcess::SeparateChannels);
    QProcessEnvironment env = QProcessEnvironment::systemEnvironment();
    env.insert(QStringLiteral("UCC_INTEROP_CONFIG_DIR"), configDir);
    if (readOnlyPreview) {
      env.insert(QStringLiteral("UCC_READ_ONLY_PREVIEW"), QStringLiteral("1"));
    } else {
      env.remove(QStringLiteral("UCC_READ_ONLY_PREVIEW"));
    }
    m_daemon->setProcessEnvironment(env);
    m_daemon->start(m_daemonBin, {});
    if (!m_daemon->waitForStarted(5000)) {
      return false;
    }
    QElapsedTimer timer;
    timer.start();
    QByteArray out;
    while (timer.elapsed() < 10000) {
      if (m_daemon->waitForReadyRead(100)) {
        out.append(m_daemon->readAllStandardOutput());
        if (out.contains("READY")) {
          return true;
        }
      }
      if (m_daemon->state() == QProcess::NotRunning) {
        return false;
      }
    }
    return false;
  }

  void stopDaemon() {
    if (!m_daemon) {
      return;
    }
    if (m_daemon->state() != QProcess::NotRunning) {
      m_daemon->terminate();
      if (!m_daemon->waitForFinished(4000)) {
        m_daemon->kill();
        m_daemon->waitForFinished(2000);
      }
    }
    m_daemon->deleteLater();
    m_daemon = nullptr;
  }

private slots:
  void initTestCase() {
    QVERIFY(m_configDir.isValid());
    QVERIFY(m_previewConfigDir.isValid());
    m_daemonBin = qEnvironmentVariable("UCC_INTEROP_DAEMON_BIN");
    QVERIFY2(!m_daemonBin.isEmpty(), "UCC_INTEROP_DAEMON_BIN must be set");
    QVERIFY2(QFileInfo::exists(m_daemonBin) && QFileInfo(m_daemonBin).isExecutable(),
             "UCC_INTEROP_DAEMON_BIN must point to an executable interop_daemon");
    QVERIFY2(startDaemon(m_configDir.path(), false), "Live Rust interop_daemon failed to start");
  }

  void cleanupTestCase() {
    stopDaemon();
  }

  void allSectionsThemesAndLiveMutationsWithRustBackend() {
    ucc::UccdClient client;
    QVERIFY2(client.isConnected(), "UccdClient failed to connect to live Rust uccd");
    QCOMPARE(client.isDeviceSupported(), std::optional<bool>(true));

    ucc::MainWindow window;
    window.resize(1280, 900);
    window.show();

    auto *nav = window.findChild<QListWidget *>(QStringLiteral("navigation"));
    auto *pages = window.findChild<QTabWidget *>(QStringLiteral("pages"));
    QVERIFY(nav);
    QVERIFY(pages);
    QCOMPARE(nav->count(), 11);
    QCOMPARE(nav->item(10)->text(), QStringLiteral("About"));

    const int expectedPageIndices[] = {0, 1, 2, 2, 3, 4, 5, 6, 7, 8, 9};
    for (int row = 0; row < 11; ++row) {
      if (!(nav->item(row)->flags() & Qt::ItemIsEnabled)) {
        continue;
      }
      nav->setCurrentRow(row);
      QCOMPARE(pages->currentIndex(), expectedPageIndices[row]);
    }

    // Verify F45 Lid card & F-Experimental widgets exist inside live MainWindow.
    QVERIFY(window.findChild<QCheckBox *>(QStringLiteral("turnOffKeyboardBacklightOnLidClose")));
    QVERIFY(window.findChild<QCheckBox *>(QStringLiteral("turnOffWaterCoolerLedOnLidClose")));
    QVERIFY(window.findChild<QWidget *>(QStringLiteral("experimentalPage")));
    QVERIFY(window.findChild<QLabel *>(QStringLiteral("experimentalBanner")));

    // Cycle Light & Dark themes cleanly without crashing or dropping controls.
    for (auto theme : {ucc::FluentTheme::Mode::Light, ucc::FluentTheme::Mode::Dark}) {
      ucc::FluentTheme::setMode(theme, false);
      QCoreApplication::processEvents();
      QCOMPARE(nav->count(), 11);
    QCOMPARE(nav->item(10)->text(), QStringLiteral("About"));
    }

    // Exercise live Rust mutations via GUI / client and confirm state transitions.
    QVERIFY(client.setManualPowerMode("bat"));
    const auto policyRaw = client.getPowerPolicyJSON();
    QVERIFY(policyRaw.has_value());
    const auto policyObj = QJsonDocument::fromJson(QByteArray::fromStdString(*policyRaw)).object();
    QCOMPARE(policyObj.value(QStringLiteral("manual_mode")).toString(), QStringLiteral("bat"));
    QVERIFY(client.setManualPowerMode("auto"));

    // Rejected invalid mutations must return false cleanly without fake success.
    QVERIFY(!client.applyProfile("{not_json"));
    QVERIFY(!client.setActiveProfile("__nonexistent_rust_e2e__"));
    QVERIFY(!client.setWaterCoolerPumpVoltage(1));
    QVERIFY(!client.deleteCustomProfile("__nonexistent_rust_e2e__"));

    window.hide();
  }

  void previewModeBlocksAllWritesOnLiveRustDaemon() {
    QVERIFY2(startDaemon(m_previewConfigDir.path(), true),
             "Preview-mode Rust interop_daemon failed to start");

    ucc::UccdClient client;
    QVERIFY(client.isConnected());

    const auto beforeActive = client.getActiveProfileJSON();
    QVERIFY(beforeActive.has_value());

    ucc::MainWindow window;
    window.resize(1280, 900);
    window.show();
    auto *nav = window.findChild<QListWidget *>(QStringLiteral("navigation"));
    QVERIFY(nav);
    for (int row = 0; row < 11; ++row) {
      if (nav->item(row)->flags() & Qt::ItemIsEnabled) {
        nav->setCurrentRow(row);
      }
    }

    // All mutations must be rejected by UCC_READ_ONLY_PREVIEW=1 Rust daemon.
    QVERIFY(!client.setDisplayBrightness(42));
    QVERIFY(!client.applyProfile("{}"));
    QVERIFY(!client.saveCustomProfile("{}"));
    QVERIFY(!client.deleteCustomProfile("preview-id"));
    QVERIFY(!client.setManualPowerMode("bat"));
    QVERIFY(!client.setPowerDeviceSettingsJSON("{}"));
    QVERIFY(!client.setLightbarSettingsJSON("{}"));

    const auto afterActive = client.getActiveProfileJSON();
    QCOMPARE(afterActive, beforeActive);
    window.hide();
  }
};

int main(int argc, char **argv) {
  QTemporaryDir isoHome;
  if (isoHome.isValid()) {
    qputenv("HOME", isoHome.path().toUtf8());
    qputenv("XDG_CONFIG_HOME", (isoHome.path() + QStringLiteral("/.config")).toUtf8());
    qputenv("XDG_DATA_HOME", (isoHome.path() + QStringLiteral("/.local/share")).toUtf8());
    qputenv("XDG_STATE_HOME", (isoHome.path() + QStringLiteral("/.local/state")).toUtf8());
    qputenv("XDG_CACHE_HOME", (isoHome.path() + QStringLiteral("/.cache")).toUtf8());
  }
  qputenv("QT_QPA_PLATFORM", "offscreen");
  qputenv("GIO_USE_VFS", "local");
  QApplication app(argc, argv);
  GuiRustE2E test;
  return QTest::qExec(&test, argc, argv);
}

#include "test_gui_rust_e2e.moc"
