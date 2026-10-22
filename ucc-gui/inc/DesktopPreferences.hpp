// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include "PreviewMode.hpp"
#include <QFile>
#include <QJsonDocument>
#include <QJsonObject>
#include <QSettings>
namespace ucc {
inline bool importDesktopPreferences(const QString &path,QString &error) {
  if(readOnlyPreview) {error="Preference import is disabled in read-only preview";return false;}
  QFile file(path);if(!file.open(QIODevice::ReadOnly) || file.size()>16384) {error="Cannot read desktop preferences";return false;}
  const auto doc=QJsonDocument::fromJson(file.readAll());const auto settings=doc.object();
  const auto theme=settings["theme"].toString();
  if(!doc.isObject() || !settings["startMinimized"].isBool() || (theme!="light" && theme!="dark" && theme!="auto")) {error="Invalid desktop preferences";return false;}
  QSettings desktop("UniwillControlCenter","desktop"),appearance("UniwillControlCenter","appearance");
  desktop.setValue("startMinimized",settings["startMinimized"].toBool());appearance.setValue("themeMode",theme=="light" ? 0 : theme=="dark" ? 1 : 2);desktop.sync();appearance.sync();
  if(desktop.status()!=QSettings::NoError || appearance.status()!=QSettings::NoError) {error="Could not save desktop preferences";return false;}
  return true;
}
inline bool startMinimizedPreference() {return !readOnlyPreview && QSettings("UniwillControlCenter","desktop").value("startMinimized",false).toBool();}
}
