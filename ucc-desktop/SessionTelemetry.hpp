// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <QObject>
#include <QJsonObject>
#include <QThread>
#include <QTimer>
#include <QString>

namespace ucc {
struct SessionPaths {
  QString supplies="/sys/class/power_supply";
  QString pci="/sys/bus/pci/devices";
  QString proc="/proc";
  QString nvidiaPrefix="/dev/nvidia";
};
class SessionReader {
public:
  explicit SessionReader(SessionPaths paths={}):m_paths(std::move(paths)) {}
  QJsonObject snapshot(bool includeProcesses) const;
  QJsonObject processIdentity(qint64 pid) const;
  bool holdsNvidiaDevice(qint64 pid) const;
  static QString read(const QString &path);
private:
  SessionPaths m_paths;
};
class SessionSampler:public QObject {
  Q_OBJECT
public:
  explicit SessionSampler(SessionPaths paths):m_reader(std::move(paths)) {}
public slots:
  void sample(bool processes) {emit ready(m_reader.snapshot(processes));}
signals:
  void ready(const QJsonObject &snapshot);
private:
  SessionReader m_reader;
};
class DesktopMonitor:public QObject {
  Q_OBJECT
public:
  explicit DesktopMonitor(bool idlePolling=false,SessionPaths paths={},QObject *parent=nullptr);
  ~DesktopMonitor() override;
  void setActive(bool active);
  void refresh();
  void refreshProcesses();
  bool isActive() const {return m_active;}
  bool isBusy() const {return m_busy;}
  QJsonObject snapshot() const {return m_snapshot;}
signals:
  void updated(const QJsonObject &snapshot);
  void requested(bool includeProcesses);
private:
  void request(bool processes);
  bool m_againProcesses=false;
  QThread m_thread;
  QTimer m_timer;
  QJsonObject m_snapshot;
  bool m_idlePolling=false,m_active=false,m_busy=false,m_again=false;
};
QString batterySummary(const QJsonObject &snapshot);
}
