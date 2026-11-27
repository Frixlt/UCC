#include "GuiAudit.hpp"

void GuiAudit::aboutLoggingPreferenceAndFolderAction() {
    control("TestVisualFixture");ucc::MainWindow window;window.resize(1024,768);window.show();
    auto *aboutBtn=window.findChild<QPushButton*>("aboutNavigation");QVERIFY(aboutBtn);
    QTest::mouseClick(aboutBtn,Qt::LeftButton);
    auto *page=window.findChild<QScrollArea*>("aboutPage");QVERIFY(page);page->verticalScrollBar()->setValue(page->verticalScrollBar()->maximum());QTest::qWait(300);
    auto *toggle=page->findChild<QCheckBox*>("saveApplicationLogs");auto *open=page->findChild<QPushButton*>("openLogsDirectory");QVERIFY(toggle);QVERIFY(open);
    QCOMPARE(toggle->parentWidget()->objectName(),QString("card"));
    QCOMPARE(page->findChild<QLabel*>("logsDirectoryPath")->font().pixelSize(),12);
    QCOMPARE(page->findChild<QLabel*>("logsDirectoryPath")->text(),ucc::AppLogging::directory());
    QCOMPARE(page->horizontalScrollBar()->maximum(),0);
#ifdef UCC_READ_ONLY_PREVIEW
    QVERIFY(!toggle->isEnabled());QVERIFY(!open->isEnabled());
    QVERIFY(QMetaObject::invokeMethod(open,"clicked",Q_ARG(bool,false)));
    QVERIFY(QMetaObject::invokeMethod(toggle,"toggled",Q_ARG(bool,true)));
    QVERIFY(!ucc::AppLogging::isEnabled());QVERIFY(!QDir(ucc::AppLogging::directory()).exists());
#else
    QVERIFY(toggle->isEnabled());QTest::mouseClick(toggle,Qt::LeftButton);QVERIFY(ucc::AppLogging::isEnabled());
    QVERIFY(QSettings("UniwillControlCenter","diagnostics").value("saveLogs").toBool());
    QTest::mouseClick(toggle,Qt::LeftButton);QVERIFY(!ucc::AppLogging::isEnabled());
    QDesktopServices::setUrlHandler("file",this,"captureLogDirectory");
    const auto restore=qScopeGuard([] {QDesktopServices::unsetUrlHandler("file");});
    QTest::mouseClick(open,Qt::LeftButton);QCOMPARE(openedLogDirectory,QUrl::fromLocalFile(ucc::AppLogging::directory()));
#endif
    window.hide();QVERIFY(stats().value("writes").toArray().isEmpty());
  }
void GuiAudit::aboutBuildCreditsAndBundledLicense() {
    control("TestVisualFixture");ucc::MainWindow window;window.resize(1024,768);window.show();
    auto *aboutBtn=window.findChild<QPushButton*>("aboutNavigation");QVERIFY(aboutBtn);
    QCOMPARE(aboutBtn->text(),QString("About"));QTest::mouseClick(aboutBtn,Qt::LeftButton);QTest::qWait(300);
    auto *page=window.findChild<QScrollArea*>("aboutPage");QVERIFY(page);
    QCOMPARE(window.findChild<QLabel*>("aboutMark")->text(),QString("ucc-rs"));
    QCOMPARE(window.findChild<QLabel*>("aboutVersion")->text(),QString(UCC_VERSION));
    QCOMPARE(window.findChild<QLabel*>("aboutCommit")->text(),QString(UCC_RELEASE).isEmpty() ? QString("Source archive") : QString(UCC_RELEASE));
    QCOMPARE(window.findChild<QLabel*>("aboutQtVersion")->text(),QString(qVersion()));
    QCOMPARE(window.findChild<QLabel*>("aboutArchitecture")->text(),QString("Rust daemon · Qt 6"));
    QStringList urls;for(auto *button:page->findChildren<QPushButton*>()) urls<<button->property("sourceUrl").toString();
    QVERIFY(urls.contains("https://www.rust-lang.org/"));
    QVERIFY(urls.contains("https://github.com/nanomatters/ucc"));
    QVERIFY(urls.contains("https://github.com/tuxedocomputers/tuxedo-control-center"));
    QVERIFY(urls.contains("https://github.com/tuxedocomputers/tuxedo-drivers"));
    QCOMPARE(page->horizontalScrollBar()->maximum(),0);
    bool licenseLoaded=false;
    QTimer::singleShot(0,&window,[&] {
      auto *dialog=window.findChild<QDialog*>("aboutLicenseDialog");
      if(dialog) {licenseLoaded=dialog->findChild<QTextBrowser*>("aboutLicenseText")->toPlainText().contains("GNU GENERAL PUBLIC LICENSE");dialog->reject();}
    });
    QTest::mouseClick(page->findChild<QPushButton*>("aboutLicense"),Qt::LeftButton);QVERIFY(licenseLoaded);
    bool noticesLoaded=false;
    QTimer::singleShot(0,&window,[&] {
      auto *dialog=window.findChild<QDialog*>("aboutLicenseDialog");
      if(dialog) {noticesLoaded=dialog->findChild<QTextBrowser*>("aboutLicenseText")->toPlainText().contains("2019–2022 TUXEDO");dialog->reject();}
    });
    QTest::mouseClick(page->findChild<QPushButton*>("aboutNotices"),Qt::LeftButton);QVERIFY(noticesLoaded);
    window.hide();QVERIFY(stats().value("writes").toArray().isEmpty());
  }
void GuiAudit::notificationHistoryAndReadControls() {
    control("TestVisualFixture");
    ucc::MainWindow window;window.show();
    auto *center=window.findChild<ucc::NotificationCenter*>();QVERIFY(center);
    auto *bell=window.findChild<QPushButton*>("notificationBell");QVERIFY(bell);
    center->clearAll();
    auto *navigation=window.findChild<QListWidget*>("navigation");navigation->setCurrentRow(3);
    window.statusBar()->showMessage("Water cooler fan set to 70%");
    QCOMPARE(center->eventCount(),1);QCOMPARE(center->unreadCount(),1);
    center->addEvent("Profiles","Profile saved",QDateTime(QDate::currentDate(),QTime(12,34,56)));
    QCOMPARE(bell->property("unreadCount").toInt(),2);
    QTest::mouseClick(bell,Qt::LeftButton);QVERIFY(center->isVisible());QCOMPARE(center->unreadCount(),2);
    auto *list=center->findChild<QListWidget*>("notificationList");QCOMPARE(list->count(),2);
    auto *card=list->itemWidget(list->item(0));QVERIFY(card);
    QVERIFY(card->findChild<QLabel*>("notificationTitle")->text().contains("Profiles"));
    bool hasTime=false;for(auto *label:card->findChildren<QLabel*>()) hasTime |= label->text().contains("12:34:56");QVERIFY(hasTime);
    QVERIFY(list->item(0)->sizeHint().height()>70);
    QTest::mouseClick(center->findChild<QPushButton*>("notificationsReadAll"),Qt::LeftButton);
    QCOMPARE(center->eventCount(),2);QCOMPARE(center->unreadCount(),0);
    center->addEvent("Watercool Settings","Pump set to 8 V");
    QTest::mouseClick(list->viewport(),Qt::LeftButton,Qt::NoModifier,list->visualItemRect(list->item(0)).center());
    QCOMPARE(center->unreadCount(),0);
    QTest::mouseClick(center->findChild<QPushButton*>("notificationsClearAll"),Qt::LeftButton);
    QCOMPARE(center->eventCount(),0);QCOMPARE(bell->property("unreadCount").toInt(),0);
    center->addEvent("UCC","Ready");center->addEvent("UCC","");QCOMPARE(center->eventCount(),0);
    for(int i=0;i<205;++i) center->addEvent("Profiles",QString::number(i));
    QCOMPARE(center->eventCount(),200);center->hide();window.hide();
    QVERIFY(stats().value("writes").toArray().isEmpty());
  }
void GuiAudit::renderReleaseGallery() {
    const QString output=qEnvironmentVariable("UCC_SCREENSHOT_DIR");
    if(output.isEmpty()) QSKIP("Set UCC_SCREENSHOT_DIR to capture the release gallery");
    QVERIFY(QDir().mkpath(output));
    ucc::FluentTheme::setMode(qEnvironmentVariable("UCC_AUDIT_THEME")=="dark" ? ucc::FluentTheme::Mode::Dark : ucc::FluentTheme::Mode::Light,false);
    control("TestVisualFixture");
    ucc::MainWindow window;window.resize(1440,1000);window.show();
    auto *navigation=window.findChild<QListWidget*>("navigation");QVERIFY(navigation);
    auto *aboutBtn=window.findChild<QPushButton*>("aboutNavigation");QVERIFY(aboutBtn);
    auto *processes=window.findChild<ucc::SessionProcessesWidget*>();QVERIFY(processes);
    const QStringList names={"overview","profiles","cooler-settings","watercool-settings","monitor","lighting","power-devices","battery-processes","experimental","about"};
    QCOMPARE(navigation->count(),9);
    for(int page=0;page<names.size();++page) {
      if(page<9) navigation->setCurrentRow(page); else QTest::mouseClick(aboutBtn,Qt::LeftButton);
      QTest::qWait(page==0 || page==4 ? 2400 : 450);
      if(page==7) {
        processes->monitor()->setActive(false);QTRY_VERIFY(!processes->monitor()->isBusy());
        processes->setSnapshot({{"batteries",QJsonArray{QJsonObject{{"name","BAT0"},{"capacity",76},{"status","Discharging"},{"powerWatts",12.5}}}},
          {"gpuState","active"},{"processesSampled",true},{"processes",QJsonArray{QJsonObject{{"pid",424242},{"startTime","123456"},{"name","Example GPU application"}}}}});
      }
      QVERIFY(window.grab().save(output+"/"+names[page]+".png"));
    }
    navigation->setCurrentRow(0);
    control("TestDriverHealth",{QString::fromUtf8(R"({"healthy":false,"message":"Проблема с драйверами · демонстрация\nНе загружен аппаратный драйвер TUXEDO. Проверьте сборку DKMS для текущего ядра."})")});
    auto *dashboard=window.findChild<ucc::DashboardTab*>();QVERIFY(dashboard);
    dashboard->refreshDriverHealth();
    auto *banner=window.findChild<QLabel*>("driverHealthBanner");QVERIFY(banner);
    QTRY_VERIFY(banner->isVisible());QTest::qWait(350);
    QVERIFY(window.grab().save(output+"/driver-warning.png"));
    window.hide();
    QVERIFY2(stats()["writes"].toArray().isEmpty(),"Gallery capture must never write hardware or settings");
  }
void GuiAudit::renderReferencePages() {
    ucc::FluentTheme::setMode(qEnvironmentVariable("UCC_AUDIT_THEME")=="dark" ? ucc::FluentTheme::Mode::Dark : ucc::FluentTheme::Mode::Light,false);
    control("TestVisualFixture");
    ucc::MainWindow window;
    window.resize(1280,900);window.show();QTest::qWait(2400);
    auto *navigation=window.findChild<QListWidget*>("navigation");QVERIFY(navigation);
    auto *aboutBtn=window.findChild<QPushButton*>("aboutNavigation");QVERIFY(aboutBtn);
    auto *profilesCombo=window.findChild<QComboBox*>("profileName");QVERIFY(profilesCombo);QCOMPARE(profilesCombo->count(),3);
    auto *keyboard=window.findChild<ucc::KeyboardVisualizerWidget*>();QVERIFY(keyboard);
    QVERIFY(window.findChild<ucc::FanControlTab*>()->isWaterCoolerEnabled());
    bool hasConnectedStatus=false;
    for(auto *label:window.findChildren<QLabel*>()) hasConnectedStatus |= label->text()=="Connected";
    QVERIFY2(hasConnectedStatus,"Overview must reflect the current cooler connection, not its initial placeholder");
    const QString output=qEnvironmentVariable("UCC_SCREENSHOT_DIR");
    if(!output.isEmpty()) QVERIFY(QDir().mkpath(output));
    const QList<QPair<int,QString>> refPages={{0,"overview"},{1,"profiles"},{2,"cooler-settings"},{3,"watercool-settings"},{4,"monitor"},{5,"lighting"},{9,"about"}};
    for(const auto &[row,name]:refPages){if(row<9) navigation->setCurrentRow(row);else QTest::mouseClick(aboutBtn,Qt::LeftButton);QTest::qWait(row==3 ? 2400 : 400);if(row==2 || row==3){auto *cooling=window.findChild<QTabWidget *>("coolingPages");QVERIFY(cooling);QCOMPARE(cooling->currentIndex(),row-2);QVERIFY(!cooling->tabBar()->isVisible());}if(!output.isEmpty()) QVERIFY(window.grab().save(output+"/"+name+".png"));}
    if(!output.isEmpty()) {
      QTest::mouseClick(aboutBtn,Qt::LeftButton);QTest::qWait(350);
      auto *about=window.findChild<QScrollArea*>("aboutPage");QVERIFY(about);
      about->verticalScrollBar()->setValue(about->verticalScrollBar()->maximum());
      QTest::qWait(80);QVERIFY(window.grab().save(output+"/about-bottom.png"));
      about->verticalScrollBar()->setValue(0);
      QTimer::singleShot(100,&window,[&] {
        auto *dialog=window.findChild<QDialog*>("aboutLicenseDialog");
        if(dialog) {dialog->grab().save(output+"/about-license.png");dialog->reject();}
      });
      QTest::mouseClick(about->findChild<QPushButton*>("aboutLicense"),Qt::LeftButton);
      navigation->setCurrentRow(1);QTest::qWait(350);
      for(auto *scroll:window.findChild<QTabWidget*>("pages")->widget(1)->findChildren<QScrollArea*>()) scroll->verticalScrollBar()->setValue(scroll->verticalScrollBar()->maximum());
      QTest::qWait(80);QVERIFY(window.grab().save(output+"/profiles-bottom.png"));
      profilesCombo->showPopup();QTest::qWait(80);QVERIFY(profilesCombo->view()->window()->grab().save(output+"/profiles-popup.png"));profilesCombo->hidePopup();
      navigation->setCurrentRow(0);QTest::qWait(350);
      auto *fan=window.findChild<QComboBox*>("waterCoolerFanSelector");QVERIFY(fan);fan->showPopup();QTest::qWait(80);QVERIFY(fan->view()->window()->grab().save(output+"/fan-popup.png"));fan->hidePopup();
      navigation->setCurrentRow(4);QTest::qWait(350);
      auto *monitor=window.findChild<ucc::MonitorTab*>();QVERIFY(monitor);
      for(auto *toggle:monitor->findChildren<QCheckBox*>()) if(toggle->text()=="Unified Graph") toggle->setChecked(true);
      QTest::qWait(100);
      for(auto *view:monitor->findChildren<QChartView*>()) if(view->isVisible()) {
        const auto target=view->mapFromScene(view->chart()->mapToScene(view->chart()->plotArea().center()));
        QTest::mouseClick(view->viewport(),Qt::LeftButton,Qt::NoModifier,target);
        QTest::keyPress(monitor,Qt::Key_Control);QTest::mouseMove(view->viewport(),target+QPoint(12,0));QTest::qWait(100);
        break;
      }
      QVERIFY(window.grab().save(output+"/monitor-overlays.png"));QTest::keyRelease(monitor,Qt::Key_Control);
      navigation->setCurrentRow(5);QTest::qWait(350);
      auto keys=keyboard->findChildren<QPushButton*>();QVERIFY(!keys.isEmpty());QTest::mouseClick(keys.first(),Qt::LeftButton,Qt::ControlModifier);
      QVERIFY(window.grab().save(output+"/keyboard-selected.png"));
      QColorDialog dialog(QColor("#EF00D8"),&window);dialog.setOption(QColorDialog::DontUseNativeDialog,true);dialog.show();QTest::qWait(100);
      QVERIFY(dialog.grab().save(output+"/color-dialog.png"));dialog.reject();
    }
    if(!output.isEmpty()) {
      navigation->setCurrentRow(0);QTest::qWait(350);
      auto *center=window.findChild<ucc::NotificationCenter*>();QVERIFY(center);center->clearAll();
      center->addEvent("Keyboard and Hardware","Keyboard profile 'Main' saved");
      center->addEvent("Profiles","Profile 'Custom' saved");
      center->addEvent("Watercool Settings","Water cooler pump set to 8 V");
      center->addEvent("Overview","Water cooler fan set to 70%");
      QTest::mouseClick(window.findChild<QPushButton*>("notificationBell"),Qt::LeftButton);QTest::qWait(100);
      QVERIFY(center->grab().save(output+"/notifications.png"));
      QVERIFY(window.grab().save(output+"/notifications-context.png"));center->hide();
    }
    window.resize(1024,768);
    for(const auto &[row,name]:refPages){if(row<9) navigation->setCurrentRow(row);else QTest::mouseClick(aboutBtn,Qt::LeftButton);QTest::qWait(350);QVERIFY(window.width()<=1024);if(!output.isEmpty()) QVERIFY(window.grab().save(output+"/"+name+"-small.png"));}
    window.hide();
    QVERIFY2(stats().value("writes").toArray().isEmpty(),"Rendering pages must not change hardware or profile settings");
  }
void GuiAudit::keyboardSelectionDoesNotChangeBacklight() {
    ucc::KeyboardVisualizerWidget keyboard(128);keyboard.resize(1000,340);keyboard.show();
    keyboard.setGlobalColor(QColor("#EF00D8"));
    QSignalSpy changes(&keyboard,&ucc::KeyboardVisualizerWidget::colorsChanged);
    const auto before=keyboard.getJSONState();
    auto keys=keyboard.findChildren<QPushButton*>();QVERIFY(keys.size()>2);
    QTest::mouseClick(keys[0],Qt::LeftButton,Qt::ControlModifier);
    QTest::mouseClick(keys[1],Qt::LeftButton,Qt::ControlModifier);
    QCOMPARE(keyboard.getJSONState(),before);QCOMPARE(changes.count(),0);
    QVERIFY(QMetaObject::invokeMethod(&keyboard,"onColorChanged",Q_ARG(QColor,QColor("#224466"))));
    const auto after=keyboard.getJSONState();int changed=0;
    for(int i=0;i<after.size();++i) if(before[i]!=after[i]) ++changed;
    QCOMPARE(changed,2);
  }
void GuiAudit::previewMustNotSaveLocalMonitorPreferences() {
#ifndef UCC_READ_ONLY_PREVIEW
    QSKIP("Only applies to preview build");
#endif
    const auto file=QDir::homePath()+"/.config/uccrc";
    QFile::remove(file);
    ucc::MonitorTab tab(client.get());
    auto toggles=tab.findChildren<QCheckBox*>();
    QVERIFY(!toggles.isEmpty());
    toggles.first()->setChecked(!toggles.first()->isChecked());
    QVERIFY2(!QFile::exists(file),"Preview must not write local settings either");
  }
void GuiAudit::previewMustBlockEveryWritePath() {
#ifndef UCC_READ_ONLY_PREVIEW
    QSKIP("Only applies to the read-only prototype build");
#endif
    client->setDisplayBrightness(40);
    client->applyProfile("{}");
    client->saveCustomProfile("{}");
    client->deleteCustomProfile("preview");
    client->setManualPowerMode("bat");
    client->setPowerDeviceSettingsJSON("{}");
    client->setLightbarSettingsJSON("{}");
    client->triggerLightbarAnimation(true);
    client->triggerLightbarAnimation(false);
    client->scanPowerDevices();
    client->registerPowerRequest("test","Preview guard","ac");
    client->unregisterPowerRequest("test");
    client->setWaterCoolerFanSpeed(40);
    client->setWaterCoolerPumpVoltage(1);
    client->setWaterCoolerLEDColor(1,2,3,0);
    client->turnOffWaterCoolerLED();
    client->setKeyboardBacklight("{}");
    ucc::FanControlTab tab(client.get(),profiles.get(),true);
    tab.sendWaterCoolerEnable(true);
    QVERIFY(QMetaObject::invokeMethod(&tab,"onFanSpeedChanged",Q_ARG(int,40)));
    QVERIFY(QMetaObject::invokeMethod(&tab,"onPumpVoltageChanged",Q_ARG(int,0)));
    QVERIFY2(stats().value("writes").toArray().isEmpty(), "Read-only prototype must never dispatch hardware or settings writes");
    QCOMPARE(client->getWaterCoolerFanSpeed().value_or(-1),100);
    QVERIFY(stats().value("reads").toObject().value("GetWaterCoolerFanSpeed").toInt()>0);
  }
void GuiAudit::aboutTabStaysPinnedToSidebarBottom() {
    control("TestVisualFixture");
    ucc::MainWindow window;
    window.resize(1024,768);
    window.show();
    QTest::qWait(50);
    auto *navigation=window.findChild<QListWidget*>("navigation");
    auto *aboutBtn=window.findChild<QPushButton*>("aboutNavigation");
    auto *footer=window.findChild<QWidget*>("sidebarFooter");
    auto *pages=window.findChild<QTabWidget*>("pages");
    QVERIFY(navigation && aboutBtn && footer && pages);
    QCOMPARE(navigation->count(),9);

    const int ySmall=aboutBtn->mapTo(&window,QPoint(0,0)).y();
    const int navBottomSmall=navigation->mapTo(&window,QPoint(0,navigation->height())).y();
    const int footerTopSmall=footer->mapTo(&window,QPoint(0,0)).y();
    QVERIFY2(ySmall>=navBottomSmall,"About button must sit below scrollable navigation list");
    QVERIFY2(footerTopSmall-(ySmall+aboutBtn->height())<=24,"About button must be magnetized above sidebar footer");

    window.resize(1280,1100);
    QTest::qWait(50);
    const int yLarge=aboutBtn->mapTo(&window,QPoint(0,0)).y();
    const int footerTopLarge=footer->mapTo(&window,QPoint(0,0)).y();
    QVERIFY2(yLarge>ySmall+200,"About button must track window bottom when height grows");
    QVERIFY2(footerTopLarge-(yLarge+aboutBtn->height())<=24,"About button must stay pinned above footer at tall window size");

    QTest::mouseClick(aboutBtn,Qt::LeftButton);
    QCOMPARE(pages->currentIndex(),8);
    QVERIFY(aboutBtn->isChecked());
    QCOMPARE(navigation->currentRow(),-1);
    QCOMPARE(window.findChild<QLabel*>("pageTitle")->text(),QString("About"));

    navigation->setCurrentRow(0);
    QCOMPARE(pages->currentIndex(),0);
    QVERIFY(!aboutBtn->isChecked());
    window.hide();
    QVERIFY(stats().value("writes").toArray().isEmpty());
  }
