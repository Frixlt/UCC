#pragma once
#include "PreviewMode.hpp"

#include <QDBusConnection>
#include <QDBusMessage>
#include <QDBusPendingCallWatcher>
#include <QObject>
#include <QPointer>
#include <QStringList>
#include <QVariantMap>
#include <functional>
#include <memory>
#include <optional>

namespace ucc {
// Watchers belong to the consumer, so destroying a tab discards its late replies.
inline void readUccdAsync(QObject *consumer, const QString &method, const QVariantList &args,
                          std::function<void(std::optional<QVariant>)> ready)
{
  if (!mayDispatchUccdMethod(method)) { ready(std::nullopt); return; }
  auto message = QDBusMessage::createMethodCall("com.uniwill.uccd", "/com/uniwill/uccd",
                                                "com.uniwill.uccd", method);
  message.setArguments(args);
  message.setAutoStartService(false);
  auto *watcher = new QDBusPendingCallWatcher(QDBusConnection::systemBus().asyncCall(message, 1500), consumer);
  QObject::connect(watcher, &QDBusPendingCallWatcher::finished, consumer,
    [ready = std::move(ready)](QDBusPendingCallWatcher *done) {
      const auto reply = done->reply();
      done->deleteLater();
      if (reply.type() == QDBusMessage::ErrorMessage || reply.arguments().isEmpty()) ready(std::nullopt);
      else ready(reply.arguments().front());
    });
}

inline void readUccdBatch(QObject *consumer, const QStringList &methods,
                          std::function<void(QVariantMap)> ready)
{
  if (methods.isEmpty()) { ready({}); return; }
  // Limit per-batch concurrent D-Bus calls below the daemon's 8-per-sender Polkit read cap
  // so multi-metric refreshes do not trigger spurious transient auth rejections (`TA-16`).
  static constexpr qsizetype kMaxBatchInFlight = 4;
  struct BatchState : std::enable_shared_from_this<BatchState> {
    QPointer<QObject> consumer;
    QStringList methods;
    std::function<void(QVariantMap)> ready;
    QVariantMap values;
    qsizetype nextIndex = 0;
    qsizetype remaining = 0;
    qsizetype inFlight = 0;

    void pump() {
      while (inFlight < kMaxBatchInFlight && nextIndex < methods.size()) {
        if (!consumer) return;
        const QString method = methods.at(nextIndex++);
        ++inFlight;
        auto self = shared_from_this();
        readUccdAsync(consumer.data(), method, {}, [self, method](std::optional<QVariant> value) {
          --self->inFlight;
          if (value) self->values[method] = *value;
          if (--self->remaining == 0) {
            self->ready(self->values);
          } else {
            self->pump();
          }
        });
      }
    }
  };
  auto state = std::make_shared<BatchState>();
  state->consumer = consumer;
  state->methods = methods;
  state->ready = std::move(ready);
  state->remaining = methods.size();
  state->pump();
}
}
