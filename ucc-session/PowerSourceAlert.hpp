// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <QObject>
#include <QJsonDocument>
#include <QJsonObject>
#include <QTimer>
#include <QDBusConnection>
#include <QDBusMessage>
#include <QDBusPendingCall>
#include "PreviewMode.hpp"

namespace ucc {
class PowerSourceAlert : public QObject {
public:
  explicit PowerSourceAlert(QDBusConnection bus=QDBusConnection::sessionBus(),QObject* parent=nullptr)
      : QObject(parent),m_bus(bus) {
    m_debounce.setSingleShot(true);m_debounce.setInterval(800);
    connect(&m_debounce,&QTimer::timeout,this,[this]{
      if(readOnlyPreview || m_source==m_announced) return;
      m_announced=m_source;
      auto call=QDBusMessage::createMethodCall("org.freedesktop.Notifications","/org/freedesktop/Notifications","org.freedesktop.Notifications","Notify");
      const bool battery=m_source=="bat";
      call.setArguments({"Frixit Control Center",quint32(0),battery ? "battery" : "ac-adapter",
        battery ? "Питание от батареи" : "Питание подключено",
        battery ? "Ноутбук работает от батареи." : m_source=="type_c" ? "Подключено питание USB-C." : "Подключено питание от сети.",
        QStringList{},QVariantMap{},qint32(5000)});
      m_bus.asyncCall(call,1500);
    });
  }
  void reset() {m_debounce.stop();m_source.clear();m_announced.clear();}
  void policyChanged(const QString& json) {
    const auto source=QJsonDocument::fromJson(json.toUtf8()).object()["real_power_source"].toString();
    if(source!="ac" && source!="bat" && source!="type_c") return;
    if(m_source.isEmpty()) {m_source=m_announced=source;return;}
    if(m_source==source) return;
    m_source=source;m_debounce.start();
  }
private:
  QDBusConnection m_bus;
  QTimer m_debounce;
  QString m_source,m_announced;
};
}
