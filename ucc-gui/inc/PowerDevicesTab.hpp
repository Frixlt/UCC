// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <QWidget>
#include <QJsonObject>
#include <QJsonArray>
#include <QTableWidget>
#include <QCheckBox>
#include <QComboBox>
#include <QLabel>
#include <QLineEdit>
#include <QPushButton>
#include <array>
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
  std::array<std::array<QComboBox *,3>,3> m_cpuCombos{};
  QTableWidget *m_devices = nullptr;
  QCheckBox *m_enabled = nullptr,*m_nvidia = nullptr,*m_wakeOnlyLidKbd = nullptr,*m_showAdvancedPcie = nullptr;
  QLabel *m_status = nullptr;
  QLineEdit *m_search = nullptr;
  QPushButton *m_save = nullptr,*m_scan = nullptr;
};
}
