// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <QDBusConnection>
#include <QJsonObject>
#include <QObject>
#include <QPointer>
#include <QProcess>
#include <QTimer>
#include <functional>
#include <optional>
namespace ucc
{
struct RefreshPlan
{
  QString output, currentMode, targetMode;
  int width = 0, height = 0;
  double currentHz = 0, targetHz = 0;
  bool sameSelection( const RefreshPlan& other ) const;
};
class RefreshAdvisor : public QObject
{
  Q_OBJECT
public:
  explicit RefreshAdvisor( bool kdeSession, QString command = "kscreen-doctor", QString drm = "/sys/class/drm",
                           QDBusConnection bus = QDBusConnection::sessionBus(), QObject* parent = nullptr );
  ~RefreshAdvisor() override;
  void policyChanged( const QString& json );
  void sleeping( bool sleeping );
  static std::optional< RefreshPlan > plan( const QJsonObject& configuration, const QString& source,
                                            const QString& drm );
  static bool applied( const QJsonObject& configuration, const RefreshPlan& plan );
signals:
  void result( bool applied, const QString& message );
private slots:
  void actionInvoked( uint id, const QString& action );
  void notificationClosed( uint id, uint reason );

private:
  bool enabled() const;
  void propose();
  void invalidate();
  void closeNotification();
  void run( const QStringList& arguments, std::function< void( bool, QByteArray ) > done );
  void notify( const QString& title, const QString& body, const std::optional< RefreshPlan >& plan = {} );
  void report( bool success, const RefreshPlan& plan );
  bool m_kde = false, m_sleeping = false, m_wanted = false;
  QString m_command, m_drm, m_source;
  QDBusConnection m_bus;
  QTimer m_debounce;
  QPointer< QProcess > m_process;
  std::optional< RefreshPlan > m_pending;
  uint m_notification = 0;
  quint64 m_generation = 0;
  int m_retries = 0;
};
class SessionSleepMonitor : public QObject
{
  Q_OBJECT
public:
  explicit SessionSleepMonitor( QObject* parent = nullptr );
signals:
  void sleeping( bool sleeping );
private slots:
  void prepare( bool value )
  {
    emit sleeping( value );
  }
};
} // namespace ucc
