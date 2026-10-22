// SPDX-License-Identifier: GPL-3.0-or-later
#include "RefreshAdvisor.hpp"
#include "PreviewMode.hpp"
#include <QDBusMessage>
#include <QDBusPendingCallWatcher>
#include <QDBusPendingReply>
#include <QDir>
#include <QFile>
#include <QJsonArray>
#include <QJsonDocument>
#include <QRegularExpression>
#include <QSettings>
#include <cmath>
#include <memory>
namespace ucc
{
namespace
{
constexpr auto notificationService = "org.freedesktop.Notifications";
constexpr auto notificationPath = "/org/freedesktop/Notifications";
constexpr auto notificationInterface = "org.freedesktop.Notifications";
bool validSource( const QString& s )
{
  return s == "ac" || s == "type_c" || s == "bat";
}
bool identifier( const QString& s )
{
  return QRegularExpression( "^[A-Za-z0-9_:@+-]{1,128}$" ).match( s ).hasMatch();
}
bool internal( const QString& s )
{
  return s.startsWith( "eDP-" ) || s.startsWith( "LVDS-" );
}
bool nvidiaPanel( const QString& name, const QString& root )
{
  for( const auto& entry : QDir( root ).entryList( QDir::Dirs | QDir::NoDotAndDotDot ) )
    if( entry.endsWith( "-" + name ) )
    {
      const auto card = entry.left( entry.size() - name.size() - 1 );
      QFile vendor( root + "/" + card + "/device/vendor" );
      if( vendor.open( QIODevice::ReadOnly ) && vendor.readAll().trimmed() == "0x10de" )
        return true;
    }
  return false;
}
QJsonObject currentMode( const QJsonObject& output )
{
  for( const auto& v : output["modes"].toArray() )
    if( v.toObject()["id"] == output["currentModeId"] )
      return v.toObject();
  return {};
}
} // namespace
bool RefreshPlan::sameSelection( const RefreshPlan& o ) const
{
  return output == o.output && currentMode == o.currentMode && targetMode == o.targetMode && width == o.width &&
         height == o.height && std::abs( currentHz - o.currentHz ) < 0.1 && std::abs( targetHz - o.targetHz ) < 0.1;
}
std::optional< RefreshPlan > RefreshAdvisor::plan( const QJsonObject& configuration, const QString& source,
                                                   const QString& drm )
{
  if( !validSource( source ) )
    return {};
  for( const auto& v : configuration["outputs"].toArray() )
  {
    const auto output = v.toObject();
    const auto name = output["name"].toString();
    if( !internal( name ) || !identifier( name ) || !output["connected"].toBool() || !output["enabled"].toBool() )
      continue;
    const auto current = currentMode( output ), size = current["size"].toObject();
    const int w = size["width"].toInt(), h = size["height"].toInt();
    const double currentHz = current["refreshRate"].toDouble();
    if( w <= 0 || h <= 0 || !std::isfinite( currentHz ) || currentHz <= 0 )
      continue;
    const double preferred = source == "bat" ? 60 : nvidiaPanel( name, drm ) ? 300 : 240;
    QJsonObject selected;
    double best = -1;
    for( const auto& m : output["modes"].toArray() )
    {
      const auto mode = m.toObject(), s = mode["size"].toObject();
      const double hz = mode["refreshRate"].toDouble();
      if( s["width"].toInt() != w || s["height"].toInt() != h || !identifier( mode["id"].toString() ) ||
          !std::isfinite( hz ) )
        continue;
      if( source == "bat" ? std::abs( hz - 60 ) > 2 : ( hz < 100 || hz > preferred + 2 ) )
        continue;
      if( selected.isEmpty() || ( source == "bat" ? std::abs( hz - 60 ) < std::abs( best - 60 ) : hz > best ) )
      {
        selected = mode;
        best = hz;
      }
    }
    if( selected.isEmpty() || std::abs( currentHz - best ) < 2 )
      continue;
    return RefreshPlan{ name, current["id"].toString(), selected["id"].toString(), w, h, currentHz, best };
  }
  return {};
}
bool RefreshAdvisor::applied( const QJsonObject& configuration, const RefreshPlan& p )
{
  for( const auto& v : configuration["outputs"].toArray() )
  {
    const auto output = v.toObject(), mode = currentMode( output ), size = mode["size"].toObject();
    if( output["name"] == p.output && output["connected"].toBool() && output["enabled"].toBool() &&
        mode["id"] == p.targetMode && size["width"].toInt() == p.width && size["height"].toInt() == p.height &&
        std::abs( mode["refreshRate"].toDouble() - p.targetHz ) < 2 )
      return true;
  }
  return false;
}
RefreshAdvisor::RefreshAdvisor( bool kdeSession, QString command, QString drm, QDBusConnection bus, QObject* parent )
    : QObject( parent ), m_kde( kdeSession ), m_command( std::move( command ) ), m_drm( std::move( drm ) ),
      m_bus( std::move( bus ) )
{
  m_debounce.setSingleShot( true );
  m_debounce.setInterval( 800 );
  connect( &m_debounce, &QTimer::timeout, this, &RefreshAdvisor::propose );
  m_bus.connect( notificationService, notificationPath, notificationInterface, "ActionInvoked", this,
                 SLOT( actionInvoked( uint, QString ) ) );
  m_bus.connect( notificationService, notificationPath, notificationInterface, "NotificationClosed", this,
                 SLOT( notificationClosed( uint, uint ) ) );
}
RefreshAdvisor::~RefreshAdvisor()
{
  invalidate();
}
bool RefreshAdvisor::enabled() const
{
  return !readOnlyPreview && m_kde &&
         QSettings( "UniwillControlCenter", "desktop" ).value( "refreshRateNotifications", true ).toBool();
}
void RefreshAdvisor::closeNotification()
{
  if( m_notification )
  {
    auto call = QDBusMessage::createMethodCall( notificationService, notificationPath, notificationInterface,
                                                "CloseNotification" );
    call.setArguments( { m_notification } );
    call.setAutoStartService( false );
    m_bus.asyncCall( call, 1000 );
  }
  m_notification = 0;
  m_pending.reset();
}
void RefreshAdvisor::invalidate()
{
  ++m_generation;
  m_debounce.stop();
  closeNotification();
  if( m_process )
  {
    auto* process = m_process.data();
    m_process = nullptr;
    process->disconnect( this );
    process->kill();
    process->waitForFinished( 250 );
    process->deleteLater();
  }
}
void RefreshAdvisor::policyChanged( const QString& json )
{
  const auto source = QJsonDocument::fromJson( json.toUtf8() ).object()["real_power_source"].toString();
  if( !validSource( source ) )
    return;
  if( m_source.isEmpty() )
  {
    m_source = source;
    return;
  }
  if( source == m_source )
  {
    if( !enabled() )
    {
      invalidate();
      m_wanted = false;
    }
    return;
  }
  m_source = source;
  invalidate();
  m_retries = 0;
  m_wanted = true;
  if( enabled() && !m_sleeping )
    m_debounce.start();
}
void RefreshAdvisor::sleeping( bool sleep )
{
  if( sleep == m_sleeping )
    return;
  if( sleep && m_pending )
    m_wanted = true;
  m_sleeping = sleep;
  invalidate();
  if( !sleep && m_wanted && enabled() )
  {
    m_retries = 0;
    m_debounce.start( 1500 );
  }
}
void RefreshAdvisor::run( const QStringList& args, std::function< void( bool, QByteArray ) > done )
{
  if( !enabled() || m_sleeping )
    return;
  auto* process = new QProcess( this );
  m_process = process;
  auto bytes = std::make_shared< QByteArray >();
  auto failed = std::make_shared< bool >( false );
  const auto generation = m_generation;
  auto* timeout = new QTimer( process );
  timeout->setSingleShot( true );
  timeout->setInterval( 5000 );
  connect( timeout, &QTimer::timeout, process,
           [process, failed]
           {
             *failed = true;
             process->kill();
           } );
  connect( process, &QProcess::readyReadStandardError, process, [process] { process->readAllStandardError(); } );
  connect( process, &QProcess::readyReadStandardOutput, this,
           [process, bytes, failed]
           {
             *bytes += process->readAllStandardOutput();
             if( bytes->size() > 2 * 1024 * 1024 )
             {
               *failed = true;
               bytes->clear();
               process->kill();
             }
           } );
  auto finish = [this, process, bytes, failed, done, generation, timeout]( bool success )
  {
    timeout->stop();
    if( m_process == process )
      m_process = nullptr;
    process->disconnect( this );
    process->deleteLater();
    if( generation == m_generation )
      done( success && !*failed, *bytes );
  };
  connect( process, &QProcess::finished, this, [finish]( int code, QProcess::ExitStatus status )
           { finish( status == QProcess::NormalExit && code == 0 ); } );
  connect( process, &QProcess::errorOccurred, this,
           [finish]( QProcess::ProcessError e )
           {
             if( e == QProcess::FailedToStart )
               finish( false );
           } );
  timeout->start();
  process->start( m_command, args );
}
void RefreshAdvisor::propose()
{
  if( !m_wanted || !enabled() || m_sleeping )
    return;
  run( { "-j" },
       [this]( bool ok, const QByteArray& bytes )
       {
         const auto doc = QJsonDocument::fromJson( bytes );
         if( !ok || !doc.isObject() || doc.object()["outputs"].toArray().isEmpty() )
         {
           if( ++m_retries < 3 )
             m_debounce.start( 1500 );
           else
             m_wanted = false;
           return;
         }
         m_wanted = false;
         const auto offer = plan( doc.object(), m_source, m_drm );
         if( !offer )
           return;
         notify( m_source == "bat" ? "Энергосбережение дисплея" : "Питание подключено",
                 QString( "%1: сейчас %2 Гц. Переключить на %3 Гц?" )
                     .arg( offer->output )
                     .arg( offer->currentHz, 0, 'f', 0 )
                     .arg( offer->targetHz, 0, 'f', 0 ),
                 offer );
       } );
}
void RefreshAdvisor::notify( const QString& title, const QString& body, const std::optional< RefreshPlan >& offer )
{
  if( !enabled() || m_sleeping )
    return;
  closeNotification();
  const auto generation = m_generation;
  auto call = QDBusMessage::createMethodCall( notificationService, notificationPath, notificationInterface, "Notify" );
  call.setArguments(
      { "UCC", quint32( 0 ), "video-display", title, body,
        offer ? QStringList{ "apply-refresh", QString( "Установить %1 Гц" ).arg( offer->targetHz, 0, 'f', 0 ) }
              : QStringList{},
        QVariantMap{}, qint32( 60000 ) } );
  auto* watcher = new QDBusPendingCallWatcher( m_bus.asyncCall( call, 1500 ), this );
  connect( watcher, &QDBusPendingCallWatcher::finished, this,
           [this, watcher, offer, generation]
           {
             const QDBusPendingReply< uint > reply = *watcher;
             watcher->deleteLater();
             if( reply.isError() )
               return;
             const auto id = reply.value();
             if( generation != m_generation || m_sleeping || !enabled() )
             {
               auto close = QDBusMessage::createMethodCall( notificationService, notificationPath,
                                                            notificationInterface, "CloseNotification" );
               close.setArguments( { id } );
               m_bus.asyncCall( close, 1000 );
               return;
             }
             m_notification = id;
             m_pending = offer;
           } );
}
void RefreshAdvisor::actionInvoked( uint id, const QString& action )
{
  if( id != m_notification || action != "apply-refresh" || !m_pending )
    return;
  const auto selected = *m_pending;
  closeNotification();
  if( !enabled() || m_sleeping )
    return;
  run( { "-j" },
       [this, selected]( bool ok, const QByteArray& bytes )
       {
         const auto fresh = ok ? plan( QJsonDocument::fromJson( bytes ).object(), m_source, m_drm ) : std::nullopt;
         if( !fresh || !selected.sameSelection( *fresh ) || !enabled() )
         {
           emit result( false, "Display or power state changed; the old suggestion was discarded" );
           return;
         }
         run( { "output." + selected.output + ".mode." + selected.targetMode },
              [this, selected]( bool appliedCommand, const QByteArray& )
              {
                if( !appliedCommand )
                {
                  report( false, selected );
                  return;
                }
                const auto generation = m_generation;
                QTimer::singleShot(
                    250, this,
                    [this, selected, generation]
                    {
                      if( generation == m_generation )
                        run( { "-j" },
                             [this, selected]( bool ok, const QByteArray& bytes )
                             {
                               report( ok && applied( QJsonDocument::fromJson( bytes ).object(), selected ), selected );
                             } );
                    } );
              } );
       } );
}
void RefreshAdvisor::report( bool success, const RefreshPlan& p )
{
  const auto text = success ? QString( "%1: установлено %2 Гц" ).arg( p.output ).arg( p.targetHz, 0, 'f', 0 )
                            : QString( "Не удалось подтвердить изменение частоты %1" ).arg( p.output );
  emit result( success, text );
  notify( success ? "Частота изменена" : "Частота не изменена", text );
}
void RefreshAdvisor::notificationClosed( uint id, uint )
{
  if( id == m_notification )
  {
    m_notification = 0;
    m_pending.reset();
  }
}
SessionSleepMonitor::SessionSleepMonitor( QObject* parent ) : QObject( parent )
{
  QDBusConnection::systemBus().connect( "org.freedesktop.login1", "/org/freedesktop/login1",
                                        "org.freedesktop.login1.Manager", "PrepareForSleep", this,
                                        SLOT( prepare( bool ) ) );
}
} // namespace ucc
