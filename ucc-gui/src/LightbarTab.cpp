// SPDX-License-Identifier: GPL-3.0-or-later
#include "LightbarTab.hpp"
#include "FluentTheme.hpp"
#include "PreviewMode.hpp"
#include <QVBoxLayout>
#include <QHBoxLayout>
#include <QFormLayout>
#include <QTabWidget>
#include <QJsonArray>
#include <QJsonDocument>
#include <QColorDialog>
#include <QWheelEvent>
namespace ucc {
namespace {
const QStringList modes={"ac","type_c","bat"};
const QStringList animations={"Pulse (stays on)","Flash (stays off)","Rainbow","Police Strobe","Disco"};
class Selector:public QComboBox {public:using QComboBox::QComboBox;protected:void wheelEvent(QWheelEvent *e) override {e->ignore();}};
class Brightness:public QSpinBox {public:using QSpinBox::QSpinBox;protected:void wheelEvent(QWheelEvent *e) override {e->ignore();}};
}
LightbarTab::LightbarTab(UccdClient *client,QWidget *parent):QWidget(parent),m_client(client) {
  auto *layout=new QVBoxLayout(this);layout->setContentsMargins(0,0,0,0);layout->setSpacing(16);
  auto *description=new QLabel("Front lightbar settings follow the effective power mode. Notifications animate the lightbar while the UCC session agent is running.");
  description->setWordWrap(true);description->setObjectName("muted");layout->addWidget(description);
  m_managed=new QCheckBox("Enable UCC lightbar control");m_managed->setObjectName("lightbarManaged");layout->addWidget(m_managed);
  auto *card=FluentTheme::createCard();auto *cardLayout=new QVBoxLayout(card);
  auto *tabs=new QTabWidget;tabs->setObjectName("lightbarModes");cardLayout->addWidget(tabs);layout->addWidget(card);
  for(int i=0;i<3;++i) {
    auto &mode=m_modes[static_cast<size_t>(i)];auto *page=new QWidget;auto *form=new QFormLayout(page);form->setContentsMargins(16,20,16,20);form->setSpacing(20);
    mode.enabled=new QCheckBox("Enable in this mode");mode.enabled->setObjectName("lightbarEnabled_"+modes[i]);form->addRow(mode.enabled);
    mode.animation=new Selector;mode.animation->addItems(animations);mode.animation->setObjectName("lightbarAnimation_"+modes[i]);form->addRow("Notification animation",mode.animation);
    mode.brightness=new Brightness;mode.brightness->setRange(0,255);mode.brightness->setObjectName("lightbarBrightness_"+modes[i]);form->addRow("Brightness (0–255)",mode.brightness);
    mode.color=new Selector;mode.color->setObjectName("lightbarColor_"+modes[i]);mode.color->addItem("Auto (majority keyboard color)","Auto");
    const QStringList names={"Red","Green","Blue","White","Cyan","Magenta","Yellow"};
    const QList<QColor> colors={Qt::red,Qt::green,Qt::blue,Qt::white,Qt::cyan,Qt::magenta,Qt::yellow};
    for(int n=0;n<names.size();++n) mode.color->addItem(names[n],colors[n].name());
    mode.color->addItem("Custom…","custom");form->addRow("Color",mode.color);
    mode.custom=new QPushButton("Choose custom color");form->addRow(QString(),mode.custom);
    connect(mode.custom,&QPushButton::clicked,this,[this,i] {
      auto &current=m_modes[static_cast<size_t>(i)];const auto picked=QColorDialog::getColor(current.rgb,this,"Lightbar color");
      if(picked.isValid()) {current.rgb=picked;current.custom->setText("Custom: "+picked.name());current.color->setCurrentIndex(current.color->findData("custom"));}
    });
    tabs->addTab(page,QStringList{"AC","USB-C","Battery"}[i]);
  }
  m_silent=new QCheckBox("Silent mode — suppress notification animations");m_silent->setObjectName("lightbarSilent");layout->addWidget(m_silent);
  m_welcome=new QCheckBox("Welcome animation when the daemon starts");m_welcome->setObjectName("lightbarWelcome");layout->addWidget(m_welcome);
  auto *buttons=new QHBoxLayout;auto *reloadButton=new QPushButton("Reload");buttons->addWidget(reloadButton);buttons->addStretch();
  m_test=new QPushButton("Test saved animation");m_test->setObjectName("testLightbar");buttons->addWidget(m_test);
  m_save=new QPushButton("Save and apply");m_save->setObjectName("saveLightbar");m_save->setProperty("primary",true);buttons->addWidget(m_save);layout->addLayout(buttons);
  auto *hint=new QLabel("Test uses the saved settings for the current effective power mode and bypasses Silent mode. A disabled mode stays off. Flash ends off; the other animations restore the selected color.");
  hint->setWordWrap(true);hint->setObjectName("muted");layout->addWidget(hint);
  m_status=new QLabel;m_status->setObjectName("lightbarStatus");m_status->setWordWrap(true);layout->addWidget(m_status);layout->addStretch();
  connect(reloadButton,&QPushButton::clicked,this,&LightbarTab::reload);
  connect(m_save,&QPushButton::clicked,this,&LightbarTab::save);
  connect(m_test,&QPushButton::clicked,this,[this] {
    if(readOnlyPreview) return;
    emit message(m_client->triggerLightbarAnimation(true) ? "Lightbar animation started" : "Animation did not start; check hardware, control enable and the current power mode");showStatus();
  });
  connect(m_client,&UccdClient::connectionStatusChanged,this,[this](bool){reload();});
  connect(m_client,&UccdClient::lightbarSettingsChanged,this,[this](const QString &){reload();});
  connect(m_client,&UccdClient::powerPolicyChanged,this,[this](const QString &){if(isVisible()) showStatus();});
  reload();
}
void LightbarTab::reload() {
  const auto json=m_client->getLightbarSettingsJSON();
  const auto doc=json ? QJsonDocument::fromJson(QByteArray::fromStdString(*json)) : QJsonDocument();
  const bool available=doc.isObject() && doc.object()["power_settings"].isObject();
  m_save->setEnabled(available && !readOnlyPreview);m_test->setEnabled(available && !readOnlyPreview);
  m_managed->setEnabled(available && !readOnlyPreview);m_silent->setEnabled(available && !readOnlyPreview);m_welcome->setEnabled(available && !readOnlyPreview);
  if(!available) {m_status->setText("Lightbar controls require the updated UCC daemon.");return;}
  m_settings=doc.object();m_managed->setChecked(m_settings["managed"].toBool());m_silent->setChecked(m_settings["silent_mode"].toBool());m_welcome->setChecked(m_settings["welcome_anim"].toBool());
  for(int i=0;i<3;++i) {
    auto &mode=m_modes[static_cast<size_t>(i)];const auto source=m_settings["power_settings"].toObject()[modes[i]].toObject();
    mode.enabled->setChecked(source["enabled"].toBool());mode.brightness->setValue(source["brightness_val"].toInt());mode.animation->setCurrentIndex(qMax(0,mode.animation->findText(source["mode"].toString())));
    const auto rgb=source["color_mode"].toArray();
    if(rgb.size()==3) {mode.rgb=QColor(rgb[0].toInt(),rgb[1].toInt(),rgb[2].toInt());const int preset=mode.color->findData(mode.rgb.name());mode.color->setCurrentIndex(preset<0 ? mode.color->findData("custom") : preset);}
    else mode.color->setCurrentIndex(0);
    mode.custom->setText("Custom: "+mode.rgb.name());
  }
  showStatus();
}
QJsonObject LightbarTab::collect() const {
  auto settings=m_settings;settings["managed"]=m_managed->isChecked();settings["silent_mode"]=m_silent->isChecked();settings["welcome_anim"]=m_welcome->isChecked();
  auto profiles=settings["power_settings"].toObject();
  for(int i=0;i<3;++i) {
    const auto &mode=m_modes[static_cast<size_t>(i)];auto p=profiles[modes[i]].toObject();
    p["enabled"]=mode.enabled->isChecked();p["brightness_val"]=mode.brightness->value();p["mode"]=mode.animation->currentText();
    const auto selected=mode.color->currentData().toString();
    if(selected=="Auto") p["color_mode"]="Auto";
    else {const auto color=selected=="custom" ? mode.rgb : QColor(selected);p["color_mode"]=QJsonArray{color.red(),color.green(),color.blue()};}
    profiles[modes[i]]=p;
  }
  settings["power_settings"]=profiles;return settings;
}
void LightbarTab::save() {
  if(readOnlyPreview) return;
  if(m_client->setLightbarSettingsJSON(QJsonDocument(collect()).toJson(QJsonDocument::Compact).toStdString())) {emit message("Lightbar settings saved and applied");reload();}
  else emit message("Lightbar settings could not be saved");
}
void LightbarTab::showStatus() {
  const auto json=m_client->getLightbarStatusJSON();
  const auto status=json ? QJsonDocument::fromJson(QByteArray::fromStdString(*json)).object() : QJsonObject();
  if(status.isEmpty()) {m_status->setText("Lightbar status unavailable");return;}
  if(!status["available"].toBool()) {m_status->setText("Lightbar hardware not detected. Settings can still be saved.");return;}
  m_status->setText(QString("%1 zones · Hardware brightness 0–%2 · Mode: %3 · %4").arg(status["zones"].toInt()).arg(status["maxBrightness"].toInt()).arg(status["mode"].toString(),status["managed"].toBool() ? "UCC control enabled" : "UCC control disabled"));
  if(!status["ok"].toBool(true)) m_status->setText(m_status->text()+" · "+status["error"].toString());
}
}
