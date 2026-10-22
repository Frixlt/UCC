// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QJsonArray>
#include <QJsonObject>
#include <QSysInfo>
#include <QStringList>
#include <optional>
namespace ucc {
// No module loading or hardware writes. The optional backend result detects a
// daemon that started before its device appeared and still holds a dummy backend.
inline QJsonObject inspectDriverHealth(const QString &root="/",std::optional<bool> backendReady={}) {
  auto path=[&](const QString &relative){return QDir(root).filePath(relative);};
  QStringList issues;
  const bool loaded=QFileInfo::exists(path("sys/module/tuxedo_io"));
  if(!loaded) issues << "Не загружен аппаратный драйвер TUXEDO: датчики, вентиляторы и часть управления недоступны. Проверьте сборку DKMS для текущего ядра.";
  else if(!QFileInfo::exists(path("dev/tuxedo_io"))) issues << "Драйвер TUXEDO загружен, но аппаратный интерфейс /dev/tuxedo_io отсутствует.";
  else if(backendReady && !*backendReady) issues << "Служба запущена без рабочего аппаратного интерфейса. После загрузки драйвера перезапустите uccd.service.";
  QFile board(path("sys/class/dmi/id/board_name"));
  const QString model=board.open(QIODevice::ReadOnly) ? QString::fromUtf8(board.readAll()).trimmed() : QString{};
  if(QStringList{"YAOSHI Series-X6AR55xY","X6AR55xY","X6AR5xxY"}.contains(model)) {
    const QDir leds(path("sys/class/leds"));
    if(leds.entryList({"rgb:kbd_backlight*"},QDir::Dirs|QDir::NoDotAndDotDot).isEmpty()) issues << "Не обнаружен драйвер RGB-клавиатуры ITE 8291.";
    if(!QFileInfo::exists(leds.filePath("rgb:lightbar/multi_intensity")) && !QFileInfo::exists(leds.filePath("lightbar/multi_intensity"))) issues << "Не обнаружен аппаратный интерфейс Lightbar.";
  }
  const auto kernel=QSysInfo::kernelVersion();
  return {{"healthy",issues.isEmpty()},{"kernel",kernel},{"issues",QJsonArray::fromStringList(issues)},
          {"message",issues.isEmpty() ? QString{} : "Проблема с драйверами · ядро "+kernel+"\n"+issues.join('\n')}};
}
inline QJsonObject unavailableDriverHealth() {
  return {{"healthy",false},{"message","Нет связи со службой Control Center. Состояние аппаратных драйверов проверить не удалось."}};
}
}
