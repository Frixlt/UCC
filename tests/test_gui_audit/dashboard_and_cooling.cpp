#include "GuiAudit.hpp"

void GuiAudit::powerPolicySignalReachesClientWithoutPolling() {
    QSignalSpy spy(client.get(), &ucc::UccdClient::powerPolicyChanged);
    const QString json=R"({"real_power_source":"ac","current_mode":"bat","manual_mode":"bat"})";
    control("TestEmitPowerPolicy",{json});
    QTRY_COMPARE(spy.count(),1);
    QCOMPARE(spy.takeFirst().at(0).toString(),json);
  }
void GuiAudit::dashboardWaterCoolerSelectors() {
    ucc::SystemMonitor monitor(client.get());
    ucc::DashboardTab tab(&monitor,profiles.get(),true);
    auto *fan=tab.findChild<QComboBox*>("waterCoolerFanSelector");
    auto *pump=tab.findChild<QComboBox*>("waterCoolerPumpSelector");
    QVERIFY(fan);QVERIFY(pump);
    QStringList options;
    for(int i=0;i<fan->count();++i) options.append(fan->itemText(i));
    QCOMPARE(options,QStringList({"Auto","20%","40%","60%","70%","80%","90%","100%"}));
    QCOMPARE(pump->count(),4);
    QCOMPARE(pump->itemText(2),QString("8 V"));
    QCOMPARE(pump->itemData(2).toInt(),3);
    QVERIFY(stats().value("writes").toArray().isEmpty());
    tab.show();tab.setWaterCoolerEnabled(true);tab.refreshWaterCoolerStatus();
    QTRY_COMPARE_WITH_TIMEOUT(pump->currentData().toInt(),3,2200);
    QCOMPARE(fan->currentText(),QString("100%"));
    QVERIFY(stats().value("writes").toArray().isEmpty());
#ifdef UCC_READ_ONLY_PREVIEW
    fan->setCurrentIndex(2);QVERIFY(QMetaObject::invokeMethod(fan,"activated",Q_ARG(int,2)));
    pump->setCurrentIndex(3);QVERIFY(QMetaObject::invokeMethod(pump,"activated",Q_ARG(int,3)));
    QCOMPARE(fan->currentText(),QString("40%"));
    QCOMPARE(pump->currentText(),QString("11 V"));
    QVERIFY(stats().value("writes").toArray().isEmpty());
    tab.show();tab.setWaterCoolerEnabled(true);tab.refreshWaterCoolerStatus();
    QTest::qWait(1700);
    QCOMPARE(fan->currentText(),QString("40%"));
    QCOMPARE(pump->currentText(),QString("11 V"));
    QVERIFY(stats().value("writes").toArray().isEmpty());
#endif
  }
void GuiAudit::dashboardSelectionsApplyImmediatelyFromAuto() {
    control("TestCoolerAuto",{true});control("TestCoolerEnabled",{true});
    ucc::SystemMonitor monitor(client.get());
    ucc::DashboardTab tab(&monitor,profiles.get(),true);tab.show();tab.setWaterCoolerEnabled(true);
    auto *fan=tab.findChild<QComboBox*>("waterCoolerFanSelector");
    auto *pump=tab.findChild<QComboBox*>("waterCoolerPumpSelector");
    QVERIFY(fan);QVERIFY(pump);tab.refreshWaterCoolerStatus();
    QTRY_COMPARE_WITH_TIMEOUT(fan->currentText(),QString("Auto"),2200);
    QVERIFY(fan->isEnabled());QVERIFY(pump->isEnabled());
    QVERIFY(stats().value("writes").toArray().isEmpty());
    fan->setCurrentIndex(2);QVERIFY(QMetaObject::invokeMethod(fan,"activated",Q_ARG(int,2)));
    pump->setCurrentIndex(3);QVERIFY(QMetaObject::invokeMethod(pump,"activated",Q_ARG(int,3)));
    fan->setCurrentIndex(0);QVERIFY(QMetaObject::invokeMethod(fan,"activated",Q_ARG(int,0)));
#ifdef UCC_READ_ONLY_PREVIEW
    QVERIFY(stats().value("writes").toArray().isEmpty());
#else
    QCOMPARE(stats().value("writes").toArray(),QJsonArray({"SetWaterCoolerFanSpeed","SetWaterCoolerPumpVoltage","SetWaterCoolerAutoControl"}));
#endif
  }
void GuiAudit::displayingManualControlsMustNotStopCooling() {
    ucc::FanControlTab tab(client.get(),profiles.get(),true);
    tab.setWaterCoolerEnabled(true);tab.setWaterCoolerAutoControl(false);
    QVERIFY(QMetaObject::invokeMethod(&tab,"onConnected"));
    QVERIFY2(stats().value("writes").toArray().isEmpty(),"Loading connected manual controls must not send Off or 0% commands");
    tab.setWaterCoolerEnabled(false);
    QVERIFY2(stats().value("writes").toArray().isEmpty(),"Reflecting disabled state must not send hardware commands");
  }
void GuiAudit::waterPollingIsAsyncAndPreservesValuesOnFailure() {
    ucc::FanControlTab tab(client.get(),profiles.get(),true);
    tab.setWaterCoolerEnabled(true);tab.setWaterCoolerAutoControl(false);tab.show();
    qint64 maxGap=0;QElapsedTimer elapsed;elapsed.start();qint64 previous=0;
    QTimer heartbeat;connect(&heartbeat,&QTimer::timeout,this,[&]{auto now=elapsed.elapsed();maxGap=std::max(maxGap,now-previous);previous=now;});heartbeat.start(10);
    QTest::qWait(2700);heartbeat.stop();
    QVERIFY2(maxGap<100,"Water status polling must not block the GUI event loop");
    auto *combo=pump(tab);QVERIFY(combo);QCOMPARE(combo->currentText(),QString("8V"));
    QVERIFY(stats().value("writes").toArray().isEmpty());
    control("TestFailReads",{true});QTest::qWait(2200);
    QVERIFY2(combo->isEnabled(),"A timeout must not be treated as a confirmed disconnection");
    QCOMPARE(combo->currentText(),QString("8V"));
    QVERIFY(stats().value("writes").toArray().isEmpty());
  }
void GuiAudit::dashboardBoundsOverlappingReads() {
    ucc::SystemMonitor monitor;
    ucc::DashboardTab tab(&monitor,profiles.get(),true);tab.setWaterCoolerEnabled(true);tab.show();
    QElapsedTimer elapsed;elapsed.start();tab.refreshWaterCoolerStatus();
    QVERIFY2(elapsed.elapsed()<100,"Dashboard status refresh must be asynchronous");
    for(int i=0;i<20;++i) tab.refreshWaterCoolerStatus();
    QTest::qWait(400);
    QCOMPARE(stats().value("reads").toObject().value("GetWaterCoolerConnected").toInt(),1);
  }
void GuiAudit::dashboardShowsDriverFailureAndClearsAfterRecovery() {
    control("TestVisualFixture");
    control("TestDriverHealth",{QString(R"({"healthy":false,"message":"Driver missing for current kernel"})")});
    ucc::MainWindow window;window.show();
    auto *banner=window.findChild<QLabel*>("driverHealthBanner");QVERIFY(banner);
    QTRY_VERIFY(!banner->isHidden());QVERIFY(banner->text().contains("Driver missing"));
    control("TestDriverHealth",{QString(R"({"healthy":true,"message":""})")});
    window.findChild<ucc::DashboardTab*>()->refreshDriverHealth();
    QTRY_VERIFY(banner->isHidden());
    QVERIFY(stats()["writes"].toArray().isEmpty());
  }
