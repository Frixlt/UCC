// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <QWidget>
#include <QWindow>
#ifdef UCC_HAVE_KWINDOWSYSTEM
#include <KWindowSystem>
#endif
namespace ucc {
inline QString takeActivationToken() {
  const auto token=qEnvironmentVariable("XDG_ACTIVATION_TOKEN");
  qunsetenv("XDG_ACTIVATION_TOKEN");return token;
}
inline void activateGuiWindow(QWidget *window,const QString &token) {
  window->setWindowState(window->windowState() & ~Qt::WindowMinimized);
  window->show();window->raise();
#ifdef UCC_HAVE_KWINDOWSYSTEM
  if(KWindowSystem::isPlatformWayland() || KWindowSystem::isPlatformX11()) {
    if(!token.isEmpty()) KWindowSystem::setCurrentXdgActivationToken(token);
    KWindowSystem::activateWindow(window->windowHandle());return;
  }
#else
  Q_UNUSED(token);
#endif
  window->activateWindow();if(auto *handle=window->windowHandle()) handle->requestActivate();
}
}
