// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <QCoreApplication>
#include <QSocketNotifier>
#include <csignal>
#include <unistd.h>
#include <fcntl.h>
#include <memory>
#include <cerrno>

// Self-pipe: the signal handler performs only an async-signal-safe write.
// Quitting through Qt runs the monitor destructor and reaps its subprocess.
class SessionSignals {
public:
  SessionSignals() {
    if(pipe2(m_fds,O_NONBLOCK|O_CLOEXEC)!=0) qFatal("Could not create session shutdown pipe");
    s_fd=m_fds[1];m_notifier=std::make_unique<QSocketNotifier>(m_fds[0],QSocketNotifier::Read);
    QObject::connect(m_notifier.get(),&QSocketNotifier::activated,[](QSocketDescriptor,QSocketNotifier::Type){QCoreApplication::quit();});
    m_oldTerm=std::signal(SIGTERM,handle);m_oldInt=std::signal(SIGINT,handle);
  }
  ~SessionSignals() {std::signal(SIGTERM,m_oldTerm);std::signal(SIGINT,m_oldInt);s_fd=-1;m_notifier.reset();close(m_fds[0]);close(m_fds[1]);}
private:
  static void handle(int) {const int saved=errno;const char byte=1;if(s_fd>=0) {const auto ignored=write(s_fd,&byte,1);(void)ignored;}errno=saved;}
  using Handler=void(*)(int);
  Handler m_oldTerm=SIG_DFL,m_oldInt=SIG_DFL;
  inline static volatile sig_atomic_t s_fd=-1;
  int m_fds[2]={-1,-1};
  std::unique_ptr<QSocketNotifier> m_notifier;
};
