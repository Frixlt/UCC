// SPDX-License-Identifier: GPL-3.0-or-later
#include <QApplication>
#include <QDBusConnection>
#include <QDebug>
#include <cstdio>
#include <unistd.h>
#include "DesktopTray.hpp"
#include "PreviewMode.hpp"
#include "../ucc-session/SessionSignals.hpp"
int main(int argc,char **argv) {
  for(int i=1;i<argc;++i) if(QByteArray(argv[i])=="--help") {std::puts("ucc-desktop-tray: battery, power modes and NVIDIA process management");return 0;}
  if(getuid()==0) {std::fputs("Run the desktop tray as your desktop user\n",stderr);return 1;}
  QApplication app(argc,argv);app.setApplicationName("ucc-desktop-tray");app.setOrganizationName("UniwillControlCenter");app.setQuitOnLastWindowClosed(false);
  const auto service=ucc::readOnlyPreview ? "com.uniwill.UccDesktopTrayPreview" : "com.uniwill.UccDesktopTray";
  auto bus=QDBusConnection::sessionBus();if(!bus.isConnected()) {qCritical("No desktop session bus");return 1;}if(!bus.registerService(service)) return 0;
  if(!QSystemTrayIcon::isSystemTrayAvailable()) {qCritical("No system tray is available in this desktop session");return 1;}
  SessionSignals shutdownSignals;ucc::UccdClient client;ucc::DesktopTray tray(&client);return app.exec();
}
