// SPDX-License-Identifier: GPL-3.0-or-later
#include "PowerDevicesTab.hpp"
#include "FluentTheme.hpp"
#include "PreviewMode.hpp"
#include <QVBoxLayout>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QSpinBox>
#include <QComboBox>
#include <QJsonDocument>
#include <QScrollArea>
#include <QWheelEvent>

namespace ucc {
namespace {
const QStringList modes={"ac","type_c","bat"};
class ValueSpin : public QSpinBox {
public:
  using QSpinBox::QSpinBox;
protected:
  void wheelEvent(QWheelEvent *event) override {event->ignore();}
};
class ValueSelector : public QComboBox {
public:
  using QComboBox::QComboBox;
protected:
  void wheelEvent(QWheelEvent *event) override {event->ignore();}
};
QComboBox *selector(const QStringList &values,const QString &selected,bool allowDefault=false) {
  auto *combo=new ValueSelector;
  if(allowDefault) combo->addItem("Profile default",QString());
  for(const auto &value:values) combo->addItem(value,value);
  int index=combo->findData(selected);
  if(index<0 && !selected.isEmpty()) {combo->addItem(selected,selected);index=combo->count()-1;}
  combo->setCurrentIndex(qMax(0,index));return combo;
}
void tableStyle(QTableWidget *table) {
  table->verticalHeader()->hide();table->setAlternatingRowColors(true);
  table->setSelectionMode(QAbstractItemView::NoSelection);
  table->setEditTriggers(QAbstractItemView::NoEditTriggers);
  table->setWordWrap(false);
}
}
PowerDevicesTab::PowerDevicesTab(UccdClient *client,QWidget *parent) : QWidget(parent),m_client(client) {
  auto *layout=new QVBoxLayout(this);layout->setContentsMargins(0,0,0,0);layout->setSpacing(12);
  auto *description=new QLabel("Choose each device's behavior on AC, USB-C power and battery. These policies follow the effective power mode, including manual overrides.");
  description->setWordWrap(true);description->setObjectName("muted");layout->addWidget(description);
  m_enabled=new QCheckBox("Enable per-mode CPU and device policies");m_enabled->setObjectName("powerDevicesEnabled");
  m_enabled->setEnabled(!readOnlyPreview);layout->addWidget(m_enabled);
  auto *cpuCard=FluentTheme::createCard();auto *cpuLayout=new QVBoxLayout(cpuCard);
  auto *title=new QLabel("CPU and PCIe power policy");title->setObjectName("cardTitle");cpuLayout->addWidget(title);
  m_cpu=new QTableWidget(3,4);m_cpu->setObjectName("cpuPowerPolicyTable");
  m_cpu->setHorizontalHeaderLabels({"Mode","Governor","Energy preference","PCIe ASPM"});tableStyle(m_cpu);
  m_cpu->horizontalHeader()->setSectionResizeMode(QHeaderView::Stretch);
  m_cpu->setMinimumHeight(148);m_cpu->setMaximumHeight(168);cpuLayout->addWidget(m_cpu);layout->addWidget(cpuCard);
  m_nvidia=new QCheckBox("Keep NVIDIA awake (Blackwell workaround)");m_nvidia->setObjectName("keepNvidiaAwake");
  m_nvidia->setToolTip("Keeps the NVIDIA display controller in D0 and enables persistence. Overrides its PCI runtime PM auto value.");
  m_nvidia->setEnabled(!readOnlyPreview);layout->addWidget(m_nvidia);
  auto *toolbar=new QHBoxLayout;
  m_search=new QLineEdit;m_search->setPlaceholderText("Filter devices by name or path");toolbar->addWidget(m_search,1);
  auto *reloadButton=new QPushButton("Reload");toolbar->addWidget(reloadButton);
  m_scan=new QPushButton("Re-scan hardware");m_scan->setObjectName("scanPowerDevices");toolbar->addWidget(m_scan);
  m_save=new QPushButton("Save and apply");m_save->setObjectName("savePowerDevices");m_save->setProperty("primary",true);toolbar->addWidget(m_save);layout->addLayout(toolbar);
  m_devices=new QTableWidget(0,5);m_devices->setObjectName("powerDeviceTable");
  m_devices->setHorizontalHeaderLabels({"Device","AC","Type-C","Battery","Last application"});tableStyle(m_devices);
  m_devices->horizontalHeader()->setSectionResizeMode(0,QHeaderView::Stretch);
  for(int column=1;column<5;++column) m_devices->horizontalHeader()->setSectionResizeMode(column,QHeaderView::ResizeToContents);
  layout->addWidget(m_devices,1);
  m_status=new QLabel;m_status->setObjectName("powerDevicesStatus");m_status->setWordWrap(true);layout->addWidget(m_status);
  connect(m_search,&QLineEdit::textChanged,this,[this]{filter();});
  connect(reloadButton,&QPushButton::clicked,this,&PowerDevicesTab::reload);
  connect(m_save,&QPushButton::clicked,this,&PowerDevicesTab::save);
  connect(m_scan,&QPushButton::clicked,this,&PowerDevicesTab::scan);
  connect(m_client,&UccdClient::powerDeviceStatusChanged,this,&PowerDevicesTab::showStatus);
  connect(m_client,&UccdClient::connectionStatusChanged,this,[this](bool connected){
    if(connected) reload();else {m_save->setEnabled(false);m_scan->setEnabled(false);m_status->setText("Service unavailable");}
  });
  reload();
}
void PowerDevicesTab::reload() {
  const auto json=m_client->getPowerDeviceSettingsJSON();
  const auto document=json ? QJsonDocument::fromJson(QByteArray::fromStdString(*json)) : QJsonDocument();
  const bool available=document.isObject();
  m_save->setEnabled(available && !readOnlyPreview);m_scan->setEnabled(available && !readOnlyPreview);
  if(!available) {m_status->setText("Per-device controls require the updated UCC daemon.");return;}
  m_settings=document.object();m_enabled->setChecked(m_settings["enabled"].toBool(false));
  m_nvidia->setChecked(m_settings["keepNvidiaAwake"].toBool(false));
  const auto cpu=m_settings["cpu"].toObject();
  for(int row=0;row<3;++row) {
    m_cpu->setItem(row,0,new QTableWidgetItem(QStringList{"AC","Type-C","Battery"}[row]));
    const auto policy=cpu[modes[row]].toObject();
    m_cpu->setCellWidget(row,1,selector({"performance","powersave","ondemand","conservative","schedutil","userspace"},policy["governor"].toString(),true));
    m_cpu->setCellWidget(row,2,selector({"performance","balance_performance","balance_power","power","default"},policy["epp"].toString(),true));
    m_cpu->setCellWidget(row,3,selector({"default","performance","powersave","powersupersave"},policy["aspm"].toString(),true));
  }
  m_entries=m_settings["devices"].toArray();m_devices->setRowCount(0);m_devices->setRowCount(static_cast<int>(m_entries.size()));
  for(int row=0;row<m_entries.size();++row) {
    const auto entry=m_entries[row].toObject();const auto type=entry["type"].toString(),path=entry["path"].toString();
    auto *label=new QTableWidgetItem(entry["name"].toString());label->setToolTip(type.toUpper()+" · "+path);m_devices->setItem(row,0,label);
    for(int column=1;column<=3;++column) {
      const auto value=entry[modes[column-1]+"_val"].toString();
      QStringList values;
      if(type=="pci" || type=="usb") values={"on","auto"};
      else if(type=="wifi") values={"off","on"};
      else if(type=="service") values={"active","inactive"};
      const bool turbo=path.endsWith("/no_turbo"), watchdog=path.endsWith("/nmi_watchdog");
      if(turbo || watchdog) values={"0","1"};
      if(!values.isEmpty()) {
        auto *combo=selector(values,value);
        if(type=="pci" || type=="usb") {combo->setItemText(0,"Always on");combo->setItemText(1,"Power save (auto)");}
        else if(type=="wifi") {combo->setItemText(0,"Max performance");combo->setItemText(1,"Power save");}
        else if(type=="service") {combo->setItemText(0,"Enabled");combo->setItemText(1,"Disabled");}
        else if(turbo) {combo->setItemText(0,"Enabled");combo->setItemText(1,"Disabled");}
        else if(watchdog) {combo->setItemText(0,"Disabled");combo->setItemText(1,"Enabled");}
        m_devices->setCellWidget(row,column,combo);
      }
      else {
        auto *spin=new ValueSpin;spin->setRange(0,path.contains("dirty_writeback") ? 60000 : path.contains("power_save") ? 3600 : 1);
        if(path.contains("dirty_writeback")) spin->setSuffix(" cs");
        else if(path.contains("power_save")) {spin->setSuffix(" s");spin->setSpecialValueText("Off");}
        spin->setValue(value.toInt());m_devices->setCellWidget(row,column,spin);
      }
    }
    m_devices->setItem(row,4,new QTableWidgetItem("—"));
  }
  filter();
  if(auto status=m_client->getPowerDeviceStatusJSON()) showStatus(QString::fromStdString(*status));
}
QJsonObject PowerDevicesTab::collect() const {
  auto result=m_settings;result["enabled"]=m_enabled->isChecked();result["keepNvidiaAwake"]=m_nvidia->isChecked();
  auto cpu=result["cpu"].toObject();
  for(int row=0;row<3;++row) {
    auto policy=cpu[modes[row]].toObject();
    for(int column=1;column<=3;++column)
      policy[QStringList{"governor","epp","aspm"}[column-1]]=qobject_cast<QComboBox *>(m_cpu->cellWidget(row,column))->currentData().toString();
    cpu[modes[row]]=policy;
  }
  result["cpu"]=cpu;auto entries=m_entries;
  for(int row=0;row<entries.size();++row) {
    auto entry=entries[row].toObject();
    for(int column=1;column<=3;++column) {
      auto *widget=m_devices->cellWidget(row,column);
      const QString value=qobject_cast<QComboBox *>(widget) ? qobject_cast<QComboBox *>(widget)->currentData().toString() : QString::number(qobject_cast<QSpinBox *>(widget)->value());
      entry[modes[column-1]+"_val"]=value;
    }
    entries[row]=entry;
  }
  result["devices"]=entries;return result;
}
void PowerDevicesTab::save() {
  if(readOnlyPreview) return;
  const auto json=QJsonDocument(collect()).toJson(QJsonDocument::Compact).toStdString();
  if(m_client->setPowerDeviceSettingsJSON(json)) {emit message("Power policies saved; applying to the current mode");reload();}
  else emit message("Power policies could not be saved; check values and daemon logs");
}
void PowerDevicesTab::scan() {
  if(readOnlyPreview) return;
  if(m_client->scanPowerDevices()) {emit message("Hardware scan complete; existing per-mode settings preserved");reload();}
  else emit message("Hardware scan failed");
}
void PowerDevicesTab::filter() {
  const auto query=m_search->text();
  for(int row=0;row<m_entries.size();++row) {
    const auto entry=m_entries[row].toObject();
    m_devices->setRowHidden(row,!(entry["name"].toString()+" "+entry["path"].toString()).contains(query,Qt::CaseInsensitive));
  }
}
void PowerDevicesTab::showStatus(const QString &json) {
  const auto status=QJsonDocument::fromJson(json.toUtf8()).object();
  if(status.isEmpty()) {m_status->setText("No application results yet");return;}
  m_status->setText(status["enabled"].toBool() ? "Last applied mode: "+status["mode"].toString() : "Per-mode device policies are disabled");
  const auto cpu=status["cpu"].toObject();
  if(!cpu["governor"].toString().isEmpty()) m_status->setText(m_status->text()+" · CPU: "+cpu["governor"].toString()+" / "+cpu["epp"].toString());
  if(status.contains("aspmApplied") && !status["aspmApplied"].toBool()) m_status->setText(m_status->text()+" · PCIe ASPM could not be applied");
  for(const auto &value:status["devices"].toArray()) {
    const auto result=value.toObject();
    for(int row=0;row<m_entries.size();++row) if(m_entries[row].toObject()["path"]==result["path"]) {
      auto *item=m_devices->item(row,4);if(!item) continue;
      item->setText(result["ok"].toBool() ? "Applied: "+result["value"].toString() : "Unavailable / failed");
      item->setToolTip(result["guarded"].toBool() ? "NVIDIA D0 workaround overrides auto" : result["path"].toString());
    }
  }
}
}
