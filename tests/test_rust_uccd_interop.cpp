// SPDX-License-Identifier: GPL-3.0-or-later

#include <QCoreApplication>
#include <QDBusConnection>
#include <QDBusError>
#include <QDBusMessage>
#include <QDBusVirtualObject>
#include <QElapsedTimer>
#include <QEventLoop>
#include <QFileInfo>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QProcess>
#include <QProcessEnvironment>
#include <QScopeGuard>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QTest>
#include <QTextStream>
#include <QTimer>
#include <QVariantMap>

#include "UccdClient.hpp"


namespace
{
class MockInteropDaemon : public QDBusVirtualObject
{
  QString m_fanMode = QStringLiteral("real_rpm");
  QString m_systemInfoJson = QStringLiteral(
      R"({"sysVendor":"MECHREVO","boardName":"GM7IX0N","manufacturer":"uniwill","laptopModel":"Uniwill Alias","platformBackend":"uwcc","mux":{"supported":true,"activeGpu":"dgpu","bootMode":"dgpu_only"}})");

  static QVariantMap makeFanMap(qlonglong ts, int temp, int speed, std::optional<int> realRpm)
  {
    QVariantMap out{
        {QStringLiteral("temp"), QVariantMap{{QStringLiteral("timestamp"), ts}, {QStringLiteral("data"), temp}}},
        {QStringLiteral("speed"), QVariantMap{{QStringLiteral("timestamp"), ts}, {QStringLiteral("data"), speed}}},
    };
    if (realRpm.has_value()) {
      out.insert(QStringLiteral("realRpm"),
                 QVariantMap{{QStringLiteral("timestamp"), ts}, {QStringLiteral("data"), *realRpm}});
    }
    return out;
  }

public:
  QString introspect(const QString &) const override
  {
    return QStringLiteral(
        "<interface name=\"com.uniwill.uccd\">"
        "<method name=\"IsDeviceSupported\"><arg type=\"b\" direction=\"out\"/></method>"
        "</interface>");
  }

  bool handleMessage(const QDBusMessage &msg, const QDBusConnection &bus) override
  {
    const QString member = msg.member();
    if (member == QStringLiteral("TestSetFanMode")) {
      m_fanMode = msg.arguments().value(0).toString();
      bus.send(msg.createReply(true));
      return true;
    }
    if (member == QStringLiteral("TestSetSystemInfo")) {
      m_systemInfoJson = msg.arguments().value(0).toString();
      bus.send(msg.createReply(true));
      return true;
    }
    if (member == QStringLiteral("IsDeviceSupported")) {
      bus.send(msg.createReply(true));
      return true;
    }
    if (member == QStringLiteral("GetSystemInfoJSON")) {
      bus.send(msg.createReply(m_systemInfoJson));
      return true;
    }
    if (member == QStringLiteral("GetFanDataCPU")) {
      if (m_fanMode == QStringLiteral("real_rpm")) {
        bus.send(msg.createReply(makeFanMap(1001, 64, 50, 3250)));
      } else if (m_fanMode == QStringLiteral("zero_rpm")) {
        bus.send(msg.createReply(makeFanMap(1002, 40, 0, 0)));
      } else {
        bus.send(msg.createReply(makeFanMap(1003, 64, 50, std::nullopt)));
      }
      return true;
    }
    if (member == QStringLiteral("GetFanDataGPU1")) {
      if (m_fanMode == QStringLiteral("real_rpm")) {
        bus.send(msg.createReply(makeFanMap(1001, 60, 45, 2800)));
      } else if (m_fanMode == QStringLiteral("zero_rpm")) {
        bus.send(msg.createReply(makeFanMap(1002, 38, 0, 0)));
      } else {
        bus.send(msg.createReply(makeFanMap(1003, 60, 45, std::nullopt)));
      }
      return true;
    }
    if (member == QStringLiteral("GetFanDataGPU2")) {
      if (m_fanMode == QStringLiteral("real_rpm")) {
        bus.send(msg.createReply(makeFanMap(1001, 59, 45, 3200)));
      } else if (m_fanMode == QStringLiteral("zero_rpm")) {
        bus.send(msg.createReply(makeFanMap(1002, 37, 0, 0)));
      } else {
        bus.send(msg.createReply(makeFanMap(1003, 59, 45, std::nullopt)));
      }
      return true;
    }
    bus.send(msg.createErrorReply(QDBusError::UnknownMethod, QStringLiteral("Unsupported mock method")));
    return true;
  }
};
} // namespace

class TestRustUccdInterop : public QObject
{
  Q_OBJECT

private:
  QTemporaryDir m_configDir;
  QString m_daemonBin;
  QProcess *m_daemon = nullptr;
  QProcess *m_mockDaemon = nullptr;

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

  bool startMockDaemon()
  {
    stopMockDaemon();
    m_mockDaemon = new QProcess(this);
    m_mockDaemon->setProcessChannelMode(QProcess::SeparateChannels);
    m_mockDaemon->start(QCoreApplication::applicationFilePath(),
                        {QStringLiteral("--serve-mock-dbus")});
    if (!m_mockDaemon->waitForStarted(5000)) {
      qWarning() << "Failed to start mock D-Bus daemon:" << m_mockDaemon->errorString();
      return false;
    }

    QElapsedTimer timer;
    timer.start();
    QByteArray out;
    while (timer.elapsed() < 5000) {
      if (m_mockDaemon->waitForReadyRead(100)) {
        out.append(m_mockDaemon->readAllStandardOutput());
        if (out.contains("READY")) {
          return true;
        }
      }
      if (m_mockDaemon->state() == QProcess::NotRunning) {
        qWarning() << "mock D-Bus daemon exited early:" << m_mockDaemon->readAllStandardError();
        return false;
      }
    }
    return false;
  }

  void stopMockDaemon()
  {
    if (!m_mockDaemon) {
      return;
    }
    if (m_mockDaemon->state() != QProcess::NotRunning) {
      m_mockDaemon->terminate();
      if (!m_mockDaemon->waitForFinished(4000)) {
        m_mockDaemon->kill();
        m_mockDaemon->waitForFinished(2000);
      }
    }
    m_mockDaemon->deleteLater();
    m_mockDaemon = nullptr;
  }

  static void callMockControl(const QString &method, const QVariantList &args = {})
  {
    QDBusMessage msg = QDBusMessage::createMethodCall(
        QStringLiteral("com.uniwill.uccd"),
        QStringLiteral("/com/uniwill/uccd"),
        QStringLiteral("com.uniwill.uccd"),
        method);
    msg.setArguments(args);
    QDBusConnection::systemBus().call(msg);
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
    stopMockDaemon();
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

#ifdef UCC_READ_ONLY_PREVIEW
    QVERIFY(!client.enableWaterCooler(true));
    QVERIFY(!client.setWaterCoolerPumpVoltage(0));
    QVERIFY(!client.setWaterCoolerPumpVoltage(2));
    QVERIFY(!client.saveCustomKeyboardProfile(
        "interop-kbd", "Interop KBD", R"({"color":"#123456"})"));
    const auto kbdRaw = client.getCustomKeyboardProfiles();
    QVERIFY(kbdRaw.has_value());
    const QJsonDocument kbdDoc = QJsonDocument::fromJson(QByteArray::fromStdString(*kbdRaw));
    QVERIFY(kbdDoc.isArray());
    for (const QJsonValue &entry : kbdDoc.array()) {
      QVERIFY(entry.toObject().value(QStringLiteral("id")).toString() != QStringLiteral("interop-kbd"));
    }
    QVERIFY(!client.deleteCustomKeyboardProfile("interop-kbd"));
#else
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
#endif
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
#ifdef UCC_READ_ONLY_PREVIEW
    QVERIFY(!client.setActiveProfile(targetId.toStdString()));
    QTest::qWait(100);
    QCOMPARE(spy.count(), 0);
#else
    QVERIFY(client.setActiveProfile(targetId.toStdString()));

    QTRY_VERIFY_WITH_TIMEOUT(spy.count() >= 1, 2000);
    QCOMPARE(spy.last().at(0).toString(), targetId);
#endif
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
#ifdef UCC_READ_ONLY_PREVIEW
    QVERIFY(!client.setActiveProfile(nextId.toStdString()));
    QTest::qWait(100);
    QCOMPARE(profileSpy.count(), 0);
#else
    QVERIFY(client.setActiveProfile(nextId.toStdString()));
    QTRY_VERIFY_WITH_TIMEOUT(profileSpy.count() >= 1, 2000);
    QTest::qWait(100);
    QCOMPARE(profileSpy.count(), 1);
#endif
  }

  void testFanRealRpmAndNoFabrication()
  {
    {
      ucc::UccdClient liveClient;
      QVERIFY(liveClient.isConnected());
      QCOMPARE(liveClient.getFanSpeedPercent(), std::optional<int>(40));
      QCOMPARE(liveClient.getGpuFanSpeedPercent(), std::optional<int>(35));
      QVERIFY2(!liveClient.getFanSpeedRPM().has_value(),
               "Absent CPU realRpm must return std::nullopt, never speed * 60");
      QVERIFY2(!liveClient.getGpuFanSpeedRPM().has_value(),
               "Absent GPU realRpm must return std::nullopt, never speed * 60");
    }

    stopDaemon();
    const auto restoreRustDaemon = qScopeGuard([this] {
      stopMockDaemon();
      if (!startDaemon()) {
        qWarning() << "Failed to restart Rust interop_daemon after mock RPM test";
      }
    });

    QVERIFY2(startMockDaemon(), "Mock D-Bus daemon failed to start");
    ucc::UccdClient mockClient;
    QTRY_VERIFY_WITH_TIMEOUT(mockClient.isConnected(), 3000);

    callMockControl(QStringLiteral("TestSetFanMode"), {QStringLiteral("real_rpm")});
    QCOMPARE(mockClient.getFanSpeedRPM(), std::optional<int>(3250));
    QCOMPARE(mockClient.getGpuFanSpeedRPM(), std::optional<int>(3000));

    callMockControl(QStringLiteral("TestSetFanMode"), {QStringLiteral("zero_rpm")});
    QCOMPARE(mockClient.getFanSpeedRPM(), std::optional<int>(0));
    QCOMPARE(mockClient.getGpuFanSpeedRPM(), std::optional<int>(0));

    callMockControl(QStringLiteral("TestSetFanMode"), {QStringLiteral("no_rpm")});
    QCOMPARE(mockClient.getFanSpeedPercent(), std::optional<int>(50));
    QCOMPARE(mockClient.getGpuFanSpeedPercent(), std::optional<int>(45));
    QVERIFY2(!mockClient.getFanSpeedRPM().has_value(),
             "Missing CPU realRpm must return std::nullopt instead of fabricating speed * 60");
    QVERIFY2(!mockClient.getGpuFanSpeedRPM().has_value(),
             "Missing GPU realRpm must return std::nullopt instead of fabricating speed * 60");
  }

  void testSystemInfoPreservesPlatformBackendAndMux()
  {
    {
      ucc::UccdClient liveClient;
      QVERIFY(liveClient.isConnected());
      const auto liveRaw = liveClient.getSystemInfoJSON();
      QVERIFY(liveRaw.has_value());
      const QJsonDocument liveDoc = QJsonDocument::fromJson(QByteArray::fromStdString(*liveRaw));
      QVERIFY(liveDoc.isObject());
      QCOMPARE(liveDoc.object().value(QStringLiteral("platformBackend")).toString(),
               QStringLiteral("mock"));
    }

    stopDaemon();
    const auto restoreRustDaemon = qScopeGuard([this] {
      stopMockDaemon();
      if (!startDaemon()) {
        qWarning() << "Failed to restart Rust interop_daemon after mock sysinfo test";
      }
    });

    QVERIFY2(startMockDaemon(), "Mock D-Bus daemon failed to start");
    ucc::UccdClient mockClient;
    QTRY_VERIFY_WITH_TIMEOUT(mockClient.isConnected(), 3000);

    // Non-MECHREVO pass-through preserves platformBackend and mux fields.
    callMockControl(
        QStringLiteral("TestSetSystemInfo"),
        {QStringLiteral(
            R"({"sysVendor":"TUXEDO","boardName":"GM7IX0N","platformBackend":"uwcc","mux":{"supported":true,"activeGpu":"igpu","bootMode":"hybrid"}})")});
    const auto passthroughRaw = mockClient.getSystemInfoJSON();
    QVERIFY(passthroughRaw.has_value());
    const QJsonObject passthroughObj =
        QJsonDocument::fromJson(QByteArray::fromStdString(*passthroughRaw)).object();
    QCOMPARE(passthroughObj.value(QStringLiteral("platformBackend")).toString(),
             QStringLiteral("uwcc"));
    const QJsonObject passthroughMux = passthroughObj.value(QStringLiteral("mux")).toObject();
    QCOMPARE(passthroughMux.value(QStringLiteral("supported")).toBool(false), true);
    QCOMPARE(passthroughMux.value(QStringLiteral("activeGpu")).toString(), QStringLiteral("igpu"));
    QCOMPARE(passthroughMux.value(QStringLiteral("bootMode")).toString(), QStringLiteral("hybrid"));

    // MECHREVO OEM rewrite path re-serializes JSON and must preserve platformBackend and mux.
    callMockControl(
        QStringLiteral("TestSetSystemInfo"),
        {QStringLiteral(
            R"({"sysVendor":"MECHREVO","boardName":"GM7IX0N","manufacturer":"uniwill","laptopModel":"Uniwill Alias","platformBackend":"uwcc","mux":{"supported":true,"activeGpu":"dgpu","bootMode":"dgpu_only"}})")});
    const auto rewrittenRaw = mockClient.getSystemInfoJSON();
    QVERIFY(rewrittenRaw.has_value());
    const QJsonObject rewrittenObj =
        QJsonDocument::fromJson(QByteArray::fromStdString(*rewrittenRaw)).object();
    QCOMPARE(rewrittenObj.value(QStringLiteral("manufacturer")).toString(),
             QStringLiteral("MECHREVO"));
    QCOMPARE(rewrittenObj.value(QStringLiteral("laptopModel")).toString(),
             QStringLiteral("MECHREVO GM7IX0N"));
    QCOMPARE(rewrittenObj.value(QStringLiteral("platformBackend")).toString(),
             QStringLiteral("uwcc"));
    const QJsonObject rewrittenMux = rewrittenObj.value(QStringLiteral("mux")).toObject();
    QCOMPARE(rewrittenMux.value(QStringLiteral("supported")).toBool(false), true);
    QCOMPARE(rewrittenMux.value(QStringLiteral("activeGpu")).toString(), QStringLiteral("dgpu"));
    QCOMPARE(rewrittenMux.value(QStringLiteral("bootMode")).toString(), QStringLiteral("dgpu_only"));
  }
};

int main(int argc, char **argv)
{
  QCoreApplication app(argc, argv);
  if (app.arguments().contains(QStringLiteral("--serve-mock-dbus"))) {
    MockInteropDaemon service;
    auto bus = QDBusConnection::systemBus();
    if (!bus.registerVirtualObject(QStringLiteral("/com/uniwill/uccd"), &service) ||
        !bus.registerService(QStringLiteral("com.uniwill.uccd"))) {
      return 2;
    }
    QTextStream(stdout) << "READY\n" << Qt::flush;
    return app.exec();
  }

  TestRustUccdInterop test;
  return QTest::qExec(&test, argc, argv);
}
#include "test_rust_uccd_interop.moc"
