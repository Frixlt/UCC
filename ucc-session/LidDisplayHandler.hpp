// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once

#include "RefreshAdvisor.hpp"
#include <QDBusConnection>
#include <QDBusInterface>
#include <QJsonDocument>
#include <QJsonObject>
#include <QObject>
#include <QProcess>
#include <QStringList>
#include <QVariantMap>
#include <functional>

namespace ucc {

class LidDisplayHandler : public QObject {
  Q_OBJECT
public:
  using CommandRunner = std::function<void(const QString &program, const QStringList &arguments)>;

  explicit LidDisplayHandler(RefreshBackend backend, QString command = {},
                             bool connectSystemBus = true, CommandRunner runner = {},
                             QObject *parent = nullptr)
      : QObject(parent),
        m_backend(backend),
        m_command(command.isEmpty() ? defaultCommand(backend) : std::move(command)),
        m_runner(std::move(runner)) {
    if (!m_runner) {
      m_runner = [](const QString &program, const QStringList &arguments) {
        QProcess::startDetached(program, arguments);
      };
    }

    if (connectSystemBus) {
      initLidState();
      auto bus = QDBusConnection::systemBus();
      bus.connect("org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.DBus.Properties",
                  "PropertiesChanged", this, SLOT(onPropertiesChanged(QString, QVariantMap, QStringList)));
      bus.connect("org.freedesktop.UPower", "/org/freedesktop/UPower", "org.freedesktop.DBus.Properties",
                  "PropertiesChanged", this, SLOT(onPropertiesChanged(QString, QVariantMap, QStringList)));
    }
  }

  void policyChanged(const QString &json) {
    const auto source = QJsonDocument::fromJson(json.toUtf8()).object()["real_power_source"].toString();
    if (source == "ac" || source == "type_c" || source == "bat")
      m_source = source;
  }

  void deviceSettingsChanged(const QString &json) {
    const auto obj = QJsonDocument::fromJson(json.toUtf8()).object();
    m_lidConfig = obj["lid"].toObject();
  }

  void onLidStateChanged(bool closed) {
    if (!m_initialized) {
      m_initialized = true;
      m_lidClosed = closed;
      return;
    }
    if (closed == m_lidClosed)
      return;
    m_lidClosed = closed;

    if (closed) {
      const auto action = actionForSource(m_source);
      if (action == "screen_off" || action == "lock")
        setInternalPanelDpms(false);
    } else {
      setInternalPanelDpms(true);
    }
  }

  QString actionForSource(const QString &source) const {
    const QString fallback = (source == "bat") ? QStringLiteral("suspend") : QStringLiteral("screen_off");
    const auto val = m_lidConfig.value(source);
    return val.isString() ? val.toString() : fallback;
  }

  bool isLidClosed() const { return m_lidClosed; }

private slots:
  void onPropertiesChanged(const QString &interfaceName, const QVariantMap &changed, const QStringList &) {
    if (interfaceName == "org.freedesktop.login1.Manager" && changed.contains("LidClosed"))
      onLidStateChanged(changed.value("LidClosed").toBool());
    else if (interfaceName == "org.freedesktop.UPower" && changed.contains("LidIsClosed"))
      onLidStateChanged(changed.value("LidIsClosed").toBool());
  }

private:
  static QString defaultCommand(RefreshBackend backend) {
    if (backend == RefreshBackend::Hyprland)
      return QStringLiteral("hyprctl");
    if (backend == RefreshBackend::KDE)
      return QStringLiteral("kscreen-doctor");
    return {};
  }

  void initLidState() {
    QDBusInterface login1("org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager",
                          QDBusConnection::systemBus());
    if (login1.isValid()) {
      const QVariant val = login1.property("LidClosed");
      if (val.isValid()) {
        m_lidClosed = val.toBool();
        m_initialized = true;
        return;
      }
    }
    QDBusInterface upower("org.freedesktop.UPower", "/org/freedesktop/UPower", "org.freedesktop.UPower",
                          QDBusConnection::systemBus());
    if (upower.isValid()) {
      const QVariant val = upower.property("LidIsClosed");
      if (val.isValid()) {
        m_lidClosed = val.toBool();
        m_initialized = true;
        return;
      }
    }
    m_lidClosed = false;
    m_initialized = true;
  }

  void setInternalPanelDpms(bool on) {
    if (m_backend != RefreshBackend::Hyprland || m_command.isEmpty() || !m_runner)
      return;
    const QString action = on ? QStringLiteral("on") : QStringLiteral("off");
    const QString script = QStringLiteral(
        "for _, m in ipairs(hl.get_monitors()) do "
        "if m.name:match(\"^eDP%%-\") or m.name:match(\"^LVDS%%-\") then "
        "hl.dispatch(hl.dsp.dpms({action=\"%1\", monitor=m})) "
        "end end")
        .arg(action);
    m_runner(m_command, {QStringLiteral("eval"), script});
  }

  RefreshBackend m_backend;
  QString m_command;
  CommandRunner m_runner;
  QString m_source = QStringLiteral("ac");
  QJsonObject m_lidConfig;
  bool m_lidClosed = false;
  bool m_initialized = false;
};

} // namespace ucc
