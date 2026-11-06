// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include "AsyncRead.hpp"
#include "PreviewMode.hpp"
#include <QWidget>
#include <QComboBox>
#include <QCheckBox>
#include <QPushButton>
#include <QLabel>
#include <QVBoxLayout>
#include <QGridLayout>
#include <QJsonDocument>
#include <QJsonArray>
#include <QJsonObject>
#include <QSignalBlocker>
#include <QMap>
#include <QSet>
#include <QResizeEvent>
#include <algorithm>

namespace ucc {
class CpuCoreSelectionWidget : public QWidget {
  Q_OBJECT
public:
  explicit CpuCoreSelectionWidget(QWidget* parent=nullptr):QWidget(parent) {
    setObjectName("cpuCoreSelection");setSizePolicy(QSizePolicy::Preferred,QSizePolicy::Fixed);
    auto* layout=new QVBoxLayout(this);layout->setContentsMargins(0,0,0,0);layout->setSpacing(10);layout->setSizeConstraint(QLayout::SetMinimumSize);
    m_mode=new QComboBox;m_mode->setObjectName("cpuCoreSelectionMode");
    m_mode->addItem("By number of cores",false);m_mode->addItem("Choose individual CPUs",true);layout->addWidget(m_mode);
    m_panel=new QWidget;auto* panelLayout=new QVBoxLayout(m_panel);panelLayout->setContentsMargins(0,0,0,0);panelLayout->setSizeConstraint(QLayout::SetMinimumSize);
    auto* hint=new QLabel("Checked CPUs stay enabled. CPUs locked by the kernel cannot be disabled.");hint->setWordWrap(true);hint->setObjectName("muted");panelLayout->addWidget(hint);
    auto* buttons=new QHBoxLayout;
    for(const auto& kind:QStringList{"All","P","E"}) {
      auto* button=new QPushButton(kind=="All" ? "All CPUs" : kind+"-cores");button->setObjectName("select"+kind+"Cpus");buttons->addWidget(button);
      m_presets[kind]=button;connect(button,&QPushButton::clicked,this,[this,kind]{selectKind(kind);});
    }
    buttons->addStretch();panelLayout->addLayout(buttons);
    auto* gridHost=new QWidget;m_grid=new QGridLayout(gridHost);m_grid->setContentsMargins(0,0,0,0);panelLayout->addWidget(gridHost);
    m_summary=new QLabel;m_summary->setWordWrap(true);panelLayout->addWidget(m_summary);layout->addWidget(m_panel);
    m_status=new QLabel("Loading CPU types…");m_status->setWordWrap(true);m_status->setObjectName("muted");layout->addWidget(m_status);
    connect(m_mode,&QComboBox::currentIndexChanged,this,[this]{updateControls();if(!m_loading) emit selectionChanged();});
    updateControls();
    readUccdAsync(this,"GetCpuTopologyJSON",{},[this](std::optional<QVariant> reply){
      setTopology(reply ? QJsonDocument::fromJson(reply->toString().toUtf8()).array() : QJsonArray{});
    });
  }
  bool individual() const {return m_mode->currentData().toBool();}
  bool hasHeightForWidth() const override {return false;}
  void setEditable(bool editable) {m_editable=editable && !readOnlyPreview;updateControls();}
  void setProfile(const QJsonObject& cpu) {
    m_loaded=cpu;m_loading=true;
    if(m_available) for(const auto& id:cpu["enabledCores"].toArray()) if(!m_checks.contains(id.toInt())) addCpu(id.toInt(),"missing",false,{});
    m_mode->setCurrentIndex(cpu["enabledCores"].isArray() ? 1 : 0);
    applyLoadedSelection();m_loading=false;updateControls();
  }
  void setLegacyCount(int count) {
    if(individual()) return;
    m_loaded.remove("enabledCores");m_loaded["onlineCores"]=count;m_loading=true;applyLoadedSelection();m_loading=false;
  }
  void writeProfile(QJsonObject& cpu) const {
    if(individual()) {
      QJsonArray ids;for(auto it=m_checks.cbegin();it!=m_checks.cend();++it) if(it.value()->isChecked()) ids.append(it.key());
      // Keep the original mask intact until topology information is available.
      if(ids.isEmpty()) ids=m_loaded["enabledCores"].toArray();
      cpu["enabledCores"]=ids;cpu["onlineCores"]=ids.size();
    } else cpu.remove("enabledCores");
  }
signals:
  void selectionChanged();
  void contentHeightChanged(int height);
protected:
  void resizeEvent(QResizeEvent* event) override {
    QWidget::resizeEvent(event);
    emit contentHeightChanged(event->size().height());
  }
private:
  void setTopology(const QJsonArray& topology) {
    m_loading=true;
    for(const auto& value:topology) {
      const auto row=value.toObject();const int id=row["id"].toInt(-1);if(id<0 || m_checks.contains(id)) continue;
      addCpu(id,row["type"].toString(),!row["hotpluggable"].toBool(),row);
    }
    // Retain saved IDs from a different topology until the user removes them.
    for(const auto& id:m_loaded["enabledCores"].toArray()) if(!m_checks.contains(id.toInt())) addCpu(id.toInt(),"missing",false,{});
    m_available=!topology.isEmpty();applyLoadedSelection();m_loading=false;updateControls();
    m_status->setText(m_available ? "P = performance core · E = efficient core" : "Individual CPU selection requires CPU topology from the updated daemon.");
  }
  void addCpu(int id,const QString& type,bool mandatory,const QJsonObject& info) {
    auto* check=new QCheckBox(QString("CPU %1%2").arg(id).arg(type=="P" || type=="E" ? " · "+type : type=="missing" ? " (unavailable)" : ""));
    check->setObjectName(QString("enabledCpu%1").arg(id));check->setProperty("cpuId",id);
    check->setToolTip(mandatory ? "This CPU cannot be disabled by the kernel." : QString("Physical core %1 · sibling CPUs: %2").arg(info["physicalCore"].toInt(id)).arg(info["siblings"].toString()));
    m_checks[id]=check;m_types[id]=type;if(mandatory) m_mandatory.insert(id);
    const int pos=static_cast<int>(m_checks.size())-1;m_grid->addWidget(check,pos/4,pos%4);
    connect(check,&QCheckBox::toggled,this,[this,check](bool selected){
      if(m_loading) return;
      if(!selected && std::none_of(m_checks.cbegin(),m_checks.cend(),[](auto* c){return c->isChecked();})) {QSignalBlocker guard(check);check->setChecked(true);}
      updateSummary();emit selectionChanged();
    });
  }
  void applyLoadedSelection() {
    QSet<int> ids;const auto mask=m_loaded["enabledCores"].toArray();for(const auto& id:mask) ids.insert(id.toInt());
    const int count=m_loaded["onlineCores"].toInt(static_cast<int>(m_checks.size()));int index=0;
    for(auto it=m_checks.begin();it!=m_checks.end();++it,++index) {
      const QSignalBlocker guard(it.value());
      it.value()->setChecked(m_mandatory.contains(it.key()) || (m_loaded["enabledCores"].isArray() ? ids.contains(it.key()) : count<0 || index<count));
    }
    if(!m_checks.isEmpty() && std::none_of(m_checks.cbegin(),m_checks.cend(),[](auto* c){return c->isChecked();})) m_checks.first()->setChecked(true);
    updateSummary();
  }
  void selectKind(const QString& kind) {
    m_loading=true;
    for(auto it=m_checks.begin();it!=m_checks.end();++it) it.value()->setChecked(m_mandatory.contains(it.key()) || (kind=="All" && m_types[it.key()]!="missing") || m_types[it.key()]==kind);
    if(!m_checks.isEmpty() && std::none_of(m_checks.cbegin(),m_checks.cend(),[](auto* c){return c->isChecked();})) m_checks.first()->setChecked(true);
    m_loading=false;updateSummary();emit selectionChanged();
  }
  void updateSummary() {
    int count=0,p=0,e=0;
    for(auto it=m_checks.cbegin();it!=m_checks.cend();++it) if(it.value()->isChecked()) {++count;p+=m_types[it.key()]=="P";e+=m_types[it.key()]=="E";}
    m_summary->setText(QString("Enabled: %1 CPUs · %2 P / %3 E").arg(count).arg(p).arg(e));
  }
  void updateControls() {
    m_panel->setVisible(individual());m_mode->setEnabled(m_editable && m_available);
    for(auto it=m_checks.begin();it!=m_checks.end();++it) it.value()->setEnabled(m_editable && !m_mandatory.contains(it.key()));
    for(auto it=m_presets.begin();it!=m_presets.end();++it) it.value()->setEnabled(m_editable && m_available && (it.key()=="All" || m_types.values().contains(it.key())));
    layout()->invalidate();
    setMinimumHeight(layout()->totalSizeHint().height());
    updateGeometry();
    emit contentHeightChanged(minimumHeight());
  }
  QComboBox* m_mode;
  QWidget* m_panel;
  QGridLayout* m_grid;
  QLabel *m_summary,*m_status;
  QMap<int,QCheckBox*> m_checks;
  QMap<int,QString> m_types;
  QMap<QString,QPushButton*> m_presets;
  QSet<int> m_mandatory;
  QJsonObject m_loaded;
  bool m_loading=false,m_editable=false,m_available=false;
};
}
