// SPDX-License-Identifier: GPL-3.0-or-later
#include "SessionTelemetry.hpp"
#include <QFile>
#include <QFileInfo>
#include <QDir>
#include <QJsonArray>
#include <QDateTime>
#include <QRegularExpression>
#include <optional>
#include <cmath>
#include <algorithm>
#include <unistd.h>

namespace ucc {
QString SessionReader::read(const QString &path) {QFile file(path);return file.open(QIODevice::ReadOnly) ? QString::fromUtf8(file.read(4096)).trimmed() : QString();}
namespace {
std::optional<double> number(const QString &path) {bool ok=false;const double value=SessionReader::read(path).toDouble(&ok);return ok && std::isfinite(value) ? std::optional(value) : std::nullopt;}
QJsonValue value(std::optional<double> number) {return number ? QJsonValue(*number) : QJsonValue(QJsonValue::Null);}
}
QJsonObject SessionReader::processIdentity(qint64 pid) const {
  if(getuid()==0 || pid<=1 || pid==getpid()) return {};
  const auto path=m_paths.proc+"/"+QString::number(pid);
  if(QFileInfo(path).ownerId()!=getuid()) return {};
  const auto stat=read(path+"/stat");const auto close=stat.lastIndexOf(')');
  if(close<0) return {};
  const auto fields=stat.mid(close+1).split(' ',Qt::SkipEmptyParts);
  if(fields.size()<=19 || fields[0]=="Z" || fields[0]=="X") return {};
  bool ok=false;const auto start=fields[19].toULongLong(&ok);if(!ok || !start) return {};
  const auto name=read(path+"/comm").left(256);
  return {{"pid",pid},{"startTime",QString::number(start)},{"name",name.isEmpty() ? QString("unknown") : name}};
}
bool SessionReader::holdsNvidiaDevice(qint64 pid) const {
  const auto directory=m_paths.proc+"/"+QString::number(pid)+"/fd";
  for(const auto &name:QDir(directory).entryList(QDir::Files|QDir::System|QDir::NoDotAndDotDot)) {
    char buffer[4096];const auto path=QFile::encodeName(directory+"/"+name);const auto size=readlink(path.constData(),buffer,sizeof(buffer)-1);
    if(size<=0) continue;
    const auto target=QString::fromLocal8Bit(buffer,static_cast<qsizetype>(size));
    if(!target.startsWith(m_paths.nvidiaPrefix)) continue;
    const auto suffix=target.mid(m_paths.nvidiaPrefix.size());
    if(suffix=="ctl" || suffix=="-uvm" || suffix=="-uvm-tools" || suffix=="-modeset" ||
       (!suffix.isEmpty() && std::all_of(suffix.begin(),suffix.end(),[](QChar c){return c.isDigit();}))) return true;
  }
  return false;
}
QJsonObject SessionReader::snapshot(bool includeProcesses) const {
  QJsonArray batteries,cards,processes;
  for(const auto &name:QDir(m_paths.supplies).entryList(QDir::Dirs|QDir::NoDotAndDotDot,QDir::Name)) {
    const auto path=m_paths.supplies+"/"+name;
    if(read(path+"/type")!="Battery" || read(path+"/scope")=="Device" || read(path+"/present")=="0") continue;
    auto capacity=number(path+"/capacity");if(capacity && (*capacity<0 || *capacity>100)) capacity.reset();
    auto watts=number(path+"/power_now");
    if(watts) *watts=std::abs(*watts)/1e6;
    else {const auto amps=number(path+"/current_now"),volts=number(path+"/voltage_now");if(amps && volts) watts=std::abs(*amps)*std::abs(*volts)/1e12;}
    if(watts && !std::isfinite(*watts)) watts.reset();
    batteries.append(QJsonObject{{"name",name},{"capacity",value(capacity)},{"status",read(path+"/status")},{"powerWatts",value(watts)}});
  }
  bool anyActive=false,anyUnknown=false;
  for(const auto &name:QDir(m_paths.pci).entryList(QDir::Dirs|QDir::NoDotAndDotDot,QDir::Name)) {
    const auto path=m_paths.pci+"/"+name;
    if(read(path+"/vendor")!="0x10de" || !read(path+"/class").startsWith("0x03")) continue;
    const auto status=read(path+"/power/runtime_status");
    anyActive |= status=="active";anyUnknown |= status!="active" && status!="suspended" && status!="off";
    cards.append(QJsonObject{{"pci",name},{"status",status.isEmpty() ? "unknown" : status}});
  }
  const QString state=cards.isEmpty() ? "unavailable" : anyActive ? "active" : anyUnknown ? "unknown" : "suspended";
  bool truncated=false;
  if(includeProcesses && (anyActive || anyUnknown)) {
    for(const auto &name:QDir(m_paths.proc).entryList(QDir::Dirs|QDir::NoDotAndDotDot,QDir::Name)) {
      if(QThread::currentThread()->isInterruptionRequested()) break;
      bool ok=false;const auto pid=name.toLongLong(&ok);if(!ok) continue;
      const auto before=processIdentity(pid);if(before.isEmpty() || !holdsNvidiaDevice(pid)) continue;
      const auto after=processIdentity(pid);if(before["startTime"]!=after["startTime"]) continue;
      if(processes.size()>=512) {truncated=true;break;}
      processes.append(after);
    }
  }
  return {{"batteries",batteries},{"gpuCards",cards},{"gpuState",state},{"processes",processes},
    {"processesSampled",includeProcesses},{"truncated",truncated},{"timestamp",QDateTime::currentMSecsSinceEpoch()}};
}
DesktopMonitor::DesktopMonitor(bool idlePolling,SessionPaths paths,QObject *parent)
  :QObject(parent),m_idlePolling(idlePolling) {
  auto *worker=new SessionSampler(std::move(paths));worker->moveToThread(&m_thread);
  connect(&m_thread,&QThread::finished,worker,&QObject::deleteLater);
  connect(this,&DesktopMonitor::requested,worker,&SessionSampler::sample);
  connect(worker,&SessionSampler::ready,this,[this](const QJsonObject &snapshot) {
    m_busy=false;m_snapshot=snapshot;emit updated(snapshot);
    if(m_again) {const bool processes=m_active || m_againProcesses;m_again=false;m_againProcesses=false;request(processes);}
  });
  connect(&m_timer,&QTimer::timeout,this,&DesktopMonitor::refresh);m_thread.start();
  if(m_idlePolling) {m_timer.start(30000);refresh();}
}
DesktopMonitor::~DesktopMonitor() {m_timer.stop();m_thread.requestInterruption();m_thread.quit();m_thread.wait();}
void DesktopMonitor::setActive(bool active) {
  if(m_active==active) return;
  m_active=active;
  if(active) {m_timer.start(2000);refresh();}
  else if(m_idlePolling) m_timer.start(30000);else m_timer.stop();
}
void DesktopMonitor::refresh() {request(m_active);}
void DesktopMonitor::refreshProcesses() {request(true);}
void DesktopMonitor::request(bool processes) {
  if(m_busy) {m_again=true;m_againProcesses |= processes;return;}
  m_busy=true;emit requested(processes);
}
QString batterySummary(const QJsonObject &snapshot) {
  QStringList parts;
  for(const auto &value:snapshot["batteries"].toArray()) {
    const auto b=value.toObject();QString text=b["name"].toString()+": ";
    text+=b["capacity"].isDouble() ? QString::number(b["capacity"].toDouble(),'f',0)+"%" : QString("—");
    text+=" · "+b["status"].toString("Unknown");
    if(b["powerWatts"].isDouble()) text+=" · "+QString(b["status"]=="Charging" ? "+" : b["status"]=="Discharging" ? "−" : "")+QString::number(b["powerWatts"].toDouble(),'f',1)+" W";
    parts<<text;
  }
  return parts.isEmpty() ? QString("Battery data unavailable") : parts.join("\n");
}
}
