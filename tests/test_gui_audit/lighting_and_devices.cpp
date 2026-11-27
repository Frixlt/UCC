#include "GuiAudit.hpp"

void GuiAudit::standaloneTrayShowsModesAndHonorsCommandFailures() {
    control("TestVisualFixture");QTemporaryDir root;ucc::SessionPaths paths{root.path()+"/sys",root.path()+"/pci",root.path()+"/proc",root.path()+"/nvidia"};ucc::DesktopTray tray(client.get(),paths);
    QAction *automatic=nullptr,*battery=nullptr;
    for(auto *action:tray.menu()->findChildren<QAction*>()) {if(action->data()==QString("auto")) automatic=action;if(action->data()==QString("bat")) battery=action;}
    QVERIFY(automatic);QVERIFY(battery);QTRY_VERIFY(automatic->isChecked());
#ifdef UCC_READ_ONLY_PREVIEW
    QVERIFY(!battery->isEnabled());battery->trigger();QVERIFY(stats()["writes"].toArray().isEmpty());
#else
    QVERIFY(battery->isEnabled());battery->trigger();QTRY_VERIFY(stats()["writes"].toArray().contains("SetManualPowerMode"));
    control("TestFailControl",{true});QTRY_VERIFY(battery->isEnabled());battery->trigger();QTRY_VERIFY(automatic->isChecked());
#endif
    const auto plain=ucc::DesktopTray::statusIcon({{"current_mode","ac"},{"manual_mode","auto"}},{},"Balanced").pixmap(64,64).toImage();
    const auto forced=ucc::DesktopTray::statusIcon({{"current_mode","ac"},{"manual_mode","bat"}},{},"Balanced").pixmap(64,64).toImage();QVERIFY(plain!=forced);
  }
void GuiAudit::batteryAndProcessPageStaysReadOnlyUntilConfirmed() {
    control("TestVisualFixture");ucc::MainWindow window;window.resize(1280,900);window.show();auto *nav=window.findChild<QListWidget*>("navigation");QVERIFY(nav);nav->setCurrentRow(9);QTest::qWait(350);
    QCOMPARE(window.findChild<QLabel*>("pageTitle")->text(),QString("Battery & Processes"));
    auto *refresh=window.findChild<QCheckBox*>("refreshRateNotifications");QVERIFY(refresh);
    QSettings preferences("UniwillControlCenter","desktop");const auto previous=preferences.value("refreshRateNotifications");
    const auto restorePreference=qScopeGuard([&]{if(previous.isValid()) preferences.setValue("refreshRateNotifications",previous);else preferences.remove("refreshRateNotifications");preferences.sync();});
    const bool requested=!refresh->isChecked();refresh->setChecked(requested);preferences.sync();
#ifdef UCC_READ_ONLY_PREVIEW
    QVERIFY(!refresh->isEnabled());QCOMPARE(preferences.value("refreshRateNotifications"),previous);
#else
    QVERIFY(refresh->isEnabled());QCOMPARE(preferences.value("refreshRateNotifications").toBool(),requested);
#endif
    auto *widget=window.findChild<ucc::SessionProcessesWidget*>();QVERIFY(widget);widget->monitor()->setActive(false);QTRY_VERIFY(!widget->monitor()->isBusy());
    const QJsonObject process{{"pid",424242},{"startTime","123456"},{"name","Example GPU application"}};
    const QJsonObject snapshot{{"batteries",QJsonArray{QJsonObject{{"name","BAT0"},{"capacity",76},{"status","Discharging"},{"powerWatts",12.5}}}},{"gpuState","active"},{"processesSampled",true},{"processes",QJsonArray{process}}};widget->setSnapshot(snapshot);
    QVERIFY(window.findChild<QLabel*>("sessionBattery")->text().contains("76%"));QVERIFY(window.findChild<QLabel*>("sessionBattery")->text().contains("12.5 W"));auto *table=window.findChild<QTableWidget*>("sessionGpuProcesses");QVERIFY(table);QCOMPARE(table->rowCount(),1);table->selectRow(0);
    auto *selected=window.findChild<QPushButton*>("terminateGpuSelected");QVERIFY(selected);
#ifdef UCC_READ_ONLY_PREVIEW
    QVERIFY(!selected->isEnabled());QVERIFY(!window.findChild<QPushButton*>("terminateGpuAll")->isEnabled());
#else
    QVERIFY(selected->isEnabled());QTimer::singleShot(0,[]{auto *box=qobject_cast<QMessageBox*>(QApplication::activeModalWidget());QVERIFY(box);QCOMPARE(box->defaultButton(),box->button(QMessageBox::No));box->reject();});QTest::mouseClick(selected,Qt::LeftButton);QCOMPARE(table->rowCount(),1);
#endif
    const auto output=qEnvironmentVariable("UCC_SCREENSHOT_DIR");if(!output.isEmpty()) {QVERIFY(QDir().mkpath(output));QVERIFY(window.grab().save(output+"/battery-processes.png"));QVERIFY(ucc::DesktopTray::statusIcon({{"current_mode","ac"},{"manual_mode","bat"}},snapshot,"max perfomance").pixmap(64,64).save(output+"/tray-manual.png"));}
    window.showMinimized();QTest::qWait(50);QVERIFY(!widget->monitor()->isActive());window.hide();QVERIFY(!widget->monitor()->isActive());
    QVERIFY(stats()["writes"].toArray().isEmpty());
  }
void GuiAudit::sessionAgentRelaysOnlyNotificationEvents() {
    const auto agentPath=QCoreApplication::applicationDirPath()+"/ucc-session-agent";
    if(!QFile::exists(agentPath)) QSKIP("Session agent not built");
    control("TestVisualFixture");QTemporaryDir root;QVERIFY(root.isValid());
    QFile monitor(root.path()+"/dbus-monitor");QVERIFY(monitor.open(QIODevice::WriteOnly));
    monitor.write("#!/bin/sh\nprintf '%s\\n' 'method call time=1 sender=:1.2 -> destination=:1.3 serial=4 path=/org/freedesktop/Notifications; interface=org.freedesktop.Notifications; member=Notify' '   string \"private body\"'\nexec /bin/sleep 5\n");monitor.close();QVERIFY(monitor.setPermissions(QFile::ReadOwner|QFile::WriteOwner|QFile::ExeOwner));
    QProcess agent;auto environment=QProcessEnvironment::systemEnvironment();environment.insert("PATH",root.path()+":"+environment.value("PATH"));environment.insert("XDG_RUNTIME_DIR",root.path());agent.setProcessEnvironment(environment);agent.start(agentPath,{});QVERIFY(agent.waitForStarted());
    const auto cleanup=qScopeGuard([&]{if(agent.state()!=QProcess::NotRunning) {agent.terminate();if(!agent.waitForFinished(1500)) {agent.kill();agent.waitForFinished();}}});
#ifdef UCC_READ_ONLY_PREVIEW
    QTRY_COMPARE(agent.state(),QProcess::NotRunning);QVERIFY(stats()["writes"].toArray().isEmpty());
#else
    QTRY_VERIFY(stats()["writes"].toArray().contains("TriggerLightbarAnimation"));
    QCOMPARE(stats()["writes"].toArray().size(),1);
    agent.terminate();QVERIFY(agent.waitForFinished(1500));QCOMPARE(agent.exitStatus(),QProcess::NormalExit);QCOMPARE(agent.exitCode(),0);
#endif
  }
void GuiAudit::lightbarKeepsSourceValuesAndRejectsFailedCommands() {
    control("TestVisualFixture");ucc::MainWindow window;window.resize(1280,900);window.show();
    auto *navigation=window.findChild<QListWidget*>("navigation");QVERIFY(navigation);navigation->setCurrentRow(8);QTest::qWait(350);
    QCOMPARE(window.findChild<QLabel*>("pageTitle")->text(),QString("Lightbar"));
    QCOMPARE(window.findChild<QTabWidget*>("pages")->currentIndex(),7);
    auto *ac=window.findChild<QSpinBox*>("lightbarBrightness_ac"),*usb=window.findChild<QSpinBox*>("lightbarBrightness_type_c");QVERIFY(ac);QVERIFY(usb);QCOMPARE(ac->value(),0);QCOMPARE(usb->value(),148);
    QVERIFY(!window.findChild<QCheckBox*>("lightbarEnabled_bat")->isChecked());
    auto *save=window.findChild<QPushButton*>("saveLightbar"),*test=window.findChild<QPushButton*>("testLightbar");QVERIFY(save);QVERIFY(test);
#ifdef UCC_READ_ONLY_PREVIEW
    QVERIFY(!save->isEnabled());QVERIFY(!test->isEnabled());
#else
    usb->setValue(99);QTest::mouseClick(save,Qt::LeftButton);
    const auto saved=QJsonDocument::fromJson(control("TestLastLightbar").toString().toUtf8()).object()["power_settings"].toObject();
    QCOMPARE(saved["ac"].toObject()["brightness_val"].toInt(),0);QCOMPARE(saved["type_c"].toObject()["brightness_val"].toInt(),99);QCOMPARE(saved["ac"].toObject()["color_mode"].toArray(),(QJsonArray{43,47,255}));
    QTest::mouseClick(test,Qt::LeftButton);QVERIFY(stats()["writes"].toArray().contains("TriggerLightbarAnimation"));
    control("TestFailControl",{true});QVERIFY(!client->setLightbarSettingsJSON("{}"));QVERIFY(!client->triggerLightbarAnimation(true));QVERIFY(!client->triggerLightbarAnimation(false));
#endif
    const auto output=qEnvironmentVariable("UCC_SCREENSHOT_DIR");if(!output.isEmpty()) {QVERIFY(QDir().mkpath(output));QVERIFY(window.grab().save(output+"/lightbar.png"));}
  }
void GuiAudit::calibratedKeyboardKeepsPhysicalLedIdentities() {
    ucc::KeyboardVisualizerWidget keyboard(126,50,nullptr,true);
    const auto buttons = keyboard.findChildren<QPushButton *>();
    QMap<int,QPushButton *> leds;
    for (auto *button : buttons) if(button->property("ledId").isValid()) leds[button->property("ledId").toInt()] = button;
    QCOMPARE(leds.size(),101);
    QCOMPARE(leds[0]->text(),QString("LCtrl"));
    QCOMPARE(leds[21]->text(),QString("LShift"));
    QCOMPARE(leds[56]->text(),QString("Enter"));
    QCOMPARE(leds[118]->text(),QString("SC"));
    QCOMPARE(leds[119]->text(),QString("PrtSc"));
    keyboard.resize(1200,360);keyboard.show();QTest::qWait(20);
    QCOMPARE(keyboard.findChild<QScrollArea *>()->horizontalScrollBar()->maximum(),0);
    QTest::mouseClick(leds[21],Qt::LeftButton,Qt::ShiftModifier);
    QTest::mouseClick(leds[56],Qt::LeftButton,Qt::ShiftModifier);
    QVERIFY(QMetaObject::invokeMethod(&keyboard,"onColorChanged",Qt::DirectConnection,Q_ARG(QColor,QColor(1,2,3))));
    const auto states=keyboard.getJSONState();
    QCOMPARE(states[21].toObject()["red"].toInt(),1);
    QCOMPARE(states[56].toObject()["blue"].toInt(),3);
    QCOMPARE(states[77].toObject()["red"].toInt(),255);
    QVERIFY(leds[21]->toolTip().contains("LED #21"));
    const QString output=qEnvironmentVariable("UCC_SCREENSHOT_DIR");
    if(!output.isEmpty()) QVERIFY(keyboard.grab().save(output+"/personal-keyboard.png"));
  }
void GuiAudit::sparseKeyboardZonesKeepSavedPhysicalIndices() {
    ucc::KeyboardVisualizerWidget keyboard(4,50,nullptr,true,QJsonArray{0,21,56,66});
    const auto buttons=keyboard.findChildren<QPushButton *>();
    int count=0;for(auto *button:buttons) count+=button->property("ledId").isValid();
    QCOMPARE(count,4);
    for(auto *button:buttons) if(button->property("ledId").toInt()==56) {
      QCOMPARE(button->property("zoneId").toInt(),2);
      QCOMPARE(button->text(),QString("Enter"));
    }
  }
void GuiAudit::deviceMatrixKeepsThreeIndependentModes() {
    control("TestVisualFixture");ucc::MainWindow window;window.resize(1280,900);window.show();
    auto *navigation=window.findChild<QListWidget *>("navigation");navigation->setCurrentRow(7);
    QCOMPARE(window.findChild<QLabel *>("pageTitle")->text(),QString("Power & Devices"));
    QVERIFY(window.findChild<QLabel *>("subtitle")->text().contains("modules"));
    auto *table=window.findChild<QTableWidget *>("powerDeviceTable");QVERIFY(table);QCOMPARE(table->rowCount(),4);
    auto *cpu=window.findChild<QTableWidget *>("cpuPowerPolicyTable");QVERIFY(cpu);QCOMPARE(cpu->rowCount(),3);
    auto *battery=qobject_cast<QComboBox *>(table->cellWidget(0,3));QVERIFY(battery);
    QCOMPARE(battery->currentData().toString(),QString("auto"));
    battery->setCurrentIndex(battery->findData("on"));
    auto *lidAc=window.findChild<QComboBox *>("lidActionAc");QVERIFY(lidAc);
    auto *lidTypeC=window.findChild<QComboBox *>("lidActionTypeC");QVERIFY(lidTypeC);
    auto *lidBat=window.findChild<QComboBox *>("lidActionBat");QVERIFY(lidBat);
    auto *wakeOnly=window.findChild<QCheckBox *>("wakeOnlyLidAndKeyboard");QVERIFY(wakeOnly);
    auto *lidOffKbd=window.findChild<QCheckBox *>("turnOffKeyboardBacklightOnLidClose");QVERIFY(lidOffKbd);
    auto *lidOffWcLed=window.findChild<QCheckBox *>("turnOffWaterCoolerLedOnLidClose");QVERIFY(lidOffWcLed);
    QCOMPARE(lidAc->currentData().toString(),QString("screen_off"));
    QCOMPARE(lidTypeC->currentData().toString(),QString("screen_off"));
    QCOMPARE(lidBat->currentData().toString(),QString("suspend"));
    QVERIFY(wakeOnly->isChecked());
    QVERIFY(lidOffKbd->isChecked());
    QVERIFY(!lidOffWcLed->isChecked());
    auto *save=window.findChild<QPushButton *>("savePowerDevices");QVERIFY(save);
#ifdef UCC_READ_ONLY_PREVIEW
    QVERIFY(!save->isEnabled());
    QVERIFY(!lidAc->isEnabled());
    QVERIFY(!wakeOnly->isEnabled());
    QVERIFY(!lidOffKbd->isEnabled());
    QVERIFY(!lidOffWcLed->isEnabled());
    QVERIFY(stats()["writes"].toArray().isEmpty());
#else
    lidOffWcLed->setChecked(true);
    QVERIFY(save->isEnabled());save->click();
    const auto payload=QJsonDocument::fromJson(control("TestLastPowerDevices").toString().toUtf8()).object();
    const auto entry=payload["devices"].toArray()[0].toObject();
    QCOMPARE(entry["ac_val"].toString(),QString("on"));
    QCOMPARE(entry["type_c_val"].toString(),QString("auto"));
    QCOMPARE(entry["bat_val"].toString(),QString("on"));
    QCOMPARE(payload["lid"]["ac"].toString(),QString("screen_off"));
    QCOMPARE(payload["lid"]["bat"].toString(),QString("suspend"));
    QVERIFY(payload["lid"]["wakeOnlyLidAndKeyboard"].toBool());
    QVERIFY(payload["lid"]["turnOffKeyboardBacklightOnLidClose"].toBool());
    QVERIFY(payload["lid"]["turnOffWaterCoolerLedOnLidClose"].toBool());
#endif
    QTest::qWait(80);
    const QString output=qEnvironmentVariable("UCC_SCREENSHOT_DIR");
    if(!output.isEmpty()) QVERIFY(window.grab().save(output+"/power-devices.png"));
  }
void GuiAudit::rejectedPolicyCommandsAreNotReportedAsSuccessful() {
    control("TestFailControl",{true});
    QVERIFY(!client->setManualPowerMode("bat"));
    QVERIFY(!client->setPowerDeviceSettingsJSON("{}"));
    QVERIFY(!client->scanPowerDevices());
    QVERIFY(!client->registerPowerRequest("render","test","ac"));
    QVERIFY(!client->unregisterPowerRequest("render"));
    QVERIFY(!client->setKeyboardBacklight("[]"));
  }
