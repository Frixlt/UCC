// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <QObject>
#include <QProcess>
#include <QTimer>
#include <QElapsedTimer>
#include <functional>

// dbus-monitor can observe method calls; a normal signal subscription cannot.
// Discard arguments immediately: notification text is never logged or forwarded.
class NotificationMonitor:public QObject {
public:
  explicit NotificationMonitor(std::function<void()> notify,QObject *parent=nullptr)
    :QObject(parent),m_notify(std::move(notify)) {
    m_restart.setSingleShot(true);m_restart.setInterval(5000);
    connect(&m_restart,&QTimer::timeout,this,[this]{start();});
    connect(&m_process,&QProcess::readyReadStandardOutput,this,[this]{feed(m_process.readAllStandardOutput());});
    connect(&m_process,&QProcess::readyReadStandardError,this,[this]{m_process.readAllStandardError();});
    connect(&m_process,&QProcess::finished,this,[this](int,QProcess::ExitStatus){m_pending.clear();m_dropping=false;if(!m_stopping) m_restart.start();});
    connect(&m_process,&QProcess::errorOccurred,this,[this](QProcess::ProcessError){if(!m_stopping) m_restart.start();});
  }
  ~NotificationMonitor() override {m_stopping=true;m_restart.stop();m_process.terminate();if(!m_process.waitForFinished(500)) {m_process.kill();m_process.waitForFinished(500);}}
  void start() {
    if(m_process.state()!=QProcess::NotRunning) return;
    m_process.start("dbus-monitor",{"--session","type='method_call',interface='org.freedesktop.Notifications',member='Notify'"});
  }
  void feed(const QByteArray &bytes) {
    for(char c:bytes) {
      if(c=='\n') {
        if(!m_dropping && isNotification(m_pending) && (!m_last.isValid() || m_last.elapsed()>=100)) {m_last.start();m_notify();}
        m_pending.clear();m_dropping=false;
      } else if(!m_dropping) {
        if(m_pending.size()<4096) m_pending+=c;else {m_pending.clear();m_dropping=true;}
      }
    }
  }
  static bool isNotification(const QByteArray &line) {
    return line.startsWith("method call ") && line.contains(" interface=org.freedesktop.Notifications;") &&
      line.trimmed().endsWith(" member=Notify");
  }
private:
  QProcess m_process;
  QTimer m_restart;
  QElapsedTimer m_last;
  std::function<void()> m_notify;
  QByteArray m_pending;
  bool m_dropping=false,m_stopping=false;
};
