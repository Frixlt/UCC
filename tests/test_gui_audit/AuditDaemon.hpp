#pragma once

#include <QtTest>
#include <QApplication>
#include <QStatusBar>
#include <QTabBar>
#include <QDBusVirtualObject>
#include <QDBusConnection>
#include <QDBusMessage>
#include <QProcess>
#include <QJsonDocument>
#include <QJsonObject>
#include <QJsonArray>
#include <QTimer>
#include <QFile>
#include <QTemporaryDir>
#include <QDir>
#include <QVariantAnimation>
#include <QScrollArea>
#include <QScrollBar>
#include <QSlider>
#include <QColorDialog>
#include <QScopeGuard>
#include "FanControlTab.hpp"
#include "NotificationCenter.hpp"
#include "version.h"
#include "AppLogging.hpp"
#include <QDesktopServices>
#include <QTextBrowser>
#include "MainWindow.hpp"
#include "SessionProcessesWidget.hpp"
#include "DesktopTray.hpp"
#include "DesktopPreferences.hpp"
#include <QSettings>
#include <QMessageBox>
#include "FluentTheme.hpp"
#include "../GuiVisualFixture.hpp"
#include "DashboardTab.hpp"
#include "MonitorTab.hpp"
#include "SystemMonitor.hpp"
#include "BatterySettingsWidget.hpp"


class AuditDaemon : public QDBusVirtualObject {
  QJsonArray writes;
  QJsonObject reads;
  QString runtimeProfile,lastAppliedProfile,lastDeviceSettings,lastLightbar,driverHealth;
  bool failReads=false,failControl=false;
  bool visualFixture=false;
  bool coolerAuto=false;
  bool coolerEnabled=false;
  bool chargeWindowTest=false;
  bool ctgpTest=false,ctgpSupported=false,ctgpAvailable=false;
  int ctgpDefault=0,ctgpMax=0;
  int chargeStart=40,chargeEnd=80;
  int applyDelayMs=0;
  bool deviceSupported=false;
  QString chargeType="Custom";
  QString customProfile,lastSavedProfile,chargingMode="stationary";
  QString customFanProfiles,builtinFanProfiles;
public:
  QString introspect(const QString &) const override {
    return "<interface name=\"com.uniwill.uccd\"><method name=\"GetWaterCoolerConnected\"><arg type=\"b\" direction=\"out\"/></method></interface>";
  }
  bool handleMessage(const QDBusMessage &msg,const QDBusConnection &bus) override {
    const auto method=msg.member(); QVariant result;
    if(method=="GetCpuTopologyJSON") {
      QJsonArray rows;for(int id=0;id<24;++id) rows.append(QJsonObject{{"id",id},{"online",id<8},{"hotpluggable",id!=0},{"type",id<8 ? "P" : "E"},{"physicalCore",id},{"siblings",QString::number(id)}});
      bus.send(msg.createReply(QString::fromUtf8(QJsonDocument(rows).toJson())));return true;
    }
    if(method=="TestCustomProfile") {customProfile=msg.arguments().value(0).toString();bus.send(msg.createReply(true));return true;}
    if(method=="TestCustomFanProfiles") {customFanProfiles=msg.arguments().value(0).toString();bus.send(msg.createReply(true));return true;}
    if(method=="GetCustomFanProfilesJSON" && !customFanProfiles.isEmpty()) {bus.send(msg.createReply(customFanProfiles));return true;}
    if(method=="GetFanProfileNames" && !builtinFanProfiles.isEmpty()) {bus.send(msg.createReply(builtinFanProfiles));return true;}
    if(method=="TestBuiltinFanProfiles") {builtinFanProfiles=msg.arguments().value(0).toString();bus.send(msg.createReply(true));return true;}
    if(method=="TestLastSavedProfile") {bus.send(msg.createReply(lastSavedProfile));return true;}
    if(method=="GetCustomProfilesJSON" && !customProfile.isEmpty()) {bus.send(msg.createReply("["+customProfile+"]"));return true;}
    if(method=="GetChargingProfilesAvailable") {bus.send(msg.createReply(QString("[\"stationary\",\"high_capacity\"]")));return true;}
    if(method=="GetCurrentChargingProfile") {bus.send(msg.createReply(chargingMode));return true;}
    if(method=="TestDriverHealth") {driverHealth=msg.arguments().value(0).toString();bus.send(msg.createReply(true));return true;}
    if(method=="GetDriverHealthJSON") {bus.send(msg.createReply(driverHealth.isEmpty() ? QString("{\"healthy\":true}") : driverHealth));return true;}
    if(method=="TestRuntimeProfile") {runtimeProfile=msg.arguments().value(0).toString();bus.send(msg.createReply(true));return true;}
    if(method=="TestFailControl") {failControl=msg.arguments().value(0).toBool();bus.send(msg.createReply(true));return true;}
    if(method=="TestLastLightbar") {bus.send(msg.createReply(lastLightbar));return true;}
    if(method=="TestLastPowerDevices") {bus.send(msg.createReply(lastDeviceSettings));return true;}
    if(method=="TestLastAppliedProfile") {bus.send(msg.createReply(lastAppliedProfile));return true;}
    if(method=="GetActiveProfileJSON" && !runtimeProfile.isEmpty()) {bus.send(msg.createReply(runtimeProfile));return true;}
    if(visualFixture) if(auto reply=visualFixtureReply(method)){bus.send(msg.createReply(*reply));return true;}
    if (method=="TestEmitPowerPolicy") {
      auto signal=QDBusMessage::createSignal("/com/uniwill/uccd","com.uniwill.uccd","PowerPolicyChanged");
      signal.setArguments({msg.arguments().value(0)});bus.send(signal);result=true;
    }
    else if (method=="TestCoolerAuto") {coolerAuto=msg.arguments().value(0).toBool();result=true;}
    else if(method=="TestCoolerEnabled") {coolerEnabled=msg.arguments().value(0).toBool();result=true;}
    else if(method=="IsWaterCoolerEnabled") result=coolerEnabled;
    else if(method=="IsWaterCoolerAutoControlEnabled") result=coolerAuto;
    else if (method=="TestVisualFixture") {visualFixture=true;result=true;}
    else if (method=="TestChargeWindow") {chargeWindowTest=true;chargeStart=40;chargeEnd=80;chargeType="Custom";result=true;}
    else if (method=="TestCtgp") {ctgpTest=true;ctgpSupported=msg.arguments().value(0).toBool();ctgpAvailable=msg.arguments().value(1).toBool();ctgpDefault=msg.arguments().value(2).toInt();ctgpMax=msg.arguments().value(3).toInt();result=true;}
    else if (ctgpTest && method=="GetCTGPAdjustmentSupported") {result=ctgpSupported;}
    else if (ctgpTest && method=="GetNVIDIAPowerCTRLAvailable") {result=ctgpAvailable;}
    else if (ctgpTest && method=="GetNVIDIAPowerCTRLDefaultPowerLimit") {result=ctgpDefault;}
    else if (ctgpTest && method=="GetNVIDIAPowerCTRLMaxPowerLimit") {result=ctgpMax;}
    else if (chargeWindowTest && method=="GetChargeStartAvailableThresholds") {result=QString("[40,60]");}
    else if (chargeWindowTest && method=="GetChargeEndAvailableThresholds") {result=QString("[80,90,100]");}
    else if (chargeWindowTest && method=="GetChargeType") {result=chargeType;}
    else if (chargeWindowTest && method=="GetChargeStartThreshold") {result=chargeStart;}
    else if (chargeWindowTest && method=="GetChargeEndThreshold") {result=chargeEnd;}
    else if (method=="TestStats") result=QString::fromUtf8(QJsonDocument(QJsonObject{{"writes",writes},{"reads",reads}}).toJson());
    else if (method=="TestReset") { customProfile.clear();customFanProfiles.clear();builtinFanProfiles.clear();lastSavedProfile.clear();chargingMode="stationary";writes={};reads={};runtimeProfile.clear();driverHealth.clear();lastAppliedProfile.clear();lastDeviceSettings.clear();lastLightbar.clear();failReads=false;failControl=false;visualFixture=false;coolerAuto=false;coolerEnabled=false;chargeWindowTest=false;ctgpTest=false;ctgpSupported=false;ctgpAvailable=false;ctgpDefault=0;ctgpMax=0;chargeStart=40;chargeEnd=80;applyDelayMs=0;deviceSupported=false;chargeType="Custom";result=true; }
    else if (method=="TestDelayApply") { applyDelayMs=msg.arguments().value(0).toInt();result=true; }
    else if (method=="TestDeviceSupported") { deviceSupported=msg.arguments().value(0).toBool();result=true; }
    else if (method=="TestFailReads") { failReads=msg.arguments().value(0).toBool();result=true; }
    else if (method=="ApplyProfile" && applyDelayMs>0) {
      const int delay=applyDelayMs;const bool fail=failControl;const QString arg=msg.arguments().value(0).toString();
      QTimer::singleShot(delay,this,[this,msg,bus,fail,arg] {
        if(fail) {bus.send(msg.createReply(false));return;}
        writes.append(QStringLiteral("ApplyProfile"));lastAppliedProfile=arg;bus.send(msg.createReply(true));
      });
      return true;
    }
    else if (method=="TriggerLightbarAnimation" || method=="ScanPowerDevices" || method.startsWith("Register") || method.startsWith("Unregister") || method.startsWith("Set") || method.startsWith("TurnOff") || method=="EnableWaterCooler" || method.startsWith("Apply") || method.startsWith("Save") || method.startsWith("Delete") || method.startsWith("Revert")) {
      if(failControl) {bus.send(msg.createReply(false));return true;}
      if(method=="SaveCustomProfile") customProfile=lastSavedProfile=msg.arguments().value(0).toString();
      if(method=="SetChargingProfile") chargingMode=msg.arguments().value(0).toString();
      if(method=="SetLightbarSettingsJSON") lastLightbar=msg.arguments().value(0).toString();
      if(method=="SetPowerDeviceSettingsJSON") lastDeviceSettings=msg.arguments().value(0).toString();
      writes.append(method);if(method=="ApplyProfile") lastAppliedProfile=msg.arguments().value(0).toString();result=true;
    } else if (method=="IsDeviceSupported") result=deviceSupported;
    else if (method.endsWith("Supported") || method=="GetWebcamSWStatus" || method=="GetFnLockStatus" || method=="IsWaterCoolerEnabled") result=false;
    else if (method=="GetAvailableGovernors" || method=="GetAvailableEPPs") result=QString("[]");
    else if (method.startsWith("GetWaterCooler") || method=="GetMonitorDataSince") {
      reads[method]=reads.value(method).toInt()+1;
      if (method=="GetWaterCoolerConnected") result=true;
      else if (method=="GetWaterCoolerAvailable") result=false;
      else if (method=="GetWaterCoolerPumpLevel") result=3;
      else if (method=="GetWaterCoolerFanSpeed") result=100;
      else if (method=="GetMonitorDataSince") result=QByteArray();
      const bool fail=failReads;
      QTimer::singleShot(300,this,[msg,bus,result,fail] {
        bus.send(fail ? msg.createErrorReply(QDBusError::Failed,"Test read failure") : msg.createReply(result));
      });
      return true;
    } else if (method.endsWith("JSON") || (method.endsWith("Available") || method.endsWith("Thresholds")) || method=="GetFanProfileNames") {
      result=QString(method=="GetSettingsJSON" || method=="GetActiveProfileJSON" ? "{}" : "[]");
    } else if (method.startsWith("GetCurrent") || method=="GetPowerState") result=QString();
    else result=0;
    bus.send(msg.createReply(result));return true;
  }
};
