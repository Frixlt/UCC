#pragma once

#include "AuditDaemon.hpp"
#include <QAbstractButton>
#include <QAbstractSpinBox>
#include <algorithm>

class GuiAudit : public QObject {
  Q_OBJECT
  QProcess daemon;
  QUrl openedLogDirectory;
public slots:
  void captureLogDirectory(const QUrl &url) { openedLogDirectory = url; }
private:
  std::unique_ptr<ucc::UccdClient> client;
  std::unique_ptr<ucc::ProfileManager> profiles;
  QVariant control(const QString &method, QVariantList args = {}) {
    auto msg = QDBusMessage::createMethodCall("com.uniwill.uccd", "/com/uniwill/uccd", "com.uniwill.uccd", method);
    msg.setArguments(args);
    return QDBusConnection::systemBus().call(msg).arguments().value(0);
  }
  QJsonObject stats() { return QJsonDocument::fromJson(control("TestStats").toString().toUtf8()).object(); }
  QComboBox *pump(ucc::FanControlTab &tab) {
    for (auto *combo : tab.findChildren<QComboBox *>()) if (combo->findText("8V") >= 0) return combo;
    return nullptr;
  }
  static bool hasAncestorOfType(const QWidget *widget, const QMetaObject &meta) {
    for (const QWidget *p = widget ? widget->parentWidget() : nullptr; p; p = p->parentWidget()) {
      if (p->metaObject()->inherits(&meta)) return true;
    }
    return false;
  }
  void verifyWindowFitsLayout(ucc::MainWindow &window, const QSize &targetSize, const QString &context) {
    QCoreApplication::processEvents();
    const QSize minHint = window.minimumSizeHint();
    QVERIFY2(minHint.width() <= 1024,
      qPrintable(QString("%1: minimumSizeHint().width()=%2 exceeds 1024").arg(context).arg(minHint.width())));
    QVERIFY2(minHint.height() <= 768,
      qPrintable(QString("%1: minimumSizeHint().height()=%2 exceeds 768").arg(context).arg(minHint.height())));
    QVERIFY2(window.width() <= targetSize.width(),
      qPrintable(QString("%1: window.width()=%2 exceeds target %3").arg(context).arg(window.width()).arg(targetSize.width())));
    QVERIFY2(window.height() <= targetSize.height(),
      qPrintable(QString("%1: window.height()=%2 exceeds target %3").arg(context).arg(window.height()).arg(targetSize.height())));
    auto *pages = window.findChild<QTabWidget *>("pages");
    QVERIFY(pages);
    QWidget *activePage = pages->currentWidget();
    QVERIFY(activePage);
    QList<QScrollArea *> scrollAreas;
    if (auto *selfScroll = qobject_cast<QScrollArea *>(activePage)) scrollAreas.append(selfScroll);
    scrollAreas.append(activePage->findChildren<QScrollArea *>());
    for (auto *scroll : scrollAreas) {
      if (!scroll->isVisibleTo(&window)) continue;
      if (hasAncestorOfType(scroll, ucc::KeyboardVisualizerWidget::staticMetaObject)) continue;
      const int hMax = scroll->horizontalScrollBar() ? scroll->horizontalScrollBar()->maximum() : 0;
      QVERIFY2(hMax == 0,
        qPrintable(QString("%1: horizontal overflow in scroll area '%2' (max=%3)").arg(context, scroll->objectName()).arg(hMax)));
    }
    for (auto *child : activePage->findChildren<QWidget *>()) {
      if (!child->isVisibleTo(&window)) continue;
      const bool isControl = qobject_cast<QAbstractButton *>(child) != nullptr
        || qobject_cast<QComboBox *>(child) != nullptr
        || qobject_cast<QLineEdit *>(child) != nullptr
        || qobject_cast<QAbstractSpinBox *>(child) != nullptr
        || qobject_cast<QSlider *>(child) != nullptr;
      if (!isControl) continue;
      if (hasAncestorOfType(child, ucc::KeyboardVisualizerWidget::staticMetaObject)) continue;
      if (qobject_cast<QComboBox *>(child->parentWidget()) || qobject_cast<QAbstractSpinBox *>(child->parentWidget())) continue;
      QVERIFY2(child->width() > 0 && child->height() > 0,
        qPrintable(QString("%1: collapsed control %2('%3') %4x%5")
          .arg(context, QString::fromLatin1(child->metaObject()->className()), child->objectName()).arg(child->width()).arg(child->height())));
      const QPoint topLeft = child->mapTo(&window, QPoint(0, 0));
      QVERIFY2(topLeft.x() >= 0 && topLeft.x() + child->width() <= window.width(),
        qPrintable(QString("%1: clipped control %2('%3') x=%4..%5 in window width %6")
          .arg(context, QString::fromLatin1(child->metaObject()->className()), child->objectName())
          .arg(topLeft.x()).arg(topLeft.x() + child->width()).arg(window.width())));
    }
    if (auto *deviceTable = activePage->findChild<QTableWidget *>("powerDeviceTable"); deviceTable && deviceTable->isVisibleTo(&window)) {
      const int hMax = deviceTable->horizontalScrollBar() ? deviceTable->horizontalScrollBar()->maximum() : 0;
      QVERIFY2(hMax == 0,
        qPrintable(QString("%1: powerDeviceTable horizontal overflow (max=%2)").arg(context).arg(hMax)));
      QVERIFY2(deviceTable->columnWidth(0) >= 120,
        qPrintable(QString("%1: powerDeviceTable Device column too narrow (%2px)").arg(context).arg(deviceTable->columnWidth(0))));
    }
  }
private slots:
  void initTestCase();
  void init();
  void desktopPreferenceImportDoesNotContactTheDaemon();
  void profileRefreshRateSavesAndChargingLivesOutsideProfiles();
  void individualCpuSelectionSavesIdsAndPreservesKernelLockedCpu();
  void standaloneTrayShowsModesAndHonorsCommandFailures();
  void batteryAndProcessPageStaysReadOnlyUntilConfirmed();
  void sessionAgentRelaysOnlyNotificationEvents();
  void lightbarKeepsSourceValuesAndRejectsFailedCommands();
  void calibratedKeyboardKeepsPhysicalLedIdentities();
  void sparseKeyboardZonesKeepSavedPhysicalIndices();
  void deviceMatrixKeepsThreeIndependentModes();
  void rejectedPolicyCommandsAreNotReportedAsSuccessful();
  void powerPolicySignalReachesClientWithoutPolling();
  void themeCycleAndAutomaticScheme();
  void themeChangesPreserveTelemetryAndKeyboardColors();
  void dashboardWaterCoolerSelectors();
  void dashboardSelectionsApplyImmediatelyFromAuto();
  void displayingManualControlsMustNotStopCooling();
  void waterPollingIsAsyncAndPreservesValuesOnFailure();
  void dashboardBoundsOverlappingReads();
  void chartBoundsOverlappingReadsAndHonorsPause();
  void monitorScrollsBetweenFetchesAndFreezesOnPause();
  void sidebarNavigationPreservesPages();
  void overviewRowsAlignWithExtraGpuMetadata();
  void monitorPowerAxisUsesLaptopRange();
  void profilesUseOneCompactSelector();
  void ctgpSliderGuardsUnavailableAndInvertedBounds();
  void pageEntranceFinishesAfterRapidNavigation();
  void pageEntranceUsesCurrentThemeCanvas();
  void entranceCanBeDisabledAndPreservesEditorFocus();
  void aboutLoggingPreferenceAndFolderAction();
  void displayBrightnessCommitsOnReleaseAndIgnoresMonitorDuringDrag();
  void dashboardShowsDriverFailureAndClearsAfterRecovery();
  void aboutBuildCreditsAndBundledLicense();
  void profilePayloadPreservesManualWaterCoolerValues();
  void notificationHistoryAndReadControls();
  void renderReleaseGallery();
  void renderReferencePages();
  void keyboardSelectionDoesNotChangeBacklight();
  void previewMustNotSaveLocalMonitorPreferences();
  void previewMustBlockEveryWritePath();
  void fanControlPumpComboFindsDiscreteIntCodes();
  void batterySettingsRaisesWindowSetsEndBeforeStart();
  void applyFailureDoesNotClaimSuccessOrSendWaterCooler();
  void asyncApplyAllowsImmediateWindowCloseWithoutLateWaterCooler();
  void rejectedProfileSelectionKeepsPreviousActiveProfile();
  void slowSetActiveProfileDoesNotBlockEventLoop();
  void prepareForSleepResumeSetsSidebarStatus();
  void readUccdBatchBoundsConcurrencyAndCompletesAllMethods();
  void monitorTabPreservesLateCrossProducerPointsWithoutDuplicates();
  void deviceSupportStatusDistinguishesOfflineFromUnsupported();
  void aboutTabStaysPinnedToSidebarBottom();
  void watercoolCrosshairsStayVisibleAcrossTabsAndPartialReads();
  void cleanupTestCase();
};
