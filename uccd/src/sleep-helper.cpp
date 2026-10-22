// SPDX-License-Identifier: GPL-3.0-or-later
#include "SleepState.hpp"
#include <QCoreApplication>
#include <QDBusConnection>
#include <QDBusMessage>
#include <QProcess>
#include <QTextStream>
#include <QTimer>

namespace
{
bool systemctl( const QString& verb, QByteArray& output, QString& error )
{
  QProcess command;
  QStringList args{ verb, "uccd.service" };
  if( verb == "show" )
    args << "--property=ActiveState" << "--value";
  command.start( "/usr/bin/systemctl", args );
  if( !command.waitForStarted( 2000 ) )
  {
    error = "Cannot start systemctl";
    return false;
  }
  if( !command.waitForFinished( 20000 ) )
  {
    command.kill();
    command.waitForFinished( 1000 );
    error = "systemctl timed out";
    return false;
  }
  output = command.readAllStandardOutput().trimmed();
  if( command.exitStatus() != QProcess::NormalExit || command.exitCode() != 0 )
  {
    error = "systemctl " + verb + " failed: " + QString::fromUtf8( command.readAllStandardError() );
    return false;
  }
  return true;
}
bool get( const QString& method, QVariant& value, QString& error )
{
  auto call = QDBusMessage::createMethodCall( "com.uniwill.uccd", "/com/uniwill/uccd", "com.uniwill.uccd", method );
  call.setAutoStartService( false );
  const auto reply = QDBusConnection::systemBus().call( call, QDBus::Block, 3000 );
  if( reply.type() != QDBusMessage::ReplyMessage || reply.arguments().size() != 1 )
  {
    error = "Cannot capture " + method + ": " + reply.errorMessage();
    return false;
  }
  value = reply.arguments()[0];
  return true;
}
} // namespace
int main( int argc, char** argv )
{
  QCoreApplication app( argc, argv );
  const auto args = app.arguments();
  QString error;
#ifdef UCC_READ_ONLY_PREVIEW
  QTextStream( stderr ) << "Sleep integration is disabled in read-only previews\n";
  return 1;
#endif
  if( ::geteuid() != 0 || args.size() != 2 || ( args[1] != "prepare" && args[1] != "resume" ) )
  {
    QTextStream( stderr ) << "Usage (root only): ucc-sleep prepare|resume\n";
    return 1;
  }
  ucc::SleepOperations ops;
  ops.active = []( bool& active, QString& err )
  {
    QByteArray state;
    if( !systemctl( "show", state, err ) )
      return false;
    if( state == "active" || state == "activating" || state == "reloading" )
    {
      active = true;
      return true;
    }
    if( state == "inactive" || state == "failed" )
    {
      active = false;
      return true;
    }
    err = "Daemon state is unstable: " + QString::fromUtf8( state );
    return false;
  };
  ops.capture = []( ucc::SleepState& state, QString& err )
  {
    QVariant policy, cooler;
    if( !get( "GetPowerPolicyJSON", policy, err ) || !get( "IsWaterCoolerEnabled", cooler, err ) )
      return false;
    state.manualMode = QJsonDocument::fromJson( policy.toString().toUtf8() ).object()["manual_mode"].toString();
    if( !ucc::SleepState::validMode( state.manualMode ) || cooler.metaType().id() != QMetaType::Bool )
    {
      err = "Invalid daemon runtime state";
      return false;
    }
    state.coolerEnabled = cooler.toBool();
    return true;
  };
  ops.stop = []( QString& err )
  {
    QByteArray output;
    return systemctl( "stop", output, err );
  };
  ops.start = []( QString& err )
  {
    QByteArray output;
    return systemctl( "start", output, err );
  };
  const ucc::SleepStateStore store;
  const bool success =
      args[1] == "prepare" ? ucc::prepareSleep( store, ops, error ) : ucc::resumeSleep( store, ops, error );
  if( !success )
    QTextStream( stderr ) << error << '\n';
  return success ? 0 : 1;
}
