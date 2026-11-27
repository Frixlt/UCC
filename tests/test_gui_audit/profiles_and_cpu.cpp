#include "GuiAudit.hpp"

void GuiAudit::desktopPreferenceImportDoesNotContactTheDaemon() {
    QTemporaryDir root;QFile input(root.path()+"/preferences.json");QVERIFY(input.open(QIODevice::WriteOnly));input.write("{\"theme\":\"dark\",\"startMinimized\":true}");input.close();
    QSettings desktop("UniwillControlCenter","desktop"),appearance("UniwillControlCenter","appearance");const auto oldMin=desktop.value("startMinimized"),oldTheme=appearance.value("themeMode");
    const auto restore=qScopeGuard([&]{if(oldMin.isValid()) desktop.setValue("startMinimized",oldMin);else desktop.remove("startMinimized");if(oldTheme.isValid()) appearance.setValue("themeMode",oldTheme);else appearance.remove("themeMode");desktop.sync();appearance.sync();});
    const auto before=stats();QProcess importer;importer.start(QCoreApplication::applicationDirPath()+"/ucc-gui",{"--import-preferences",input.fileName(),"--preferences-only"});QVERIFY(importer.waitForStarted());QTRY_COMPARE(importer.state(),QProcess::NotRunning);
#ifdef UCC_READ_ONLY_PREVIEW
    QCOMPARE(importer.exitCode(),1);desktop.sync();appearance.sync();QCOMPARE(desktop.value("startMinimized"),oldMin);QCOMPARE(appearance.value("themeMode"),oldTheme);
#else
    QCOMPARE(importer.exitCode(),0);desktop.sync();appearance.sync();QVERIFY(desktop.value("startMinimized").toBool());QCOMPARE(appearance.value("themeMode").toInt(),1);
#endif
    QCOMPARE(stats(),before);
  }
void GuiAudit::profileRefreshRateSavesAndChargingLivesOutsideProfiles() {
    control("TestVisualFixture");
    control("TestCustomProfile",{QString(R"({"id":"custom-rate","name":"Custom rate","display":{"useRefRate":true,"refreshRate":60,"useResolution":false},"chargingProfile":"high_capacity","fan":{"fanProfile":"balanced-cooling"}})")});
    ucc::MainWindow window;window.resize(1280,1000);window.show();
    auto* nav=window.findChild<QListWidget*>("navigation");nav->setCurrentRow(1);
    auto* names=window.findChild<QComboBox*>("profileName");QVERIFY(names);
    names->setCurrentIndex(names->findData("custom-rate"));
    auto* rate=window.findChild<QComboBox*>("profileRefreshRate");QVERIFY(rate);
    QCOMPARE(rate->currentData().toInt(),60);
    auto* lidOffKbd=window.findChild<QCheckBox*>("turnOffKeyboardBacklightOnLidClose");QVERIFY(lidOffKbd);
    auto* lidOffWcLed=window.findChild<QCheckBox*>("turnOffWaterCoolerLedOnLidClose");QVERIFY(lidOffWcLed);
    QVERIFY(lidOffKbd->isChecked());QVERIFY(!lidOffWcLed->isChecked());
    auto* charging=window.findChild<QComboBox*>("globalChargingProfile");QVERIFY(charging);
    QTRY_COMPARE(charging->count(),2);QVERIFY(!charging->isVisible());
    QCOMPARE(charging->currentData().toString(),QString("stationary"));
#ifndef UCC_READ_ONLY_PREVIEW
    QVERIFY(lidOffKbd->isEnabled());QVERIFY(lidOffWcLed->isEnabled());
    lidOffKbd->setChecked(false);lidOffWcLed->setChecked(true);
    rate->setCurrentIndex(rate->findData(240));
    window.findChild<QPushButton*>("saveProfile")->click();
    QTRY_VERIFY(!control("TestLastSavedProfile").toString().isEmpty());
    auto saved=QJsonDocument::fromJson(control("TestLastSavedProfile").toString().toUtf8()).object();
    QCOMPARE(saved["display"].toObject()["refreshRate"].toInt(),240);
    QVERIFY(saved["display"].toObject()["useRefRate"].toBool());
    QVERIFY(!saved["lid"].toObject()["turnOffKeyboardBacklightOnLidClose"].toBool());
    QVERIFY(saved["lid"].toObject()["turnOffWaterCoolerLedOnLidClose"].toBool());
    QVERIFY(!saved.contains("chargingProfile"));QVERIFY(!saved.contains("chargeType"));
    rate->setCurrentIndex(0);window.findChild<QPushButton*>("saveProfile")->click();
    saved=QJsonDocument::fromJson(control("TestLastSavedProfile").toString().toUtf8()).object();
    QVERIFY(!saved["display"].toObject()["useRefRate"].toBool());
#else
    QVERIFY(!lidOffKbd->isEnabled());QVERIFY(!lidOffWcLed->isEnabled());
#endif
    const auto output=qEnvironmentVariable("UCC_SCREENSHOT_DIR");
    if(!output.isEmpty()) {QTest::qWait(450);for(auto* scroll:window.findChildren<QScrollArea*>()) if(scroll->widget() && scroll->widget()->isAncestorOf(rate)) scroll->ensureWidgetVisible(rate);QTest::qWait(100);QDir().mkpath(output);QVERIFY(window.grab().save(output+"/profile-refresh.png"));}
    nav->setCurrentRow(8);QVERIFY(charging->isVisible());
#ifndef UCC_READ_ONLY_PREVIEW
    const auto before=stats()["writes"].toArray().size();
    charging->setCurrentIndex(charging->findData("high_capacity"));
    window.findChild<QPushButton*>("saveBatterySettings")->click();
    QTRY_VERIFY(stats()["writes"].toArray().size()>before);
    QVERIFY(stats()["writes"].toArray().contains("SetChargingProfile"));
#endif
    if(!output.isEmpty()) QVERIFY(window.grab().save(output+"/battery-charging.png"));
  }
void GuiAudit::individualCpuSelectionSavesIdsAndPreservesKernelLockedCpu() {
    control("TestVisualFixture");
    control("TestCustomProfile",{QString(R"({"id":"core-mask","name":"Core mask","cpu":{"onlineCores":3,"enabledCores":[0,8,10]},"fan":{"fanProfile":"balanced-cooling"}})")});
    ucc::MainWindow window;window.resize(1280,1000);window.show();
    auto* nav=window.findChild<QListWidget*>("navigation");nav->setCurrentRow(1);
    auto* names=window.findChild<QComboBox*>("profileName");names->setCurrentIndex(names->findData("core-mask"));
    auto* mode=window.findChild<QComboBox*>("cpuCoreSelectionMode");QVERIFY(mode);QCOMPARE(mode->currentIndex(),1);
    QTRY_VERIFY(window.findChild<QCheckBox*>("enabledCpu23"));
    auto* boot=window.findChild<QCheckBox*>("enabledCpu0");QVERIFY(boot->isChecked());QVERIFY(!boot->isEnabled());
    auto* c8=window.findChild<QCheckBox*>("enabledCpu8");auto* c9=window.findChild<QCheckBox*>("enabledCpu9");
    QVERIFY(c8->text().contains("E"));QVERIFY(c8->isChecked());QVERIFY(!c9->isChecked());
#ifndef UCC_READ_ONLY_PREVIEW
    c8->setChecked(false);c9->setChecked(true);window.findChild<QPushButton*>("saveProfile")->click();
    auto saved=QJsonDocument::fromJson(control("TestLastSavedProfile").toString().toUtf8()).object()["cpu"].toObject();
    QCOMPARE(saved["enabledCores"].toArray(),QJsonArray({0,9,10}));QCOMPARE(saved["onlineCores"].toInt(),3);
    window.findChild<QPushButton*>("selectECpus")->click();
    QVERIFY(boot->isChecked());QVERIFY(!window.findChild<QCheckBox*>("enabledCpu1")->isChecked());
    QVERIFY(window.findChild<QCheckBox*>("enabledCpu23")->isChecked());
#endif
    // The dynamically revealed grid must expand the card, rather than clip its CPUs.
    QTest::qWait(100);
    auto* selectionWidget=window.findChild<ucc::CpuCoreSelectionWidget*>();
    auto* lastCpu=window.findChild<QCheckBox*>("enabledCpu23");
    QVERIFY(selectionWidget->rect().contains(lastCpu->mapTo(selectionWidget,QPoint(0,lastCpu->height()-1))));
    for(auto* sibling:selectionWidget->parentWidget()->findChildren<QWidget*>(QString(),Qt::FindDirectChildrenOnly)) {
      if(sibling==selectionWidget || sibling->isHidden()) continue;
      QVERIFY2(!selectionWidget->geometry().intersects(sibling->geometry()),qPrintable(QString("%1: selection %2,%3 %4x%5; sibling %6,%7 %8x%9").arg(sibling->metaObject()->className()).arg(selectionWidget->x()).arg(selectionWidget->y()).arg(selectionWidget->width()).arg(selectionWidget->height()).arg(sibling->x()).arg(sibling->y()).arg(sibling->width()).arg(sibling->height())));
    }
    const auto output=qEnvironmentVariable("UCC_SCREENSHOT_DIR");
    if(!output.isEmpty()) {
      QTest::qWait(450);auto* selection=window.findChild<ucc::CpuCoreSelectionWidget*>();QVERIFY(selection);
      for(auto* scroll:window.findChildren<QScrollArea*>()) if(scroll->widget() && scroll->widget()->isAncestorOf(selection)) scroll->ensureWidgetVisible(selection);
      QTest::qWait(100);QDir().mkpath(output);QVERIFY(window.grab().save(output+"/cpu-selection.png"));
    }
#ifndef UCC_READ_ONLY_PREVIEW
    mode->setCurrentIndex(0);window.findChild<QPushButton*>("saveProfile")->click();
    QTRY_VERIFY(selectionWidget->height()<150);
    saved=QJsonDocument::fromJson(control("TestLastSavedProfile").toString().toUtf8()).object()["cpu"].toObject();
    QVERIFY(!saved.contains("enabledCores"));
#else
    QVERIFY(!mode->isEnabled());
#endif
  }
void GuiAudit::profilesUseOneCompactSelector() {
    control("TestVisualFixture");
    ucc::MainWindow window;window.resize(1024,768);window.show();
    auto *navigation=window.findChild<QListWidget*>("navigation");QVERIFY(navigation);
    navigation->setCurrentRow(1);QTest::qWait(350);
    auto *combo=window.findChild<QComboBox*>("profileName");QVERIFY(combo);
    QVERIFY(!window.findChild<QListWidget*>("profileList"));
    QCOMPARE(combo->count(),3);QVERIFY(combo->isVisible());
    QPushButton *apply=nullptr;
    for(auto *button:window.findChild<QTabWidget*>("pages")->widget(1)->findChildren<QPushButton*>())
      if(button->text()=="Apply") apply=button;
    QVERIFY(apply);
    QCOMPARE(combo->mapTo(&window,QPoint()).y(),apply->mapTo(&window,QPoint()).y());
    QTest::mouseClick(combo,Qt::LeftButton,Qt::NoModifier,QPoint(combo->width()-12,combo->height()/2));
    QTRY_VERIFY(combo->view()->isVisible());
    // Qt suppresses release just after opening until the pointer moves away
    // from the arrow (or its double-click timer expires).
    QTest::mouseMove(combo->view()->viewport(),combo->view()->visualRect(combo->model()->index(0,0)).center());
    QTest::mouseClick(combo->view()->viewport(),Qt::LeftButton,Qt::NoModifier,
      combo->view()->visualRect(combo->model()->index(0,0)).center());
    QCOMPARE(combo->currentData().toString(),QString("quiet"));
    QCOMPARE(window.findChild<QTextEdit*>()->toPlainText(),QString("Low noise"));
    combo->setCurrentIndex(2);QCOMPARE(combo->currentData().toString(),QString("performance"));
    QVERIFY(stats().value("writes").toArray().isEmpty());
  }
void GuiAudit::ctgpSliderGuardsUnavailableAndInvertedBounds() {
    control("TestVisualFixture");
    control("TestCustomProfile",{QString(R"({"id":"ctgp-edge","name":"cTGP Edge","nvidiaCTGPOffset":20,"fan":{"fanProfile":"balanced-cooling"}})")});
    for(const auto& [sup,avail,defW,maxW] : std::array<std::tuple<bool,bool,int,int>,3>{{
        {true,false,0,0},
        {true,true,80,0},
        {true,true,120,80},
    }}) {
      control("TestCtgp",{sup,avail,defW,maxW});
      ucc::MainWindow window;window.resize(1280,900);window.show();
      auto* nav=window.findChild<QListWidget*>("navigation");QVERIFY(nav);
      nav->setCurrentRow(1);
      auto* names=window.findChild<QComboBox*>("profileName");QVERIFY(names);
      names->setCurrentIndex(names->findData("ctgp-edge"));
      auto* widget=window.findChild<QWidget*>("ctgpWidget");
      auto* slider=window.findChild<QSlider*>("ctgpSlider");
      QVERIFY(widget);QVERIFY(slider);
      QVERIFY(!widget->isVisible());
      QVERIFY(!slider->isEnabled());
    }
    control("TestCtgp",{true,true,80,175});
    ucc::MainWindow window;window.resize(1280,900);window.show();
    window.findChild<QListWidget*>("navigation")->setCurrentRow(1);
    auto* names=window.findChild<QComboBox*>("profileName");
    names->setCurrentIndex(names->findData("ctgp-edge"));
    auto* widget=window.findChild<QWidget*>("ctgpWidget");
    auto* slider=window.findChild<QSlider*>("ctgpSlider");
    QVERIFY(widget->isVisibleTo(&window));
    QVERIFY(slider->isEnabled());
    QCOMPARE(slider->minimum(),80);
    QCOMPARE(slider->maximum(),175);
    QCOMPARE(slider->value(),100);
  }
void GuiAudit::displayBrightnessCommitsOnReleaseAndIgnoresMonitorDuringDrag() {
#ifdef UCC_READ_ONLY_PREVIEW
    QSKIP("Hardware writes are disabled in preview");
#else
    control("TestVisualFixture");
    ucc::MainWindow window; window.resize(1280,900); window.show();
    auto *slider = window.findChild<QSlider*>("displayBrightnessSlider"); QVERIFY(slider);
    auto *monitor = window.findChild<ucc::SystemMonitor*>(); QVERIFY(monitor);
    QTRY_VERIFY(monitor->displayBrightness() >= 0);
    QVERIFY(!slider->hasTracking());
    QCOMPARE(stats()["writes"].toArray().size(),0);
    slider->setSliderDown(true);
    for (int value = 30; value <= 80; ++value) slider->setSliderPosition(value);
    monitor->displayBrightnessChanged();
    QCOMPARE(slider->sliderPosition(),80);
    QCOMPARE(stats()["writes"].toArray().size(),0);
    slider->setSliderDown(false);
    QTRY_COMPARE(stats()["writes"].toArray(),QJsonArray({"SetDisplayBrightness"}));
    QTRY_COMPARE(monitor->displayBrightness(),80);
    window.hide();
#endif
  }
void GuiAudit::profilePayloadPreservesManualWaterCoolerValues() {
#ifdef UCC_READ_ONLY_PREVIEW
    QSKIP("Profile writes are disabled in preview");
#endif
    control("TestVisualFixture");
    control("TestRuntimeProfile",{QString(R"({"id":"balanced","name":"Balanced","fan":{"autoControlWC":false,"manualFanSpeed":70,"manualPumpVoltage":3}})")});
    ucc::MainWindow window;
    QVERIFY(QMetaObject::invokeMethod(&window,"onApplyClicked"));
    auto payload=QJsonDocument::fromJson(control("TestLastAppliedProfile").toString().toUtf8()).object()["fan"].toObject();
    QCOMPARE(payload["manualFanSpeed"].toInt(),70);QCOMPARE(payload["manualPumpVoltage"].toInt(),3);
    QVERIFY(!payload["autoControlWC"].toBool(true));
  }
void GuiAudit::fanControlPumpComboFindsDiscreteIntCodes() {
    control("TestBuiltinFanProfiles",{QString(R"([
      {"id":"fan-silent","name":"Silent [Built-in]"},
      {"id":"fan-balanced","name":"Balanced [Built-in]"},
      {"id":"fan-freezy","name":"Freezy [Built-in]"}
    ])")});
    control("TestCustomFanProfiles",{QString(R"([
      {"id":"air-only","name":"Air Only","json":{"tableCPU":[{"temp":40,"speed":30}],"tableGPU":[{"temp":40,"speed":30}]}},
      {"id":"wc-custom","name":"WC Custom","json":{"tableCPU":[{"temp":40,"speed":40}],"tableWaterCoolerFan":[{"temp":40,"speed":50}],"tablePump":[{"temp":40,"speed":2}]}}
    ])")});
    ucc::ProfileManager localProfiles(client.get());
    ucc::FanControlTab tab(client.get(),&localProfiles,true);
    auto *combo=pump(tab);
    QVERIFY(combo);
    QCOMPARE(combo->itemData(0).typeId(), static_cast<int>(QMetaType::Int));
    QVERIFY2(combo->findData(0)>=0,"Pump combo must store int 0 (Off) so findData(int) matches D-Bus codes");
    QVERIFY2(combo->findData(2)>=0,"Pump combo must store int 2 (8V) / discrete codes");

    // In normal Cooler Settings mode, air-only & all built-ins are shown; in Watercool mode ONLY 1 default + WC custom profiles.
    auto *fp=tab.fanProfileCombo();QVERIFY(fp);
    QVERIFY(fp->findData("air-only")>=0);
    QVERIFY(fp->findData("wc-custom")>=0);
    tab.setWaterCoolerMode(true);
    QCOMPARE(fp->count(),2);
    QVERIFY(fp->findData("fan-balanced")>=0);
    QVERIFY(fp->findData("wc-custom")>=0);
    QVERIFY(fp->findData("air-only")<0);
    tab.setWaterCoolerMode(false);
    QVERIFY(fp->findData("air-only")>=0);
  }
void GuiAudit::batterySettingsRaisesWindowSetsEndBeforeStart() {
#ifdef UCC_READ_ONLY_PREVIEW
    QSKIP("Only applies to writable build");
#endif
    control("TestChargeWindow");
    ucc::BatterySettingsWidget widget(client.get());
    auto *limit=widget.findChild<QComboBox*>("globalChargeLimit");QVERIFY(limit);
    QTRY_COMPARE(limit->currentData().toString(),QString("stationary"));
    limit->setCurrentIndex(limit->findData("reduced"));
    widget.findChild<QPushButton*>("saveBatterySettings")->click();
    const auto wr=stats()["writes"].toArray();
    int idxStart=-1,idxEnd=-1;
    for(int i=0;i<wr.size();++i) {
      if(wr[i].toString()=="SetChargeStartThreshold") idxStart=i;
      if(wr[i].toString()=="SetChargeEndThreshold") idxEnd=i;
    }
    QVERIFY(idxStart>=0 && idxEnd>=0);
    QVERIFY2(idxEnd<idxStart,"Raising charge window 40-80 -> 60-90 must raise SetChargeEndThreshold BEFORE SetChargeStartThreshold");
  }
void GuiAudit::applyFailureDoesNotClaimSuccessOrSendWaterCooler() {
#ifdef UCC_READ_ONLY_PREVIEW
    QSKIP("Only applies to writable build");
#endif
    control("TestVisualFixture");
    ucc::MainWindow window;window.resize(1280,900);window.show();
    auto *nav=window.findChild<QListWidget*>("navigation");nav->setCurrentRow(1);
    auto *names=window.findChild<QComboBox*>("profileName");QVERIFY(names && names->count()>0);
    control("TestFailControl",{true});
    const auto beforeWrites=stats()["writes"].toArray().size();
    QVERIFY(QMetaObject::invokeMethod(&window,"onApplyClicked",Qt::DirectConnection));
    QTRY_VERIFY(window.statusBar()->currentMessage().contains("Failed to apply"));
    QVERIFY(!window.statusBar()->currentMessage().contains("Profile applied"));
    const auto afterWrites=stats()["writes"].toArray();
    QCOMPARE(afterWrites.size(),beforeWrites);
    QVERIFY(!afterWrites.contains("EnableWaterCooler"));
  }
void GuiAudit::asyncApplyAllowsImmediateWindowCloseWithoutLateWaterCooler() {
#ifdef UCC_READ_ONLY_PREVIEW
    QSKIP("Only applies to writable build");
#endif
    control("TestVisualFixture");
    ucc::MainWindow window;window.resize(1280,900);window.show();
    auto *nav=window.findChild<QListWidget*>("navigation");nav->setCurrentRow(1);
    auto *names=window.findChild<QComboBox*>("profileName");QVERIFY(names && names->count()>0);
    control("TestDelayApply",{800});
    QElapsedTimer timer;timer.start();
    QVERIFY(QMetaObject::invokeMethod(&window,"onApplyClicked",Qt::DirectConnection));
    QTest::qWait(40);
    const qint64 t0=timer.elapsed();
    window.close();
    QCoreApplication::processEvents();
    QVERIFY2(timer.elapsed()-t0<250,"closeEvent must not block waiting on slow ApplyProfile");
    QTest::qWait(900);
    QVERIFY2(!stats()["writes"].toArray().contains("EnableWaterCooler"),"Closed MainWindow must not dispatch late EnableWaterCooler after ApplyProfile finishes");
  }
void GuiAudit::rejectedProfileSelectionKeepsPreviousActiveProfile() {
#ifdef UCC_READ_ONLY_PREVIEW
    QSKIP("Only applies to writable build");
#endif
    control("TestVisualFixture");
    profiles->refresh();
    QString origActive = profiles->activeProfileId();
    QVERIFY(!origActive.isEmpty());
    QString otherId;
    for (const auto &v : profiles->defaultProfilesData()) {
      const QString cand = v.toObject()["id"].toString();
      if (!cand.isEmpty() && cand != origActive) { otherId = cand; break; }
    }
    QVERIFY(!otherId.isEmpty());

    control("TestFailControl", {true});
    QSignalSpy activeSpy(profiles.get(), &ucc::ProfileManager::activeProfileChanged);
    QSignalSpy errorSpy(profiles.get(), &ucc::ProfileManager::error);
    profiles->setActiveProfile(otherId);
    QTRY_VERIFY(!errorSpy.isEmpty());
    QCOMPARE(profiles->activeProfileId(), origActive);
    QCOMPARE(activeSpy.count(), 0);
  }
void GuiAudit::slowSetActiveProfileDoesNotBlockEventLoop() {
#ifdef UCC_READ_ONLY_PREVIEW
    QSKIP("Only applies to writable build");
#endif
    control("TestVisualFixture");
    profiles->refresh();
    QString origActive = profiles->activeProfileId();
    QString otherId;
    for (const auto &v : profiles->defaultProfilesData()) {
      const QString cand = v.toObject()["id"].toString();
      if (!cand.isEmpty() && cand != origActive) { otherId = cand; break; }
    }
    QVERIFY(!otherId.isEmpty());

    control("TestDelayApply", {350});
    int timerTicks = 0;
    QTimer tickTimer;
    QObject::connect(&tickTimer, &QTimer::timeout, &tickTimer, [&timerTicks] { ++timerTicks; });
    tickTimer.start(25);

    QSignalSpy activeSpy(profiles.get(), &ucc::ProfileManager::activeProfileChanged);
    QElapsedTimer elapsed;
    elapsed.start();
    profiles->setActiveProfile(otherId);
    QVERIFY2(elapsed.elapsed() < 150, "setActiveProfile must return immediately instead of blocking Qt event loop");
    QTRY_VERIFY_WITH_TIMEOUT(timerTicks >= 4, 300);
    QTRY_COMPARE_WITH_TIMEOUT(profiles->activeProfileId(), otherId, 1500);
    QCOMPARE(activeSpy.count(), 1);
  }
void GuiAudit::prepareForSleepResumeSetsSidebarStatus() {
    control("TestVisualFixture");
    ucc::MainWindow window;
    QVERIFY(QMetaObject::invokeMethod(&window,"onPrepareForSleep",Qt::DirectConnection,Q_ARG(bool,false)));
    QVERIFY(window.statusBar()->currentMessage().contains("Resuming from sleep"));
    auto *sidebarText = window.findChild<QLabel*>("sidebarStatusText");
    auto *sidebarDot = window.findChild<QLabel*>("sidebarStatusIndicator");
    QVERIFY(sidebarText && sidebarDot);
    // Simulate daemon drop during resume: sidebar must show resuming banner rather than "Service unavailable".
    QVERIFY(QMetaObject::invokeMethod(&window, "updateSidebarConnectionStatus", Qt::DirectConnection, Q_ARG(bool, false)));
    QCOMPARE(sidebarText->text(), QString("Resuming from sleep…"));
    QCOMPARE(sidebarDot->property("status").toString(), QString("warning"));
    // Once daemon reconnects, sidebar immediately restores "Service connected".
    QVERIFY(QMetaObject::invokeMethod(&window, "updateSidebarConnectionStatus", Qt::DirectConnection, Q_ARG(bool, true)));
    QCOMPARE(sidebarText->text(), QString("Service connected"));
    QCOMPARE(sidebarDot->property("status").toString(), QString("success"));
  }
void GuiAudit::deviceSupportStatusDistinguishesOfflineFromUnsupported() {
    QString detail;
    control("TestDeviceSupported",{false});
    QCOMPARE(client->checkDeviceSupport(&detail),ucc::DeviceSupportStatus::Unsupported);
    control("TestDeviceSupported",{true});
    QCOMPARE(client->checkDeviceSupport(&detail),ucc::DeviceSupportStatus::Supported);
    daemon.terminate();
    QVERIFY(daemon.waitForFinished());
    QTRY_COMPARE(client->checkDeviceSupport(&detail),ucc::DeviceSupportStatus::DaemonUnavailable);
  }
void GuiAudit::watercoolCrosshairsStayVisibleAcrossTabsAndPartialReads() {
    control("TestVisualFixture");
    ucc::MainWindow window;window.resize(1280,900);window.show();
    auto *navigation=window.findChild<QListWidget*>("navigation");QVERIFY(navigation);
    navigation->setCurrentRow(3);
    auto *wcFan=window.findChild<ucc::FanControlTab*>()->wcFanEditor();
    auto *pump=window.findChild<ucc::FanControlTab*>()->pumpEditor();
    QVERIFY(wcFan && pump);
    QTRY_VERIFY_WITH_TIMEOUT(wcFan->crosshairVisible() && pump->crosshairVisible(), 3500);

    // Switching Cooler Settings <-> Watercool Settings and transient read failures must not erase last valid crosshairs.
    control("TestFailReads",{true});
    navigation->setCurrentRow(2);QTest::qWait(80);
    navigation->setCurrentRow(3);QTest::qWait(500);
    QVERIFY(wcFan->crosshairVisible());
    QVERIFY(pump->crosshairVisible());
    window.hide();
    QVERIFY(stats().value("writes").toArray().isEmpty());
  }
