// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include "UccdClient.hpp"
#include "AsyncRead.hpp"
#include "PreviewMode.hpp"
#include "FluentTheme.hpp"
#include <QWidget>
#include <QFormLayout>
#include <QVBoxLayout>
#include <QComboBox>
#include <QPushButton>
#include <QLabel>
#include <QJsonDocument>
#include <QJsonArray>

namespace ucc {
class BatterySettingsWidget : public QWidget {
public:
  explicit BatterySettingsWidget(UccdClient* client,QWidget* parent=nullptr):QWidget(parent),m_client(client) {
    setObjectName("batterySettings");
    auto* layout=new QVBoxLayout(this);layout->setContentsMargins(0,8,0,8);
    auto* card=FluentTheme::createCard();layout->addWidget(card);
    auto* form=new QFormLayout(card);
    auto* title=new QLabel("Battery charging");title->setObjectName("sectionTitle");form->addRow(title);
    auto* hint=new QLabel("These settings apply to the laptop and stay the same when performance profiles change.");hint->setWordWrap(true);hint->setObjectName("muted");form->addRow(hint);
    m_mode=new QComboBox;m_mode->setObjectName("globalChargingProfile");
    m_priority=new QComboBox;m_priority->setObjectName("globalChargingPriority");
    m_limit=new QComboBox;m_limit->setObjectName("globalChargeLimit");
    m_limit->addItem("Full capacity (100%)","full");m_limit->addItem("Reduced (60–90%)","reduced");m_limit->addItem("Stationary (40–80%)","stationary");
    form->addRow("Charging mode",m_mode);form->addRow("USB-C priority",m_priority);form->addRow("Charge limit",m_limit);
    m_apply=new QPushButton("Save and apply");m_apply->setObjectName("saveBatterySettings");m_apply->setProperty("primary",true);m_apply->setEnabled(false);
    form->addRow(m_apply);m_status=new QLabel("Loading charging settings…");m_status->setWordWrap(true);form->addRow(m_status);
    m_form=form;
    connect(m_apply,&QPushButton::clicked,this,[this]{apply();});
    connect(m_client,&UccdClient::connectionStatusChanged,this,[this](bool connected){if(connected) reload();else {m_apply->setEnabled(false);m_status->setText("Service unavailable");}});
    reload();
  }
  void reload() {
    const auto generation=++m_generation;
    readUccdBatch(this,{"GetChargingProfilesAvailable","GetCurrentChargingProfile","GetChargingPrioritiesAvailable","GetCurrentChargingPriority",
      "GetChargeStartAvailableThresholds","GetChargeEndAvailableThresholds","GetChargeType","GetChargeStartThreshold","GetChargeEndThreshold"},
      [this,generation](const QVariantMap& data) {
        if(generation!=m_generation) return;
        m_current=data;
        const auto populate=[](QComboBox* combo,const QVariant& json,const QString& current) {
          combo->clear();
          for(const auto& v:QJsonDocument::fromJson(json.toString().toUtf8()).array()) {
            const auto value=v.toString();
            const QString label=value=="stationary" ? "Stationary (~80%)" : value=="balanced" ? "Balanced (~90%)" :
              value=="high_capacity" ? "Full capacity (100%)" : value=="charge_battery" ? "Charge battery" : value=="performance" ? "Performance" : value;
            combo->addItem(label,value);
          }
          combo->setCurrentIndex(combo->findData(current));
        };
        populate(m_mode,data.value("GetChargingProfilesAvailable"),data.value("GetCurrentChargingProfile").toString());
        populate(m_priority,data.value("GetChargingPrioritiesAvailable"),data.value("GetCurrentChargingPriority").toString());
        m_form->setRowVisible(m_mode,m_mode->count()>0);m_form->setRowVisible(m_priority,m_priority->count()>0);
        const auto starts=QJsonDocument::fromJson(data.value("GetChargeStartAvailableThresholds").toString().toUtf8()).array();
        const auto ends=QJsonDocument::fromJson(data.value("GetChargeEndAvailableThresholds").toString().toUtf8()).array();
        m_thresholds=!starts.isEmpty() && !ends.isEmpty();m_form->setRowVisible(m_limit,m_thresholds);
        if(m_limit->count()>3) m_limit->removeItem(3);
        const int start=data.value("GetChargeStartThreshold",-1).toInt(),end=data.value("GetChargeEndThreshold",-1).toInt();
        const bool custom=data.value("GetChargeType").toString()=="Custom";
        QString preset=custom && start==40 && end==80 ? "stationary" : custom && start==60 && end==90 ? "reduced" : "full";
        if(custom && preset=="full") {m_limit->addItem(QString("Current (%1–%2%)").arg(start).arg(end),"current");preset="current";}
        m_limit->setCurrentIndex(m_limit->findData(preset));
        const bool available=m_mode->count()>0 || m_priority->count()>0 || m_thresholds;
        m_mode->setEnabled(!readOnlyPreview);m_priority->setEnabled(!readOnlyPreview);m_limit->setEnabled(!readOnlyPreview);
        m_apply->setEnabled(available && !readOnlyPreview);
        m_status->setText(available ? "Global charging settings" : "Charging controls are not supported by this device.");
      });
  }
private:
  void apply() {
    if(readOnlyPreview || !m_apply->isEnabled()) return;
    bool ok=true;
    const auto mode=m_mode->currentData().toString(),priority=m_priority->currentData().toString();
    if(!mode.isEmpty() && mode!=m_current.value("GetCurrentChargingProfile").toString()) ok=m_client->setChargingProfile(mode.toStdString()) && ok;
    if(!priority.isEmpty() && priority!=m_current.value("GetCurrentChargingPriority").toString()) ok=m_client->setChargingPriority(priority.toStdString()) && ok;
    const auto preset=m_limit->currentData().toString();
    if(m_thresholds && preset!="current") {
      if(preset=="full") {
        if(m_current.value("GetChargeType").toString()!="Standard") ok=m_client->setChargeType("Standard") && ok;
      } else {
        const int start=preset=="stationary" ? 40 : 60,end=preset=="stationary" ? 80 : 90;
        // Only advertised presets can be selected on this hardware.
        const auto starts=QJsonDocument::fromJson(m_current.value("GetChargeStartAvailableThresholds").toString().toUtf8()).array();
        const auto ends=QJsonDocument::fromJson(m_current.value("GetChargeEndAvailableThresholds").toString().toUtf8()).array();
        if(!starts.contains(start) || !ends.contains(end)) {m_status->setText("This charge limit is not supported by the battery.");return;}
        if(m_current.value("GetChargeType").toString()!="Custom") ok=m_client->setChargeType("Custom") && ok;
        if(m_current.value("GetChargeStartThreshold").toInt()!=start) ok=m_client->setChargeStartThreshold(start) && ok;
        if(m_current.value("GetChargeEndThreshold").toInt()!=end) ok=m_client->setChargeEndThreshold(end) && ok;
      }
    }
    if(ok) {reload();m_status->setText("Charging settings saved.");}
    else m_status->setText("Some charging settings could not be saved. Reload the page to check the current state.");
  }
  UccdClient* m_client;
  QFormLayout* m_form;
  QComboBox *m_mode,*m_priority,*m_limit;
  QPushButton* m_apply;
  QLabel* m_status;
  QVariantMap m_current;
  quint64 m_generation=0;
  bool m_thresholds=false;
};
}
