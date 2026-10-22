// SPDX-License-Identifier: GPL-3.0-or-later
#include <QtTest>
#include <QTemporaryDir>
#include <QFile>
#include <QFileInfo>
#include <QDir>
#include <QJsonArray>
#include "SessionTelemetry.hpp"
#include <unistd.h>
class TestSessionTelemetry:public QObject {
  Q_OBJECT
  void file(const QString &path,const QByteArray &data) {QDir().mkpath(QFileInfo(path).absolutePath());QFile f(path);QVERIFY(f.open(QIODevice::WriteOnly));f.write(data);}
  ucc::SessionPaths paths(const QString &root) {return {root+"/supplies",root+"/pci",root+"/proc",root+"/nvidia"};}
private slots:
  void batteryUnitsFallbackAndMissingData() {
    QTemporaryDir root;auto p=paths(root.path());const auto b=p.supplies+"/BAT0/";
    file(b+"type","Battery");file(b+"status","Discharging");file(b+"capacity","73");file(b+"power_now","12500000");
    ucc::SessionReader reader(p);auto snapshot=reader.snapshot(false);auto battery=snapshot["batteries"].toArray()[0].toObject();QCOMPARE(battery["powerWatts"].toDouble(),12.5);QCOMPARE(battery["capacity"].toInt(),73);QVERIFY(ucc::batterySummary(snapshot).contains("−12.5 W"));
    QVERIFY(QFile::remove(b+"power_now"));file(b+"current_now","-2000000");file(b+"voltage_now","15000000");file(b+"status","Charging");snapshot=reader.snapshot(false);QCOMPARE(snapshot["batteries"].toArray()[0].toObject()["powerWatts"].toDouble(),30.0);QVERIFY(ucc::batterySummary(snapshot).contains("+30.0 W"));
    file(b+"capacity","bad");QVERIFY(QFile::remove(b+"current_now"));battery=reader.snapshot(false)["batteries"].toArray()[0].toObject();QVERIFY(battery["capacity"].isNull());QVERIFY(battery["powerWatts"].isNull());
    file(b+"present","0");QVERIFY(reader.snapshot(false)["batteries"].toArray().isEmpty());
  }
  void externalBatteriesAreSkippedAndEachSystemBatteryIsKept() {
    QTemporaryDir root;auto p=paths(root.path());
    for(const auto &name:QStringList{"BAT0","BAT1","Mouse"}) {file(p.supplies+"/"+name+"/type","Battery");file(p.supplies+"/"+name+"/capacity","50");}
    file(p.supplies+"/Mouse/scope","Device");ucc::SessionReader reader(p);QCOMPARE(reader.snapshot(false)["batteries"].toArray().size(),2);
  }
  void gpuDiscoverySkipsAudioAndSleepingProcessScan() {
    QTemporaryDir root;auto p=paths(root.path());const auto gpu=p.pci+"/0000:37:00.0/",audio=p.pci+"/0000:37:00.1/";
    file(gpu+"vendor","0x10de");file(gpu+"class","0x030200");file(gpu+"power/runtime_status","active");file(audio+"vendor","0x10de");file(audio+"class","0x040300");file(audio+"power/runtime_status","active");
    const auto proc=p.proc+"/123456/";QStringList fields{"S"};for(int i=0;i<18;++i) fields<<"0";fields<<"987654";file(proc+"stat",("123456 (a strange ) name) "+fields.join(' ')).toUtf8());file(proc+"comm","GPU client");QDir().mkpath(proc+"fd");QVERIFY(::symlink(QFile::encodeName(p.nvidiaPrefix+"0").constData(),QFile::encodeName(proc+"fd/7").constData())==0);
    ucc::SessionReader reader(p);auto snapshot=reader.snapshot(true);QCOMPARE(snapshot["gpuCards"].toArray().size(),1);QCOMPARE(snapshot["gpuState"].toString(),QString("active"));QCOMPARE(snapshot["processes"].toArray().size(),1);QCOMPARE(snapshot["processes"].toArray()[0].toObject()["startTime"].toString(),QString("987654"));
    file(gpu+"power/runtime_status","suspended");snapshot=reader.snapshot(true);QCOMPARE(snapshot["gpuState"].toString(),QString("suspended"));QVERIFY(snapshot["processes"].toArray().isEmpty());
    file(gpu+"power/runtime_status","unknown");snapshot=reader.snapshot(false);QVERIFY(!snapshot["processesSampled"].toBool());QVERIFY(snapshot["processes"].toArray().isEmpty());
  }
  void backgroundSamplingHonorsActivityAndCoalescesRequests() {
    QTemporaryDir root;ucc::DesktopMonitor monitor(false,paths(root.path()));QSignalSpy spy(&monitor,&ucc::DesktopMonitor::updated);QVERIFY(!monitor.isActive());QVERIFY(!monitor.isBusy());
    monitor.refreshProcesses();monitor.refreshProcesses();monitor.refresh();QTRY_VERIFY(spy.size()>=1);QTRY_VERIFY(!monitor.isBusy());QVERIFY(spy.size()<=2);QVERIFY(monitor.snapshot()["processesSampled"].toBool());
    monitor.setActive(true);QVERIFY(monitor.isActive());QTRY_VERIFY(!monitor.isBusy());monitor.setActive(false);QVERIFY(!monitor.isActive());const auto count=spy.size();QTest::qWait(100);QCOMPARE(spy.size(),count);
  }
};
QTEST_GUILESS_MAIN(TestSessionTelemetry)
#include "test_session_telemetry.moc"
