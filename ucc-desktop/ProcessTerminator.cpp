// SPDX-License-Identifier: GPL-3.0-or-later
#include "ProcessTerminator.hpp"
#include "PreviewMode.hpp"
#include <QSet>
#include <sys/syscall.h>
#include <unistd.h>
#include <signal.h>
#include <poll.h>
#include <cerrno>
#include <cstring>
#include <algorithm>
namespace ucc {
namespace {
int openProcess(qint64 pid) {return static_cast<int>(syscall(SYS_pidfd_open,pid,0));}
bool sendSignal(int fd,int sig) {return syscall(SYS_pidfd_send_signal,fd,sig,nullptr,0)==0;}
}
ProcessTerminator::ProcessTerminator(SessionPaths paths,QObject *parent):QObject(parent),m_reader(std::move(paths)) {
  m_escalation.setSingleShot(true);m_escalation.setInterval(3000);
  m_deadline.setSingleShot(true);m_deadline.setInterval(2000);
  connect(&m_escalation,&QTimer::timeout,this,[this] {
    m_starting=true;
    for(auto &target:m_targets) if(!target->done) {
      pollfd descriptor{target->fd,POLLIN,0};if(poll(&descriptor,1,0)>0) {exited(target.get());continue;}
      if(sendSignal(target->fd,SIGKILL)) target->killed=true;
      else if(errno==ESRCH) exited(target.get());
      else {m_errors<<QString("PID %1: %2").arg(target->pid).arg(QString::fromLocal8Bit(strerror(errno)));target->done=true;target->notifier->setEnabled(false);}
    }
    m_starting=false;m_deadline.start();finishIfReady();
  });
  connect(&m_deadline,&QTimer::timeout,this,[this] {
    for(auto &target:m_targets) if(!target->done) {m_errors<<QString("PID %1 has not exited").arg(target->pid);target->done=true;target->notifier->setEnabled(false);}
    finish();
  });
}
ProcessTerminator::~ProcessTerminator() {for(auto &target:m_targets) {target->notifier.reset();if(target->fd>=0) close(target->fd);}}
bool ProcessTerminator::start(const QJsonArray &selected) {
  if(m_running || selected.isEmpty()) return false;
  if(getuid()==0) {QTimer::singleShot(0,this,[this]{emit finished("Run process management as your desktop user");});return false;}
  if(readOnlyPreview) {QTimer::singleShot(0,this,[this]{emit finished("Process termination is disabled in read-only preview");});return false;}
  for(auto &target:m_targets) {target->notifier.reset();if(target->fd>=0) close(target->fd);}
  m_targets.clear();m_errors.clear();m_terminated=0;m_killed=0;m_running=true;m_starting=true;
  QSet<qint64> seen;
  for(const auto &value:selected) {
    const auto token=value.toObject();const auto pid=token["pid"].toInteger();if(seen.contains(pid)) continue;seen.insert(pid);
    const auto before=m_reader.processIdentity(pid);
    if(before.isEmpty() || token["startTime"]!=before["startTime"] || !m_reader.holdsNvidiaDevice(pid)) {m_errors<<QString("PID %1 ended or no longer matches the selected GPU process").arg(pid);continue;}
    const int fd=openProcess(pid);
    if(fd<0) {m_errors<<QString("PID %1: %2").arg(pid).arg(QString::fromLocal8Bit(strerror(errno)));continue;}
    // Open pidfd first, then recheck identity; never signal a reused numeric PID.
    const auto after=m_reader.processIdentity(pid);
    if(after.isEmpty() || after["startTime"]!=before["startTime"] || !m_reader.holdsNvidiaDevice(pid)) {close(fd);m_errors<<QString("PID %1 changed before termination").arg(pid);continue;}
    auto target=std::make_unique<Target>();target->fd=fd;target->pid=pid;target->name=after["name"].toString();
    target->notifier=std::make_unique<QSocketNotifier>(fd,QSocketNotifier::Read);
    auto *item=target.get();connect(target->notifier.get(),&QSocketNotifier::activated,this,[this,item]{exited(item);});
    m_targets.push_back(std::move(target));
    if(!sendSignal(fd,SIGTERM)) {m_errors<<QString("PID %1: %2").arg(pid).arg(QString::fromLocal8Bit(strerror(errno)));item->done=true;item->notifier->setEnabled(false);}
  }
  m_starting=false;m_escalation.start();QTimer::singleShot(0,this,&ProcessTerminator::finishIfReady);return true;
}
void ProcessTerminator::exited(Target *target) {
  if(target->done) return;
  target->done=true;target->notifier->setEnabled(false);
  if(target->killed) ++m_killed;else ++m_terminated;
  if(!m_starting) finishIfReady();
}
void ProcessTerminator::finishIfReady() {if(m_running && !m_starting && std::all_of(m_targets.begin(),m_targets.end(),[](const auto &t){return t->done;})) finish();}
void ProcessTerminator::finish() {
  if(!m_running) return;
  m_running=false;m_escalation.stop();m_deadline.stop();
  QString text=QString("Exited: %1 · Forced: %2").arg(m_terminated).arg(m_killed);
  if(!m_errors.isEmpty()) text+="\n"+m_errors.join("\n");
  emit finished(text);
}
}
