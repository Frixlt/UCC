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

int main(int argc,char **argv) {
  QCoreApplication app(argc,argv);app.setApplicationName("ucc-session-agent");
  if(app.arguments().contains("--help")) {qInfo("Relays session notification events to the UCC lightbar; no notification content is retained.");return 0;}
  if(ucc::readOnlyPreview) {qInfo("Session relay disabled in read-only preview.");return 0;}
  const auto runtime=QStandardPaths::writableLocation(QStandardPaths::RuntimeLocation);
  if(runtime.isEmpty()) {qCritical("No user runtime directory available");return 1;}
  QLockFile lock(runtime+"/ucc-session-agent.lock");if(!lock.tryLock(0)) return 0;
  SessionSignals shutdownSignals;
  ucc::UccdClient client;QJsonObject settings,policy;
  auto load=[&] {
    if(auto json=client.getLightbarSettingsJSON()) settings=QJsonDocument::fromJson(QByteArray::fromStdString(*json)).object();else settings={};
    if(auto json=client.getPowerPolicyJSON()) policy=QJsonDocument::fromJson(QByteArray::fromStdString(*json)).object();else policy={};
  };
  QObject::connect(&client,&ucc::UccdClient::connectionStatusChanged,&app,[&](bool connected){if(connected) load();else {settings={};policy={};}});
  QObject::connect(&client,&ucc::UccdClient::lightbarSettingsChanged,&app,[&](const QString &json){settings=QJsonDocument::fromJson(json.toUtf8()).object();});
  QObject::connect(&client,&ucc::UccdClient::powerPolicyChanged,&app,[&](const QString &json){policy=QJsonDocument::fromJson(json.toUtf8()).object();});
  load();
  NotificationMonitor monitor([&] {
    const auto mode=policy["current_mode"].toString();
    if(settings["managed"].toBool() && !settings["silent_mode"].toBool() && settings["power_settings"].toObject()[mode].toObject()["enabled"].toBool())
      client.triggerLightbarAnimation(false);
  });
  monitor.start();return app.exec();
}
