// SPDX-License-Identifier: GPL-3.0-or-later
#include "DesktopTray.hpp"
#include "PreviewMode.hpp"
#include "AsyncRead.hpp"
#include <QApplication>
#include <QJsonDocument>
#include <QJsonArray>
#include <QVBoxLayout>
#include <QPainter>
#include <QProcess>
#include <QDBusMessage>
#include <QDBusConnection>
#include <QDBusPendingCallWatcher>
#include <QDBusPendingReply>
#include <QTimer>
namespace ucc {
namespace {
QColor modeColor(const QString &mode) {return mode=="ac" ? QColor("#00e676") : mode=="type_c" ? QColor("#00b0ff") : mode=="bat" ? QColor("#ffea00") : QColor("#aaaaaa");}
}
DesktopTray::DesktopTray(UccdClient *client,SessionPaths paths,QObject *parent)
 :QObject(parent),m_client(client),m_paths(std::move(paths)),m_monitor(true,m_paths),m_icon(this),m_menu(new QMenu),m_gpuMenu(new QMenu("dGPU",m_menu)) {
  auto *open=m_menu->addAction("Control Center");connect(open,&QAction::triggered,this,&DesktopTray::openControlCenter);
  m_menu->addSeparator();m_battery=m_menu->addAction("Battery data unavailable");m_battery->setEnabled(false);
  m_menu->addMenu(m_gpuMenu);auto *manage=m_gpuMenu->addAction("Manage processes…");connect(manage,&QAction::triggered,this,&DesktopTray::showProcesses);
  m_killAll=m_gpuMenu->addAction("Terminate all GPU processes…");m_killAll->setEnabled(false);
  connect(m_killAll,&QAction::triggered,this,[this]{showProcesses();m_processWidget->confirmTermination(m_snapshot["processes"].toArray());});m_gpuMenu->addSeparator();
  for(int i=0;i<50;++i) {auto *action=m_gpuMenu->addAction(QString());action->setVisible(false);m_processActions<<action;connect(action,&QAction::triggered,this,[this,action]{showProcesses();m_processWidget->confirmTermination(QJsonArray{action->data().toJsonObject()});});}
  m_monitorAction=m_menu->addAction("Refresh open menu every 2 seconds");m_monitorAction->setCheckable(true);m_monitorAction->setChecked(false);connect(m_monitorAction,&QAction::toggled,this,[this]{updatePolling();});
  m_menu->addSeparator();m_status=m_menu->addAction("Service unavailable");m_status->setEnabled(false);
  auto *power=m_menu->addMenu("Power mode");m_modes=new QActionGroup(this);m_modes->setExclusive(true);m_modes->setEnabled(!readOnlyPreview && m_client->isConnected());
  for(const auto &mode:QStringList{"auto","ac","type_c","bat"}) {auto *action=power->addAction(mode=="auto" ? "Automatic" : mode=="ac" ? "AC · Performance" : mode=="type_c" ? "USB-C · Balanced" : "Battery · Power save");action->setData(mode);action->setCheckable(true);m_modes->addAction(action);}
  connect(m_modes,&QActionGroup::triggered,this,[this](QAction *action){
    if(readOnlyPreview) return;
    m_modes->setEnabled(false);
    readUccdAsync(this,"SetManualPowerMode",{action->data().toString()},[this](std::optional<QVariant> result){
      m_modes->setEnabled(m_client->isConnected() && !readOnlyPreview);
      if(!result || !result->toBool()) {m_icon.showMessage("UCC","Could not change power mode");loadPolicy();}
    });
  });
  m_menu->addSeparator();auto *quit=m_menu->addAction("Quit tray");connect(quit,&QAction::triggered,qApp,&QCoreApplication::quit);
  connect(m_menu,&QMenu::aboutToShow,this,[this]{m_menuVisible=true;updatePolling();m_monitor.refreshProcesses();loadPolicy();});
  connect(m_menu,&QMenu::aboutToHide,this,[this]{m_menuVisible=false;updatePolling();});
  connect(&m_monitor,&DesktopMonitor::updated,this,&DesktopTray::setSnapshot);
  connect(m_client,&UccdClient::powerPolicyChanged,this,&DesktopTray::setPolicy);
  connect(m_client,&UccdClient::profileChanged,this,[this](const QString &,const QString &,const QString &){loadPolicy();});
  connect(m_client,&UccdClient::connectionStatusChanged,this,[this](bool connected){if(connected) loadPolicy();else {++m_policyGeneration;m_policy={};m_status->setText("Service unavailable");m_modes->setEnabled(false);updateIcon();}});
  m_icon.setContextMenu(m_menu);connect(&m_icon,&QSystemTrayIcon::activated,this,[this](QSystemTrayIcon::ActivationReason reason){if(reason==QSystemTrayIcon::Trigger || reason==QSystemTrayIcon::DoubleClick) openControlCenter();});
  loadPolicy();updateIcon();m_icon.show();
}
DesktopTray::~DesktopTray() {m_icon.hide();if(m_dialog) delete m_dialog.data();delete m_menu;}
void DesktopTray::loadPolicy() {
  if(!m_client->isConnected()) return;
  if(m_policyLoading) {m_policyAgain=true;return;}
  m_policyLoading=true;const auto generation=m_policyGeneration;
  readUccdBatch(this,{"GetPowerPolicyJSON","GetActiveProfileJSON"},[this,generation](QVariantMap replies){
    m_policyLoading=false;if(!m_client->isConnected()) return;
    if(replies.contains("GetPowerPolicyJSON") && generation==m_policyGeneration) setPolicy(replies["GetPowerPolicyJSON"].toString());
    if(replies.contains("GetActiveProfileJSON")) m_profile=QJsonDocument::fromJson(replies["GetActiveProfileJSON"].toString().toUtf8()).object()["name"].toString();
    updateIcon();
    if(m_policyAgain) {m_policyAgain=false;loadPolicy();}
  });
}
void DesktopTray::setPolicy(const QString &json) {
  ++m_policyGeneration;
  m_policy=QJsonDocument::fromJson(json.toUtf8()).object();const auto manual=m_policy["manual_mode"].toString("auto");
  for(auto *action:m_modes->actions()) action->setChecked(action->data()==manual);
  m_modes->setEnabled(!readOnlyPreview && m_client->isConnected());
  m_status->setText(m_policy.isEmpty() ? "Power policy unavailable" : QString("Source: %1 · Mode: %2").arg(m_policy["real_power_source"].toString(),m_policy["current_mode"].toString()));updateIcon();
}
void DesktopTray::setSnapshot(const QJsonObject &snapshot) {
  const auto previous=m_snapshot["processes"].toArray();m_snapshot=snapshot;if(!snapshot["processesSampled"].toBool()) m_snapshot["processes"]=previous;
  m_battery->setText(batterySummary(snapshot).replace('\n'," · "));m_gpuMenu->setTitle("dGPU: "+snapshot["gpuState"].toString("unknown"));
  const bool sampled=snapshot["processesSampled"].toBool();const auto processes=m_snapshot["processes"].toArray();
  m_killAll->setEnabled(sampled && !processes.isEmpty() && !readOnlyPreview);
  for(int i=0;i<m_processActions.size();++i) {auto *action=m_processActions[i];const bool visible=sampled && i<processes.size();action->setVisible(visible);if(visible) {const auto p=processes[i].toObject();action->setText(QString("%1 · PID %2").arg(p["name"].toString()).arg(p["pid"].toInteger()));action->setData(p);action->setEnabled(!readOnlyPreview);}}
  updateIcon();
}
void DesktopTray::updatePolling() {m_monitor.setActive(m_dialogVisible || (m_menuVisible && m_monitorAction->isChecked()));}
void DesktopTray::showProcesses() {
  if(!m_dialog) {
    m_dialog=new QDialog;m_dialog->setWindowTitle("UCC · Battery and GPU processes");m_dialog->resize(720,520);auto *layout=new QVBoxLayout(m_dialog);
    m_processWidget=new SessionProcessesWidget(m_paths,&m_monitor,m_dialog);layout->addWidget(m_processWidget);
    connect(m_processWidget,&SessionProcessesWidget::activityChanged,this,[this](bool visible){m_dialogVisible=visible;updatePolling();});
    connect(m_processWidget,&SessionProcessesWidget::message,this,[this](const QString &message){m_icon.showMessage("UCC",message);});
  }
  m_dialog->show();m_dialog->raise();m_dialog->activateWindow();
}
void DesktopTray::openControlCenter() {
#ifdef UCC_READ_ONLY_PREVIEW
  constexpr auto service="com.uniwill.UccGuiPreview";constexpr auto path="/com/uniwill/UccGuiPreview";
#else
  constexpr auto service="com.uniwill.UccGui";constexpr auto path="/com/uniwill/UccGui";
#endif
  auto call=QDBusMessage::createMethodCall(service,path,"com.uniwill.UccGui","Activate");auto *watcher=new QDBusPendingCallWatcher(QDBusConnection::sessionBus().asyncCall(call,3000),this);
  connect(watcher,&QDBusPendingCallWatcher::finished,this,[this,watcher] {const QDBusPendingReply<> reply=*watcher;watcher->deleteLater();if(reply.isError()) {const auto executable=QCoreApplication::applicationDirPath()+"/ucc-gui";if(!QProcess::startDetached(executable,{})) m_icon.showMessage("UCC","Could not launch Control Center");}});
}
QIcon DesktopTray::statusIcon(const QJsonObject &policy,const QJsonObject &snapshot,const QString &profile) {
  QPixmap pixmap(64,64);pixmap.fill(Qt::transparent);QPainter painter(&pixmap);painter.setRenderHint(QPainter::Antialiasing);
  const auto manual=policy["manual_mode"].toString("auto");const bool forced=manual!="auto";
  QPen ring(modeColor(policy["current_mode"].toString()),4);if(forced) ring.setStyle(Qt::DashLine);painter.setPen(ring);painter.drawEllipse(QRectF(8,8,48,48));
  const auto fill=forced ? modeColor(manual) : QColor(Qt::white);painter.setPen(QPen(fill,3));painter.setBrush(fill);
  if(profile.contains("perf",Qt::CaseInsensitive) || profile.contains("max",Qt::CaseInsensitive)) {painter.setBrush(Qt::NoBrush);painter.drawArc(QRectF(17,17,30,30),0,180*16);painter.drawLine(QPointF(32,32),QPointF(43,21));}
  else if(profile.contains("quiet",Qt::CaseInsensitive) || profile.contains("cool",Qt::CaseInsensitive) || profile.contains("breezy",Qt::CaseInsensitive)) {painter.drawEllipse(QPointF(32,32),3,3);for(int i=0;i<4;++i) {painter.save();painter.translate(32,32);painter.rotate(i*90);painter.drawEllipse(QRectF(1,-18,8,15));painter.restore();}}
  else painter.drawPolygon(QPolygonF{{35,16},{23,35},{30,35},{27,48},{41,28},{34,28}});
  const auto batteries=snapshot["batteries"].toArray();const auto state=batteries.isEmpty() ? QString() : batteries[0].toObject()["status"].toString();
  if(state=="Charging" || state=="Discharging") {painter.setPen(Qt::NoPen);painter.setBrush(QColor("#111111"));painter.drawEllipse(QPointF(51,13),10,10);painter.setPen(QPen(state=="Charging" ? QColor("#00e676") : QColor("#ff5252"),3));painter.drawLine(51,7,51,19);if(state=="Charging") {painter.drawLine(46,13,56,13);}else {painter.drawLine(47,16,51,20);painter.drawLine(51,20,55,16);}}
  if(forced) {painter.setPen(Qt::NoPen);painter.setBrush(QColor("#111111"));painter.drawEllipse(QPointF(51,51),10,10);painter.setPen(modeColor(manual));auto font=painter.font();font.setPixelSize(14);font.setBold(true);painter.setFont(font);painter.drawText(QRect(42,41,18,20),Qt::AlignCenter,"M");}
  return QIcon(pixmap);
}
void DesktopTray::updateIcon() {m_icon.setIcon(statusIcon(m_policy,m_snapshot,m_profile));m_icon.setToolTip(QString("UCC · %1\n%2\n%3\ndGPU: %4").arg(m_policy["current_mode"].toString("unknown"),m_profile,batterySummary(m_snapshot),m_snapshot["gpuState"].toString("unknown")));}
}
