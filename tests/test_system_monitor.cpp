#include <QtTest>
#include <QDBusVirtualObject>
#include <QDBusConnection>
#include <QDBusMessage>
#include <QProcess>
#include <QElapsedTimer>
#include <QTimer>
#include "SystemMonitor.hpp"

class FakeDaemon : public QDBusVirtualObject {
  int gpuReads = 0;
  int brightnessWrites = 0;
  int brightness = 70;
public:
  QString introspect(const QString &) const override {
    QString xml = "<interface name=\"com.uniwill.uccd\">";
    for (const auto &name : {"GetDGpuInfoValuesJSON", "GetIGpuInfoValuesJSON", "GetCpuPowerValuesJSON"})
      xml += QString("<method name=\"%1\"><arg type=\"s\" direction=\"out\"/></method>").arg(name);
    xml += "</interface>";
    return xml;
  }
  bool handleMessage(const QDBusMessage &msg, const QDBusConnection &bus) override {
    const QString name = msg.member();
    QVariant value;
    if (name == "GetTestBrightnessWrites") value = brightnessWrites;
    else if (name == "SetDisplayBrightness") {
      ++brightnessWrites;
      const int target = msg.arguments().value(0).toInt();
      QTimer::singleShot(300, this, [this,msg,bus,target] {
        const bool success = target != 13;
        if (success) brightness = target;
        bus.send(msg.createReply(success));
      });
      return true;
    }
    else if (name == "GetTestGpuReads") value = gpuReads;
    else if (name == "GetDGpuInfoValuesJSON") {
      ++gpuReads;
      QTimer::singleShot(300, this, [msg,bus] {
        bus.send(msg.createReply(QString("{\"temp\":52,\"coreFrequency\":1200,\"powerDraw\":45}")));
      });
      return true;
    } else if (name == "GetIGpuInfoValuesJSON") value = QString("{}");
    else if (name == "GetCpuPowerValuesJSON") value = QString("{\"powerDraw\":21}");
    else if (name.startsWith("GetFanData")) {
      value = QVariantMap{{"temp",QVariantMap{{"timestamp",qlonglong(1)},{"data",55}}},
                          {"speed",QVariantMap{{"timestamp",qlonglong(1)},{"data",40}}}};
    } else if (name == "GetCpuFrequencyMHz") value = 2400;
    else if (name == "GetWaterCoolerFanSpeed") value = 100;
    else if (name == "GetWaterCoolerPumpLevel") value = 3;
    else if (name == "GetDisplayBrightness") value = brightness;
    else if (name == "GetWebcamSWStatus" || name == "GetFnLockStatus") value = true;
    else {
      bus.send(msg.createErrorReply(QDBusError::UnknownMethod,"Unused test method"));
      return true;
    }
    bus.send(msg.createReply(value));
    return true;
  }
};

class MonitorTest : public QObject {
  Q_OBJECT
  QProcess daemon;
  int gpuReads() {
    const auto reply = QDBusConnection::systemBus().call(QDBusMessage::createMethodCall(
      "com.uniwill.uccd","/com/uniwill/uccd","com.uniwill.uccd","GetTestGpuReads"));
    return reply.arguments().value(0).toInt();
  }
private slots:
  void initTestCase() {
    daemon.start(QCoreApplication::applicationFilePath(), {"--serve"});
    QVERIFY(daemon.waitForStarted());
    QVERIFY(daemon.waitForReadyRead());
    QCOMPARE(daemon.readAllStandardOutput().trimmed(), QByteArray("READY"));
  }
  void lazyPollingKeepsEventLoopResponsive() {
    ucc::SystemMonitor monitor;
    int beats = 0;
    QTimer heartbeat;
    connect(&heartbeat,&QTimer::timeout,this,[&]{++beats;});
    heartbeat.start(10);
    QElapsedTimer elapsed; elapsed.start();
    monitor.setMonitoringActive(true);
    QTest::qWait(2700);
    QVERIFY2(gpuReads() <= 2,"One GPU snapshot per slow refresh, not one request per metric");
    QVERIFY2(beats >= elapsed.elapsed()/20,"Slow daemon replies must not block the UI event loop");
    QCOMPARE(monitor.gpuTemp(),QString("52°C"));
    QCOMPARE(monitor.gpuPower(),QString("45.0 W"));
    monitor.setMonitoringActive(false);
    const int reads = gpuReads();
    QTest::qWait(2100);
    QCOMPARE(gpuReads(),reads);
  }
  void brightnessWritesAreAsyncCoalescedAndRecoverFromFailure() {
    ucc::SystemMonitor monitor;
    int beats = 0; QTimer heartbeat;
    connect(&heartbeat,&QTimer::timeout,this,[&]{++beats;}); heartbeat.start(10);
    QElapsedTimer elapsed; elapsed.start();
    monitor.setDisplayBrightness(30);
    for (int value = 31; value <= 80; ++value) monitor.setDisplayBrightness(value);
    QVERIFY(elapsed.elapsed() < 100);
    QTRY_COMPARE_WITH_TIMEOUT(monitor.displayBrightness(),80,2000);
    const auto reply = QDBusConnection::systemBus().call(QDBusMessage::createMethodCall(
      "com.uniwill.uccd","/com/uniwill/uccd","com.uniwill.uccd","GetTestBrightnessWrites"));
    QCOMPARE(reply.arguments().value(0).toInt(),2);
    QVERIFY(beats > 25);
    monitor.setDisplayBrightness(13);
    QTest::qWait(450);
    QCOMPARE(monitor.displayBrightness(),80);
    monitor.setDisplayBrightness(60);
    QTRY_COMPARE_WITH_TIMEOUT(monitor.displayBrightness(),60,1500);
  }
  void synchronousBrightnessSetterHonorsDaemonFailure() {
    ucc::UccdClient client;
    QVERIFY(!client.setDisplayBrightness(13));
  }
  void displayModeWaitsForTheAsynchronousSnapshot() {
    QProcess display;
    display.start(QCoreApplication::applicationDirPath()+"/ucc-gui", {"--display","1"});
    QVERIFY(display.waitForStarted());
    QVERIFY(display.waitForFinished(4000));
    QCOMPARE(display.exitCode(),0);
    QVERIFY2(display.readAllStandardOutput().contains("gpu_temp=52°C"),
             "--display must wait for fresh metrics instead of printing placeholders");
  }
  void cleanupTestCase() { daemon.terminate(); daemon.waitForFinished(); }
};

int main(int argc,char **argv) {
  QCoreApplication app(argc,argv);
  if (app.arguments().contains("--serve")) {
    FakeDaemon service;
    auto bus=QDBusConnection::systemBus();
    if (!bus.registerVirtualObject("/com/uniwill/uccd",&service) ||
        !bus.registerService("com.uniwill.uccd")) return 2;
    QTextStream(stdout) << "READY\n" << Qt::flush;
    return app.exec();
  }
  MonitorTest test;
  return QTest::qExec(&test,argc,argv);
}
#include "test_system_monitor.moc"
