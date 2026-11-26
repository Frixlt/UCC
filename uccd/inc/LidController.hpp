// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once

#include "PowerDeviceController.hpp"
#include "WakeupPolicy.hpp"
#include <QDBusConnection>
#include <QDBusInterface>
#include <QDBusMessage>
#include <QDBusReply>
#include <QDBusServiceWatcher>
#include <QDBusUnixFileDescriptor>
#include <QObject>
#include <QTimer>
#include <QVariantMap>
#include <filesystem>
#include <fstream>
#include <functional>
#include <nlohmann/json.hpp>
#include <string>
#include <syslog.h>

struct LidActions {
  std::function<void(const std::string &action)> performAction;
  std::function<void(bool wakeOnlyLidAndKeyboard)> applyWakeup;
  int powerChangeDebounceMs = 2000;
};

class LidController : public QObject {
  Q_OBJECT
public:
  explicit LidController(bool connectSystemBus = true, LidActions actions = {}, QObject *parent = nullptr)
      : QObject(parent),
        m_actions(std::move(actions)),
        m_lidSettings(PowerDeviceController::defaultLidSettings()) {
    if (!m_actions.performAction) {
      m_actions.performAction = [](const std::string &action) { performLogin1Action(action); };
    }
    if (!m_actions.applyWakeup) {
      m_actions.applyWakeup = [](bool wakeOnly) {
        const auto res = applyWakeupPolicy(wakeOnly);
        syslog(LOG_INFO, "[WakeupPolicy] Applied (wakeOnly=%s): enabled=%d disabled=%d failed=%d",
               wakeOnly ? "true" : "false", res.enabled, res.disabled, res.failed);
      };
    }

    m_powerChangeTimer.setSingleShot(true);
    connect(&m_powerChangeTimer, &QTimer::timeout, this, &LidController::onPowerChangeTimeout);

    if (connectSystemBus) {
      acquireInhibitor();
      initLidStateFromSystem();

      auto bus = QDBusConnection::systemBus();
      bus.connect("org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.DBus.Properties",
                  "PropertiesChanged", this, SLOT(onPropertiesChanged(QString, QVariantMap, QStringList)));
      bus.connect("org.freedesktop.UPower", "/org/freedesktop/UPower", "org.freedesktop.DBus.Properties",
                  "PropertiesChanged", this, SLOT(onPropertiesChanged(QString, QVariantMap, QStringList)));

      m_loginWatcher = new QDBusServiceWatcher(
          "org.freedesktop.login1", bus, QDBusServiceWatcher::WatchForOwnerChange, this);
      connect(m_loginWatcher, &QDBusServiceWatcher::serviceOwnerChanged, this,
              [this](const QString &, const QString &, const QString &newOwner) {
                if (!newOwner.isEmpty())
                  acquireInhibitor();
              });
    }
  }

  void setSettings(const nlohmann::json &lidSettings) {
    nlohmann::json normalized = PowerDeviceController::defaultLidSettings();
    if (lidSettings.is_object()) {
      for (const auto &[key, val] : lidSettings.items())
        normalized[key] = val;
    }
    m_lidSettings = std::move(normalized);
    if (m_actions.applyWakeup)
      m_actions.applyWakeup(wakeOnlyLidAndKeyboard());
  }

  void setPowerSource(const std::string &source) {
    if (source != "ac" && source != "type_c" && source != "bat")
      return;
    if (source == m_powerSource)
      return;
    m_powerSource = source;
    if (m_lidClosed) {
      const auto action = actionForSource(m_powerSource);
      if (isPowerAction(action))
        m_powerChangeTimer.start(m_actions.powerChangeDebounceMs);
      else
        m_powerChangeTimer.stop();
    }
  }

  void onLidStateChanged(bool closed) {
    if (!m_lidStateInitialized) {
      m_lidStateInitialized = true;
      m_lidClosed = closed;
      return;
    }
    if (closed == m_lidClosed)
      return;
    m_lidClosed = closed;
    m_powerChangeTimer.stop();
    if (!closed)
      return;

    const auto action = actionForSource(m_powerSource);
    if (action == "suspend" || action == "hibernate") {
      if (m_actions.applyWakeup)
        m_actions.applyWakeup(wakeOnlyLidAndKeyboard());
      if (m_actions.performAction)
        m_actions.performAction(action);
    } else if (action == "poweroff" || action == "lock") {
      if (m_actions.performAction)
        m_actions.performAction(action);
    }
  }

  bool isLidClosed() const { return m_lidClosed; }

  std::string actionForSource(const std::string &source) const {
    const std::string fallback = (source == "bat") ? "suspend" : "screen_off";
    if (!m_lidSettings.is_object() || !m_lidSettings.contains(source))
      return fallback;
    const auto &val = m_lidSettings[source];
    return val.is_string() ? val.get<std::string>() : fallback;
  }

  bool wakeOnlyLidAndKeyboard() const {
    if (!m_lidSettings.is_object())
      return true;
    return m_lidSettings.value("wakeOnlyLidAndKeyboard", true);
  }

  bool hasInhibitor() const { return m_inhibitFd.isValid(); }

private slots:
  void onPropertiesChanged(const QString &interfaceName, const QVariantMap &changed, const QStringList &) {
    if (interfaceName == "org.freedesktop.login1.Manager" && changed.contains("LidClosed"))
      onLidStateChanged(changed.value("LidClosed").toBool());
    else if (interfaceName == "org.freedesktop.UPower" && changed.contains("LidIsClosed"))
      onLidStateChanged(changed.value("LidIsClosed").toBool());
  }

  void onPowerChangeTimeout() {
    if (!m_lidClosed)
      return;
    const auto action = actionForSource(m_powerSource);
    if (!isPowerAction(action))
      return;
    if ((action == "suspend" || action == "hibernate") && m_actions.applyWakeup)
      m_actions.applyWakeup(wakeOnlyLidAndKeyboard());
    if (m_actions.performAction)
      m_actions.performAction(action);
  }

private:
  static bool isPowerAction(const std::string &action) {
    return action == "suspend" || action == "hibernate" || action == "poweroff";
  }

  void acquireInhibitor() {
    auto call = QDBusMessage::createMethodCall(
        "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager", "Inhibit");
    call.setArguments({QStringLiteral("handle-lid-switch"), QStringLiteral("Frixit Control Center"),
                       QStringLiteral("Manage configurable lid close actions"), QStringLiteral("block")});
    call.setAutoStartService(false);
    const QDBusReply<QDBusUnixFileDescriptor> reply = QDBusConnection::systemBus().call(call, QDBus::Block, 3000);
    if (reply.isValid() && reply.value().isValid()) {
      m_inhibitFd = reply.value();
      syslog(LOG_INFO, "[LidController] Acquired handle-lid-switch inhibitor from logind");
    } else {
      syslog(LOG_WARNING, "[LidController] Failed to acquire handle-lid-switch inhibitor: %s",
             qPrintable(reply.error().message()));
    }
  }

  void initLidStateFromSystem() {
    QDBusInterface iface("org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager",
                         QDBusConnection::systemBus());
    if (iface.isValid()) {
      const QVariant val = iface.property("LidClosed");
      if (val.isValid()) {
        m_lidClosed = val.toBool();
        m_lidStateInitialized = true;
        return;
      }
    }
    std::error_code ec;
    const std::filesystem::path lidDir("/proc/acpi/button/lid");
    if (std::filesystem::exists(lidDir, ec)) {
      for (const auto &entry : std::filesystem::directory_iterator(lidDir, ec)) {
        std::ifstream stateFile(entry.path() / "state");
        std::string line;
        if (std::getline(stateFile, line)) {
          m_lidClosed = (line.find("closed") != std::string::npos);
          m_lidStateInitialized = true;
          return;
        }
      }
    }
    m_lidClosed = false;
    m_lidStateInitialized = true;
  }

  static void performLogin1Action(const std::string &action) {
    if (action == "screen_off" || action == "ignore")
      return;
    QString method;
    bool withInteractive = true;
    if (action == "suspend")
      method = QStringLiteral("Suspend");
    else if (action == "hibernate")
      method = QStringLiteral("Hibernate");
    else if (action == "poweroff")
      method = QStringLiteral("PowerOff");
    else if (action == "lock") {
      method = QStringLiteral("LockSessions");
      withInteractive = false;
    } else
      return;

    auto call = QDBusMessage::createMethodCall(
        "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager", method);
    if (withInteractive)
      call.setArguments({false});
    call.setAutoStartService(false);
    const auto reply = QDBusConnection::systemBus().call(call, QDBus::Block, 3000);
    if (reply.type() == QDBusMessage::ErrorMessage) {
      syslog(LOG_ERR, "[LidController] logind %s failed: %s", qPrintable(method), qPrintable(reply.errorMessage()));
    } else {
      syslog(LOG_INFO, "[LidController] Executed lid action '%s' via logind", action.c_str());
    }
  }

  LidActions m_actions;
  nlohmann::json m_lidSettings;
  std::string m_powerSource = "ac";
  bool m_lidClosed = false;
  bool m_lidStateInitialized = false;
  QDBusUnixFileDescriptor m_inhibitFd;
  QTimer m_powerChangeTimer;
  QDBusServiceWatcher *m_loginWatcher = nullptr;
};
