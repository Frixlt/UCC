// SPDX-License-Identifier: GPL-3.0-or-later
#include "SessionProcessesWidget.hpp"
#include "PreviewMode.hpp"
#include <QVBoxLayout>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QMessageBox>
#include <QShowEvent>
#include <QHideEvent>
#include <QSet>
#include <QSignalBlocker>
namespace ucc {
SessionProcessesWidget::SessionProcessesWidget(SessionPaths paths,DesktopMonitor *monitor,QWidget *parent)
  :QWidget(parent),m_monitor(monitor ? monitor : new DesktopMonitor(false,paths,this)),
   m_job(new ProcessTerminator(paths,this)),m_ownsMonitor(!monitor) {
  auto *layout=new QVBoxLayout(this);layout->setContentsMargins(0,0,0,0);layout->setSpacing(16);
  m_battery=new QLabel("Battery data unavailable");m_battery->setObjectName("sessionBattery");m_battery->setWordWrap(true);layout->addWidget(m_battery);
  m_gpu=new QLabel("dGPU: unknown");m_gpu->setObjectName("sessionGpuState");layout->addWidget(m_gpu);
  auto *hint=new QLabel("Only your visible NVIDIA processes are listed. Battery power is the measured charge or discharge rate. Sleeping GPU checks do not wake the GPU.");hint->setWordWrap(true);hint->setObjectName("muted");layout->addWidget(hint);
  m_table=new QTableWidget(0,2);m_table->setObjectName("sessionGpuProcesses");m_table->setHorizontalHeaderLabels({"PID","Process"});m_table->verticalHeader()->hide();m_table->horizontalHeader()->setSectionResizeMode(0,QHeaderView::ResizeToContents);m_table->horizontalHeader()->setSectionResizeMode(1,QHeaderView::Stretch);m_table->setEditTriggers(QAbstractItemView::NoEditTriggers);m_table->setSelectionBehavior(QAbstractItemView::SelectRows);m_table->setSelectionMode(QAbstractItemView::ExtendedSelection);m_table->setAlternatingRowColors(true);layout->addWidget(m_table,1);
  auto *buttons=new QHBoxLayout;m_refresh=new QPushButton("Refresh");buttons->addWidget(m_refresh);buttons->addStretch();m_selected=new QPushButton("Terminate selected…");m_selected->setObjectName("terminateGpuSelected");buttons->addWidget(m_selected);m_all=new QPushButton("Terminate all listed…");m_all->setObjectName("terminateGpuAll");buttons->addWidget(m_all);layout->addLayout(buttons);
  m_status=new QLabel("Select a process to manage it");m_status->setObjectName("sessionProcessStatus");m_status->setWordWrap(true);layout->addWidget(m_status);
  connect(m_monitor,&DesktopMonitor::updated,this,&SessionProcessesWidget::setSnapshot);
  connect(m_refresh,&QPushButton::clicked,m_monitor,&DesktopMonitor::refresh);
  connect(m_table,&QTableWidget::itemSelectionChanged,this,&SessionProcessesWidget::updateActions);
  connect(m_selected,&QPushButton::clicked,this,[this]{confirmTermination(selected());});
  connect(m_all,&QPushButton::clicked,this,[this]{QJsonArray all;for(int row=0;row<m_table->rowCount();++row) all.append(m_table->item(row,0)->data(Qt::UserRole).toJsonObject());confirmTermination(all);});
  connect(m_job,&ProcessTerminator::finished,this,[this](const QString &text){m_status->setText(text);emit message(text);m_table->setEnabled(true);updateActions();m_monitor->refresh();});
  updateActions();if(!m_monitor->snapshot().isEmpty()) setSnapshot(m_monitor->snapshot());
}
void SessionProcessesWidget::setSnapshot(const QJsonObject &snapshot) {
  m_battery->setText(batterySummary(snapshot));m_gpu->setText("dGPU: "+snapshot["gpuState"].toString("unknown"));
  if(!snapshot["processesSampled"].toBool()) return;
  QSet<QString> selection;for(const auto &item:selected()) {const auto p=item.toObject();selection.insert(QString::number(p["pid"].toInteger())+":"+p["startTime"].toString());}
  const QSignalBlocker blocker(m_table);const auto processes=snapshot["processes"].toArray();m_table->setRowCount(0);m_table->setRowCount(static_cast<int>(processes.size()));
  for(int row=0;row<processes.size();++row) {
    const auto p=processes[row].toObject();auto *pid=new QTableWidgetItem(QString::number(p["pid"].toInteger()));pid->setData(Qt::UserRole,p);m_table->setItem(row,0,pid);m_table->setItem(row,1,new QTableWidgetItem(p["name"].toString()));
    if(selection.contains(pid->text()+":"+p["startTime"].toString())) for(int column=0;column<2;++column) m_table->item(row,column)->setSelected(true);
  }
  if(!m_job->isRunning()) m_status->setText(snapshot["truncated"].toBool() ? "Showing the first 512 matching processes; refresh after terminating these" : processes.isEmpty() ? "No visible GPU processes" : QString("GPU processes: %1 · Refreshes every 2 seconds while visible").arg(processes.size()));
  updateActions();
}
QJsonArray SessionProcessesWidget::selected() const {
  QJsonArray result;for(const auto &index:m_table->selectionModel()->selectedRows()) if(auto *item=m_table->item(index.row(),0)) result.append(item->data(Qt::UserRole).toJsonObject());return result;
}
void SessionProcessesWidget::updateActions() {m_selected->setEnabled(!readOnlyPreview && !m_job->isRunning() && !selected().isEmpty());m_all->setEnabled(!readOnlyPreview && !m_job->isRunning() && m_table->rowCount()>0);}
void SessionProcessesWidget::confirmTermination(const QJsonArray &processes) {
  if(readOnlyPreview || m_job->isRunning() || processes.isEmpty()) return;
  QMessageBox box(QMessageBox::Warning,"Terminate GPU processes",QString("Terminate %1 selected process(es)?").arg(processes.size()),QMessageBox::Yes|QMessageBox::No,this);
  QStringList names;for(const auto &value:processes) {const auto p=value.toObject();names<<QString("%1 (PID %2)").arg(p["name"].toString()).arg(p["pid"].toInteger());}
  box.setTextFormat(Qt::PlainText);
  box.setInformativeText(names.join("\n")+"\n\nSIGTERM first; SIGKILL after 3 seconds if still running.");box.setDefaultButton(QMessageBox::No);
  if(box.exec()!=QMessageBox::Yes) return;
  if(m_job->start(processes)) {m_status->setText("Waiting for selected processes to exit…");m_table->setEnabled(false);updateActions();}
}
void SessionProcessesWidget::showEvent(QShowEvent *event) {QWidget::showEvent(event);if(m_ownsMonitor) m_monitor->setActive(true);emit activityChanged(true);}
void SessionProcessesWidget::hideEvent(QHideEvent *event) {QWidget::hideEvent(event);if(m_ownsMonitor) m_monitor->setActive(false);emit activityChanged(false);}
}
