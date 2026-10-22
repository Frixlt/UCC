// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include "SessionProcessesWidget.hpp"
#include "UccdClient.hpp"
#include <QSystemTrayIcon>
#include <QMenu>
#include <QDialog>
#include <QActionGroup>
#include <QPointer>
class KStatusNotifierItem;
namespace ucc {
class DesktopTray:public QObject {
  Q_OBJECT
public:
  explicit DesktopTray(UccdClient *client,SessionPaths paths={},QObject *parent=nullptr);
  ~DesktopTray() override;
  QMenu *menu() const {return m_menu;}
  void setSnapshot(const QJsonObject &snapshot);
  void setPolicy(const QString &json);
  static QIcon statusIcon(const QJsonObject &policy,const QJsonObject &snapshot,const QString &profile);
public slots:
  void openControlCenter();
  void showProcesses();
private:
  void updateIcon();
  void showMessage(const QString &title,const QString &message);
  void updatePolling();
  void loadPolicy();
  UccdClient *m_client;
  SessionPaths m_paths;
  DesktopMonitor m_monitor;
  QSystemTrayIcon m_icon;
  KStatusNotifierItem *m_nativeIcon=nullptr;
  QString m_usedNativeToken;
  QMenu *m_menu,*m_gpuMenu;
  QAction *m_battery,*m_status,*m_monitorAction,*m_killAll;
  QActionGroup *m_modes;
  QList<QAction*> m_processActions;
  QPointer<QDialog> m_dialog;
  QPointer<SessionProcessesWidget> m_processWidget;
  QJsonObject m_snapshot,m_policy;
  QString m_profile;
  bool m_menuVisible=false,m_dialogVisible=false,m_policyLoading=false,m_policyAgain=false;
  quint64 m_policyGeneration=0;
};
}
