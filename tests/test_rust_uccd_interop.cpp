// SPDX-License-Identifier: GPL-3.0-or-later

#include <QCoreApplication>
#include <QElapsedTimer>
#include <QEventLoop>
#include <QFileInfo>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QProcess>
#include <QProcessEnvironment>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QTest>
#include <QTimer>
#include <QVariantMap>

#include "UccdClient.hpp"

class TestRustUccdInterop : public QObject
{
  Q_OBJECT

private:
  QTemporaryDir m_configDir;
  QString m_daemonBin;
  QProcess *m_daemon = nullptr;

  bool startDaemon()
  {
    stopDaemon();
    m_daemon = new QProcess(this);
    m_daemon->setProcessChannelMode(QProcess::SeparateChannels);
    QProcessEnvironment env = QProcessEnvironment::systemEnvironment();
    env.insert(QStringLiteral("UCC_INTEROP_CONFIG_DIR"), m_configDir.path());
    m_daemon->setProcessEnvironment(env);
    m_daemon->start(m_daemonBin, {});
    if (!m_daemon->waitForStarted(5000)) {
      qWarning() << "Failed to start interop_daemon:" << m_daemon->errorString();
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
        qWarning() << "interop_daemon exited early:" << m_daemon->readAllStandardError();
        return false;
      }
    }
    qWarning() << "Timed out waiting for READY; stdout=" << out
               << "stderr=" << m_daemon->readAllStandardError();
    return false;
  }

  void stopDaemon()
  {
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

  static QStringList collectDefaultProfileIds(ucc::UccdClient &client)
  {
    const auto raw = client.getDefaultProfilesJSON();
    if (!raw.has_value()) {
      return {};
    }
    const QJsonDocument doc = QJsonDocument::fromJson(QByteArray::fromStdString(*raw));
    if (!doc.isArray()) {
      return {};
    }
    QStringList ids;
    for (const QJsonValue &val : doc.array()) {
      const QString id = val.toObject().value(QStringLiteral("id")).toString();
      if (!id.isEmpty()) {
        ids.append(id);
      }
    }
    return ids;
  }

private slots:
  void initTestCase()
  {
    QVERIFY2(m_configDir.isValid(), "Failed to create temporary config dir");
    m_daemonBin = qEnvironmentVariable("UCC_INTEROP_DAEMON_BIN");
    QVERIFY2(!m_daemonBin.isEmpty(), "UCC_INTEROP_DAEMON_BIN must be set");
    QVERIFY2(QFileInfo::exists(m_daemonBin) && QFileInfo(m_daemonBin).isExecutable(),
             "UCC_INTEROP_DAEMON_BIN must point to an executable binary");
    QVERIFY2(startDaemon(), "Rust interop_daemon failed to start");
  }

  void cleanupTestCase()
  {
    stopDaemon();
  }

  void testMetadataAndQueries()
  {
    ucc::UccdClient client;
    QVERIFY(client.isConnected());
    QCOMPARE(client.isDeviceSupported(), std::optional<bool>(true));

    const auto sysInfoRaw = client.getSystemInfoJSON();
    QVERIFY(sysInfoRaw.has_value());
    const QJsonDocument sysDoc = QJsonDocument::fromJson(QByteArray::fromStdString(*sysInfoRaw));
    QVERIFY(sysDoc.isObject());

    const QStringList defaultIds = collectDefaultProfileIds(client);
    QVERIFY(!defaultIds.isEmpty());

    const auto activeRaw = client.getActiveProfileJSON();
    QVERIFY(activeRaw.has_value());
    const QJsonDocument activeDoc = QJsonDocument::fromJson(QByteArray::fromStdString(*activeRaw));
    QVERIFY(activeDoc.isObject());
    QVERIFY(!activeDoc.object().value(QStringLiteral("id")).toString().isEmpty());

    QVERIFY(client.getSettingsJSON().has_value());
    QVERIFY(client.getPowerState().has_value());

    const auto coreCount = client.getCpuCoreCount();
    QVERIFY(coreCount.has_value());
    QVERIFY(*coreCount >= 1);

    QCOMPARE(client.getWaterCoolerSupported(), std::optional<bool>(true));
    QCOMPARE(client.getKeyboardBacklightControlSupported(), std::optional<bool>(true));
  }

  void testBoolFalsePropagation_TClient01()
  {
    ucc::UccdClient client;
    QVERIFY(client.isConnected());

    QVERIFY(!client.setActiveProfile("__nonexistent_profile__"));
    QVERIFY(!client.deleteCustomProfile("__nonexistent_id__"));
    QVERIFY(!client.setWaterCoolerPumpVoltage(1));
    QVERIFY(!client.applyProfile("{invalid"));
    QVERIFY(!client.saveCustomProfile("{invalid"));
    QVERIFY(!client.setFanProfileCPU("{invalid"));
    QVERIFY(!client.setFanProfileDGPU("{invalid"));
    QVERIFY(!client.applyFanProfiles("{invalid"));
    QVERIFY(!client.setPowerDeviceSettingsJSON("{invalid"));
    QVERIFY(!client.setLightbarSettingsJSON("{invalid"));
    QVERIFY(!client.setKeyboardBacklight("{invalid"));
    QVERIFY(!client.setBatchStateMap({{"invalid_state", "no_such_profile"}}));
  }

  void testWaterCoolerPumpAndKeyboardProfiles_TClient02()
  {
    ucc::UccdClient client;
    QVERIFY(client.isConnected());

    QVERIFY(client.enableWaterCooler(true));
    QVERIFY(client.setWaterCoolerPumpVoltage(0));
    QCOMPARE(client.getWaterCoolerPumpLevel(), std::optional<int>(0));

    QVERIFY(!client.setWaterCoolerPumpVoltage(1));

    QVERIFY(client.setWaterCoolerPumpVoltage(2));
    QCOMPARE(client.getWaterCoolerPumpLevel(), std::optional<int>(2));

    QVERIFY(client.setWaterCoolerPumpVoltage(4));
    QCOMPARE(client.getWaterCoolerPumpLevel(), std::optional<int>(4));

    QVERIFY(client.saveCustomKeyboardProfile(
        "interop-kbd", "Interop KBD", R"({"color":"#123456"})"));
    const auto kbdRaw = client.getCustomKeyboardProfiles();
    QVERIFY(kbdRaw.has_value());
    const QJsonDocument kbdDoc = QJsonDocument::fromJson(QByteArray::fromStdString(*kbdRaw));
    QVERIFY(kbdDoc.isArray());
    bool foundKbd = false;
    for (const QJsonValue &entry : kbdDoc.array()) {
      if (entry.toObject().value(QStringLiteral("id")).toString() == QStringLiteral("interop-kbd")) {
        foundKbd = true;
        break;
      }
    }
    QVERIFY(foundKbd);
    QVERIFY(client.deleteCustomKeyboardProfile("interop-kbd"));
  }

  void testProfileSwitchEmitsSignal()
  {
    ucc::UccdClient client;
    QVERIFY(client.isConnected());

    const QStringList ids = collectDefaultProfileIds(client);
    QVERIFY(ids.size() >= 2);

    const auto activeRaw = client.getActiveProfileJSON();
    QVERIFY(activeRaw.has_value());
    const QString currentId = QJsonDocument::fromJson(QByteArray::fromStdString(*activeRaw))
                                  .object()
                                  .value(QStringLiteral("id"))
                                  .toString();
    QString targetId;
    for (const QString &candidate : ids) {
      if (candidate != currentId) {
        targetId = candidate;
        break;
      }
    }
    QVERIFY(!targetId.isEmpty());

    QSignalSpy spy(&client, &ucc::UccdClient::profileChanged);
    QVERIFY(spy.isValid());
    QVERIFY(client.setActiveProfile(targetId.toStdString()));

    QTRY_VERIFY_WITH_TIMEOUT(spy.count() >= 1, 2000);
    QCOMPARE(spy.last().at(0).toString(), targetId);
  }

  void testMonitoringSnapshotAndBinaryHistory_TClient12()
  {
    ucc::UccdClient client;
    QVERIFY(client.isConnected());

    QVariantMap received;
    bool callbackCalled = false;
    QEventLoop loop;
    QTimer timeout;
    timeout.setSingleShot(true);
    connect(&timeout, &QTimer::timeout, &loop, &QEventLoop::quit);
    timeout.start(3000);

    client.requestMonitoringSnapshot(true, [&](QVariantMap data) {
      received = std::move(data);
      callbackCalled = true;
      loop.quit();
    });
    if (!callbackCalled) {
      loop.exec();
    }

    QVERIFY(callbackCalled);
    QVERIFY(!received.isEmpty());
    QVERIFY(received.contains(QStringLiteral("GetDisplayBrightness")));

    const auto history = client.getMonitorDataSince(0);
    QVERIFY(history.has_value());
  }

  void testDaemonRestartReconnect_TClient12()
  {
    ucc::UccdClient client;
    QVERIFY(client.isConnected());

    QSignalSpy connSpy(&client, &ucc::UccdClient::connectionStatusChanged);
    QSignalSpy profileSpy(&client, &ucc::UccdClient::profileChanged);
    QVERIFY(connSpy.isValid());
    QVERIFY(profileSpy.isValid());

    stopDaemon();
    QTRY_VERIFY_WITH_TIMEOUT(!client.isConnected(), 5000);

    QVERIFY(startDaemon());
    QTRY_VERIFY_WITH_TIMEOUT(client.isConnected(), 5000);

    const QStringList ids = collectDefaultProfileIds(client);
    QVERIFY(ids.size() >= 2);
    const auto activeRaw = client.getActiveProfileJSON();
    QVERIFY(activeRaw.has_value());
    const QString currentId = QJsonDocument::fromJson(QByteArray::fromStdString(*activeRaw))
                                  .object()
                                  .value(QStringLiteral("id"))
                                  .toString();
    const QString nextId = (ids.at(0) == currentId) ? ids.at(1) : ids.at(0);

    profileSpy.clear();
    QVERIFY(client.setActiveProfile(nextId.toStdString()));
    QTRY_VERIFY_WITH_TIMEOUT(profileSpy.count() >= 1, 2000);
    QTest::qWait(100);
    QCOMPARE(profileSpy.count(), 1);
  }
};

QTEST_MAIN(TestRustUccdInterop)
#include "test_rust_uccd_interop.moc"
