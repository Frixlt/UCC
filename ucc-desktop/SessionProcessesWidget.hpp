// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include "SessionTelemetry.hpp"
#include "ProcessTerminator.hpp"
#include <QWidget>
#include <QLabel>
#include <QPushButton>
#include <QTableWidget>
namespace ucc {
class SessionProcessesWidget:public QWidget {
  Q_OBJECT
public:
  explicit SessionProcessesWidget(SessionPaths paths={},DesktopMonitor *monitor=nullptr,QWidget *parent=nullptr);
  void setSnapshot(const QJsonObject &snapshot);
  void confirmTermination(const QJsonArray &processes);
  DesktopMonitor *monitor() const {return m_monitor;}
signals:
  void message(const QString &text);
  void activityChanged(bool visible);
protected:
  void showEvent(QShowEvent *event) override;
  void hideEvent(QHideEvent *event) override;
private:
  QJsonArray selected() const;
  void updateActions();
  DesktopMonitor *m_monitor;
  ProcessTerminator *m_job;
  bool m_ownsMonitor;
  QLabel *m_battery,*m_gpu,*m_status;
  QTableWidget *m_table;
  QPushButton *m_selected,*m_all,*m_refresh;
};
}
