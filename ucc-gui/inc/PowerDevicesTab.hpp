// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <QWidget>
#include <QJsonObject>
#include <QJsonArray>
#include <QTableWidget>
#include <QCheckBox>
#include <QLabel>
#include <QLineEdit>
#include <QPushButton>
#include "UccdClient.hpp"

namespace ucc {
class PowerDevicesTab : public QWidget {
  Q_OBJECT
public:
  explicit PowerDevicesTab(UccdClient *client,QWidget *parent=nullptr);
  void reload();
signals:
  void message(const QString &text);
private:
  void save();
  void scan();
  void showStatus(const QString &json);
  void filter();
  QJsonObject collect() const;
  UccdClient *m_client;
  QJsonObject m_settings;
  QJsonArray m_entries;
  QTableWidget *m_cpu,*m_devices;
  QCheckBox *m_enabled,*m_nvidia;
  QLabel *m_status;
  QLineEdit *m_search;
  QPushButton *m_save,*m_scan;
};
}
