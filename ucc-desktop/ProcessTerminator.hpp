// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include "SessionTelemetry.hpp"
#include <QJsonArray>
#include <QSocketNotifier>
#include <QTimer>
#include <memory>
#include <vector>
namespace ucc {
class ProcessTerminator:public QObject {
  Q_OBJECT
public:
  explicit ProcessTerminator(SessionPaths paths={},QObject *parent=nullptr);
  ~ProcessTerminator() override;
  bool start(const QJsonArray &selected);
  bool isRunning() const {return m_running;}
signals:
  void finished(const QString &summary);
private:
  struct Target {int fd=-1;qint64 pid=0;QString name;bool done=false,killed=false;std::unique_ptr<QSocketNotifier> notifier;};
  void exited(Target *target);
  void finishIfReady();
  void finish();
  SessionReader m_reader;
  QTimer m_escalation,m_deadline;
  std::vector<std::unique_ptr<Target>> m_targets;
  QStringList m_errors;
  bool m_running=false,m_starting=false;
  int m_terminated=0,m_killed=0;
};
}
