#include "GuiAudit.hpp"
#include "PowerDevicesTab.hpp"

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
    control("TestVisualFixture");ucc::MainWindow window;window.resize(1280,900);window.show();auto *nav=window.findChild<QListWidget*>("navigation");QVERIFY(nav);nav->setCurrentRow(7);QTest::qWait(350);
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
    auto *navigation=window.findChild<QListWidget*>("navigation");QVERIFY(navigation);navigation->setCurrentRow(5);QTest::qWait(350);
    QCOMPARE(window.findChild<QLabel*>("pageTitle")->text(),QString("Lighting"));
    QCOMPARE(window.findChild<QTabWidget*>("pages")->currentIndex(),4);
    auto *lightingTabs=window.findChild<QTabWidget*>("lightingTabs");QVERIFY(lightingTabs);
    QVERIFY(!lightingTabs->tabsClosable());
    QVERIFY(lightingTabs->tabBar()->isHidden());
    auto *segBar=window.findChild<QFrame*>("lightingSegmentedBar");QVERIFY(segBar);QVERIFY(segBar->isVisible());
    const auto segButtons=segBar->findChildren<QPushButton*>("lightingSegmentButton");
    QCOMPARE(segButtons.size(),3);
    QVERIFY(segButtons[0]->isChecked());QVERIFY(!segButtons[1]->isChecked());QVERIFY(!segButtons[2]->isChecked());
    QCOMPARE(lightingTabs->count(),3);
    QCOMPARE(lightingTabs->tabText(0),QString("Keyboard"));
    QCOMPARE(lightingTabs->tabText(1),QString("Lightbar"));
    QCOMPARE(lightingTabs->tabText(2),QString("Logo"));
    QTest::mouseClick(segButtons[1],Qt::LeftButton);QTest::qWait(100);
    QCOMPARE(lightingTabs->currentIndex(),1);
    QVERIFY(!segButtons[0]->isChecked());QVERIFY(segButtons[1]->isChecked());
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

    // Verify vivid true-to-hardware colors in BOTH Light and Dark themes (not washed-out pastel)
    QVERIFY(QMetaObject::invokeMethod(&keyboard,"onColorChanged",Qt::DirectConnection,Q_ARG(QColor,QColor("#EF00D8"))));
    const QRegularExpression bgRe("background-color:\\s*(#[0-9a-fA-F]{6})");
    for(auto theme:{ucc::FluentTheme::Mode::Light,ucc::FluentTheme::Mode::Dark}) {
      ucc::FluentTheme::setMode(theme,false);
      QCoreApplication::processEvents();
      const auto m=bgRe.match(leds[21]->styleSheet());
      QVERIFY(m.hasMatch());
      const QColor rendered(m.captured(1));
      QVERIFY2(rendered.hsvSaturation()>140, qPrintable(QString("Key color should stay vivid in theme %1, got %2 (sat=%3)").arg(int(theme)).arg(rendered.name()).arg(rendered.hsvSaturation())));
    }
    ucc::FluentTheme::setMode(ucc::FluentTheme::Mode::Light,false);
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
    auto *navigation=window.findChild<QListWidget *>("navigation");navigation->setCurrentRow(6);
    QCOMPARE(window.findChild<QLabel *>("pageTitle")->text(),QString("Power & Devices"));
    QVERIFY(window.findChild<QLabel *>("subtitle")->text().contains("modules"));
    auto *pdTab=window.findChild<ucc::PowerDevicesTab*>();QVERIFY(pdTab);
    QVERIFY(!pdTab->findChild<QCheckBox*>("turnOffKeyboardBacklightOnLidClose"));
    QVERIFY(!pdTab->findChild<QCheckBox*>("turnOffWaterCoolerLedOnLidClose"));
    QVERIFY(!pdTab->findChild<QComboBox*>("lidActionAc"));
    QVERIFY(!pdTab->findChild<QComboBox*>("lidActionTypeC"));
    QVERIFY(!pdTab->findChild<QComboBox*>("lidActionBat"));
    auto *table=window.findChild<QTableWidget *>("powerDeviceTable");QVERIFY(table);QCOMPARE(table->rowCount(),4);
    auto *cpuGrid=window.findChild<QWidget *>("cpuPowerPolicyGrid");QVERIFY(cpuGrid);QCOMPARE(cpuGrid->findChildren<QComboBox *>().size(),9);
    auto *advPcie=window.findChild<QCheckBox *>("showAdvancedPcieDevices");QVERIFY(advPcie);QVERIFY(!advPcie->isChecked());
    auto *battery=qobject_cast<QComboBox *>(table->cellWidget(0,3));QVERIFY(battery);
    QCOMPARE(battery->currentData().toString(),QString("auto"));
    battery->setCurrentIndex(battery->findData("on"));
    auto *wakeOnly=window.findChild<QCheckBox *>("wakeOnlyLidAndKeyboard");QVERIFY(wakeOnly);
    QVERIFY(wakeOnly->isChecked());
    auto *save=window.findChild<QPushButton *>("savePowerDevices");QVERIFY(save);
#ifdef UCC_READ_ONLY_PREVIEW
    QVERIFY(!save->isEnabled());
    QVERIFY(!wakeOnly->isEnabled());
    QVERIFY(stats()["writes"].toArray().isEmpty());
#else
    wakeOnly->setChecked(false);
    QVERIFY(save->isEnabled());save->click();
    const auto payload=QJsonDocument::fromJson(control("TestLastPowerDevices").toString().toUtf8()).object();
    const auto entry=payload["devices"].toArray()[0].toObject();
    QCOMPARE(entry["ac_val"].toString(),QString("on"));
    QCOMPARE(entry["type_c_val"].toString(),QString("auto"));
    QCOMPARE(entry["bat_val"].toString(),QString("on"));
    QCOMPARE(payload["lid"]["ac"].toString(),QString("screen_off"));
    QCOMPARE(payload["lid"]["bat"].toString(),QString("suspend"));
    QVERIFY(!payload["lid"]["wakeOnlyLidAndKeyboard"].toBool());
    QVERIFY(payload["lid"]["turnOffKeyboardBacklightOnLidClose"].toBool());
    QVERIFY(!payload["lid"]["turnOffWaterCoolerLedOnLidClose"].toBool());
#endif
    QTest::qWait(80);
    verifyWindowFitsLayout(window, QSize(1280,900), "power-devices-1280x900");
    if(QTest::currentTestFailed()) return;
    const QString output=qEnvironmentVariable("UCC_SCREENSHOT_DIR");
    if(!output.isEmpty()) QVERIFY(window.grab().save(output+"/power-devices.png"));
    window.resize(1024,768);
    QTest::qWait(100);
    verifyWindowFitsLayout(window, QSize(1024,768), "power-devices-1024x768");
    if(QTest::currentTestFailed()) return;
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
