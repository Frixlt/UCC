// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <QObject>
#include <QJsonObject>
#include <QDBusConnection>
#include <QDBusMessage>
#include <QDBusPendingCallWatcher>
#include <QDBusPendingReply>
namespace ucc {
class DriverAlert : public QObject {
public:
  explicit DriverAlert(QDBusConnection bus=QDBusConnection::sessionBus(),QObject *parent=nullptr):QObject(parent),m_bus(bus) {}
  void update(const QJsonObject &status) {
    const QString problem=status.value("healthy").toBool() ? QString{} : status.value("message").toString();
    if(problem!=m_current) {m_current=problem;m_notified.clear();++m_generation;m_pending=false;if(m_id) {close(m_id);m_id=0;}}
    if(problem.isEmpty() || m_pending || m_notified==problem) return;
    auto call=QDBusMessage::createMethodCall("org.freedesktop.Notifications","/org/freedesktop/Notifications","org.freedesktop.Notifications","Notify");
    call.setArguments({"Frixit Control Center",quint32(0),"dialog-error", "Проблема с Control Center",problem,QStringList{},QVariantMap{{"urgency",quint8(2)}},qint32(0)});
    m_pending=true;const auto generation=m_generation;
    auto *watcher=new QDBusPendingCallWatcher(m_bus.asyncCall(call,1500),this);
    connect(watcher,&QDBusPendingCallWatcher::finished,this,[this,generation,watcher]{
      const QDBusPendingReply<uint> reply=*watcher;watcher->deleteLater();
      if(generation!=m_generation) {if(!reply.isError()) close(reply.value());return;}
      m_pending=false;
      if(!reply.isError()) {m_id=reply.value();m_notified=m_current;}
    });
  }
private:
  void close(uint id) {
    auto call=QDBusMessage::createMethodCall("org.freedesktop.Notifications","/org/freedesktop/Notifications","org.freedesktop.Notifications","CloseNotification");
    call.setArguments({id});m_bus.asyncCall(call,1000);
  }
  QDBusConnection m_bus;
  QString m_current,m_notified;
  quint64 m_generation=0;
  uint m_id=0;
  bool m_pending=false;
};
}
