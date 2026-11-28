// SPDX-License-Identifier: GPL-3.0-or-later
#include "LightbarTab.hpp"
#include "FluentTheme.hpp"
#include "PreviewMode.hpp"
#include "AsyncRead.hpp"
#include <QVBoxLayout>
#include <QHBoxLayout>
#include <QFormLayout>
#include <QTabWidget>
#include <QScrollArea>
#include <QJsonArray>
#include <QJsonDocument>
#include <QColorDialog>
#include <QWheelEvent>
namespace ucc {
namespace {
const QStringList modes={"ac","type_c","bat"};
const QStringList animations={"Pulse (stays on)","Pulse (stays off)","Flash (stays off)","Rainbow","Police Strobe","Disco"};
class Selector:public QComboBox {public:using QComboBox::QComboBox;protected:void wheelEvent(QWheelEvent *e) override {e->ignore();}};
class Brightness:public QSpinBox {public:using QSpinBox::QSpinBox;protected:void wheelEvent(QWheelEvent *e) override {e->ignore();}};
}
LightbarTab::LightbarTab(UccdClient *client,QWidget *parent):QWidget(parent),m_client(client) {
  auto *outer=new QVBoxLayout(this);outer->setContentsMargins(0,0,0,0);
  auto *scroll=new QScrollArea(this);scroll->setWidgetResizable(true);scroll->setFrameShape(QFrame::NoFrame);
  outer->addWidget(scroll);
  auto *body=new QWidget;scroll->setWidget(body);
  auto *layout=new QVBoxLayout(body);layout->setContentsMargins(0,0,0,0);layout->setSpacing(16);
  auto *description=new QLabel("Front lightbar settings follow the effective power mode. Notifications animate the lightbar while the UCC session agent is running.");
  description->setWordWrap(true);description->setObjectName("muted");layout->addWidget(description);
  m_managed=new QCheckBox("Enable UCC lightbar control");m_managed->setObjectName("lightbarManaged");layout->addWidget(m_managed);
  auto *card=FluentTheme::createCard();auto *cardLayout=new QVBoxLayout(card);
  auto *tabs=new QTabWidget;tabs->setObjectName("lightbarModes");cardLayout->addWidget(tabs);layout->addWidget(card);
  for(int i=0;i<3;++i) {
    auto &mode=m_modes[static_cast<size_t>(i)];auto *page=new QWidget;auto *form=new QFormLayout(page);form->setContentsMargins(16,20,16,20);form->setSpacing(20);
    mode.enabled=new QCheckBox("Enable in this mode");mode.enabled->setObjectName("lightbarEnabled_"+modes[i]);form->addRow(mode.enabled);
    mode.animation=new Selector;mode.animation->setSizeAdjustPolicy(QComboBox::AdjustToMinimumContentsLengthWithIcon);mode.animation->setMinimumContentsLength(12);mode.animation->addItems(animations);mode.animation->setObjectName("lightbarAnimation_"+modes[i]);form->addRow("Notification animation",mode.animation);
    mode.brightness=new Brightness;mode.brightness->setRange(0,255);mode.brightness->setObjectName("lightbarBrightness_"+modes[i]);form->addRow("Brightness (0–255)",mode.brightness);
    mode.color=new Selector;mode.color->setSizeAdjustPolicy(QComboBox::AdjustToMinimumContentsLengthWithIcon);mode.color->setMinimumContentsLength(12);mode.color->setObjectName("lightbarColor_"+modes[i]);mode.color->addItem("Auto (majority keyboard color)","Auto");
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
  auto *hint=new QLabel("Test uses the saved settings for the current effective power mode and bypasses Silent mode. A disabled mode stays off. Flash and Pulse (stays off) end off; the other animations restore the selected color.");
  hint->setWordWrap(true);hint->setObjectName("muted");layout->addWidget(hint);
  m_status=new QLabel;m_status->setObjectName("lightbarStatus");m_status->setWordWrap(true);layout->addWidget(m_status);layout->addStretch();
  connect(reloadButton,&QPushButton::clicked,this,&LightbarTab::reload);
  connect(m_save,&QPushButton::clicked,this,&LightbarTab::save);
  connect(m_test,&QPushButton::clicked,this,[this] {
    if(readOnlyPreview || !m_client) return;
    m_test->setEnabled(false);
    m_client->triggerLightbarAnimationAsync(true,this,[this](bool ok,const QString &) {
      m_test->setEnabled(!readOnlyPreview);
      emit message(ok ? "Lightbar animation started" : "Animation did not start; check hardware, control enable and the current power mode");
      showStatus();
    });
  });
  connect(m_client,&UccdClient::connectionStatusChanged,this,[this](bool connected){if(connected) reload(); else showStatus();});
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
  settings["power_settings"]=profiles;
  return settings;
}
void LightbarTab::save() {
  if(readOnlyPreview || !m_client) return;
  const auto payload=QJsonDocument(collect()).toJson(QJsonDocument::Compact).toStdString();
  m_save->setEnabled(false);
  m_test->setEnabled(false);
  m_status->setText("Saving lightbar settings...");
  m_client->setLightbarSettingsJSONAsync(payload,this,[this](bool ok,const QString &errMsg) {
    readUccdAsync(this,QStringLiteral("GetLightbarStatusJSON"),{},[this,ok,errMsg](std::optional<QVariant> val) {
      m_save->setEnabled(!readOnlyPreview);
      m_test->setEnabled(!readOnlyPreview);
      const auto status=val ? QJsonDocument::fromJson(val->toString().toUtf8()).object() : QJsonObject();
      if(ok) {
        if(!status.isEmpty() && !status["ok"].toBool(true) && status["available"].toBool(true)) {
          const QString err=status["error"].toString();
          emit message(err.isEmpty() ? QStringLiteral("Lightbar settings could not be applied") : QStringLiteral("Lightbar error: %1").arg(err));
        } else {
          emit message("Lightbar settings saved and applied");
        }
      } else {
        const QString err=status["error"].toString().isEmpty() ? errMsg : status["error"].toString();
        emit message(err.isEmpty() ? QStringLiteral("Lightbar settings could not be saved") : QStringLiteral("Lightbar settings could not be applied: %1").arg(err));
      }
      if(status.isEmpty()) {m_status->setText("Lightbar status unavailable");return;}
      if(!status["available"].toBool()) {m_status->setText("Lightbar hardware not detected. Settings can still be saved.");return;}
      m_status->setText(QString("%1 zones · Hardware brightness 0–%2 · Mode: %3 · %4").arg(status["zones"].toInt()).arg(status["maxBrightness"].toInt()).arg(status["mode"].toString(),status["managed"].toBool() ? "UCC control enabled" : "UCC control disabled"));
      if(!status["ok"].toBool(true)) m_status->setText(m_status->text()+" · "+status["error"].toString());
    });
  });
}
void LightbarTab::showStatus() {
  readUccdAsync(this,QStringLiteral("GetLightbarStatusJSON"),{},[this](std::optional<QVariant> val) {
    const auto status=val ? QJsonDocument::fromJson(val->toString().toUtf8()).object() : QJsonObject();
    if(status.isEmpty()) {m_save->setEnabled(false);m_test->setEnabled(false);m_managed->setEnabled(false);m_silent->setEnabled(false);m_welcome->setEnabled(false);m_status->setText("Lightbar status unavailable");return;}
    if(!status["available"].toBool()) {m_status->setText("Lightbar hardware not detected. Settings can still be saved.");return;}
    m_status->setText(QString("%1 zones · Hardware brightness 0–%2 · Mode: %3 · %4").arg(status["zones"].toInt()).arg(status["maxBrightness"].toInt()).arg(status["mode"].toString(),status["managed"].toBool() ? "UCC control enabled" : "UCC control disabled"));
    if(!status["ok"].toBool(true)) m_status->setText(m_status->text()+" · "+status["error"].toString());
  });
}

LogoTab::LogoTab(UccdClient *client,QWidget *parent):QWidget(parent),m_client(client) {
  setObjectName("logoLightingPage");
  auto *outer=new QVBoxLayout(this);outer->setContentsMargins(0,0,0,0);
  auto *scroll=new QScrollArea(this);scroll->setWidgetResizable(true);scroll->setFrameShape(QFrame::NoFrame);
  outer->addWidget(scroll);
  auto *body=new QWidget;scroll->setWidget(body);
  auto *layout=new QVBoxLayout(body);layout->setContentsMargins(0,0,0,0);layout->setSpacing(16);

  auto *description=new QLabel("Control the lid logo RGB LED independently of the front lightbar.");
  description->setWordWrap(true);description->setObjectName("muted");layout->addWidget(description);

  auto *card=FluentTheme::createCard();card->setProperty("role","lidLogoCard");
  auto *form=new QFormLayout(card);form->setContentsMargins(20,18,20,18);form->setSpacing(14);
  auto *title=new QLabel("Lid logo LED");title->setObjectName("cardTitle");form->addRow(title);
  m_enabled=new QCheckBox("Enable lid logo backlight");m_enabled->setObjectName("lidLogoEnabled");form->addRow(m_enabled);
  m_brightness=new Brightness;m_brightness->setRange(0,255);m_brightness->setValue(255);m_brightness->setObjectName("lidLogoBrightness");form->addRow("Brightness (0–255)",m_brightness);
  m_colorButton=new QPushButton("Color: #ffffff");m_colorButton->setObjectName("lidLogoColor");form->addRow("Color",m_colorButton);
  connect(m_colorButton,&QPushButton::clicked,this,[this] {
    if(readOnlyPreview) return;
    const auto picked=QColorDialog::getColor(m_rgb,this,"Lid logo color");
    if(picked.isValid()) {m_rgb=picked;m_colorButton->setText("Color: "+picked.name());}
  });
  layout->addWidget(card);

  auto *buttons=new QHBoxLayout;
  auto *reloadButton=new QPushButton("Reload");buttons->addWidget(reloadButton);buttons->addStretch();
  m_save=new QPushButton("Save and apply");m_save->setObjectName("saveLidLogo");m_save->setProperty("primary",true);buttons->addWidget(m_save);
  layout->addLayout(buttons);

  m_status=new QLabel;m_status->setObjectName("lidLogoStatus");m_status->setWordWrap(true);layout->addWidget(m_status);
  layout->addStretch(1);

  connect(reloadButton,&QPushButton::clicked,this,&LogoTab::reload);
  connect(m_save,&QPushButton::clicked,this,&LogoTab::save);
  connect(m_client,&UccdClient::connectionStatusChanged,this,[this](bool connected){if(connected) reload(); else showStatus();});
  connect(m_client,&UccdClient::lightbarSettingsChanged,this,[this](const QString &){reload();});
  reload();
}

void LogoTab::reload() {
  const auto json=m_client->getLightbarSettingsJSON();
  const auto doc=json ? QJsonDocument::fromJson(QByteArray::fromStdString(*json)) : QJsonDocument();
  const bool available=doc.isObject() && doc.object()["power_settings"].isObject();
  if(!available) {
    m_hasLidLogo=false;
    m_enabled->setEnabled(false);m_brightness->setEnabled(false);m_colorButton->setEnabled(false);m_save->setEnabled(false);
    m_status->setText("Lid logo controls require the updated UCC daemon.");
    return;
  }
  m_settings=doc.object();
  if(m_settings["lid_logo"].isObject()) {
    const auto logo=m_settings["lid_logo"].toObject();
    m_enabled->setChecked(logo["enabled"].toBool(false));
    m_brightness->setValue(qBound(0,logo["brightness"].toInt(255),255));
    const auto rgb=logo["color"].toArray();
    if(rgb.size()==3) m_rgb=QColor(qBound(0,rgb[0].toInt(255),255),qBound(0,rgb[1].toInt(255),255),qBound(0,rgb[2].toInt(255),255));
    else m_rgb=Qt::white;
  } else {
    m_enabled->setChecked(false);
    m_brightness->setValue(255);
    m_rgb=Qt::white;
  }
  m_colorButton->setText("Color: "+m_rgb.name());
  showStatus();
}

void LogoTab::save() {
  if(readOnlyPreview || !m_hasLidLogo || !m_client) return;
  QJsonObject logo;
  logo["enabled"]=m_enabled->isChecked();
  logo["brightness"]=m_brightness->value();
  logo["color"]=QJsonArray{m_rgb.red(),m_rgb.green(),m_rgb.blue()};
  m_save->setEnabled(false);
  m_status->setText("Saving lid logo settings...");
  readUccdAsync(this,QStringLiteral("GetLightbarSettingsJSON"),{},[this,logo](std::optional<QVariant> latestVal) {
    auto merged=m_settings;
    if(latestVal) {
      const auto latestDoc=QJsonDocument::fromJson(latestVal->toString().toUtf8());
      if(latestDoc.isObject() && latestDoc.object()["power_settings"].isObject()) merged=latestDoc.object();
    }
    merged["lid_logo"]=logo;
    const auto payload=QJsonDocument(merged).toJson(QJsonDocument::Compact).toStdString();
    m_client->setLightbarSettingsJSONAsync(payload,this,[this](bool ok,const QString &errMsg) {
      readUccdAsync(this,QStringLiteral("GetLightbarStatusJSON"),{},[this,ok,errMsg](std::optional<QVariant> val) {
        const auto status=val ? QJsonDocument::fromJson(val->toString().toUtf8()).object() : QJsonObject();
        m_save->setEnabled(m_hasLidLogo && !readOnlyPreview);
        if(ok) {
          emit message("Lid logo settings saved and applied");
        } else {
          const QString err=status["error"].toString().isEmpty() ? errMsg : status["error"].toString();
          emit message(err.isEmpty() ? QStringLiteral("Lid logo settings could not be applied") : QStringLiteral("Lid logo error: %1").arg(err));
        }
        if(status.isEmpty()) {
          m_hasLidLogo=false;
          m_enabled->setEnabled(false);m_brightness->setEnabled(false);m_colorButton->setEnabled(false);m_save->setEnabled(false);
          m_status->setText("Lid logo status unavailable");
          return;
        }
        m_hasLidLogo=status["hasLidLogo"].toBool();
        const bool interactive=m_hasLidLogo && !readOnlyPreview;
        m_enabled->setEnabled(interactive);m_brightness->setEnabled(interactive);m_colorButton->setEnabled(interactive);m_save->setEnabled(interactive);
        if(!m_hasLidLogo) {
          m_status->setText("Lid logo LED hardware not detected on this device.");
          return;
        }
        m_status->setText(m_enabled->isChecked() ? "Lid logo LED detected · Enabled" : "Lid logo LED detected · Disabled");
        if(!status["ok"].toBool(true)) m_status->setText(m_status->text()+" · "+status["error"].toString());
      });
    });
  });
}

void LogoTab::showStatus() {
  readUccdAsync(this,QStringLiteral("GetLightbarStatusJSON"),{},[this](std::optional<QVariant> val) {
    const auto status=val ? QJsonDocument::fromJson(val->toString().toUtf8()).object() : QJsonObject();
    if(status.isEmpty()) {
      m_hasLidLogo=false;
      m_enabled->setEnabled(false);m_brightness->setEnabled(false);m_colorButton->setEnabled(false);m_save->setEnabled(false);
      m_status->setText("Lid logo status unavailable");
      return;
    }
    m_hasLidLogo=status["hasLidLogo"].toBool();
    const bool interactive=m_hasLidLogo && !readOnlyPreview;
    m_enabled->setEnabled(interactive);m_brightness->setEnabled(interactive);m_colorButton->setEnabled(interactive);m_save->setEnabled(interactive);
    if(!m_hasLidLogo) {
      m_status->setText("Lid logo LED hardware not detected on this device.");
      return;
    }
    m_status->setText(m_enabled->isChecked() ? "Lid logo LED detected · Enabled" : "Lid logo LED detected · Disabled");
    if(!status["ok"].toBool(true)) m_status->setText(m_status->text()+" · "+status["error"].toString());
  });
}
}
