// SPDX-License-Identifier: GPL-3.0-or-later
#include <QCoreApplication>
#include <QStandardPaths>
#include <QLockFile>
#include <QDir>
#include <QJsonDocument>
#include <QJsonObject>
#include <QDebug>
#include "UccdClient.hpp"
#include "PreviewMode.hpp"
#include "NotificationMonitor.hpp"
#include "SessionSignals.hpp"
#include "RefreshAdvisor.hpp"
#include "LidDisplayHandler.hpp"
#include "DriverAlert.hpp"
#include "PowerSourceAlert.hpp"
#include "DriverHealth.hpp"
#include "AsyncRead.hpp"

int main(int argc,char **argv) {
  QCoreApplication app(argc,argv);app.setApplicationName("ucc-session-agent");
  if(app.arguments().contains("--help")) {qInfo("UCC desktop notification relay and KDE/Hyprland display-refresh suggestions. No notification content is retained.");return 0;}
  if(ucc::readOnlyPreview) {qInfo("Session relay disabled in read-only preview.");return 0;}
  const auto runtime=QStandardPaths::writableLocation(QStandardPaths::RuntimeLocation);
  if(runtime.isEmpty()) {qCritical("No user runtime directory available");return 1;}
  QLockFile lock(runtime+"/ucc-session-agent.lock");if(!lock.tryLock(0)) return 0;
  SessionSignals shutdownSignals;
  ucc::UccdClient client;QJsonObject settings,policy;
  const auto desktops=qEnvironmentVariable("XDG_CURRENT_DESKTOP").split(':');
  const auto backend=desktops.contains("Hyprland",Qt::CaseInsensitive) ? ucc::RefreshBackend::Hyprland :
    desktops.contains("KDE",Qt::CaseInsensitive) ? ucc::RefreshBackend::KDE : ucc::RefreshBackend::Unsupported;
  ucc::RefreshAdvisor refresh(backend);
  quint64 profileGeneration=0;
  int profileReadAttempts=0;
  QTimer profileRetry;
  profileRetry.setSingleShot(true);
  profileRetry.setInterval(1500);
  auto requestProfile=[&] {
    const auto generation=++profileGeneration;
    ucc::readUccdAsync(&app,"GetActiveProfileJSON",{},[&,generation](std::optional<QVariant> value) {
      if(generation!=profileGeneration) return;
      if(value) refresh.profileChanged(value->toString());
      else if(++profileReadAttempts<3) profileRetry.start();
    });
  };
  QObject::connect(&profileRetry,&QTimer::timeout,&app,requestProfile);
  auto loadProfile=[&] {
    profileRetry.stop();
    profileReadAttempts=0;
    requestProfile();
  };
  ucc::PowerSourceAlert powerAlert;
  ucc::SessionSleepMonitor sleepMonitor;
  ucc::LidDisplayHandler lidDisplay(backend);
  QObject::connect(&sleepMonitor,&ucc::SessionSleepMonitor::sleeping,&refresh,&ucc::RefreshAdvisor::sleeping);
  auto load=[&] {
    if(auto json=client.getLightbarSettingsJSON()) settings=QJsonDocument::fromJson(QByteArray::fromStdString(*json)).object();else settings={};
    if(auto json=client.getPowerDeviceSettingsJSON()) lidDisplay.deviceSettingsChanged(QString::fromStdString(*json));else lidDisplay.deviceSettingsChanged("{}");
    if(auto json=client.getPowerPolicyJSON()) {const auto policyStr=QString::fromStdString(*json);policy=QJsonDocument::fromJson(policyStr.toUtf8()).object();refresh.policyChanged(policyStr);powerAlert.policyChanged(policyStr);lidDisplay.policyChanged(policyStr);}else policy={};
    loadProfile();
  };
  QObject::connect(&client,&ucc::UccdClient::connectionStatusChanged,&app,[&](bool connected){if(connected) load();else {++profileGeneration;profileRetry.stop();refresh.profileChanged("{}");settings={};policy={};powerAlert.reset();lidDisplay.deviceSettingsChanged("{}");}});
  QObject::connect(&client,&ucc::UccdClient::lightbarSettingsChanged,&app,[&](const QString &json){settings=QJsonDocument::fromJson(json.toUtf8()).object();});
  QObject::connect(&client,&ucc::UccdClient::powerPolicyChanged,&app,[&](const QString &json){policy=QJsonDocument::fromJson(json.toUtf8()).object();refresh.policyChanged(json);powerAlert.policyChanged(json);lidDisplay.policyChanged(json);loadProfile();});
  QObject::connect(&client,&ucc::UccdClient::powerDeviceStatusChanged,&app,[&](const QString &){
    ucc::readUccdAsync(&app,"GetPowerDeviceSettingsJSON",{},[&](std::optional<QVariant> value){
      if(value) lidDisplay.deviceSettingsChanged(value->toString());
    });
  });
  QObject::connect(&client,&ucc::UccdClient::profileChanged,&app,[&](const QString&,const QString&,const QString&){loadProfile();});
  load();
  ucc::DriverAlert driverAlert;
  QTimer driverPoll;driverPoll.setInterval(10000);bool driverPending=false;
  QObject::connect(&driverPoll,&QTimer::timeout,&app,[&] {
    if(driverPending) return;
    driverPending=true;
    ucc::readUccdAsync(&app,"GetDriverHealthJSON",{},[&](std::optional<QVariant> value){
      driverPending=false;
      const auto health=value ? QJsonDocument::fromJson(value->toString().toUtf8()).object() : QJsonObject{};
      driverAlert.update(health.contains("healthy") ? health : ucc::unavailableDriverHealth());
    });
  });
  // Let the user session, notifications service and kernel device probes settle.
  QTimer::singleShot(15000,&app,[&]{driverPoll.start();QMetaObject::invokeMethod(&driverPoll,"timeout");});
  NotificationMonitor monitor([&] {
    const auto mode=policy["current_mode"].toString();
    if(settings["managed"].toBool() && !settings["silent_mode"].toBool() && settings["power_settings"].toObject()[mode].toObject()["enabled"].toBool())
      client.triggerLightbarAnimation(false);
  });
  monitor.start();return app.exec();
}
