// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <QWidget>
#include <QColor>
#include <QCheckBox>
#include <QComboBox>
#include <QSpinBox>
#include <QPushButton>
#include <QLabel>
#include <QJsonObject>
#include <array>
#include "UccdClient.hpp"
namespace ucc {
class LightbarTab:public QWidget {
  Q_OBJECT
public:
  explicit LightbarTab(UccdClient *client,QWidget *parent=nullptr);
signals:
  void message(const QString &text);
private:
  void reload();
  QJsonObject collect() const;
  void save();
  void showStatus();
  struct Mode {QCheckBox *enabled;QComboBox *animation,*color;QSpinBox *brightness;QPushButton *custom;QColor rgb=Qt::white;};
  UccdClient *m_client;
  QJsonObject m_settings;
  QCheckBox *m_managed,*m_silent,*m_welcome;
  QPushButton *m_save,*m_test;
  QLabel *m_status;
  std::array<Mode,3> m_modes;
};
}
