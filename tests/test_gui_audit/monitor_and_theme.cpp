#include "GuiAudit.hpp"

void GuiAudit::themeCycleAndAutomaticScheme() {
    using namespace ucc::FluentTheme;
    control("TestVisualFixture");
    setMode(Mode::Light,false);
    ucc::MainWindow window;window.resize(1280,900);window.show();
    auto *button=window.findChild<QPushButton*>("themeToggle");QVERIFY(button);QVERIFY(button->text().isEmpty());QVERIFY(!button->icon().isNull());
    QCOMPARE(mode(),Mode::Light);QVERIFY(!isDark());
    QTest::mouseClick(button,Qt::LeftButton);
    QCOMPARE(mode(),Mode::Dark);QVERIFY(isDark());
    QVERIFY(qApp->palette().color(QPalette::Base).lightness()<100);
    QTest::mouseClick(button,Qt::LeftButton);QCOMPARE(mode(),Mode::Auto);
    updateSystemScheme(Qt::ColorScheme::Dark);QVERIFY(isDark());
    updateSystemScheme(Qt::ColorScheme::Light);QVERIFY(!isDark());
    setMode(Mode::Dark,false);updateSystemScheme(Qt::ColorScheme::Light);QVERIFY(isDark());
    setMode(Mode::Auto,false);updateSystemScheme(Qt::ColorScheme::Unknown);QVERIFY(!isDark());
    setMode(Mode::Light,false);
#ifdef UCC_READ_ONLY_PREVIEW
    QVERIFY(!QFile::exists(QSettings("UniwillControlCenter","appearance").fileName()));
#endif
    QVERIFY(stats().value("writes").toArray().isEmpty());
  }
void GuiAudit::themeChangesPreserveTelemetryAndKeyboardColors() {
    using namespace ucc::FluentTheme;
    control("TestVisualFixture");
    ucc::MonitorTab monitor(client.get());monitor.resize(1280,900);monitor.show();monitor.setMonitoringActive(true);
    QChartView *view=nullptr;
    for(auto *candidate:monitor.findChildren<QChartView*>()) if(candidate->isVisible() && !candidate->chart()->series().isEmpty()){view=candidate;break;}
    QVERIFY(view);auto *series=qobject_cast<QLineSeries*>(view->chart()->series().first());QVERIFY(series);
    QTRY_VERIFY(series->count()>0);monitor.setMonitoringActive(false);const auto points=series->points();
    ucc::KeyboardVisualizerWidget keyboard(128);keyboard.setGlobalColor(QColor("#EF00D8"));const auto keys=keyboard.getJSONState();
    QSignalSpy changes(&keyboard,&ucc::KeyboardVisualizerWidget::colorsChanged);
    for(auto theme:{Mode::Dark,Mode::Light,Mode::Dark}) {
      setMode(theme,false);QCOMPARE(series->points(),points);QCOMPARE(keyboard.getJSONState(),keys);QCOMPARE(changes.count(),0);
      QCOMPARE(view->chart()->backgroundBrush().color(),colors().surface);
      QCOMPARE(view->chart()->axes(Qt::Vertical).first()->labelsBrush().color(),colors().secondary);
      QVERIFY(colors().text.lightness() > colors().surface.lightness() ? isDark() : !isDark());
    }
    setMode(Mode::Light,false);QVERIFY(stats().value("writes").toArray().isEmpty());
  }
void GuiAudit::chartBoundsOverlappingReadsAndHonorsPause() {
    ucc::MonitorTab tab(client.get());tab.show();
    QElapsedTimer elapsed;elapsed.start();tab.setMonitoringActive(true);
    QVERIFY2(elapsed.elapsed()<100,"Graph fetch must be asynchronous");
    for(int i=0;i<20;++i) QVERIFY(QMetaObject::invokeMethod(&tab,"fetchData"));
    QTest::qWait(400);
    QCOMPARE(stats().value("reads").toObject().value("GetMonitorDataSince").toInt(),1);
    tab.setMonitoringActive(false);
    QVERIFY(QMetaObject::invokeMethod(&tab,"fetchData"));QTest::qWait(50);
    QCOMPARE(stats().value("reads").toObject().value("GetMonitorDataSince").toInt(),1);
  }
void GuiAudit::monitorScrollsBetweenFetchesAndFreezesOnPause() {
    control("TestVisualFixture");
    ucc::MonitorTab tab(client.get());tab.resize(1280,900);tab.show();
    tab.setMonitoringActive(true);
    QChartView *view=nullptr;
    for(auto *candidate:tab.findChildren<QChartView*>())
      if(candidate->isVisible() && !candidate->chart()->series().isEmpty()) { view=candidate;break; }
    QVERIFY(view);
    auto *axis=qobject_cast<QDateTimeAxis*>(view->chart()->axes(Qt::Horizontal).value(0));
    QVERIFY(axis);
    auto *series=qobject_cast<QLineSeries*>(view->chart()->series().value(0));QVERIFY(series);
    QTRY_VERIFY(series->count()>0);
    const auto points=series->points();
    const auto initial=axis->max();
    QTest::qWait(250);
    QVERIFY2(axis->max()>initial,"Timeline must move between two-second data fetches");
    QCOMPARE(series->points(),points);
    const auto lag=axis->max().msecsTo(QDateTime::currentDateTime());
    QVERIFY(lag>=1900 && lag<2300);
    QTest::keyClick(&tab,Qt::Key_Space);
    const auto paused=axis->max();QTest::qWait(150);QCOMPARE(axis->max(),paused);
    QTest::keyClick(&tab,Qt::Key_Space);
    QTRY_VERIFY(axis->max()>paused);
    tab.hide();const auto hidden=axis->max();QTest::qWait(150);QCOMPARE(axis->max(),hidden);
    tab.show();QTRY_VERIFY(axis->max()>hidden);
    QCheckBox *unified=nullptr;
    for(auto *box:tab.findChildren<QCheckBox*>())
      if(box->text()=="Unified Graph") unified=box;
    QVERIFY(unified);unified->setChecked(true);
    QDateTimeAxis *unifiedAxis=nullptr;
    for(auto *candidate:tab.findChildren<QChartView*>())
      if(candidate->isVisible()) unifiedAxis=qobject_cast<QDateTimeAxis*>(candidate->chart()->axes(Qt::Horizontal).value(0));
    QVERIFY(unifiedAxis);QVERIFY(unifiedAxis!=axis);
    const auto unifiedInitial=unifiedAxis->max();QTest::qWait(100);
    QVERIFY(unifiedAxis->max()>unifiedInitial);
    QTest::keyClick(&tab,Qt::Key_Space);
    const auto unifiedPaused=unifiedAxis->max();QTest::qWait(100);
    QCOMPARE(unifiedAxis->max(),unifiedPaused);
    QTest::keyClick(&tab,Qt::Key_Space);
    unified->setChecked(false);
    auto *coreCard=tab.findChild<QFrame*>("cpuCoreLoadCard");
    auto *coreChk=tab.findChild<QCheckBox*>("monitorCpuCoreLoad");
    auto *coreGrid=tab.findChild<ucc::CpuCoreGridWidget*>("cpuCoreGridWidget");
    QVERIFY(coreCard && coreChk && coreGrid);
    QVERIFY(coreChk->isChecked());
    QVERIFY(coreGrid->coreCount()>0);
    coreChk->setChecked(false);
    QVERIFY(!coreCard->isVisibleTo(&tab));
    coreChk->setChecked(true);
    tab.setMonitoringActive(false);
    const auto stopped=axis->max();QTest::qWait(150);QCOMPARE(axis->max(),stopped);
    QVERIFY(stats().value("writes").toArray().isEmpty());
  }
void GuiAudit::sidebarNavigationPreservesPages() {
    control("TestVisualFixture");
    ucc::MainWindow window;
    auto *brand=window.findChild<QLabel*>("brand");QVERIFY(brand);
    QCOMPARE(brand->text(),QString("ucc-rusted"));
    auto *navigation=window.findChild<QListWidget*>("navigation");
    auto *aboutBtn=window.findChild<QPushButton*>("aboutNavigation");
    QVERIFY2(navigation && aboutBtn,"Fluent shell must expose 9 top sections plus bottom-pinned About button");
    QCOMPARE(navigation->count(),9);
    auto *pages=window.findChild<QTabWidget*>("pages");
    QVERIFY(pages);
    window.show();
    const int pageIndices[]={0,1,2,2,3,4,5,6,7};
    for(int index=0;index<9;++index) {
      if (!(navigation->item(index)->flags() & Qt::ItemIsEnabled)) continue;
      navigation->setCurrentRow(index);
      QCOMPARE(pages->currentIndex(),pageIndices[index]);
      QVERIFY(!aboutBtn->isChecked());
    }
    QTest::mouseClick(aboutBtn,Qt::LeftButton);
    QCOMPARE(pages->currentIndex(),8);
    QVERIFY(aboutBtn->isChecked());
    QVERIFY(window.findChild<QWidget *>("experimentalPage"));
    QVERIFY(window.findChild<QLabel *>("experimentalBanner"));
    const QStringList iconNames={"Lighting","Power & Devices","Battery & Processes","Experimental"};
    QList<QImage> renderedIcons;
    for(const auto &name:iconNames) renderedIcons.append(ucc::FluentTheme::icon(name).pixmap(24,24).toImage());
    for(int i=0;i<renderedIcons.size();++i)
      for(int j=i+1;j<renderedIcons.size();++j)
        QVERIFY2(renderedIcons[i]!=renderedIcons[j],qPrintable(QString("Duplicate sidebar icon between '%1' and '%2'").arg(iconNames[i],iconNames[j])));
    QVERIFY(!window.statusBar()->isVisible());
    auto *footer=window.findChild<QWidget *>("sidebarFooter");QVERIFY(footer);
    window.statusBar()->showMessage("Keyboard profile 'Main' saved");
    auto labels=footer->findChildren<QLabel *>("sidebarStatusText");QVERIFY(labels.size()>=3);
    for(auto theme:{ucc::FluentTheme::Mode::Light,ucc::FluentTheme::Mode::Dark}) {
      ucc::FluentTheme::setMode(theme,false);QTest::qWait(20);
      for(auto *label:labels) {
        QCOMPARE(label->font(),labels.first()->font());
        QCOMPARE(label->font().pixelSize(),12);
        QCOMPARE(label->font().weight(),QFont::Normal);
        QCOMPARE(label->palette().color(QPalette::WindowText),ucc::FluentTheme::colors().secondary);
        QCOMPARE(label->mapTo(footer,QPoint()).x(),labels.first()->mapTo(footer,QPoint()).x());
        QVERIFY(!label->text().contains("<b>"));
      }
    }
    window.hide();
    QVERIFY(stats().value("writes").toArray().isEmpty());
  }
void GuiAudit::overviewRowsAlignWithExtraGpuMetadata() {
    control("TestVisualFixture");
    ucc::MainWindow window;window.resize(1280,900);window.show();
    QLabel *cpu=nullptr,*gpu=nullptr;
    const auto findTemperatures = [&] {
      for(auto *label:window.findChildren<QLabel*>("heroValue")) {
        if(label->text()=="52") cpu=label;
        if(label->text()=="51") gpu=label;
      }
      return cpu && gpu;
    };
    QTRY_VERIFY_WITH_TIMEOUT(findTemperatures(),5000);
    QVERIFY(cpu);QVERIFY(gpu);
    QCOMPARE(cpu->mapTo(&window,QPoint()).y(),gpu->mapTo(&window,QPoint()).y());
    auto cardFor=[](QWidget *widget){while(widget && widget->objectName()!="card") widget=widget->parentWidget();return widget;};
    auto *cpuCard=cardFor(cpu),*gpuCard=cardFor(gpu);QVERIFY(cpuCard);QVERIFY(gpuCard);
    auto *primary=cpuCard->findChild<QWidget*>("primaryMetrics");QVERIFY(primary);
    auto *primaryGrid=qobject_cast<QGridLayout*>(primary->layout());QVERIFY(primaryGrid);
    QCOMPARE(primaryGrid->rowCount(),2);
    QVERIFY(gpuCard->findChild<QWidget*>("gpuDetails"));
    QVERIFY2(cpuCard->height()<=250,"CPU card should use compact vertical spacing");
    QVERIFY2(gpuCard->height()<=250,"GPU metadata must fit a compact card");
    window.resize(1024,768);QTest::qWait(100);
    QCOMPARE(cpu->mapTo(&window,QPoint()).y(),gpu->mapTo(&window,QPoint()).y());
    QVERIFY(cpuCard->height()<=270);QVERIFY(gpuCard->height()<=270);
    QVERIFY(stats().value("writes").toArray().isEmpty());
  }
void GuiAudit::monitorPowerAxisUsesLaptopRange() {
    control("TestVisualFixture");
    ucc::MonitorTab tab(client.get());tab.show();tab.setMonitoringActive(true);
    QValueAxis *powerAxis=nullptr;
    for(auto *view:tab.findChildren<QChartView*>())
      for(auto *series:view->chart()->series())
        if(series->property("_metricKey").toString()=="cpuPower")
          powerAxis=qobject_cast<QValueAxis*>(view->chart()->axes(Qt::Vertical).value(0));
    QVERIFY(powerAxis);QCOMPARE(powerAxis->min(),0.0);QCOMPARE(powerAxis->max(),250.0);
    QTest::qWait(250);QCOMPARE(powerAxis->max(),250.0);
    QVERIFY(stats().value("writes").toArray().isEmpty());
  }
void GuiAudit::pageEntranceFinishesAfterRapidNavigation() {
    control("TestVisualFixture");
    ucc::MainWindow window;window.resize(1280,900);window.show();QTest::qWait(350);
    auto *navigation=window.findChild<QListWidget*>("navigation");QVERIFY(navigation);
    navigation->setCurrentRow(1);QTest::qWait(35);
    auto *overlay=window.findChild<QWidget*>("pageEntranceOverlay");QVERIFY(overlay);
    auto *animation=overlay->findChild<QVariantAnimation*>("pageEntranceAnimation");QVERIFY(animation);
    QVERIFY(overlay->isVisible());QCOMPARE(animation->state(),QAbstractAnimation::Running);
    QVERIFY(overlay->testAttribute(Qt::WA_TransparentForMouseEvents));
    navigation->setCurrentRow(2);navigation->setCurrentRow(3);navigation->setCurrentRow(5);
    QTest::qWait(350);QVERIFY(!overlay->isVisible());
    QCOMPARE(animation->state(),QAbstractAnimation::Stopped);
    QCOMPARE(window.findChild<QTabWidget*>("pages")->currentIndex(),4);
    navigation->setCurrentRow(1);QTest::qWait(35);window.resize(1024,768);
    QTest::qWait(350);QVERIFY(!overlay->isVisible());
    verifyWindowFitsLayout(window, QSize(1024,768), "profiles-after-entrance");
    if(QTest::currentTestFailed()) return;
    QVERIFY(stats().value("writes").toArray().isEmpty());
  }
void GuiAudit::pageEntranceUsesCurrentThemeCanvas() {
    control("TestVisualFixture");
    ucc::MainWindow window;window.resize(1280,900);window.show();QTest::qWait(350);
    auto *navigation=window.findChild<QListWidget*>("navigation");QVERIFY(navigation);
    auto *overlay=window.findChild<QWidget*>("pageEntranceOverlay");QVERIFY(overlay);
    auto *animation=overlay->findChild<QVariantAnimation*>("pageEntranceAnimation");QVERIFY(animation);
    const QString output=qEnvironmentVariable("UCC_SCREENSHOT_DIR");
    if(!output.isEmpty()) QVERIFY(QDir().mkpath(output));
    for(auto mode:{ucc::FluentTheme::Mode::Dark,ucc::FluentTheme::Mode::Light}) {
      ucc::FluentTheme::setMode(mode,false);
      const QString theme=mode==ucc::FluentTheme::Mode::Dark ? "dark" : "light";
      for(int page:{1,2,3,4,5,0}) {
        navigation->setCurrentRow(page);QTest::qWait(20);
        QVERIFY(overlay->isVisible());animation->pause();animation->setCurrentTime(0);
        const auto frame=overlay->grab().toImage();
        QCOMPARE(frame.pixelColor(10,10),ucc::FluentTheme::colors().canvas);
        for(int time:{0,40,110,210}) {
          animation->setCurrentTime(time);
          if(!output.isEmpty()) QVERIFY(window.grab().save(output+QString("/%1-page-%2-%3.png").arg(theme).arg(page).arg(time)));
        }
        animation->resume();QTest::qWait(40);
        QVERIFY(!overlay->isVisible());
      }
    }
    QVERIFY(stats().value("writes").toArray().isEmpty());
  }
void GuiAudit::entranceCanBeDisabledAndPreservesEditorFocus() {
    control("TestVisualFixture");
    const auto previous=qgetenv("UCC_REDUCED_MOTION");
    const bool wasSet=qEnvironmentVariableIsSet("UCC_REDUCED_MOTION");
    const auto restore=qScopeGuard([&]{if(wasSet) qputenv("UCC_REDUCED_MOTION",previous);else qunsetenv("UCC_REDUCED_MOTION");});
    qputenv("UCC_REDUCED_MOTION","1");
    ucc::MainWindow window;window.resize(1280,900);window.show();
    auto *navigation=window.findChild<QListWidget*>("navigation");QVERIFY(navigation);
    navigation->setCurrentRow(1);QTest::qWait(50);
    auto *overlay=window.findChild<QWidget*>("pageEntranceOverlay");QVERIFY(overlay);
    QVERIFY(!overlay->isVisible());
    qputenv("UCC_REDUCED_MOTION","0");
    auto *combo=window.findChild<QComboBox*>("profileName");QVERIFY(combo);
    combo->setFocus();auto *focused=QApplication::focusWidget();QVERIFY(focused);
    combo->setCurrentIndex(0);QTest::qWait(35);QVERIFY(overlay->isVisible());
    QCOMPARE(QApplication::focusWidget(),focused);
    const QString output=qEnvironmentVariable("UCC_SCREENSHOT_DIR");
    if(!output.isEmpty()) {
      QVERIFY(QDir().mkpath(output));
      auto *animation=overlay->findChild<QVariantAnimation*>("pageEntranceAnimation");QVERIFY(animation);
      for(int time:{40,110,210}) {
        animation->setCurrentTime(time);
        QVERIFY(window.grab().save(output+QString("/entrance-%1.png").arg(time)));
      }
    }
    QTest::qWait(300);QVERIFY(!overlay->isVisible());
    QCOMPARE(QApplication::focusWidget(),focused);
    QVERIFY(stats().value("writes").toArray().isEmpty());
  }
void GuiAudit::readUccdBatchBoundsConcurrencyAndCompletesAllMethods() {
    control("TestVisualFixture");
    QVariantMap received;
    bool done = false;
    QObject owner;
    const QStringList methods = {
      "GetFanDataCPU", "GetFanDataGPU1", "GetFanDataGPU2",
      "GetCpuFrequencyMHz", "GetCpuPowerValuesJSON", "GetDGpuInfoValuesJSON",
      "GetIGpuInfoValuesJSON", "GetWaterCoolerFanSpeed", "GetWaterCoolerPumpLevel",
      "GetDisplayBrightness", "GetWebcamSWStatus", "GetFnLockStatus"
    };
    ucc::readUccdBatch(&owner, methods, [&](QVariantMap result) {
      received = result;
      done = true;
    });
    QTRY_VERIFY_WITH_TIMEOUT(done, 3000);
    QVERIFY(received.contains("GetFanDataCPU"));
    QVERIFY(received.contains("GetWaterCoolerFanSpeed"));
    QVERIFY(received.contains("GetDisplayBrightness"));
    QCOMPARE(received.size(), methods.size());
  }
void GuiAudit::monitorTabPreservesLateCrossProducerPointsWithoutDuplicates() {
    control("TestVisualFixture");
    ucc::MonitorTab tab(client.get());
    auto appendMetric = [](QByteArray &out, uint8_t metricId, int64_t ts, double val) {
      uint32_t cnt = 1;
      out.append(reinterpret_cast<const char*>(&metricId), 1);
      out.append(reinterpret_cast<const char*>(&cnt), sizeof(cnt));
      out.append(reinterpret_cast<const char*>(&ts), sizeof(ts));
      out.append(reinterpret_cast<const char*>(&val), sizeof(val));
    };

    const qint64 now = QDateTime::currentMSecsSinceEpoch();
    QByteArray batch1;
    appendMetric(batch1, 0, now - 1000, 61.0); // cpuTemp at t-1000
    QVERIFY(QMetaObject::invokeMethod(&tab, "applyBinaryData", Qt::DirectConnection, Q_ARG(QByteArray, batch1)));

    // Overlap batch repeats cpuTemp@t-1000 AND delivers late fan point cpuFanDuty@(now-1200) from another producer.
    QByteArray batch2;
    appendMetric(batch2, 0, now - 1000, 61.0); // duplicate cpuTemp -> must be deduplicated
    appendMetric(batch2, 0, now - 500, 64.0);  // new cpuTemp
    appendMetric(batch2, 1, now - 1200, 48.0); // late cpuFanDuty earlier than cpuTemp high-water
    QVERIFY(QMetaObject::invokeMethod(&tab, "applyBinaryData", Qt::DirectConnection, Q_ARG(QByteArray, batch2)));
    QVERIFY(QMetaObject::invokeMethod(&tab, "commitSeries", Qt::DirectConnection));

    QLineSeries *cpuTempSeries = nullptr;
    QLineSeries *cpuDutySeries = nullptr;
    for (auto *view : tab.findChildren<QChartView*>()) {
      for (auto *abs : view->chart()->series()) {
        auto *s = qobject_cast<QLineSeries*>(abs);
        if (!s) continue;
        const auto key = s->property("_metricKey").toString();
        if (key == "cpuTemp") cpuTempSeries = s;
        if (key == "cpuFanDuty") cpuDutySeries = s;
      }
    }
    QVERIFY(cpuTempSeries && cpuDutySeries);
    QCOMPARE(cpuTempSeries->count(), 2);
    QCOMPARE(cpuDutySeries->count(), 1);
    QCOMPARE(cpuDutySeries->at(0).y(), 48.0);
  }
