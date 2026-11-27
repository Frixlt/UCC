#pragma once

#include "AuditDaemon.hpp"

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
  void prepareForSleepResumeSetsSidebarStatus();
  void deviceSupportStatusDistinguishesOfflineFromUnsupported();
  void cleanupTestCase();
};
